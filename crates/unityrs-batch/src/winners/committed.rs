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
    failures: Option<&'a [bool]>,
    fs: &'a dyn FileSystem,
    /// An input the finisher's one stat found committed after a hand-over
    /// left its commit's slot set (`docs/plans/batch.md` 2.4).
    found: Option<u32>,
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
            failures: Some(failures),
            fs,
            found: None,
        })
    }
    /// Uses only received terminal state and the controller's commit fact.
    /// No post-extraction table and no filesystem-based failure inference.
    pub fn received(
        done: &'a Path,
        states: &'a [InputState],
        fs: &'a dyn FileSystem,
        stopped: &dyn Fn() -> bool,
    ) -> Result<Self, Error> {
        for state in states {
            check(stopped)?;
            if state.phase != InputPhase::Committed {
                continue;
            }
            if state.committed.is_none()
                || (state.failure_bundle.is_none() && state.received.is_none())
            {
                return Err(Error::internal());
            }
            if state.received.is_some_and(|r| {
                !matches!(
                    r.state,
                    crate::attempt::State::Done | crate::attempt::State::Failed
                )
            }) {
                return Err(Error::internal());
            }
        }
        Ok(Self {
            done,
            states,
            failures: None,
            fs,
            found: None,
        })
    }
    /// Also read `input` as committed: its rename landed before the hand-over.
    #[must_use]
    pub fn with_found(mut self, input: Option<u32>) -> Self {
        self.found = input;
        self
    }
    fn index(&self, input: u32) -> Result<usize, Error> {
        let index = usize::try_from(input).map_err(|_| Error::internal())?;
        if self.found != Some(input)
            && self
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
        let index = self.index(input)?;
        if let Some(failures) = self.failures {
            return failures.get(index).copied().ok_or_else(Error::internal);
        }
        let state = self.states.get(index).ok_or_else(Error::internal)?;
        Ok(state.failure_bundle.is_some()
            || state
                .received
                .is_some_and(|r| r.state == crate::attempt::State::Failed))
    }
}
