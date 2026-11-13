//! Read-only adapter over frozen controller receipts and numeric `done/`
//! directories. Construct after all attempts/peeks and their accounts clear.
use crate::{
    phase::{Error, check},
    phase1::{InputPhase, InputState},
    spool::FileSystem,
};
use std::{
    io::Read,
    path::{Path, PathBuf},
};

pub struct Committed<'a> {
    done: &'a Path,
    states: &'a [InputState],
    failures: &'a [bool],
    fs: &'a dyn FileSystem,
}
impl<'a> Committed<'a> {
    /// `failures` includes attempt failures as well as pre-dispatch failures;
    /// `failure_bundle` alone cannot describe every committed failure.
    pub fn new(
        done: &'a Path,
        states: &'a [InputState],
        failures: &'a [bool],
        fs: &'a dyn FileSystem,
        stopped: &dyn Fn() -> bool,
    ) -> Result<Self, Error> {
        if states.len() != failures.len() {
            return Err(Error::internal());
        }
        for state in states {
            check(stopped)?;
            if state.phase != InputPhase::Committed {
                return Err(Error::internal());
            }
        }
        Ok(Self {
            done,
            states,
            failures,
            fs,
        })
    }
    fn index(&self, input: u32) -> Result<usize, Error> {
        let index = usize::try_from(input).map_err(|_| Error::internal())?;
        if self
            .states
            .get(index)
            .is_none_or(|s| s.phase != InputPhase::Committed)
        {
            return Err(Error::internal());
        }
        Ok(index)
    }
    fn path(&self, mut input: u32) -> Result<PathBuf, Error> {
        let mut digits = [b'0'; 10];
        let mut start = digits.len();
        loop {
            start -= 1;
            *digits.get_mut(start).ok_or_else(Error::internal)? =
                b'0' + u8::try_from(input % 10).map_err(|_| Error::internal())?;
            input /= 10;
            if input == 0 {
                break;
            }
        }
        let mut path = PathBuf::new();
        path.try_reserve_exact(
            self.done
                .as_os_str()
                .len()
                .checked_add(27)
                .ok_or_else(Error::internal)?,
        )
        .map_err(|_| Error::memory())?;
        path.push(self.done);
        path.push(
            std::str::from_utf8(digits.get(start..).ok_or_else(Error::internal)?)
                .map_err(|_| Error::internal())?,
        );
        path.push("records.ndjson");
        Ok(path)
    }
}
impl super::Inputs for Committed<'_> {
    fn open(&self, input: u32) -> Result<Box<dyn Read + Send>, Error> {
        self.index(input)?;
        Ok(self.fs.open(&self.path(input)?)?)
    }
    fn failed(&self, input: u32) -> Result<bool, Error> {
        self.failures
            .get(self.index(input)?)
            .copied()
            .ok_or_else(Error::internal)
    }
}
