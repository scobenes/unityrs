//! 2.6 steps 2 and 4, the engine's to drive before the cut: X's own open on
//! `M_X`, the private Environment, and each bundle's marked rows reached
//! (`docs/plans/batch.md` 2.6; review round 1's B4, round 2's S1 and S2).
//!
//! A [`Registration`] is the engine's to drive in 2.6's order:
//! [`Registration::open`] is step 2 (X's own open on `M_X` by a
//! `Source::Path`'s rules, the stream reader, after the members' ids are
//! checked, in bytewise order and none repeated, and the stop), then
//! step 3 is the engine's ([`Registration::own`] gives X's own handle),
//! then [`Registration::member`] registers each member in bytewise order of
//! id with `load_dependency`, handing the engine each loaded bundle (for
//! the view's name table and its `same_named_cab` rows) and reaching its
//! marked rows, its peek bits loaded at that reach into a buffer charged to
//! `M_X` and dropped after ([`PeekBits`], [`Bits`]); and
//! [`Registration::finish`] registers X last, from the bytes of its own
//! open, compares the registered header and directory with the own
//! handle's, and reaches X's rows. [`register`] runs all of it for a view
//! as staged, with no step 3. Every Environment call is checked as 2.6
//! step 7 says: its run-level class first, then the evictions (X's own
//! bare `limit(attempt_evictions, <value>)`, never a member's), then the
//! rest. Every core call, and every charge of a table registration makes,
//! is under the refusal probe on its own, with the engine's hooks (2.7;
//! review round 3's P1). [`peek`] is the peek pass's function, and
//! [`register_bytes`] the tests' path to the engine's, from bytes.
//!
//! The core has no entry point that registers X's slot over its own
//! handle's parse (step 4's "X's slot shares its own handle's
//! allocation"): the slot shares the own open's bytes, so nothing is read
//! twice, but parses them again on a dependency meter of its own; its
//! resources are served through the own handle by the core's redirect over
//! shared `Bytes`.

use std::path::Path;
use std::sync::Arc;

use unityrs_core::budget::{Reservation, Unbounded};
use unityrs_core::detect::{FileType, check_file_type, check_file_type_with_reader};
use unityrs_core::env::{EnvOptions, Environment, Source, read_path};
use unityrs_core::files::{BundleFile, File, parse_file_with_reader};
use unityrs_core::limits::{Charge, Limits, Meter, MeteredVec, heap_cost};
use unityrs_core::pystr::simplify_name;
use unityrs_core::stream::ReaderKind;
use unityrs_core::{Bytes, Error, FileContext, LimitError, LimitKind, RawStr};

use super::classify::{self, Class, Reason};
use super::seam::{self, Hooks, Registered};
use super::table;
use super::{Failure, View};

/// A staged bundle's bytes: a spool file, or bytes in hand.
#[derive(Clone, Copy, Debug)]
pub enum Data<'a> {
    /// Its staged file.
    Path(&'a Path),
    /// Its bytes.
    Bytes(&'a Bytes),
}

/// A bundle of the view as staged.
#[derive(Clone, Copy, Debug)]
pub struct Staged<'a> {
    /// Its spec id.
    pub id: &'a str,
    /// The name the core opens it under (its spool name).
    pub name: &'a RawStr,
    /// Its bytes.
    pub data: Data<'a>,
}

/// A bundle's peek bits (2.6 step 4's peek table): its top-level row count
/// and a bit per row in directory order, set for a bundle, packed eight to
/// a byte, in a buffer charged to its meter before it is allocated.
#[derive(Debug)]
pub struct Bits {
    rows: u32,
    packed: MeteredVec<u8>,
}

impl Bits {
    /// The bits of `rows` rows, all clear, charged to `meter` first: the
    /// buffer a peek fills, or an attempt reads a peek file into.
    pub fn charged(rows: u32, meter: &Arc<Meter>) -> Result<Self, Class> {
        let n = usize::try_from(rows.div_ceil(8)).unwrap_or(usize::MAX);
        let mut packed = table::with_capacity(meter, n)?;
        for _ in 0..n {
            table::push(&mut packed, 0u8)?;
        }
        Ok(Self { rows, packed })
    }

    /// A copy of `bits` (as a peek file holds them) in a buffer charged to
    /// `meter` first, the charge under the refusal probe of `hooks`: what a
    /// [`PeekBits`] loader gives (review round 4's T2), so a refusal no
    /// budget counted is the stop `host_memory` there too.
    pub fn copied(bits: &Self, meter: &Arc<Meter>, hooks: &dyn Hooks) -> Result<Self, Failure> {
        let mut out = seam::probe(hooks, || Self::charged(bits.rows, meter)).map_err(own)?;
        out.bytes_mut().copy_from_slice(bits.bytes());
        Ok(out)
    }

    /// The row count.
    #[must_use]
    pub fn rows(&self) -> u32 {
        self.rows
    }

    /// The packed bits, `ceil(rows / 8)` bytes: a peek file's bytes.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        self.packed.as_slice()
    }

    /// The packed bits to fill (a peek file's bytes, read into them).
    pub fn bytes_mut(&mut self) -> &mut [u8] {
        self.packed.as_mut_slice()
    }

    /// Row `i`'s bit.
    #[must_use]
    pub fn get(&self, i: usize) -> bool {
        self.packed
            .as_slice()
            .get(i / 8)
            .is_some_and(|b| b & (1 << (i % 8)) != 0)
    }

    /// Sets row `i`'s bit.
    pub fn set(&mut self, i: usize) {
        if let Some(b) = self.packed.as_mut_slice().get_mut(i / 8) {
            *b |= 1 << (i % 8);
        }
    }
}

/// Where an attempt reads a bundle's peek bits at its reach: the engine's
/// peek file, read into [`Bits::charged`] on `meter` (`M_X`). A refused
/// charge is X's own reason, an I/O error `host_io`; the bits are checked
/// against the registered directory's row count here.
pub trait PeekBits {
    /// The bits of the bundle `id`, charged to `meter`, the charge under
    /// the refusal probe of `hooks` ([`Bits::copied`] makes them so).
    fn load(&self, id: &str, meter: &Arc<Meter>, hooks: &dyn Hooks) -> Result<Bits, Failure>;
}

/// The first 21 bytes the core's detection reads of an entry.
const PEEK: u64 = 21;

/// A member's failure: `dependency_failed(<id>, <reason>)`, the member's
/// own reason inside (a code is `container_error`'s).
pub(super) fn dependency_failed(id: &str, c: Class) -> Failure {
    let inner = match c {
        Class::Run(r) => return Failure::Run(r),
        Class::Skip(r) | Class::Input(r) => container(r),
    };
    Failure::Input(Reason {
        reason: format!("dependency_failed({id}, {})", inner.reason),
        detail: inner.detail,
        cut: inner.cut,
    })
}

/// A load's error the bytes decide is the bundle's `container_error`.
fn container(r: Reason) -> Reason {
    if r.reason == "decode_error" {
        Reason {
            reason: "container_error".to_owned(),
            detail: r.detail,
            cut: r.cut,
        }
    } else {
        r
    }
}

/// An open's failure: a code is the bundle's `container_error`.
fn opened(e: &Error) -> Class {
    match classify::error(e) {
        Class::Skip(r) | Class::Input(r) => Class::Input(container(r)),
        run @ Class::Run(_) => run,
    }
}

/// X's own class as its failure: a charge to `M_X` or a read of its own.
fn own(c: Class) -> Failure {
    Failure::from(c)
}

/// The cutter's own fault: a run failure, `internal`.
fn internal() -> Failure {
    Failure::Run(Reason::plain("internal"))
}

/// Step 2: the bundle opened on `meter` by a `Source::Path`'s rules (the
/// stream reader), its bytes, and their charge when they were in hand
/// (`Data::Bytes`, charged to `meter` as a staged file's read is). A file
/// of another kind is `not_a_bundle`. The read (or the charge), the
/// detection and the parse are each under the probe of `hooks` on their
/// own.
fn open(
    name: &RawStr,
    data: Data<'_>,
    context: &Arc<FileContext>,
    meter: &Arc<Meter>,
    hooks: &dyn Hooks,
) -> Result<(Bytes, Option<Charge>, Arc<BundleFile>), Class> {
    let limits = &context.limits;
    let (bytes, charge) = match data {
        Data::Path(p) => {
            let read = seam::probe(hooks, || {
                read_path(p, limits, meter).map_err(|e| opened(&e))
            });
            (read?, None)
        }
        Data::Bytes(b) => {
            let len = u64::try_from(b.len()).unwrap_or(u64::MAX);
            if len > limits.max_input_bytes {
                let e = Error::Limit(LimitError::refused(
                    LimitKind::InputBytes,
                    len,
                    limits.max_input_bytes,
                ));
                return Err(opened(&e));
            }
            let charge = seam::probe(hooks, || {
                meter.charge(heap_cost(len)).map_err(|e| table::refused(&e))
            })?;
            (b.clone(), Some(charge))
        }
    };
    let ty = seam::probe(hooks, || {
        check_file_type_with_reader(&bytes, limits, ReaderKind::Stream)
            .map_err(|e| opened(&e.into()))
    })?;
    let file = seam::probe(hooks, || {
        parse_file_with_reader(
            name.clone(),
            bytes.clone(),
            ty,
            None,
            meter,
            context,
            ReaderKind::Stream,
        )
        .map_err(|e| opened(&e))
    })?;
    match file {
        File::Bundle(b) => Ok((bytes, charge, b)),
        _ => Err(Class::Input(Reason::plain("not_a_bundle"))),
    }
}

/// Whether top-level row `i` of `b` is a bundle, as the peek reads it: a
/// `Limit` there is the bundle's failure, as a host's error or a refusal
/// is, and any other error "not a container".
fn is_bundle_entry(b: &BundleFile, i: usize) -> Result<bool, Error> {
    match b.entry_range(i, 0..PEEK) {
        Ok(head) => match check_file_type(&head, &Limits::default()) {
            Ok(ty) => Ok(ty == FileType::BundleFile),
            Err(e @ unityrs_core::detect::DetectError::Allocator(_)) => Err(e.into()),
            Err(_) => Ok(false),
        },
        Err(e @ (Error::Limit(_) | Error::Io(_) | Error::Budget(_))) => Err(e),
        // Allocator provenance can be nested in an entry, tree, read or
        // dereference error. Keep the single recursive classifier authoritative.
        Err(e) => match classify::error(&e) {
            Class::Run(_) => Err(e),
            Class::Skip(_) | Class::Input(_) => Ok(false),
        },
    }
}

/// A peek's class as its outcome (2.6 step 4's peek rule): a refusal can
/// only be the allocator's, since the peek's budget's limit is its meter's
/// bound, which the meter checks first, so a `budget(...)` is `host_memory`;
/// a run-level class stays one; anything else is the bundle's peek failure.
fn peeked(c: Class) -> Failure {
    match c {
        Class::Run(r) => Failure::Run(r),
        Class::Skip(r) | Class::Input(r) if r.reason.starts_with("budget(") => {
            Failure::Run(Reason::plain("host_memory"))
        }
        Class::Skip(r) | Class::Input(r) => Failure::Input(r),
    }
}

/// The peek (2.6 step 4's peek table): the bundle opened by step 2's rules
/// on `meter`, the peek's own, then a bit per top-level row in directory
/// order, a dropped row included, set for a bundle, in a buffer charged on
/// that meter first. The stop is checked between entries. Its failure is
/// the bundle's peek failure (`Failure::Input`): its open's, or a row's
/// `Limit`; a refusal is the allocator's, `host_memory`; an I/O error
/// `host_io`.
pub fn peek(
    name: &RawStr,
    data: Data<'_>,
    context: &Arc<FileContext>,
    meter: &Arc<Meter>,
    hooks: &dyn Hooks,
) -> Result<Bits, Failure> {
    let (_, _held, b) = open(name, data, context, meter, hooks).map_err(peeked)?;
    let n = b.directory().len();
    let rows = u32::try_from(n).map_err(|_| internal())?;
    let mut bits = seam::probe(hooks, || Bits::charged(rows, meter)).map_err(peeked)?;
    for i in 0..n {
        if hooks.stopped() {
            return Err(Failure::Stopped);
        }
        if seam::probe(hooks, || is_bundle_entry(&b, i).map_err(|e| opened(&e))).map_err(peeked)? {
            bits.set(i);
        }
    }
    Ok(bits)
}

/// One Environment call of registration, checked as 2.6 step 7 says: under
/// the refusal probe, its run-level class first, then the evictions (X's
/// own bare reason), and only then the call's own error, given back for
/// the caller to name.
fn call<T>(
    env: &Environment,
    admit: &Admit<'_>,
    f: impl FnOnce() -> Result<T, Error>,
) -> Result<Result<T, Error>, Failure> {
    seam::allocator_check(admit.hooks).map_err(Failure::from)?;
    let before = admit.hooks.refusals();
    let out = f();
    #[cfg(feature = "test-hooks")]
    let out = match admit.hooks.environment_fault(env) {
        Some(error) => Err(error),
        None => out,
    };
    admit.hooks.environment(env);
    seam::allocator_check(admit.hooks).map_err(Failure::from)?;
    if let Err(e) = &out
        && let Class::Run(r) = seam::probed(admit.hooks, before, classify::error(e))
    {
        return Err(Failure::Run(r));
    }
    seam::evictions(env, admit.max_evictions).map_err(own)?;
    Ok(out)
}

/// What a reach's failure is named for: X's own row, or a member's.
#[derive(Clone, Copy)]
enum Whose<'a> {
    Own,
    Member(&'a str),
}

impl Whose<'_> {
    /// A load's or a reach's error as the input's failure.
    fn failed(self, e: &Error, path: &RawStr, admit: &Admit<'_>, before: Option<u64>) -> Failure {
        let c = seam::probed(admit.hooks, before, classify::error(e));
        match (self, c) {
            (_, Class::Run(r)) => Failure::Run(r),
            (Self::Own, _) => own(seam::probed(
                admit.hooks,
                before,
                classify::entry_error(e, path),
            )),
            (Self::Member(id), c) => dependency_failed(id, c),
        }
    }
}

/// Reaches `b`'s nested containers (2.6 step 4): each top-level row whose
/// bit is set, in directory order, but a dropped row, one whose key
/// (`simplify_name`) a later row of the bundle takes. The rows' keys are
/// computed once into a buffer, and a permutation of the rows (a `u32`
/// each) sorted in place by (key, row), a row dropped when the next in that
/// order has its key; the buffer, the permutation and the dropped rows'
/// bits are charged to `M_X` and dropped after, each charge (and each
/// `simplify_name`) under the probe on its own. Each `get_cab` is an
/// Environment call ([`call`]).
fn reach(
    env: &Environment,
    b: &BundleFile,
    bits: &Bits,
    whose: Whose<'_>,
    admit: &Admit<'_>,
) -> Result<(), Failure> {
    let meter = admit.meter;
    let dir = b.directory();
    let _sample = super::read::Sample(b.meter(), admit.hooks);
    let n = dir.len();
    if u32::try_from(n).ok() != Some(bits.rows()) {
        return Err(internal());
    }
    let hooks = admit.hooks;
    // The keys, one buffer; each row's end in it.
    let mut keys: MeteredVec<u8> = MeteredVec::new(meter);
    let mut ends: MeteredVec<usize> =
        seam::probe(hooks, || table::with_capacity(meter, n)).map_err(own)?;
    for row in dir {
        let key = seam::probe(hooks, || {
            simplify_name(&row.path, meter).map_err(|e| table::refused(&e))
        })
        .map_err(own)?;
        seam::probe(hooks, || {
            keys.extend_from_slice(key.as_bytes())
                .map_err(|e| table::refused(&e))
        })
        .map_err(own)?;
        let end = keys.len();
        seam::probe(hooks, || table::push(&mut ends, end)).map_err(own)?;
    }
    let key = |i: u32| {
        let i = usize::try_from(i).unwrap_or(usize::MAX);
        let start = i
            .checked_sub(1)
            .and_then(|p| ends.as_slice().get(p))
            .copied();
        let end = ends.as_slice().get(i).copied().unwrap_or(0);
        keys.as_slice()
            .get(start.unwrap_or(0)..end)
            .unwrap_or_default()
    };
    let mut order: MeteredVec<u32> =
        seam::probe(hooks, || table::with_capacity(meter, n)).map_err(own)?;
    for i in 0..bits.rows() {
        seam::probe(hooks, || table::push(&mut order, i)).map_err(own)?;
    }
    order
        .as_mut_slice()
        .sort_unstable_by(|a, b| key(*a).cmp(key(*b)).then(a.cmp(b)));
    let mut dropped = seam::probe(hooks, || Bits::charged(bits.rows(), meter)).map_err(own)?;
    for pair in order.as_slice().windows(2) {
        if let [a, b] = pair
            && key(*a) == key(*b)
        {
            dropped.set(usize::try_from(*a).unwrap_or(usize::MAX));
        }
    }
    drop(order);
    for (i, row) in dir.iter().enumerate() {
        if !bits.get(i) || dropped.get(i) {
            continue;
        }
        let before = admit.hooks.refusals();
        if let Err(e) = call(env, admit, || env.get_cab(&row.path, meter))? {
            return Err(whose.failed(&e, &row.path, admit, before));
        }
    }
    Ok(())
}

/// What registration needs beside the view.
#[derive(Clone, Copy)]
pub struct Admit<'a> {
    /// The private Environment's options: `EnvOptions::batch(ctx_X)` with
    /// `dep_cache_bytes` set to `D` (2.2); its context's limits and budget
    /// are the attempt's.
    pub env: &'a EnvOptions,
    /// `M_X`.
    pub meter: &'a Arc<Meter>,
    /// The stop flag and the refusal count.
    pub hooks: &'a dyn Hooks,
    /// `max_attempt_evictions`.
    pub max_evictions: u64,
}

/// A view's registration, as the engine drives it (module docs).
pub struct Registration<'a> {
    admit: Admit<'a>,
    input: Staged<'a>,
    env: Environment,
    own: Arc<BundleFile>,
    bytes: Bytes,
    held: Option<Charge>,
    /// The members' ids, in the order they are registered.
    ids: &'a [&'a str],
    /// How many of them are registered.
    next: usize,
}

/// Whether `ids` are in strictly ascending bytewise order (so none is
/// repeated) and none is X's own `x`.
fn in_order(ids: &[&str], x: &str) -> bool {
    ids.windows(2)
        .all(|w| matches!(w, [a, b] if a.as_bytes() < b.as_bytes()))
        && !ids.contains(&x)
}

impl<'a> Registration<'a> {
    /// Step 2: the members' `ids` checked first (in bytewise order, none
    /// repeated, X's not among them; any other is `internal`, the caller's
    /// fault, review round 3's P4), then the stop, then X's own open on
    /// `M_X`, and the private Environment, empty.
    pub fn open(input: Staged<'a>, ids: &'a [&'a str], admit: Admit<'a>) -> Result<Self, Failure> {
        if !in_order(ids, input.id) {
            return Err(internal());
        }
        if admit.hooks.stopped() {
            return Err(Failure::Stopped);
        }
        let context = &admit.env.context;
        let (bytes, held, own) = open(input.name, input.data, context, admit.meter, admit.hooks)?;
        Ok(Self {
            admit,
            input,
            env: Environment::new(admit.env.clone()),
            own,
            bytes,
            held,
            ids,
            next: 0,
        })
    }

    /// Current private Environment figures, including a failed member call.
    #[must_use]
    pub fn stats(&self) -> unityrs_core::env::EnvStats {
        self.env.stats()
    }

    /// X's own handle, on `M_X`: step 3 reads its AssetBundle object.
    #[must_use]
    pub fn own(&self) -> &Arc<BundleFile> {
        &self.own
    }

    /// Step 4 for one member, after the stop check: its load (the next of
    /// the ids [`Self::open`] took; any other is `internal`), the loaded
    /// bundle handed to `loaded` (the name table), then its reach, its bits
    /// loaded from `bits` and dropped after. A failure the member makes is
    /// `dependency_failed(<id>, <reason>)`.
    pub fn member(
        &mut self,
        m: &Staged<'a>,
        bits: &dyn PeekBits,
        loaded: &mut dyn FnMut(&str, &BundleFile) -> Result<(), Failure>,
    ) -> Result<(), Failure> {
        let admit = self.admit;
        if admit.hooks.stopped() {
            return Err(Failure::Stopped);
        }
        if self.ids.get(self.next) != Some(&m.id) {
            return Err(internal());
        }
        self.next += 1;
        let whose = Whose::Member(m.id);
        let env = &self.env;
        let before = admit.hooks.refusals();
        let source = match m.data {
            Data::Bytes(b) => Source::Bytes {
                name: m.name.clone(),
                data: b.clone(),
            },
            Data::Path(p) => Source::Path(p.to_path_buf()),
        };
        let file = call(env, &admit, || env.load_dependency(source))?
            .map_err(|e| whose.failed(&e, m.name, &admit, before))?;
        let File::Bundle(b) = file else {
            return Err(dependency_failed(
                m.id,
                Class::Input(Reason::plain("not_a_bundle")),
            ));
        };
        loaded(m.id, &b)?;
        let bits = bits.load(m.id, admit.meter, admit.hooks)?;
        reach(env, &b, &bits, whose, &admit)
    }

    /// X last, every member registered (else `internal`), after the stop
    /// check: its load from its own open's bytes, the registered header and
    /// directory compared with the own handle's ([`same_open`]), the bundle
    /// handed to `loaded`, then X's reach.
    pub fn finish(
        self,
        bits: &dyn PeekBits,
        loaded: &mut dyn FnMut(&str, &BundleFile) -> Result<(), Failure>,
    ) -> Result<Registered, Failure> {
        let admit = self.admit;
        if self.next != self.ids.len() {
            return Err(internal());
        }
        if admit.hooks.stopped() {
            return Err(Failure::Stopped);
        }
        let env = &self.env;
        let before = admit.hooks.refusals();
        let source = Source::Bytes {
            name: self.input.name.clone(),
            data: self.bytes.clone(),
        };
        let file = call(env, &admit, || env.load_dependency(source))?.map_err(|e| {
            match seam::probed(admit.hooks, before, classify::error(&e)) {
                Class::Skip(r) | Class::Input(r) => Failure::Input(container(r)),
                Class::Run(r) => Failure::Run(r),
            }
        })?;
        let File::Bundle(slot) = file else {
            return Err(Failure::Input(Reason::plain("not_a_bundle")));
        };
        same_open(&slot, &self.own)?;
        loaded(self.input.id, &slot)?;
        let bits = bits.load(self.input.id, admit.meter, admit.hooks)?;
        reach(env, &slot, &bits, Whose::Own, &admit)?;
        drop(slot);
        Ok(Registered {
            env: self.env,
            own: self.own,
            _held: self.held,
        })
    }
}

/// Step 4's comparison: X's slot, read by the memory reader's rules, against
/// its own handle, read by the stream reader's. A difference (a header
/// string with no NUL in its first 32,767 bytes, which the two readers
/// return differently) fails X, `container_error`, detail `malformed`.
pub fn same_open(slot: &BundleFile, own: &BundleFile) -> Result<(), Failure> {
    if slot.header() != own.header() || slot.directory() != own.directory() {
        return Err(Failure::Input(container(Reason::decode_error("malformed"))));
    }
    Ok(())
}

/// 2.6 steps 2 and 4 for a view as staged, with no step 3: the members
/// sorted in bytewise order of id (X among them is passed over), their ids
/// checked (a repeated one is `internal`) and the stop, before X's own
/// open; then the members in that order; then X ([`Registration`]).
pub fn register(
    input: Staged<'_>,
    members: &[Staged<'_>],
    bits: &dyn PeekBits,
    admit: Admit<'_>,
    loaded: &mut dyn FnMut(&str, &BundleFile) -> Result<(), Failure>,
) -> Result<Registered, Failure> {
    let mut sorted: Vec<&Staged<'_>> = members.iter().filter(|m| m.id != input.id).collect();
    sorted.sort_by(|a, b| a.id.as_bytes().cmp(b.id.as_bytes()));
    let ids: Vec<&str> = sorted.iter().map(|m| m.id).collect();
    let mut reg = Registration::open(input, &ids, admit)?;
    for m in sorted {
        reg.member(m, bits, loaded)?;
    }
    reg.finish(bits, loaded)
}

/// The bits [`register_bytes`] peeked, served at each reach as the engine
/// serves a peek file: copied into a buffer charged to `M_X`.
struct Peeked<'a> {
    ids: Vec<&'a str>,
    bits: Vec<Bits>,
}

impl PeekBits for Peeked<'_> {
    fn load(&self, id: &str, meter: &Arc<Meter>, hooks: &dyn Hooks) -> Result<Bits, Failure> {
        let Some(i) = self.ids.iter().position(|x| *x == id) else {
            return Err(internal());
        };
        let Some(peeked) = self.bits.get(i) else {
            return Err(internal());
        };
        Bits::copied(peeked, meter, hooks)
    }
}

/// [`register`] from bytes, each bundle named by its id and its bits
/// peeked here first, each on a meter of its own (an unbounded budget, the
/// context's limits), as the engine's peek pass would: X's peek failure
/// first, then the member's of least id (2.3, rule 1). The tests' path to
/// the engine's.
pub fn register_bytes(view: View<'_>, admit: Admit<'_>) -> Result<Registered, Failure> {
    let context = &admit.env.context;
    let mut sorted: Vec<&super::Member> = view
        .members
        .iter()
        .filter(|m| m.id != view.input.id)
        .collect();
    sorted.sort_by(|a, b| a.id.as_bytes().cmp(b.id.as_bytes()));
    let ids: Vec<&str> = sorted.iter().map(|m| m.id.as_str()).collect();
    if !in_order(&ids, &view.input.id) {
        return Err(internal());
    }
    let peek_of = |m: &super::Member| {
        let meter = Meter::for_input(&context.limits, Arc::new(Unbounded), Reservation::Input);
        peek(
            &RawStr::from(m.id.as_str()),
            Data::Bytes(&m.data),
            context,
            &meter,
            admit.hooks,
        )
    };
    let mut peeked = Peeked {
        ids: vec![view.input.id.as_str()],
        bits: vec![peek_of(view.input)?],
    };
    for m in &sorted {
        match peek_of(m) {
            Ok(b) => {
                peeked.ids.push(&m.id);
                peeked.bits.push(b);
            }
            Err(Failure::Input(r)) => return Err(dependency_failed(&m.id, Class::Input(r))),
            Err(f) => return Err(f),
        }
    }
    let names: Vec<RawStr> = sorted.iter().map(|m| RawStr::from(m.id.as_str())).collect();
    let staged: Vec<Staged<'_>> = sorted
        .iter()
        .zip(&names)
        .map(|(m, name)| Staged {
            id: &m.id,
            name,
            data: Data::Bytes(&m.data),
        })
        .collect();
    let x_name = RawStr::from(view.input.id.as_str());
    let input = Staged {
        id: &view.input.id,
        name: &x_name,
        data: Data::Bytes(&view.input.data),
    };
    register(input, &staged, &peeked, admit, &mut |_, _| Ok(()))
}
