use super::{Controller, Event};
use crate::{
    attempt,
    execution::Execution,
    job::{End, RunFailure},
    peek,
    phase1::{JobKind, PeekFacts},
};
use std::sync::Arc;

impl Controller {
    pub(super) fn drain(&mut self, execution: &mut Execution) -> Result<(), super::Halt> {
        loop {
            // Stop handling terminates the drain. Queued ends remain absent
            // from the received facts, even if their workers published words.
            self.check()?;
            match execution.receive() {
                Ok(end) => {
                    self.received = Some(end);
                    self.receive_end(execution, &end)?;
                    self.received = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => return Ok(()),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => return Err(self.internal()),
            }
        }
    }
    fn receive_end(&mut self, execution: &mut Execution, end: &End) -> Result<(), super::Halt> {
        let (kind, id, releasable, clear) = match end {
            End::Attempt(e) => (
                JobKind::Attempt,
                u32::try_from(e.input_index).map_err(|_| self.internal())?,
                e.slot_releasable,
                e.accounts.is_clear(),
            ),
            End::Peek(e) => (
                JobKind::Peek,
                e.bundle,
                e.slot_releasable,
                e.accounts.is_clear(),
            ),
        };
        let at = self
            .prepared
            .active
            .iter()
            .position(|s| s.job == Some((kind, id)))
            .ok_or_else(|| self.internal())?;
        match end {
            End::Peek(e) => {
                let bundle = self.prepared.bundles.get_mut(id as usize).ok_or(())?;
                if bundle.peek.is_some() {
                    return Err(self.internal());
                }
                bundle.peek = Some(PeekFacts {
                    input_peak: e.input_peak,
                    decode_work: e.decode_work,
                    outcome: e.outcome,
                });
                // Received facts survive even an unreleasable audited end.
                if !releasable || !clear {
                    return Err(self.internal());
                }
                let slot = self.prepared.active.get_mut(at).ok_or(())?;
                let owner = slot.peek_file.take().ok_or(())?;
                self.prepared.files.get_mut(id as usize).ok_or(())?.peek = Arc::try_unwrap(owner)
                    .map_err(|_| ())?
                    .into_inner()
                    .map_err(|_| ())?
                    .map(crate::spool::OwnedFile::into_claim);
                self.hooks.event(Event::PeekReceived(id));
                match e.state {
                    peek::State::RunFailure => {
                        return Err(self.fail(
                            e.failure
                                .unwrap_or(RunFailure::new(attempt::ReasonCode::Internal)),
                        ));
                    }
                    peek::State::Stopped => return Err(super::Halt),
                    peek::State::Done
                        if e.outcome.is_none()
                            || self.prepared.peek_words.get(id as usize).ok_or(())?.load()
                                != Ok(e.outcome.ok_or(())?) =>
                    {
                        return Err(self.internal());
                    }
                    peek::State::Done => {}
                }
            }
            End::Attempt(e) => {
                let state = self.prepared.inputs.get_mut(id as usize).ok_or(())?;
                if state.received.is_some() {
                    return Err(self.internal());
                }
                state.received = Some(crate::phase1::AttemptFacts::from_end(e));
                self.hooks.event(Event::AttemptReceived(id));
                if !releasable || !clear {
                    return Err(self.internal());
                }
                match e.state {
                    attempt::State::RunFailure => {
                        return Err(self.fail(RunFailure {
                            reason: e.reason.unwrap_or(attempt::ReasonCode::Internal),
                            detail: e.detail,
                        }));
                    }
                    attempt::State::Stopped => return Err(super::Halt),
                    attempt::State::Done | attempt::State::Failed => {
                        self.finish_attempt(execution, at, id, e)?;
                    }
                }
            }
        }
        self.prepared.active.get_mut(at).ok_or(())?.directory = None;
        self.prepared.active.get_mut(at).ok_or(())?.job = None;
        self.prepared.active.get_mut(at).ok_or(())?.dispatched = None;
        Ok(())
    }
    fn finish_attempt(
        &mut self,
        execution: &mut Execution,
        at: usize,
        id: u32,
        e: &attempt::AttemptEnd,
    ) -> Result<(), super::Halt> {
        self.check()?;
        let owner = self
            .prepared
            .active
            .get_mut(at)
            .and_then(|s| s.directory.as_mut())
            .ok_or(())?;
        if Arc::get_mut(owner).is_none() {
            return Err(self.internal());
        }
        if e.state == attempt::State::Failed {
            Arc::get_mut(owner)
                .ok_or(())?
                .remove_stoppable(&|| self.control.stopped())
                .map_err(|error| {
                    if matches!(error, crate::spool::Error::Stopped) {
                        super::Halt
                    } else {
                        self.io(error)
                    }
                })?;
            // The vacancy is not refilled until this failure
            // writer and commit finish; it uses this slot's room.
            self.write_failure(execution, id, false, Some(e))?;
        } else {
            // Keep the owner reachable across commit panics.
            let owner = self
                .prepared
                .active
                .get_mut(at)
                .and_then(|s| s.directory.take())
                .ok_or(())?;
            self.pending_directory = Some(Arc::try_unwrap(owner).map_err(|_| ())?);
            self.commit(id)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        controller::{NoHooks, PreparedSpool},
        peek::word::Outcome,
        phase1, spec,
        spool::{Counter, Host},
    };
    use std::sync::{Mutex, atomic::AtomicBool};
    use unityrs_testkit::{guard, scratch::Scratch};
    fn fixture() -> (Scratch, Controller, Execution) {
        let scratch = Scratch::new("end-protocol");
        for dir in ["in", "attempts", "done", "fail"] {
            std::fs::create_dir(scratch.join(dir)).unwrap();
        }
        let source = scratch.join("source");
        std::fs::write(
            &source,
            unityrs_testkit::bundle::build(&unityrs_testkit::bundle::BundleSpec::default()),
        )
        .unwrap();
        let spec = spec::parse(
            serde_json::to_vec(&serde_json::json!({
                "spec_version":1,"run_id":"run","attempt":1,"threads":2,
                "download_concurrency":1,"max_inflight_uncompressed_bytes":67_108_864,
                "max_decoded_bytes":16_777_216,"dep_cache_bytes":16_777_216,
                "spool":{"dir":scratch.join("spool"),"size_bytes":1_073_741_824,"reserve_bytes":0},
                "inputs":[{"id":"a","path":source}],"outputs":{"textures":{"key":"{name}.png"}},
                "sink":{"kind":"directory","root":scratch.join("sink")}
            }))
            .unwrap(),
        )
        .unwrap();
        let controller = Controller::new(
            phase1::prepare(spec).unwrap(),
            PreparedSpool::new(
                scratch.path().to_path_buf(),
                Counter::new(1 << 30, 0, 4_096).unwrap(),
                Arc::new(Host),
            )
            .unwrap(),
            Arc::new(AtomicBool::new(false)),
            Arc::new(NoHooks),
        )
        .unwrap();
        let execution = Execution::new(&controller).unwrap();
        (scratch, controller, execution)
    }
    #[test]
    fn duplicate_and_unknown_ends_neither_commit_nor_release_a_second_slot() {
        let () = guard::finishes("end identity protocol", || {
            for duplicate in [false, true] {
                let (_scratch, mut c, mut execution) = fixture();
                if duplicate {
                    c.prepared.active[0].job = Some((JobKind::Peek, 0));
                    c.prepared.active[0].peek_file = Some(Arc::new(Mutex::new(None)));
                    c.prepared.peek_words[0].publish(Outcome::Clear(0)).unwrap();
                    let end = End::Peek(peek::PeekEnd {
                        bundle: 0,
                        state: peek::State::Done,
                        outcome: Some(Outcome::Clear(0)),
                        failure: None,
                        input_peak: 7,
                        decode_work: 11,
                        claimed_bytes: 0,
                        accounts: crate::budget::Snapshot::default(),
                        slot_releasable: true,
                    });
                    c.receive_end(&mut execution, &end).unwrap();
                    assert_eq!(c.reserved_slots(), 0);
                    assert!(c.receive_end(&mut execution, &end).is_err());
                    assert_eq!(c.prepared.bundles[0].peek.unwrap().input_peak, 7);
                } else {
                    let end = End::Attempt(attempt::AttemptEnd::new(99));
                    assert!(c.receive_end(&mut execution, &end).is_err());
                }
                assert_eq!(c.failure().unwrap().reason, attempt::ReasonCode::Internal);
                assert_eq!(c.reserved_slots(), 0);
                assert_eq!(c.prepared.uncommitted.get(0), Some(false));
                execution.quiesce().unwrap();
            }
        });
    }
    #[test]
    fn a_received_leak_keeps_its_figures_and_reservation() {
        let () = guard::finishes("leaked end audit", || {
            let (_scratch, mut c, mut execution) = fixture();
            c.prepared.active[0].job = Some((JobKind::Attempt, 0));
            c.prepared.uncommitted.set(0, true).unwrap();
            let mut end = attempt::AttemptEnd::new(0);
            end.figures.input_peak = 7;
            end.accounts.input.used = 7;
            end.slot_releasable = false;
            assert!(c.receive_end(&mut execution, &End::Attempt(end)).is_err());
            assert_eq!(c.prepared.inputs[0].received.unwrap().figures.input_peak, 7);
            assert_eq!(c.reserved_slots(), 1);
            assert_eq!(c.prepared.uncommitted.get(0), Some(true));
            execution.quiesce().unwrap();
        });
    }
}
