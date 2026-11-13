//! Bounded committed-artifact ingestion. Corruption is `internal`, I/O is
//! `host_io`, and there is no input culprit. Drive through summary/EOF.
//! Member identities refer to the immediately preceding candidate, as emitted
//! by copy-out. Duplicate candidates and unit-root checks belong to the joins.
use super::{BUFFER, Figures, LINE, Status, flat::Fields};
use crate::{
    phase::{Buffer, Error, check},
    reason::{KeyRule, LeftOut},
};
use std::{
    io::{self, Read},
    sync::Arc,
};
use unityrs_core::limits::Meter;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Texture,
    Sprite,
    AtlasJson,
    AtlasSprite,
    Atlas,
}
fn kind(text: &str) -> Result<Kind, Error> {
    match text {
        "texture" => Ok(Kind::Texture),
        "sprite" => Ok(Kind::Sprite),
        "atlas_json" => Ok(Kind::AtlasJson),
        "atlas_sprite" => Ok(Kind::AtlasSprite),
        "atlas" => Ok(Kind::Atlas),
        _ => Err(Error::internal()),
    }
}
#[derive(Debug)]
pub struct Candidate<'a> {
    pub key: &'a str,
    pub unit: Option<&'a str>,
    pub input: &'a str,
    pub serialized_file: &'a str,
    pub kind: Kind,
    pub path_id: i64,
    pub md5: [u8; 16],
    pub crc32: u32,
    pub visible_hash: Option<[u8; 32]>,
    pub mode: Option<&'a str>,
    pub size: u64,
    pub etag: Option<([u8; 16], u64)>,
    pub plan_index: u64,
    pub cut: bool,
}
#[derive(Debug)]
pub struct Member<'a> {
    pub archive: u64,
    pub path: &'a str,
    pub key: &'a str,
    pub input: &'a str,
    pub md5: [u8; 16],
    pub reason: Option<LeftOut>,
    pub cut: bool,
}
#[derive(Debug)]
pub struct Skip<'a> {
    pub input: &'a str,
    pub serialized_file: &'a str,
    pub path_id: i64,
    pub class: &'a str,
    pub output: Kind,
    pub sprite: Option<u64>,
    pub reason: &'a str,
    pub detail: Option<&'a str>,
    pub cut: bool,
}
#[derive(Debug)]
pub struct Diagnostic<'a> {
    pub kind: super::DiagnosticKind,
    pub input: &'a str,
    pub bundle: Option<&'a str>,
    pub name: &'a str,
    pub cut: bool,
}
#[derive(Debug)]
pub struct Summary<'a> {
    pub id: &'a str,
    pub status: Status,
    pub reason: Option<&'a str>,
    pub detail: Option<&'a str>,
    pub figures: Option<Figures>,
    pub spooled_part: u64,
    pub claimed_bytes: u64,
    pub cut: bool,
}
#[derive(Debug)]
pub enum Row<'a> {
    Candidate(Candidate<'a>),
    Member(Member<'a>),
    Skip(Skip<'a>),
    Diagnostic(Diagnostic<'a>),
    Summary(Summary<'a>),
}

pub struct Reader<'a, R> {
    read: R,
    expected: &'a str,
    failed_input: bool,
    archives: u64,
    input: Buffer<u8>,
    position: usize,
    available: usize,
    line: Buffer<u8>,
    last_key: Buffer<u8>,
    last_md5: Option<[u8; 16]>,
    skips: u64,
    rows: u64,
    ended: bool,
    failed: bool,
}
impl<'a, R: Read> Reader<'a, R> {
    pub fn new(
        read: R,
        expected: &'a str,
        failed_input: bool,
        archives: u64,
        meter: &Arc<Meter>,
    ) -> Result<Self, Error> {
        let mut input = Buffer::new(meter, BUFFER)?;
        input.fill(0);
        Ok(Self {
            read,
            expected,
            failed_input,
            archives,
            input,
            position: 0,
            available: 0,
            line: Buffer::new(meter, LINE)?,
            last_key: Buffer::new(meter, 1024)?,
            last_md5: None,
            skips: 0,
            rows: 0,
            ended: false,
            failed: false,
        })
    }
    /// Reuse the fixed charged parser storage after validated summary/EOF.
    /// The caller drives `next` to `None` before changing committed inputs.
    pub(crate) fn reopen(
        &mut self,
        read: R,
        expected: &'a str,
        failed_input: bool,
    ) -> Result<(), Error> {
        if !self.ended || self.failed || self.position != self.available {
            return Err(Error::internal());
        }
        self.read = read;
        self.expected = expected;
        self.failed_input = failed_input;
        self.position = 0;
        self.available = 0;
        self.line.clear();
        self.last_key.clear();
        self.last_md5 = None;
        self.skips = 0;
        self.rows = 0;
        self.ended = false;
        Ok(())
    }
    fn refill(&mut self, stopped: &dyn Fn() -> bool) -> Result<bool, Error> {
        check(stopped)?;
        if self.position < self.available {
            return Ok(true);
        }
        loop {
            check(stopped)?;
            match self.read.read(&mut self.input) {
                Ok(n) => {
                    self.position = 0;
                    self.available = n;
                    return Ok(n != 0);
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e.into()),
            }
        }
    }
    fn line(&mut self, stopped: &dyn Fn() -> bool) -> Result<(), Error> {
        self.line.clear();
        loop {
            if !self.refill(stopped)? {
                return Err(Error::internal());
            }
            let part = self
                .input
                .get(self.position..self.available)
                .ok_or_else(Error::internal)?;
            let end = part.iter().position(|&b| b == b'\n');
            let n = end.map_or(part.len(), |n| n + 1);
            self.line
                .extend(part.get(..n).ok_or_else(Error::internal)?)?;
            self.position += n;
            if end.is_some() {
                return Ok(());
            }
        }
    }
    pub fn next(&mut self, stopped: &dyn Fn() -> bool) -> Result<Option<Row<'_>>, Error> {
        check(stopped)?;
        if self.failed {
            return Err(Error::internal());
        }
        if self.ended {
            return Ok(None);
        }
        self.failed = true;
        self.line(stopped)?;
        let fields = Fields::parse(&mut self.line)?;
        self.rows = self.rows.checked_add(1).ok_or_else(Error::internal)?;
        let tag = fields.text(&self.line, "type")?;
        let id = fields.text(
            &self.line,
            if tag == "input_summary" {
                "id"
            } else {
                "input"
            },
        )?;
        if id != self.expected {
            return Err(Error::internal());
        }
        if tag == "input_summary" {
            if self.refill(stopped)? {
                return Err(Error::internal());
            }
            self.ended = true;
        } else if self.failed_input {
            return Err(Error::internal());
        }
        let row = decode(&fields, &self.line)?;
        match &row {
            Row::Candidate(c) => {
                self.last_key.clear();
                self.last_key.extend(c.key.as_bytes())?;
                self.last_md5 = Some(c.md5);
            }
            Row::Member(m) => {
                if m.archive >= self.archives
                    || self.last_key.as_ref() != m.key.as_bytes()
                    || self.last_md5 != Some(m.md5)
                {
                    return Err(Error::internal());
                }
            }
            Row::Skip(_) => {
                self.skips = self.skips.checked_add(1).ok_or_else(Error::internal)?;
                self.last_md5 = None;
            }
            Row::Diagnostic(_) => self.last_md5 = None,
            Row::Summary(s) => {
                if (s.status == Status::Failed) != self.failed_input
                    || (self.failed_input && self.rows != 1)
                {
                    return Err(Error::internal());
                }
                let figures = s.figures.unwrap_or_default();
                if figures.skips != self.skips
                    || (s.status == Status::CompleteWithSkips) != (self.skips > 0)
                    || (self.failed_input && figures.unresolved_scripts != 0)
                {
                    return Err(Error::internal());
                }
            }
        }
        self.failed = false;
        Ok(Some(row))
    }
}

pub(crate) fn hex<const N: usize>(text: &str) -> Result<[u8; N], Error> {
    if text.len() != N * 2 {
        return Err(Error::internal());
    }
    let mut bytes = [0; N];
    for (slot, pair) in bytes
        .iter_mut()
        .zip(text.as_bytes().as_chunks::<2>().0.iter())
    {
        let mut value = 0;
        for &b in pair {
            value = value * 16
                + match b {
                    b'0'..=b'9' => b - b'0',
                    b'a'..=b'f' => b - b'a' + 10,
                    _ => return Err(Error::internal()),
                };
        }
        *slot = value;
    }
    Ok(bytes)
}
fn ordinal(text: &str, prefix: &str) -> Result<u64, Error> {
    let digits = text.strip_prefix(prefix).ok_or_else(Error::internal)?;
    if digits.is_empty()
        || (digits.len() > 1 && digits.starts_with('0'))
        || !digits.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(Error::internal());
    }
    digits.parse().map_err(|_| Error::internal())
}
fn key(text: &str) -> Result<&str, Error> {
    if text.is_empty() || text.len() > 1024 || text.bytes().any(|b| b < b' ' || b == 127) {
        return Err(Error::internal());
    }
    Ok(text)
}
fn candidate<'a>(f: &Fields, b: &'a [u8]) -> Result<Candidate<'a>, Error> {
    f.shape(&[
        "type",
        "key",
        "unit",
        "input",
        "serialized_file",
        "kind",
        "path_id",
        "md5",
        "crc32",
        "visible_hash",
        "mode",
        "size",
        "etag",
        "file",
        "cut",
    ])?;
    let kind = kind(f.text(b, "kind")?)?;
    if kind == Kind::Atlas {
        return Err(Error::internal());
    }
    let visible_hash = f.optional_text(b, "visible_hash")?.map(hex).transpose()?;
    let mode = f.optional_text(b, "mode")?;
    if mode.is_some() != visible_hash.is_some()
        || mode.is_some_and(|m| !matches!(m, "L" | "LA" | "RGB" | "RGBA"))
    {
        return Err(Error::internal());
    }
    let size: u64 = f.number(b, "size")?;
    let etag = f
        .optional_text(b, "etag")?
        .map(|s| {
            let (hash, parts) = s.split_once('-').ok_or_else(Error::internal)?;
            let parts = ordinal(parts, "")?;
            if size <= 16 * 1024 * 1024 || parts != (size - 1) / (16 * 1024 * 1024) + 1 {
                return Err(Error::internal());
            }
            Ok((hex(hash)?, parts))
        })
        .transpose()?;
    if (size > 16 * 1024 * 1024) != etag.is_some() {
        return Err(Error::internal());
    }
    Ok(Candidate {
        key: key(f.text(b, "key")?)?,
        unit: f.optional_text(b, "unit")?.map(key).transpose()?,
        input: f.text(b, "input")?,
        serialized_file: f.text(b, "serialized_file")?,
        kind,
        path_id: f.number(b, "path_id")?,
        md5: hex(f.text(b, "md5")?)?,
        crc32: f.number(b, "crc32")?,
        visible_hash,
        mode,
        size,
        etag,
        plan_index: ordinal(f.text(b, "file")?, "o")?,
        cut: f.boolean("cut")?,
    })
}
fn member<'a>(f: &Fields, b: &'a [u8]) -> Result<Member<'a>, Error> {
    f.shape(&[
        "type", "archive", "path", "key", "input", "md5", "reason", "cut",
    ])?;
    let reason = f
        .optional_text(b, "reason")?
        .map(|s| {
            let rule = s
                .strip_prefix("unwritable_path(")
                .and_then(|s| s.strip_suffix(')'))
                .ok_or_else(Error::internal)?;
            KeyRule::ALL
                .into_iter()
                .take(11)
                .find(|r| r.name() == rule)
                .map(LeftOut::UnwritablePath)
                .ok_or_else(Error::internal)
        })
        .transpose()?;
    Ok(Member {
        archive: f.number(b, "archive")?,
        path: f.text(b, "path")?,
        key: key(f.text(b, "key")?)?,
        input: f.text(b, "input")?,
        md5: hex(f.text(b, "md5")?)?,
        reason,
        cut: f.boolean("cut")?,
    })
}
fn summary<'a>(f: &Fields, b: &'a [u8]) -> Result<Summary<'a>, Error> {
    f.shape(&[
        "type",
        "id",
        "status",
        "reason",
        "detail",
        "input_peak",
        "dependency_peak",
        "evictions",
        "export_work",
        "decode_work_peak",
        "ms",
        "skips",
        "unresolved_scripts",
        "spooled_part",
        "claimed_bytes",
        "cut",
    ])?;
    let status = match f.text(b, "status")? {
        "complete" => Status::Complete,
        "complete_with_skips" => Status::CompleteWithSkips,
        "failed" => Status::Failed,
        _ => return Err(Error::internal()),
    };
    let reason = f.optional_text(b, "reason")?;
    if (status == Status::Failed) != reason.is_some() {
        return Err(Error::internal());
    }
    if reason.is_some_and(|s| !input_reason(s)) {
        return Err(Error::internal());
    }
    let input_peak = f.optional_number(b, "input_peak")?;
    let mut figures = Figures {
        input_peak: input_peak.unwrap_or(0),
        skips: f.number(b, "skips")?,
        unresolved_scripts: f.number(b, "unresolved_scripts")?,
        ..Figures::default()
    };
    for (slot, name) in [
        (&mut figures.dependency_peak, "dependency_peak"),
        (&mut figures.evictions, "evictions"),
        (&mut figures.export_work, "export_work"),
        (&mut figures.decode_work_peak, "decode_work_peak"),
        (&mut figures.ms, "ms"),
    ] {
        let n = f.optional_number(b, name)?;
        if n.is_some() != input_peak.is_some() {
            return Err(Error::internal());
        }
        *slot = n.unwrap_or(0);
    }
    if input_peak.is_none()
        && (status != Status::Failed || figures.skips != 0 || figures.unresolved_scripts != 0)
    {
        return Err(Error::internal());
    }
    Ok(Summary {
        id: f.text(b, "id")?,
        status,
        reason,
        detail: f.optional_text(b, "detail")?,
        figures: input_peak.map(|_| figures),
        spooled_part: f.number(b, "spooled_part")?,
        claimed_bytes: f.number(b, "claimed_bytes")?,
        cut: f.boolean("cut")?,
    })
}
fn decode<'a>(f: &Fields, b: &'a [u8]) -> Result<Row<'a>, Error> {
    match f.text(b, "type")? {
        "candidate" => candidate(f, b).map(Row::Candidate),
        "member" => member(f, b).map(Row::Member),
        "input_summary" => summary(f, b).map(Row::Summary),
        "skip" => {
            f.shape(&[
                "type",
                "input",
                "serialized_file",
                "path_id",
                "class",
                "output",
                "sprite",
                "reason",
                "detail",
                "cut",
            ])?;
            let class = f.text(b, "class")?;
            if class.len() > 64 || !class.bytes().all(|b| b.is_ascii_graphic()) {
                return Err(Error::internal());
            }
            if crate::attempt::classify::skip_code(f.text(b, "reason")?).is_none() {
                return Err(Error::internal());
            }
            Ok(Row::Skip(Skip {
                input: f.text(b, "input")?,
                serialized_file: f.text(b, "serialized_file")?,
                path_id: f.number(b, "path_id")?,
                class,
                output: kind(f.text(b, "output")?)?,
                sprite: f.optional_number(b, "sprite")?,
                reason: f.text(b, "reason")?,
                detail: f.optional_text(b, "detail")?,
                cut: f.boolean("cut")?,
            }))
        }
        "diagnostic" => {
            f.shape(&["type", "kind", "input", "bundle", "name", "cut"])?;
            let kind = match f.text(b, "kind")? {
                "same_named_cab" => super::DiagnosticKind::SameNamedCab,
                "undeclared_dependency" => super::DiagnosticKind::UndeclaredDependency,
                "nested_container" => super::DiagnosticKind::NestedContainer,
                _ => return Err(Error::internal()),
            };
            Ok(Row::Diagnostic(Diagnostic {
                kind,
                input: f.text(b, "input")?,
                bundle: f.optional_text(b, "bundle")?,
                name: f.text(b, "name")?,
                cut: f.boolean("cut")?,
            }))
        }
        _ => Err(Error::internal()),
    }
}

fn input_reason(s: &str) -> bool {
    fn plain(s: &str) -> bool {
        matches!(
            s,
            "download_failed"
                | "redirect_refused"
                | "source_missing"
                | "source_not_regular"
                | "source_changed"
                | "integrity_mismatch"
        ) || crate::attempt::classify::input(s).is_some()
    }
    if plain(s) {
        return true;
    }
    let Some(inner) = s
        .strip_prefix("dependency_failed(")
        .and_then(|s| s.strip_suffix(')'))
    else {
        return false;
    };
    // A cut id may contain comma-space. Try each split against the closed
    // inner vocabulary, without allocating or recursing on free text.
    inner
        .match_indices(", ")
        .any(|(i, _)| inner.get(i + 2..).is_some_and(plain))
}
