//! Raw central entries, bounded buffered writes and explicit deletion.
use super::{Hooks, Point, boundary};
use crate::{
    phase::{Buffer, Error, check},
    spool::workspace::{Independent, Workspace},
};
use std::{
    io::{self, Read},
    sync::Arc,
};
use unityrs_core::limits::Meter;
const CHUNK: usize = 64 * 1024;
pub(super) struct Central {
    owner: Independent,
    buffer: Buffer<u8>,
    read: Option<Box<dyn Read + Send>>,
    remaining: u64,
}
impl Central {
    pub fn new(
        workspace: &Workspace,
        meter: &Arc<Meter>,
        stopped: &dyn Fn() -> bool,
    ) -> Result<Self, Error> {
        Ok(Self {
            owner: workspace.create_independent(stopped)?,
            buffer: Buffer::new(meter, CHUNK)?,
            read: None,
            remaining: 0,
        })
    }
    pub fn append(
        &mut self,
        bytes: &[u8],
        hooks: &dyn Hooks,
        stopped: &dyn Fn() -> bool,
    ) -> Result<(), Error> {
        check(stopped)?;
        if self.read.is_some() || bytes.len() > CHUNK {
            return Err(Error::internal());
        }
        if self.buffer.len() + bytes.len() > CHUNK {
            self.flush(hooks, stopped)?;
        }
        self.buffer.extend(bytes)
    }
    fn flush(&mut self, hooks: &dyn Hooks, stopped: &dyn Fn() -> bool) -> Result<(), Error> {
        if !self.buffer.is_empty() {
            boundary(hooks, Point::CentralWrite, stopped)?;
            self.owner.append(&self.buffer, stopped)?;
            self.buffer.clear();
        }
        Ok(())
    }
    pub fn replay(
        &mut self,
        expected: u64,
        hooks: &dyn Hooks,
        stopped: &dyn Fn() -> bool,
    ) -> Result<(), Error> {
        self.flush(hooks, stopped)?;
        boundary(hooks, Point::CentralSeal, stopped)?;
        self.owner.seal(stopped)?;
        if self.owner.length() != expected {
            return Err(Error::internal());
        }
        self.read = Some(self.owner.open(stopped)?);
        self.remaining = expected;
        Ok(())
    }
    pub fn read(
        &mut self,
        out: &mut [u8],
        hooks: &dyn Hooks,
        stopped: &dyn Fn() -> bool,
    ) -> Result<usize, Error> {
        boundary(hooks, Point::CentralRead, stopped)?;
        let read = self.read.as_mut().ok_or_else(Error::internal)?;
        if self.remaining == 0 {
            let mut byte = [0];
            if read_some(read, &mut byte, stopped)? != 0 {
                return Err(Error::internal());
            }
            drop(self.read.take());
            return Ok(0);
        }
        let limit = usize::try_from(self.remaining.min(65_536))
            .map_err(|_| Error::internal())?
            .min(out.len());
        let output = out.get_mut(..limit).ok_or_else(Error::internal)?;
        let n = read_some(read, output, stopped)?;
        if n == 0 {
            return Err(Error::internal());
        }
        self.remaining -= u64::try_from(n).map_err(|_| Error::internal())?;
        Ok(n)
    }
    pub fn retire(self, hooks: &dyn Hooks, stopped: &dyn Fn() -> bool) -> Result<(), Error> {
        if self.read.is_some() || self.remaining != 0 {
            return Err(Error::internal());
        }
        boundary(hooks, Point::Retire, stopped)?;
        Ok(self.owner.retire(stopped)?)
    }
}
pub(super) fn read_some(
    read: &mut dyn Read,
    out: &mut [u8],
    stopped: &dyn Fn() -> bool,
) -> Result<usize, Error> {
    loop {
        check(stopped)?;
        match read.read(out) {
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e.into()),
            Ok(n) if n <= out.len() => return Ok(n),
            Ok(_) => return Err(Error::internal()),
        }
    }
}
