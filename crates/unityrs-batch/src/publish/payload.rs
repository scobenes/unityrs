//! Numeric committed payload access, shared with archive generation. Record
//! readers are separate; neither interface serializes trusted host paths.
use crate::{
    phase::Error,
    phase1::{InputPhase, InputState},
    spool::{self, FileSystem},
};
use std::{io::Read, path::Path};

pub trait Payloads: Sync {
    fn open(&self, input: u32, plan_index: u64) -> Result<Box<dyn Read + Send>, Error>;
}
pub struct Committed<'a> {
    pub done: &'a Path,
    pub states: &'a [InputState],
    pub fs: &'a dyn FileSystem,
}
impl Payloads for Committed<'_> {
    fn open(&self, input: u32, plan_index: u64) -> Result<Box<dyn Read + Send>, Error> {
        let state = self
            .states
            .get(usize::try_from(input).map_err(|_| Error::internal())?)
            .ok_or_else(Error::internal)?;
        if state.phase != InputPhase::Committed
            || state.committed.is_none()
            || state.failure_bundle.is_some()
            || state
                .received
                .is_none_or(|r| r.state != crate::attempt::State::Done)
        {
            return Err(Error::internal());
        }
        let directory = spool::path::indexed(self.done, "", u64::from(input))?;
        let path = spool::path::indexed(&directory, "o", plan_index)?;
        self.fs.open(&path).map_err(Error::from)
    }
}
