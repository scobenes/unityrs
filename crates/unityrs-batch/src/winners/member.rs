//! Private member projection and bounded typed B8 list reader. Strings borrow
//! one charged frame. Numeric locators never contain a spool path.
use super::codec::Decode;
use crate::{
    phase::{Buffer, Error},
    reason::{KeyRule, LeftOut},
    sort::{self, Order},
};
use std::{cmp::Ordering, sync::Arc};
use unityrs_core::limits::Meter;

#[derive(Clone, Copy, Debug)]
pub struct Member<'a> {
    pub archive: &'a str,
    pub index: u32,
    pub path: &'a str,
    pub key: &'a str,
    pub input: &'a str,
    pub md5: [u8; 16],
    pub reason: Option<LeftOut>,
    pub cut: bool,
    pub source: u32,
    pub plan_index: u64,
    pub size: u64,
    pub crc32: u32,
}
pub(super) struct Codec(Buffer<u8>);
impl Codec {
    pub fn new(meter: &Arc<Meter>) -> Result<Self, Error> {
        Ok(Self(Buffer::new(meter, sort::format::MAX_PAYLOAD)?))
    }
    pub fn encode(&mut self, m: Member<'_>) -> Result<&[u8], Error> {
        self.0.clear();
        let reason = match m.reason {
            None => 0,
            Some(LeftOut::DuplicateMember) => 1,
            Some(LeftOut::PrefixConflict) => 2,
            Some(LeftOut::UnwritablePath(rule)) => {
                u8::try_from(
                    KeyRule::ALL
                        .iter()
                        .position(|&r| r == rule)
                        .ok_or_else(Error::internal)?,
                )
                .map_err(|_| Error::internal())?
                    + 3
            }
        };
        self.0.extend(&[reason, u8::from(m.cut)])?;
        self.0.extend(&m.index.to_le_bytes())?;
        self.0.extend(&m.source.to_le_bytes())?;
        self.0.extend(&m.plan_index.to_le_bytes())?;
        self.0.extend(&m.size.to_le_bytes())?;
        self.0.extend(&m.crc32.to_le_bytes())?;
        self.0.extend(&m.md5)?;
        for (text, limit) in [
            (m.archive, 1024),
            (m.path, 3072),
            (m.key, 1024),
            (m.input, 1024),
        ] {
            if text.len() > limit {
                return Err(Error::internal());
            }
            self.0.extend(
                &u16::try_from(text.len())
                    .map_err(|_| Error::internal())?
                    .to_le_bytes(),
            )?;
            self.0.extend(text.as_bytes())?;
        }
        Ok(&self.0)
    }
}
pub(super) fn decode(bytes: &[u8]) -> Result<Member<'_>, Error> {
    let mut d = Decode(bytes);
    let reason = match d.byte()? {
        0 => None,
        1 => Some(LeftOut::DuplicateMember),
        2 => Some(LeftOut::PrefixConflict),
        n => Some(LeftOut::UnwritablePath(
            *KeyRule::ALL
                .get(usize::from(n - 3))
                .ok_or_else(Error::internal)?,
        )),
    };
    let cut = d.flag()?;
    let index = u32::from_le_bytes(d.fixed()?);
    let source = u32::from_le_bytes(d.fixed()?);
    let plan_index = u64::from_le_bytes(d.fixed()?);
    let size = u64::from_le_bytes(d.fixed()?);
    let crc32 = u32::from_le_bytes(d.fixed()?);
    let md5 = d.fixed()?;
    let archive = d.text(1024)?;
    let path = d.text(3072)?;
    let key = d.text(1024)?;
    let input = d.text(1024)?;
    if archive.is_empty() || key.is_empty() || input.is_empty() || !d.0.is_empty() {
        return Err(Error::internal());
    }
    if !matches!(reason, Some(LeftOut::UnwritablePath(_))) {
        unityrs_export::keys::check_member(path.as_bytes()).map_err(|_| Error::internal())?;
    }
    Ok(Member {
        archive,
        index,
        path,
        key,
        input,
        md5,
        reason,
        cut,
        source,
        plan_index,
        size,
        crc32,
    })
}
#[derive(Clone, Copy)]
pub(super) enum By {
    Identity,
    Select,
    Display,
    List,
}
impl Order for By {
    fn tag(&self) -> u32 {
        match self {
            Self::Identity => 111,
            Self::Select => 112,
            Self::Display => 113,
            Self::List => 114,
        }
    }
    fn validate(&self, bytes: &[u8]) -> Result<(), Error> {
        let row = decode(bytes)?;
        if matches!(self, Self::List) && row.reason.is_some() {
            return Err(Error::internal());
        }
        Ok(())
    }
    fn compare(&self, a: &[u8], b: &[u8]) -> Ordering {
        let (Ok(a), Ok(b)) = (decode(a), decode(b)) else {
            return a.cmp(b);
        };
        if matches!(self, Self::Identity) {
            return a.key.cmp(b.key).then(a.input.cmp(b.input));
        }
        a.archive
            .cmp(b.archive)
            .then_with(|| {
                if matches!(self, Self::Select) {
                    a.reason.is_some().cmp(&b.reason.is_some())
                } else {
                    Ordering::Equal
                }
            })
            .then_with(|| sort::slash_first(a.path.as_bytes(), b.path.as_bytes()))
            .then(a.key.cmp(b.key))
            .then(a.input.cmp(b.input))
    }
}
/// B8 consumes only structurally included members, in member order.
pub struct Reader<'a> {
    frames: sort::Reader<'a>,
    previous: Buffer<u8>,
    failed: bool,
}
impl Reader<'_> {
    pub fn open(
        workspace: &crate::spool::workspace::Workspace,
        file: &crate::spool::workspace::Sealed,
        meter: &Arc<Meter>,
        stopped: &dyn Fn() -> bool,
    ) -> Result<Self, Error> {
        Ok(Self {
            frames: sort::Reader::open(workspace, file, &By::List, meter, stopped)?,
            previous: Buffer::new(meter, unityrs_export::keys::KEY_MAX)?,
            failed: false,
        })
    }
    pub fn next(&mut self, stopped: &dyn Fn() -> bool) -> Result<Option<Member<'_>>, Error> {
        crate::phase::check(stopped)?;
        if self.failed {
            return Err(Error::internal());
        }
        self.failed = true;
        let row = self.frames.next(stopped)?.map(decode).transpose()?;
        if let Some(m) = &row {
            let path = m.path.as_bytes();
            // Slash-first order keeps a path beside its first descendant.
            // A list contains one archive, so one preceding path suffices.
            if path == &*self.previous
                || path
                    .strip_prefix(&*self.previous)
                    .is_some_and(|rest| rest.starts_with(b"/"))
            {
                return Err(Error::internal());
            }
            self.previous.clear();
            self.previous.extend(path)?;
        }
        self.failed = false;
        Ok(row)
    }
}
