//! A bounded key-order merge of loose winners and archive descriptors.
use super::{Expected, Source, Target};
use crate::{
    outcomes::{Code, Outcomes},
    phase::{Buffer, Error},
    publish::{Owned, archive::Descriptor},
    sink::store::Store,
    store::etag::Etag,
    winners::{
        self,
        stream::{By, Cursor},
    },
};
use std::io::Write;

pub(super) struct Merge<'a> {
    owned: &'a Owned,
    store: &'a Store,
    loose: Cursor<'static>,
    archives: Option<winners::archives::Reader<'static>>,
    archive: Descriptor,
    has_archive: bool,
    index: u64,
    current: Option<(Target, Code, Etag, u64)>,
    key: Buffer<u8>,
}
impl<'a> Merge<'a> {
    pub(super) fn open(owned: &'a Owned, store: &'a Store) -> Result<Self, Error> {
        let stopped = || owned.control.stopped();
        let meter = owned.account.meter();
        let mut this = Self {
            owned,
            store,
            loose: Cursor::read(
                &owned.workspace,
                owned.handoff.winners.as_ref(),
                &By::Key,
                meter,
                &stopped,
                true,
            )?,
            archives: owned
                .handoff
                .metadata
                .as_ref()
                .map(|m| winners::archives::Reader::open(&owned.workspace, m, meter, &stopped))
                .transpose()?,
            archive: Descriptor::new(meter)?,
            has_archive: false,
            index: 0,
            current: None,
            key: Buffer::new(meter, 1024)?,
        };
        this.next_archive()?;
        this.load()?;
        Ok(this)
    }
    fn next_archive(&mut self) -> Result<(), Error> {
        self.has_archive = false;
        if let Some(reader) = &mut self.archives {
            while let Some(next) = reader.next(&|| self.owned.control.stopped())? {
                self.archive.take(next)?;
                if self
                    .archive
                    .eligibility(self.owned.handoff.publication_blocked)
                    == crate::archive::Eligibility::Eligible
                {
                    self.has_archive = true;
                    break;
                }
            }
        }
        Ok(())
    }
    fn load(&mut self) -> Result<(), Error> {
        self.current = None;
        let loose = self.loose.row()?;
        let archive_first = self.has_archive
            && loose.is_none_or(|r| self.archive.key().is_ok_and(|key| key < r.key));
        let (key, target, code, etag, size) = if archive_first {
            let i = u64::from(self.archive.index());
            let record = self.owned.outcomes.archive(i)?;
            (
                self.archive.key()?,
                Target::Archive(i),
                record.code,
                Etag::new(record.etag()?.unwrap_or_default().as_bytes())
                    .ok_or_else(Error::internal)?,
                self.archive.size(),
            )
        } else if let Some(row) = loose {
            let code = self.owned.outcomes.loose(self.index)?;
            let tag = if code == Code::WrittenBySize {
                tag(self.store, self.index)?
            } else {
                expected(row)?
            };
            (row.key, Target::Loose(self.index), code, tag, row.size)
        } else {
            return Ok(());
        };
        self.key.clear();
        self.key.extend(self.store.settings().prefix.as_bytes())?;
        self.key.extend(key.as_bytes())?;
        self.current = Some((target, code, etag, size));
        Ok(())
    }
}
impl Source for Merge<'_> {
    fn current(&self) -> Result<Option<Expected<'_>>, Error> {
        Ok(self
            .current
            .as_ref()
            .map(|(target, code, etag, size)| Expected {
                key: std::str::from_utf8(&self.key).unwrap_or_default(),
                target: *target,
                code: *code,
                etag: etag.as_bytes(),
                size: *size,
            }))
    }
    fn advance(&mut self) -> Result<(), Error> {
        match self.current {
            Some((Target::Loose(_), ..)) => {
                self.index += 1;
                self.loose.advance(&|| self.owned.control.stopped())?;
            }
            Some((Target::Archive(_), ..)) => self.next_archive()?,
            None => return Ok(()),
        }
        self.load()
    }
}
pub(super) fn expected(row: winners::Row<'_>) -> Result<Etag, Error> {
    let mut bytes = [0; 100];
    let mut output = &mut bytes[..];
    match row.etag {
        Some((digest, count)) => write!(output, "{}-{count}", crate::digest::Hex(&digest)),
        None => write!(output, "{}", crate::digest::Hex(&row.md5)),
    }
    .map_err(|_| Error::internal())?;
    let used = 100 - output.len();
    Etag::new(bytes.get(..used).ok_or_else(Error::internal)?).ok_or_else(Error::internal)
}
pub(super) fn put_tag(store: &Store, index: u64, etag: Etag) -> Result<(), Error> {
    let mut bytes = [0; 101];
    bytes[0] = u8::try_from(etag.as_bytes().len()).map_err(|_| Error::internal())?;
    bytes
        .get_mut(1..1 + etag.as_bytes().len())
        .ok_or_else(Error::internal)?
        .copy_from_slice(etag.as_bytes());
    store
        .tags
        .lock()
        .as_ref()
        .ok_or_else(Error::internal)?
        .write(index.checked_mul(101).ok_or_else(Error::internal)?, &bytes)?;
    Ok(())
}
fn tag(store: &Store, index: u64) -> Result<Etag, Error> {
    use crate::spool::random::Random;
    let tags = store.tags.lock();
    let file = tags.as_ref().ok_or_else(Error::internal)?;
    let mut bytes = [0; 101];
    let mut offset = index.checked_mul(101).ok_or_else(Error::internal)?;
    let mut left = &mut bytes[..];
    while !left.is_empty() {
        let n = file.read_at(left, offset)?;
        if n == 0 || n > left.len() {
            return Err(Error::internal());
        }
        offset += n as u64;
        left = left.get_mut(n..).ok_or_else(Error::internal)?;
    }
    Etag::new(
        bytes
            .get(1..1 + usize::from(bytes[0]))
            .ok_or_else(Error::internal)?,
    )
    .ok_or_else(Error::internal)
}
pub(super) fn mark(
    outcomes: &Outcomes,
    target: Target,
    code: Code,
    summary: Option<crate::archive::Summary>,
    tag: Option<Etag>,
) -> Result<(), Error> {
    match target {
        Target::Loose(i) => outcomes.set(i, code),
        Target::Archive(i) => {
            let record = match summary {
                Some(s) if code.published() => crate::outcomes::Archive::success(
                    code,
                    s.size,
                    s.md5,
                    tag.as_ref()
                        .map(|t| {
                            std::str::from_utf8(if code == Code::WrittenBySize {
                                t.as_bytes()
                            } else {
                                crate::store::etag::normalize(t.as_bytes())
                            })
                        })
                        .transpose()
                        .map_err(|_| Error::internal())?,
                )?,
                _ => crate::outcomes::Archive::failed(code)?,
            };
            outcomes.set_archive(i, &record)
        }
    }
}
