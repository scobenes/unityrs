//! Canonical private decision frames. No manifest strings or spool paths.
use crate::{
    phase::{Buffer, Error},
    records::read::Candidate,
};
use std::sync::Arc;
use unityrs_core::limits::Meter;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Rule {
    FirstInput,
    PreferMd5,
    PreferInput,
    Unit,
    UnitConflict,
    Withheld,
}
impl Rule {
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::FirstInput => "first_input",
            Self::PreferMd5 => "prefer_md5",
            Self::PreferInput => "prefer_input",
            Self::Unit => "unit",
            Self::UnitConflict => "unit_conflict",
            Self::Withheld => "withheld",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum State {
    Publish,
    Withheld,
    UnitConflict,
    UnitLost,
    Prefix,
    Archive,
    Incomplete,
}
impl State {
    #[must_use]
    pub fn reason(self) -> Option<&'static str> {
        match self {
            Self::Publish => None,
            Self::Withheld => Some("withheld"),
            Self::UnitConflict => Some("unit_conflict"),
            Self::UnitLost => Some("unit_lost"),
            Self::Prefix => Some("unwritable_key(prefix_conflict)"),
            Self::Archive => Some("unwritable_key(archive_conflict)"),
            Self::Incomplete => Some("publish_incomplete"),
        }
    }
}
/// Borrowed view; source coordinates address an immutable committed output.
#[derive(Clone, Copy, Debug)]
pub struct Row<'a> {
    pub key: &'a str,
    pub unit: &'a str,
    pub input: &'a str,
    pub serialized_file: &'a str,
    pub source: u32,
    pub plan_index: u64,
    pub path_id: i64,
    pub size: u64,
    pub crc32: u32,
    pub md5: [u8; 16],
    pub visible_hash: Option<[u8; 32]>,
    pub etag: Option<([u8; 16], u64)>,
    pub kind: u8,
    pub mode: u8,
    pub cut: bool,
    pub rule: Rule,
    pub state: State,
    pub flip: bool,
    pub count: u64,
    pub archive: bool,
}
impl<'a> Row<'a> {
    #[must_use]
    pub fn marker(key: &'a str) -> Self {
        Self {
            key,
            unit: "",
            input: "",
            serialized_file: "",
            source: 0,
            plan_index: 0,
            path_id: 0,
            size: 0,
            crc32: 0,
            md5: [0; 16],
            visible_hash: None,
            etag: None,
            kind: 0,
            mode: 0,
            cut: false,
            rule: Rule::FirstInput,
            state: State::Publish,
            flip: false,
            count: 0,
            archive: false,
        }
    }
    pub(super) fn candidate(c: &Candidate<'a>, source: u32) -> Self {
        use crate::records::read::Kind;
        Self {
            key: c.key,
            unit: c.unit.unwrap_or_default(),
            input: c.input,
            serialized_file: c.serialized_file,
            source,
            plan_index: c.plan_index,
            path_id: c.path_id,
            size: c.size,
            crc32: c.crc32,
            md5: c.md5,
            visible_hash: c.visible_hash,
            etag: c.etag,
            kind: match c.kind {
                Kind::Texture => 0,
                Kind::Sprite => 1,
                Kind::AtlasJson => 2,
                Kind::AtlasSprite => 3,
                Kind::Atlas => 4,
            },
            mode: match c.mode {
                None => 0,
                Some("L") => 1,
                Some("LA") => 2,
                Some("RGB") => 3,
                _ => 4,
            },
            cut: c.cut,
            ..Self::marker(c.key)
        }
    }
}
pub(super) struct Codec {
    bytes: Buffer<u8>,
}
impl Codec {
    pub fn new(meter: &Arc<Meter>) -> Result<Self, Error> {
        Ok(Self {
            bytes: Buffer::new(meter, crate::sort::format::MAX_PAYLOAD)?,
        })
    }
    pub fn encode(&mut self, r: Row<'_>) -> Result<&[u8], Error> {
        self.bytes.clear();
        // Fixed-width numbers; optional fields have explicit, canonical tags.
        self.bytes.extend(&[
            r.rule as u8,
            r.state as u8,
            u8::from(r.flip),
            u8::from(r.archive),
            r.kind,
            r.mode,
            u8::from(r.cut),
        ])?;
        self.bytes.extend(&r.source.to_le_bytes())?;
        for n in [r.plan_index, r.size, r.count] {
            self.bytes.extend(&n.to_le_bytes())?;
        }
        self.bytes.extend(&r.path_id.to_le_bytes())?;
        self.bytes.extend(&r.crc32.to_le_bytes())?;
        self.bytes.extend(&r.md5)?;
        self.bytes.push(u8::from(r.visible_hash.is_some()))?;
        if let Some(hash) = r.visible_hash {
            self.bytes.extend(&hash)?;
        }
        self.bytes.push(u8::from(r.etag.is_some()))?;
        if let Some((hash, parts)) = r.etag {
            self.bytes.extend(&hash)?;
            self.bytes.extend(&parts.to_le_bytes())?;
        }
        for (text, limit) in [
            (r.key, 1024),
            (r.unit, 1024),
            (r.input, 1024),
            (r.serialized_file, 3072),
        ] {
            if text.len() > limit {
                return Err(Error::internal());
            }
            self.bytes.extend(
                &u16::try_from(text.len())
                    .map_err(|_| Error::internal())?
                    .to_le_bytes(),
            )?;
            self.bytes.extend(text.as_bytes())?;
        }
        Ok(&self.bytes)
    }
}
pub(super) struct Decode<'a>(pub &'a [u8]);
impl<'a> Decode<'a> {
    pub fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        let value = self.0.get(..n).ok_or_else(Error::internal)?;
        self.0 = self.0.get(n..).ok_or_else(Error::internal)?;
        Ok(value)
    }
    pub fn fixed<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        self.take(N)?.try_into().map_err(|_| Error::internal())
    }
    pub fn byte(&mut self) -> Result<u8, Error> {
        Ok(u8::from_le_bytes(self.fixed()?))
    }
    pub fn flag(&mut self) -> Result<bool, Error> {
        match self.byte()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(Error::internal()),
        }
    }
    pub fn text(&mut self, limit: usize) -> Result<&'a str, Error> {
        let n = usize::from(u16::from_le_bytes(self.fixed()?));
        if n > limit {
            return Err(Error::internal());
        }
        std::str::from_utf8(self.take(n)?).map_err(|_| Error::internal())
    }
}
pub fn decode(bytes: &[u8]) -> Result<Row<'_>, Error> {
    let mut d = Decode(bytes);
    let rule = match d.byte()? {
        0 => Rule::FirstInput,
        1 => Rule::PreferMd5,
        2 => Rule::PreferInput,
        3 => Rule::Unit,
        4 => Rule::UnitConflict,
        5 => Rule::Withheld,
        _ => return Err(Error::internal()),
    };
    let state = match d.byte()? {
        0 => State::Publish,
        1 => State::Withheld,
        2 => State::UnitConflict,
        3 => State::UnitLost,
        4 => State::Prefix,
        5 => State::Archive,
        6 => State::Incomplete,
        _ => return Err(Error::internal()),
    };
    let flip = d.flag()?;
    let archive = d.flag()?;
    let kind = d.byte()?;
    let mode = d.byte()?;
    let cut = d.flag()?;
    if kind > 4 || mode > 4 {
        return Err(Error::internal());
    }
    let source = u32::from_le_bytes(d.fixed()?);
    let plan_index = u64::from_le_bytes(d.fixed()?);
    let size = u64::from_le_bytes(d.fixed()?);
    let count = u64::from_le_bytes(d.fixed()?);
    let path_id = i64::from_le_bytes(d.fixed()?);
    let crc32 = u32::from_le_bytes(d.fixed()?);
    let md5 = d.fixed()?;
    let visible_hash = if d.flag()? { Some(d.fixed()?) } else { None };
    let etag = if d.flag()? {
        Some((d.fixed()?, u64::from_le_bytes(d.fixed()?)))
    } else {
        None
    };
    let key = d.text(1024)?;
    let unit = d.text(1024)?;
    let input = d.text(1024)?;
    let serialized_file = d.text(3072)?;
    if key.is_empty() || !d.0.is_empty() {
        return Err(Error::internal());
    }
    // Candidates and their decisions retain an input id; synthetic markers
    // have none. B4 writes pixels for every candidate kind except atlas JSON.
    if !input.is_empty() {
        let valid = match kind {
            0 | 1 | 3 => mode != 0 && visible_hash.is_some(),
            2 => mode == 0 && visible_hash.is_none(),
            _ => false,
        };
        if !valid {
            return Err(Error::internal());
        }
    }
    Ok(Row {
        key,
        unit,
        input,
        serialized_file,
        source,
        plan_index,
        path_id,
        size,
        crc32,
        md5,
        visible_hash,
        etag,
        kind,
        mode,
        cut,
        rule,
        state,
        flip,
        count,
        archive,
    })
}
