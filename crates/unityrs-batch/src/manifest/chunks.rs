//! Whole-line chunking; no duplicate on-spool row stream. A family is
//! acknowledged only after its final chunk, including the zero-chunk case.
use super::{Family, WriteError};
use crate::{
    phase::{Account, Buffer, Error},
    records::LINE,
};
pub const CHUNK: usize = 16 * 1024 * 1024;

pub trait Destination {
    fn publish(&mut self, family: Family, chunk: u64, bytes: &[u8]) -> Result<(), WriteError>;
}
pub(super) struct Chunks<'a> {
    destination: &'a mut dyn Destination,
    family: Family,
    bytes: Buffer<u8>,
    count: u64,
}
impl<'a> Chunks<'a> {
    pub fn new(
        destination: &'a mut dyn Destination,
        family: Family,
        account: &Account,
    ) -> Result<Self, WriteError> {
        Ok(Self {
            destination,
            family,
            bytes: Buffer::new(account.meter(), CHUNK)?,
            count: 0,
        })
    }
    pub fn put(&mut self, line: &[u8]) -> Result<(), WriteError> {
        if line.is_empty()
            || line.len() > LINE
            || line.last() != Some(&b'\n')
            || line
                .get(..line.len() - 1)
                .ok_or_else(Error::internal)?
                .contains(&b'\n')
        {
            return Err(Error::internal().into());
        }
        if self.bytes.len() + line.len() > CHUNK {
            self.flush()?;
        }
        self.bytes.extend(line).map_err(Into::into)
    }
    fn flush(&mut self) -> Result<(), WriteError> {
        if self.bytes.is_empty() {
            return Ok(());
        }
        self.destination
            .publish(self.family, self.count, &self.bytes)?;
        self.count = self.count.checked_add(1).ok_or_else(Error::internal)?;
        self.bytes.clear();
        Ok(())
    }
    pub fn finish(mut self) -> Result<u64, WriteError> {
        self.flush()?;
        Ok(self.count)
    }
}
/// Directory row publication uses writeback and final filesystem sync, as
/// loose files do. The normal finisher owns the final sync/head ordering.
pub struct Directory<'a> {
    pub directory: &'a crate::sink::directory::Directory,
    pub run_id: &'a str,
    pub attempt: u32,
}
impl Destination for Directory<'_> {
    fn publish(&mut self, family: Family, chunk: u64, bytes: &[u8]) -> Result<(), WriteError> {
        use std::fmt::Write;
        let mut key = String::new();
        key.try_reserve_exact(self.run_id.len() + 96)
            .map_err(|_| Error::memory())?;
        write!(
            key,
            "_unityrs/{}/{}/{}.{}.ndjson",
            self.run_id,
            self.attempt,
            family.name(),
            chunk
        )
        .map_err(|_| Error::internal())?;
        let parent = self
            .directory
            .root()
            .parent(&key, true, crate::sink::Context::Finish)
            .map_err(super::directory_error)?;
        let temp = parent
            .temporary(self.run_id, 0, crate::sink::Context::Finish)
            .map_err(super::directory_error)?;
        temp.write_all(bytes).map_err(super::directory_error)?;
        temp.writeback().map_err(super::directory_error)?;
        temp.rename(key.rsplit('/').next().ok_or_else(Error::internal)?)
            .map_err(super::directory_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct Destination {
        sizes: Vec<usize>,
        fail: bool,
    }
    impl super::Destination for Destination {
        fn publish(&mut self, _: Family, _: u64, bytes: &[u8]) -> Result<(), WriteError> {
            if self.fail && !self.sizes.is_empty() {
                return Err(WriteError::Publish);
            }
            self.sizes.push(bytes.len());
            Ok(())
        }
    }
    #[test]
    fn exact_chunk_and_line_boundaries_and_partial_family() {
        let account = Account::new(64 << 20);
        let mut line = vec![b'x'; LINE];
        *line.last_mut().unwrap() = b'\n';
        for fail in [false, true] {
            let mut destination = Destination {
                fail,
                ..Destination::default()
            };
            let mut chunks = Chunks::new(&mut destination, Family::Outputs, &account).unwrap();
            for _ in 0..=(CHUNK / LINE) {
                chunks.put(&line).unwrap();
            }
            if fail {
                assert_eq!(chunks.finish(), Err(WriteError::Publish));
                assert_eq!(destination.sizes, [CHUNK]);
            } else {
                assert_eq!(chunks.finish(), Ok(2));
                assert_eq!(destination.sizes, [CHUNK, LINE]);
            }
        }
        let mut destination = Destination::default();
        assert_eq!(
            Chunks::new(&mut destination, Family::Outputs, &account)
                .unwrap()
                .finish(),
            Ok(0)
        );
        assert_eq!(destination.sizes.len(), 0);
        let mut chunks = Chunks::new(&mut destination, Family::Outputs, &account).unwrap();
        assert!(chunks.put(b"no newline").is_err());
        assert!(chunks.put(b"a\nb\n").is_err());
        assert!(chunks.put(&vec![b'x'; LINE + 1]).is_err());
    }
}
