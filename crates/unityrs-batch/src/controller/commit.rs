use super::{Controller, Event, Point};
use crate::phase1::InputPhase;
use std::sync::atomic::Ordering;

impl Controller {
    pub(super) fn commit(&mut self, input: u32) -> Result<(), super::Halt> {
        if self.prepared.uncommitted.get(input as usize) != Some(true)
            || self.pending_commit().is_some()
        {
            return Err(self.internal());
        }
        let destination = self
            .spool
            .directory("done", input)
            .map_err(|e| self.io(e))?;
        // The seven steps are deliberately contiguous, on this thread only.
        self.pending_commit
            .store(u64::from(input), Ordering::Release); // 1
        self.hooks.point(Point::BeforeCommitLoad(input));
        if self.control.stopped() {
            // 2
            self.pending_commit.store(u64::MAX, Ordering::Release); // 3
            return Err(super::Halt);
        }
        if let Err(error) = self
            .pending_directory
            .as_mut()
            .ok_or(())?
            .rename(destination)
        {
            // 4
            // Keep the slot set, even for perform-then-fail. B9 reconciles by
            // one stat, using disk state rather than the owner's old path.
            let errno = match &error {
                crate::spool::Error::HostIo(e) => e.raw_os_error(),
                crate::spool::Error::Full(crate::spool::Full::Enospc) => Some(libc::ENOSPC),
                crate::spool::Error::Full(crate::spool::Full::Edquot) => Some(libc::EDQUOT),
                _ => None,
            };
            self.rename_failure = Some((input, errno));
            return Err(self.io(error));
        }
        self.hooks.event(Event::Renamed(input)); // 5: no fault point before trace
        self.hooks.point(Point::AfterRenameTrace(input));
        self.prepared
            .uncommitted
            .set(input as usize, false)
            .ok_or(())?; // 6
        self.prepared
            .inputs
            .get_mut(input as usize)
            .ok_or(())?
            .phase = InputPhase::Committed;
        self.pending_commit.store(u64::MAX, Ordering::Release); // 7
        self.prepared
            .inputs
            .get_mut(input as usize)
            .ok_or(())?
            .committed = Some(
            self.pending_directory
                .take()
                .ok_or(())?
                .into_claim()
                .map_err(|e| self.io(e))?,
        );
        Ok(())
    }
}
