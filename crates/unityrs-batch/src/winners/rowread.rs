//! B7's bounded public-base-row reader. No hidden fields, JSON tree or sort.
//! A reader checks its selected schema, complete line and canonical order.
use super::codec::Decode;
use crate::{
    phase::{Buffer, Error, check},
    records::{LINE, flat::Fields, read::hex},
    sort,
    spool::workspace::{Sealed, Workspace},
};
use std::{
    cmp::Ordering,
    io::{self, Read},
    sync::Arc,
};
use unityrs_core::limits::Meter;

#[derive(Clone, Copy, Debug)]
pub enum File {
    Outputs,
    Collisions,
    Archives,
    Members,
}
#[derive(Debug)]
pub struct Output<'a> {
    pub key: &'a str,
    pub kind: &'a str,
    pub unit: Option<&'a str>,
    pub input: &'a str,
    pub serialized_file: &'a str,
    pub path_id: i64,
    pub md5: [u8; 16],
    pub visible_hash: Option<[u8; 32]>,
    pub mode: Option<&'a str>,
    pub size: u64,
    pub status: &'a str,
    pub reason: Option<&'a str>,
    pub cut: bool,
}
#[derive(Debug)]
#[allow(clippy::struct_excessive_bools)] // The public collision schema's four independent booleans.
pub struct Collision<'a> {
    pub key: &'a str,
    pub input: &'a str,
    pub serialized_file: &'a str,
    pub md5: [u8; 16],
    pub visible_hash: Option<[u8; 32]>,
    pub winner: bool,
    pub rule: &'a str,
    pub flip: bool,
    pub pixels_differ: bool,
    pub cut: bool,
}
#[derive(Debug)]
pub struct Archive<'a> {
    pub key: &'a str,
    pub status: &'a str,
    pub reason: Option<&'a str>,
    pub members: u64,
    pub left_out: u64,
}
#[derive(Debug)]
pub struct Member<'a> {
    pub archive: &'a str,
    pub path: &'a str,
    pub key: &'a str,
    pub input: &'a str,
    pub md5: [u8; 16],
    pub included: bool,
    pub reason: Option<&'a str>,
    pub cut: bool,
}
#[derive(Debug)]
pub enum Row<'a> {
    Output(Output<'a>),
    Collision(Collision<'a>),
    Archive(Archive<'a>),
    Member(Member<'a>),
}
impl Row<'_> {
    fn keys(&self) -> [&str; 4] {
        match self {
            Self::Output(r) => [r.key, "", "", ""],
            Self::Collision(r) => [r.key, r.input, "", ""],
            Self::Archive(r) => [r.key, "", "", ""],
            Self::Member(r) => [r.archive, r.path, r.key, r.input],
        }
    }
}
fn bounded<'a>(f: &Fields, b: &'a [u8], name: &str, limit: usize) -> Result<&'a str, Error> {
    let text = f.text(b, name)?;
    if text.len() > limit {
        return Err(Error::internal());
    }
    Ok(text)
}
#[allow(clippy::too_many_lines)] // Four closed schemas share the same allocation-free field decoder.
fn decode(file: File, b: &mut [u8]) -> Result<Row<'_>, Error> {
    let f = Fields::parse(b)?;
    let key = bounded(&f, b, "key", 1024)?;
    if key.is_empty() {
        return Err(Error::internal());
    }
    match file {
        File::Outputs => {
            f.shape(&[
                "key",
                "kind",
                "unit",
                "input",
                "serialized_file",
                "path_id",
                "md5",
                "visible_hash",
                "mode",
                "size",
                "status",
                "reason",
                "cut",
            ])?;
            let kind = f.text(b, "kind")?;
            if !matches!(kind, "texture" | "sprite" | "atlas_json" | "atlas_sprite") {
                return Err(Error::internal());
            }
            let unit = f.optional_text(b, "unit")?;
            if unit.is_some_and(|u| u.is_empty() || u.len() > 1024) {
                return Err(Error::internal());
            }
            let visible_hash = f.optional_text(b, "visible_hash")?.map(hex).transpose()?;
            let mode = f.optional_text(b, "mode")?;
            if (kind == "atlas_json") != mode.is_none()
                || mode.is_some() != visible_hash.is_some()
                || mode.is_some_and(|m| !matches!(m, "L" | "LA" | "RGB" | "RGBA"))
            {
                return Err(Error::internal());
            }
            let status = f.text(b, "status")?;
            let reason = f.optional_text(b, "reason")?;
            if !matches!(
                (status, reason),
                ("not_written", None)
                    | (
                        "withheld",
                        Some("withheld" | "unit_conflict" | "unit_lost" | "publish_incomplete")
                    )
                    | (
                        "unwritable_key",
                        Some(
                            "unwritable_key(prefix_conflict)" | "unwritable_key(archive_conflict)"
                        )
                    )
            ) {
                return Err(Error::internal());
            }
            Ok(Row::Output(Output {
                key,
                kind,
                unit,
                input: bounded(&f, b, "input", 1024)?,
                serialized_file: bounded(&f, b, "serialized_file", 3072)?,
                path_id: f.number(b, "path_id")?,
                md5: hex(f.text(b, "md5")?)?,
                visible_hash,
                mode,
                size: f.number(b, "size")?,
                status,
                reason,
                cut: f.boolean("cut")?,
            }))
        }
        File::Collisions => {
            f.shape(&[
                "key",
                "input",
                "serialized_file",
                "md5",
                "visible_hash",
                "winner",
                "rule",
                "flip",
                "pixels_differ",
                "cut",
            ])?;
            let rule = f.text(b, "rule")?;
            if !matches!(
                rule,
                "first_input"
                    | "prefer_input"
                    | "prefer_md5"
                    | "unit"
                    | "unit_conflict"
                    | "withheld"
            ) {
                return Err(Error::internal());
            }
            let winner = f.boolean("winner")?;
            let pixels_differ = f.boolean("pixels_differ")?;
            if winner && pixels_differ {
                return Err(Error::internal());
            }
            Ok(Row::Collision(Collision {
                key,
                input: bounded(&f, b, "input", 1024)?,
                serialized_file: bounded(&f, b, "serialized_file", 3072)?,
                md5: hex(f.text(b, "md5")?)?,
                visible_hash: f.optional_text(b, "visible_hash")?.map(hex).transpose()?,
                winner,
                rule,
                flip: f.boolean("flip")?,
                pixels_differ,
                cut: f.boolean("cut")?,
            }))
        }
        File::Archives => {
            f.shape(&[
                "key", "status", "reason", "members", "left_out", "md5", "etag", "size",
            ])?;
            let status = f.text(b, "status")?;
            let reason = f.optional_text(b, "reason")?;
            let members = f.number(b, "members")?;
            if !matches!(
                (status, reason),
                ("empty" | "not_written", None) | ("withheld", Some("publish_incomplete"))
            ) || (status == "empty") != (members == 0)
                || f.optional_text(b, "md5")?.is_some()
                || f.optional_text(b, "etag")?.is_some()
                || f.optional_number::<u64>(b, "size")?.is_some()
            {
                return Err(Error::internal());
            }
            Ok(Row::Archive(Archive {
                key,
                status,
                reason,
                members,
                left_out: f.number(b, "left_out")?,
            }))
        }
        File::Members => {
            f.shape(&[
                "archive", "path", "key", "input", "md5", "included", "reason", "cut",
            ])?;
            let included = f.boolean("included")?;
            let reason = f.optional_text(b, "reason")?;
            if included != reason.is_none() || reason.is_some_and(|r| !valid_left_out(r)) {
                return Err(Error::internal());
            }
            let path = bounded(&f, b, "path", 3072)?;
            if reason.is_none_or(|r| !r.starts_with("unwritable_path(")) {
                unityrs_export::keys::check_member(path.as_bytes())
                    .map_err(|_| Error::internal())?;
            }
            Ok(Row::Member(Member {
                archive: bounded(&f, b, "archive", 1024)?,
                path,
                key,
                input: bounded(&f, b, "input", 1024)?,
                md5: hex(f.text(b, "md5")?)?,
                included,
                reason,
                cut: f.boolean("cut")?,
            }))
        }
    }
}
fn valid_left_out(reason: &str) -> bool {
    matches!(reason, "duplicate_member" | "prefix_conflict")
        || reason
            .strip_prefix("unwritable_path(")
            .and_then(|r| r.strip_suffix(')'))
            .is_some_and(|r| {
                crate::reason::KeyRule::ALL
                    .iter()
                    .take(11)
                    .any(|k| k.name() == r)
            })
}
pub struct Reader {
    read: Option<Box<dyn Read + Send>>,
    remaining: u64,
    input: Buffer<u8>,
    position: usize,
    available: usize,
    line: Buffer<u8>,
    previous: Buffer<u8>,
    file: File,
    failed: bool,
}
impl Reader {
    pub fn open(
        workspace: &Workspace,
        artifact: Option<&Sealed>,
        file: File,
        meter: &Arc<Meter>,
        stopped: &dyn Fn() -> bool,
    ) -> Result<Self, Error> {
        let mut input = Buffer::new(meter, 64 * 1024)?;
        input.fill(0);
        Ok(Self {
            read: artifact.map(|a| workspace.open(a, stopped)).transpose()?,
            remaining: artifact.map_or(0, Sealed::length),
            input,
            position: 0,
            available: 0,
            line: Buffer::new(meter, LINE)?,
            previous: Buffer::new(meter, 8 * 1024)?,
            file,
            failed: false,
        })
    }
    pub fn next(&mut self, stopped: &dyn Fn() -> bool) -> Result<Option<Row<'_>>, Error> {
        check(stopped)?;
        if self.failed {
            return Err(Error::internal());
        }
        if self.remaining == 0 {
            return Ok(None);
        }
        self.failed = true;
        self.line.clear();
        loop {
            check(stopped)?;
            if self.position == self.available {
                self.available = loop {
                    match self
                        .read
                        .as_mut()
                        .ok_or_else(Error::internal)?
                        .read(&mut self.input)
                    {
                        Ok(0) => return Err(Error::internal()),
                        Ok(n) => break n,
                        Err(e) if e.kind() == io::ErrorKind::Interrupted => check(stopped)?,
                        Err(e) => return Err(e.into()),
                    }
                };
                self.position = 0;
            }
            let bytes = self
                .input
                .get(self.position..self.available)
                .ok_or_else(Error::internal)?;
            let end = bytes.iter().position(|&b| b == b'\n');
            let n = end.map_or(bytes.len(), |n| n + 1);
            self.line
                .extend(bytes.get(..n).ok_or_else(Error::internal)?)?;
            self.remaining = self
                .remaining
                .checked_sub(u64::try_from(n).map_err(|_| Error::internal())?)
                .ok_or_else(Error::internal)?;
            self.position += n;
            if end.is_some() {
                break;
            }
            if self.remaining == 0 {
                return Err(Error::internal());
            }
        }
        let row = decode(self.file, &mut self.line)?;
        let keys = row.keys();
        if !self.previous.is_empty() {
            let mut d = Decode(&self.previous);
            let mut ordering = Ordering::Equal;
            for (i, key) in keys.iter().enumerate() {
                let previous = d.text(3072)?;
                ordering = ordering.then_with(|| {
                    if i == 1 && matches!(self.file, File::Members) {
                        sort::slash_first(previous.as_bytes(), key.as_bytes())
                    } else {
                        previous.cmp(key)
                    }
                });
            }
            if ordering.is_gt() || (ordering.is_eq() && !matches!(self.file, File::Members)) {
                return Err(Error::internal());
            }
        }
        self.previous.clear();
        for key in keys {
            self.previous.extend(
                &u16::try_from(key.len())
                    .map_err(|_| Error::internal())?
                    .to_le_bytes(),
            )?;
            self.previous.extend(key.as_bytes())?;
        }
        self.failed = false;
        Ok(Some(row))
    }
}
