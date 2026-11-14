//! The cut's state, what the plan and the export share: the records and
//! their tables, the textures and their names, the key scratch, and the
//! sites' rules (a skip row, an output to the sink, a key taken). Every
//! table is a [`MeteredVec`] on `M_X`, charged at its capacity before it
//! grows, and each string is charged before it is made (review round 2's
//! S3); the records' charge moves into the [`Cut`] they return as.

use std::io::Write as _;
use std::sync::Arc;

use unityrs_core::classid::class_name;
use unityrs_core::files::SerializedFile;
use unityrs_core::limits::{Charge, MeteredVec};
use unityrs_core::{ObjectRef, RawStr};
use unityrs_texture::Format;

use super::classify::{self, Class, Reason};
use super::kept::Kept;
use super::keys::{
    KeyRule, KeyTable, STOP, Template, TemplateKind, Var, check_member, check_prefixed,
};
use super::read::{self, Reader, Texture};
use super::seam::Seam;
use super::table;
use super::{Cut, Failure, MemberPath, Options, Output, Rejected, SheetRow, Skip, SkipOutput};

/// `BuildTarget.UnknownPlatform`, UnityPy's platform for a file that stores
/// none (format 7 and below).
pub(super) const UNKNOWN_PLATFORM: i32 = 3716;

/// A texture of the input's own files, its name and object read once,
/// each failure kept as a [`Kept`] (charged, or words).
pub(super) struct Tex {
    pub obj: ObjectRef,
    pub name: Option<Result<RawStr, Kept>>,
    read: Option<Result<Texture, Kept>>,
}

impl Tex {
    pub fn new(obj: ObjectRef) -> Self {
        Self {
            obj,
            name: None,
            read: None,
        }
    }

    fn named(&self) -> Option<&[u8]> {
        match &self.name {
            Some(Ok(n)) => Some(n.as_bytes()),
            _ => None,
        }
    }
}

/// An output's own variables (2.1): its name, its atlas, its sprite.
#[derive(Clone, Copy, Default)]
pub(super) struct Vars<'v> {
    pub name: &'v [u8],
    pub atlas: &'v [u8],
    pub sprite: &'v [u8],
}

/// Where a skip is recorded.
pub(super) struct Site<'o> {
    pub obj: &'o ObjectRef,
    pub output: SkipOutput,
    pub sprite: Option<usize>,
}

/// The records, as the cut grows them.
struct Records {
    plan_skips: MeteredVec<Skip>,
    export_skips: MeteredVec<Skip>,
    sheets: MeteredVec<SheetRow>,
    nested: MeteredVec<RawStr>,
    /// The skip rows' strings.
    strings: Charge,
    unresolved: u64,
}

/// The key scratch: an expansion, and a key joined to the sink's prefix,
/// each at most [`STOP`] bytes, allocated once after their charge.
struct Scratch {
    key: Vec<u8>,
    /// How many of `key`'s last bytes start a code point the stop cut.
    tail: usize,
    full: Vec<u8>,
    _charge: Charge,
}

/// The unwritable key's skip.
pub(super) fn unwritable(rule: KeyRule) -> Reason {
    Reason::plain(format!("unwritable_key({})", rule.name()))
}

/// Writes `template`'s expansion for an output of `obj` into `out`: how
/// many of its last bytes start a code point the stop cut
/// ([`Template::expand`]).
pub(super) fn expand(
    input: &str,
    template: &Template,
    obj: &ObjectRef,
    vars: Vars<'_>,
    out: &mut Vec<u8>,
) -> usize {
    let mut id = [0u8; 24];
    let n = {
        let mut w = id.as_mut_slice();
        let _ = write!(w, "{}", obj.path_id());
        24 - w.len()
    };
    let id = id.get(..n).unwrap_or_default();
    let ty = class_name(obj.class_id()).unwrap_or_default().as_bytes();
    template.expand(
        |v| match v {
            Var::Input | Var::SafeInput => input.as_bytes(),
            Var::PathId => id,
            Var::Type => ty,
            Var::Name | Var::SafeName => vars.name,
            Var::Atlas | Var::SafeAtlas => vars.atlas,
            Var::Sprite | Var::SafeSprite => vars.sprite,
        },
        out,
    )
}

/// The cut's state: what plan and export share.
pub(super) struct Attempt<'a> {
    pub seam: Seam<'a>,
    pub options: &'a Options,
    pub input: &'a str,
    keys: &'a mut KeyTable,
    sink: &'a mut dyn FnMut(Output) -> Result<(), Failure>,
    pub reader: Reader,
    pub texs: MeteredVec<Tex>,
    /// The textures with a name, sorted by (name, index), once every name
    /// is read: a name's first owner is the first of its run.
    owners: Option<MeteredVec<usize>>,
    /// The textures whose name's read [`Self::names`] met a refusal of
    /// `M_X`'s, in object order: read again by the next asker.
    late: MeteredVec<usize>,
    /// How many of `late`, from its first, are read, and how many of those
    /// have a name: [`Self::join_late`]'s cursor, so a read done is never
    /// looked at again (B3's review round 5, Q1).
    late_read: usize,
    late_named: usize,
    /// Whether the export runs: its skips are recorded apart.
    pub exporting: bool,
    records: Records,
    scratch: Scratch,
    /// The plan's strings (keys, member paths), until the plan is dropped.
    pub plan_strings: Charge,
}

impl<'a> Attempt<'a> {
    pub fn new(
        seam: Seam<'a>,
        options: &'a Options,
        input: &'a str,
        keys: &'a mut KeyTable,
        sink: &'a mut dyn FnMut(Output) -> Result<(), Failure>,
    ) -> Result<Self, Failure> {
        let meter = seam.meter;
        let scratch = seam.probe(|| {
            let charge = meter
                .charge(table::string(STOP).saturating_mul(2))
                .map_err(|e| table::refused(&e))?;
            let buffer = || {
                let mut v = Vec::new();
                v.try_reserve_exact(STOP)
                    .map_err(|_| table::host_memory())?;
                Ok::<_, Class>(v)
            };
            Ok(Scratch {
                key: buffer()?,
                tail: 0,
                full: buffer()?,
                _charge: charge,
            })
        })?;
        Ok(Self {
            seam,
            options,
            input,
            keys,
            sink,
            reader: Reader::new(meter)?,
            texs: MeteredVec::new(meter),
            owners: None,
            late: MeteredVec::new(meter),
            late_read: 0,
            late_named: 0,
            exporting: false,
            records: Records {
                plan_skips: MeteredVec::new(meter),
                export_skips: MeteredVec::new(meter),
                sheets: MeteredVec::new(meter),
                nested: MeteredVec::new(meter),
                strings: Charge::zero(meter),
                unresolved: 0,
            },
            scratch,
            plan_strings: Charge::zero(meter),
        })
    }

    /// The records, their charge moved into the cut.
    pub fn finish(self) -> Cut {
        let Records {
            plan_skips,
            export_skips,
            sheets,
            nested,
            mut strings,
            unresolved,
        } = self.records;
        let mut parts = |v: (Vec<_>, Charge)| {
            strings.absorb(v.1);
            v.0
        };
        let plan_skips = parts(plan_skips.into_parts());
        let export_skips = parts(export_skips.into_parts());
        let (sheets, c) = sheets.into_parts();
        strings.absorb(c);
        let (nested_containers, c) = nested.into_parts();
        strings.absorb(c);
        Cut {
            plan_skips,
            export_skips,
            unresolved_scripts: unresolved,
            sheets,
            nested_containers,
            _charge: Some(strings),
        }
    }

    /// Runs `f`, a growth of the cut's own tables, under the probe: a
    /// refusal is X's, with its reason.
    pub fn charged<T>(&self, f: impl FnOnce() -> Result<T, Class>) -> Result<T, Failure> {
        self.seam.probe(f).map_err(Failure::from)
    }

    /// Charges `bytes` of the plan's strings to `M_X`.
    pub fn hold(&mut self, bytes: u64) -> Result<(), Failure> {
        let held = &mut self.plan_strings;
        self.seam
            .probe(|| held.grow(bytes).map_err(|e| table::refused(&e)))
            .map_err(Failure::from)
    }

    /// Section 3's selection over X's own handle: every top-level row
    /// opened, in directory order, a nested container recorded; the own
    /// `SerializedFile`s in `files()` order. Each open is under the probe.
    pub fn select(&mut self) -> Result<MeteredVec<Arc<SerializedFile>>, Failure> {
        let seam = self.seam;
        let nested = &mut self.records.nested;
        super::select_own(&seam.registered.own, seam.meter, seam.hooks, &mut |path| {
            seam.probe(|| table::push(nested, path.clone()))
                .map_err(Failure::from)
        })
    }

    /// Counts an unresolved `m_Script`.
    pub fn unresolved(&mut self) {
        self.records.unresolved += 1;
    }

    /// Records a sheet row.
    pub fn sheet(&mut self, row: SheetRow) -> Result<(), Failure> {
        let sheets = &mut self.records.sheets;
        self.seam
            .probe(|| table::push(sheets, row))
            .map_err(Failure::from)
    }

    /// Hands `output` to the sink, after the stop check.
    pub fn emit(&mut self, output: Output) -> Result<(), Failure> {
        self.seam.stop()?;
        (self.sink)(output)
    }

    /// Records `class` at `site`: an object's skip is a row; an input's or
    /// run's failure ends the cut.
    pub fn record(&mut self, class: Class, site: &Site<'_>) -> Result<(), Failure> {
        match class {
            Class::Skip(reason) => self.skip(site, reason),
            Class::Input(r) => Err(Failure::Input(r)),
            Class::Run(r) => Err(Failure::Run(r)),
        }
    }

    /// Records a skip row, the plan's or the export's: its table's growth
    /// and its class's string charged before either is made.
    pub fn skip(&mut self, site: &Site<'_>, reason: Reason) -> Result<(), Failure> {
        let seam = self.seam;
        let id = site.obj.class_id();
        let mut digits = [0u8; 12];
        let n = {
            let mut w = digits.as_mut_slice();
            let _ = write!(w, "{id}");
            12 - w.len()
        };
        let class: &[u8] = match class_name(id) {
            Some(c) => c.as_bytes(),
            None => digits.get(..n).unwrap_or_default(),
        };
        let detail = reason.detail.as_ref().map_or(0, String::len);
        let cost = table::string(class.len())
            .saturating_add(table::string(reason.reason.len()))
            .saturating_add(table::string(detail));
        let records = &mut self.records;
        let rows = if self.exporting {
            &mut records.export_skips
        } else {
            &mut records.plan_skips
        };
        let strings = &mut records.strings;
        seam.probe(|| rows.reserve(1).map_err(|e| table::refused(&e)))
            .map_err(Failure::from)?;
        seam.probe(|| strings.grow(cost).map_err(|e| table::refused(&e)))
            .map_err(Failure::from)?;
        let row = Skip {
            serialized_file: site.obj.file().name().clone(),
            path_id: site.obj.path_id(),
            class: String::from_utf8_lossy(class).into_owned(),
            output: site.output,
            sprite: site.sprite,
            reason,
        };
        seam.probe(|| table::push(rows, row))
            .map_err(Failure::from)?;
        #[cfg(feature = "test-hooks")]
        seam.hooks.skipped(self.exporting);
        Ok(())
    }

    /// Reads every texture's name, in object order, and sorts the owners:
    /// once. A name's read that fails is kept for its texture; a refusal of
    /// `M_X`'s is a state of the meter and is not (B3's review round 3, S9:
    /// as round 2's S1 has it), so the texture is left unread and its name
    /// read again by the next asker ([`Self::name`]), the refusal there that
    /// asker's.
    pub fn names(&mut self) -> Result<(), Failure> {
        if self.owners.is_some() {
            return Ok(());
        }
        let seam = self.seam;
        let mut named = 0usize;
        for i in 0..self.texs.len() {
            seam.stop()?;
            let Some(t) = self.texs.as_slice().get(i) else {
                continue;
            };
            let obj = t.obj.clone();
            let name = match self.reader.peek_name(&seam, &obj) {
                Ok(n) => {
                    named += 1;
                    Ok(n)
                }
                Err(c) => match Kept::keep(seam.meter, seam.hooks, c) {
                    Ok(k) => Err(k),
                    Err(c) if Kept::metered(&c) => {
                        let late = &mut self.late;
                        seam.probe(|| table::push(late, i)).map_err(Failure::from)?;
                        continue;
                    }
                    Err(c) => return Err(c.into()),
                },
            };
            if let Some(t) = self.texs.as_mut_slice().get_mut(i) {
                t.name = Some(name);
            }
        }
        let mut owners = self.charged(|| table::with_capacity(seam.meter, named))?;
        for (i, t) in self.texs.as_slice().iter().enumerate() {
            if t.named().is_some() {
                self.charged(|| table::push(&mut owners, i))?;
            }
        }
        let texs = self.texs.as_slice();
        let name = |i: usize| texs.get(i).and_then(Tex::named).unwrap_or_default();
        owners
            .as_mut_slice()
            .sort_unstable_by(|a, b| name(*a).cmp(name(*b)).then(a.cmp(b)));
        self.owners = Some(owners);
        Ok(())
    }

    /// Texture `i`'s name, read where [`Self::names`] left it unread: its
    /// failure kept for it; a refusal of `M_X`'s returned, the asker's, and
    /// not kept.
    pub fn name(&mut self, i: usize) -> Result<(), Class> {
        let seam = self.seam;
        let Some(t) = self.texs.as_slice().get(i).filter(|t| t.name.is_none()) else {
            return Ok(());
        };
        let obj = t.obj.clone();
        let name = match self.reader.peek_name(&seam, &obj) {
            Ok(n) => Ok(n),
            Err(c) => Err(Kept::keep(seam.meter, seam.hooks, c)?),
        };
        if let Some(t) = self.texs.as_mut_slice().get_mut(i) {
            t.name = Some(name);
        }
        Ok(())
    }

    /// The first texture, in object order, named `name` ([`Self::names`]
    /// first): the sorted owners' first. The names read late are read first
    /// ([`Self::name`], whose refusal is the asker's), and, once all are,
    /// joined to the owners and sorted again, once, so `late` is empty after
    /// and no asker scans it (B3's review round 4, R6).
    pub fn owner(&mut self, name: &[u8]) -> Result<Option<usize>, Class> {
        if self.owners.is_some() && !self.late.is_empty() {
            self.join_late()?;
        }
        let Some(owners) = self.owners.as_ref().map(MeteredVec::as_slice) else {
            return Ok(None);
        };
        let texs = self.texs.as_slice();
        let of = |i: usize| texs.get(i).and_then(Tex::named).unwrap_or_default();
        let at = owners.partition_point(|i| of(*i) < name);
        Ok(owners.get(at).copied().filter(|i| of(*i) == name))
    }

    /// Reads every name [`Self::names`] left unread, from its cursor on
    /// (`late_read`: a refusal leaves it at the name refused, so an asker
    /// after reads on from there and never looks at a name read before:
    /// B3's review round 5, Q1), then joins the named ones to the owners,
    /// sorted by (name, index) again, and empties `late`: the joined table
    /// charged before it is made, the old one dropped. The join runs once,
    /// as [`Self::names`]' own sort does; a refusal of its table looks at
    /// nothing again.
    fn join_late(&mut self) -> Result<(), Class> {
        let internal = || Class::Run(Reason::plain("internal"));
        while let Some(i) = self.late.as_slice().get(self.late_read).copied() {
            #[cfg(feature = "test-counters")]
            crate::testing::late_scan();
            self.name(i)?;
            if self.texs.as_slice().get(i).and_then(Tex::named).is_some() {
                self.late_named += 1;
            }
            self.late_read += 1;
        }
        let seam = self.seam;
        let meter = seam.meter;
        let Some(owners) = self.owners.as_ref() else {
            return Err(internal());
        };
        let texs = self.texs.as_slice();
        let named = |i: &usize| texs.get(*i).and_then(Tex::named).is_some();
        let n = owners.len() + self.late_named;
        let mut joined = seam.probe(|| table::with_capacity(meter, n))?;
        for i in owners.as_slice().iter().chain(self.late.as_slice()) {
            if named(i) {
                seam.probe(|| table::push(&mut joined, *i))?;
            }
        }
        let name = |i: usize| texs.get(i).and_then(Tex::named).unwrap_or_default();
        joined
            .as_mut_slice()
            .sort_unstable_by(|a, b| name(*a).cmp(name(*b)).then(a.cmp(b)));
        self.owners = Some(joined);
        self.late = MeteredVec::new(meter);
        (self.late_read, self.late_named) = (0, 0);
        Ok(())
    }

    /// Texture `i`'s object read, once, under the probe: the texture, or
    /// its skip (kept), or the input's or the run's failure (not kept). A
    /// refusal of `M_X`'s, the read's or the keeping's, is the asker's and
    /// not kept, so the next asks again (B3's review round 2, S1).
    pub fn texture(&mut self, i: usize) -> Result<Texture, Class> {
        let seam = self.seam;
        let t = self
            .texs
            .as_mut_slice()
            .get_mut(i)
            .ok_or(Class::Run(Reason::plain("internal")))?;
        if t.read.is_none() {
            let read = match seam.probe(|| read::texture(&t.obj)) {
                Ok(t) => Ok(t),
                Err(c) => Err(Kept::keep(seam.meter, seam.hooks, c)?),
            };
            t.read = Some(read);
        }
        match &t.read {
            Some(Ok(t)) => Ok(t.clone()),
            Some(Err(k)) => Err(k.class()),
            None => Err(Class::Run(Reason::plain("internal"))),
        }
    }

    /// Texture `i`'s section 3 answers 1 to 3 ([`read::answer`]): its
    /// format, or the skip.
    pub fn selection(&self, i: usize, t: &Texture) -> Result<Format, Class> {
        let platform = self
            .texs
            .as_slice()
            .get(i)
            .and_then(|x| x.obj.file().target_platform())
            .unwrap_or(UNKNOWN_PLATFORM);
        read::answer(t, platform, self.options.formats)
    }

    /// Texture `i`'s answer at the plan: its format, or the skip (its
    /// object read's failure, then a selection skip).
    pub fn sheet_answer(&mut self, i: usize) -> Result<Format, Class> {
        let t = self.texture(i)?;
        self.selection(i, &t)
    }

    /// Expands `template` for an output of `obj` into the key scratch.
    pub fn expand(&mut self, template: &Template, obj: &ObjectRef, vars: Vars<'_>) {
        self.scratch.tail = expand(self.input, template, obj, vars, &mut self.scratch.key);
    }

    /// The key in the scratch, checked as given and after the sink's
    /// prefix (2.3): its length, or the rule it breaks.
    pub fn check_key(&mut self) -> Result<usize, KeyRule> {
        let Scratch { key, full, .. } = &mut self.scratch;
        check_prefixed(key, self.seam.prefix, full).map(str::len)
    }

    /// The key in the scratch, checked (2.3).
    pub fn scratch_key(&self) -> &[u8] {
        &self.scratch.key
    }

    /// Step 3 for one output, its key expanded into the scratch: checked as
    /// given and after the sink's prefix before the duplicate test (2.3),
    /// taken from the one table; or its skip.
    pub fn take(&mut self, site: &Site<'_>) -> Result<Option<String>, Failure> {
        if let Err(rule) = self.check_key() {
            self.skip(site, unwritable(rule))?;
            return Ok(None);
        }
        let key = std::mem::take(&mut self.scratch.key);
        let taken = self.take_checked(&key, site);
        self.scratch.key = key;
        taken
    }

    /// [`Self::take`] for a key already checked: the table's take, then the
    /// plan's string, charged before it is made.
    pub fn take_checked(&mut self, k: &[u8], site: &Site<'_>) -> Result<Option<String>, Failure> {
        let Ok(k) = std::str::from_utf8(k) else {
            return Err(Failure::Run(Reason::plain("internal")));
        };
        let keys = &mut *self.keys;
        let taken = self.seam.probe(|| keys.take(k)).map_err(Failure::from)?;
        if !taken {
            self.skip(site, Reason::plain("duplicate_key"))?;
            return Ok(None);
        }
        self.hold(table::string(k.len()))?;
        Ok(Some(k.to_owned()))
    }

    /// The archive member paths of an output of `kind` (4.7), each
    /// expanded to at most 1,025 bytes and checked by 2.3's member rules; a
    /// rejected one kept with its rule and its bytes under the text rule
    /// (review round 2's S5), a code point the expansion's stop cut dropped
    /// rather than read as invalid bytes (round 3's P3). The table and each
    /// string are charged first, the string at the length it is measured
    /// to take, and reserved at exactly that.
    pub fn member_paths(
        &mut self,
        kind: TemplateKind,
        obj: &ObjectRef,
        vars: Vars<'_>,
    ) -> Result<MeteredVec<MemberPath>, Failure> {
        let members = self.seam.members;
        let n = members.iter().filter(|m| m.kind == kind).count();
        let meter = self.seam.meter;
        let mut out = self.charged(|| table::with_capacity(meter, n))?;
        for m in members.iter().filter(|m| m.kind == kind) {
            self.expand(&m.path, obj, vars);
            let path = match check_member(&self.scratch.key) {
                Ok(p) => Ok(p.len()),
                Err(rule) => Err(rule),
            };
            let path = match path {
                Ok(len) => {
                    self.hold(table::string(len))?;
                    let p = std::str::from_utf8(&self.scratch.key).unwrap_or_default();
                    let mut s = String::new();
                    s.try_reserve_exact(len)
                        .map_err(|_| Failure::from(table::host_memory()))?;
                    s.push_str(p);
                    Ok(s)
                }
                Err(rule) => {
                    // The bytes the expansion made whole: past the stop,
                    // the path was longer than the text rule keeps.
                    let made = self.scratch.key.len();
                    let whole = made.saturating_sub(self.scratch.tail);
                    let key = &self.scratch.key;
                    let (len, _) = classify::text_len(&[key.get(..whole).unwrap_or_default()]);
                    self.hold(table::string(len))?;
                    let key = &self.scratch.key;
                    let (path, cut) = classify::try_text(&[key.get(..whole).unwrap_or_default()])
                        .map_err(|_| Failure::from(table::host_memory()))?;
                    let cut = cut || made > classify::CUT;
                    Err(Rejected { rule, path, cut })
                }
            };
            self.charged(|| {
                table::push(
                    &mut out,
                    MemberPath {
                        archive: m.archive,
                        path,
                    },
                )
            })?;
        }
        Ok(out)
    }
}
