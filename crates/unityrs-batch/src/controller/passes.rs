use super::{Controller, Event, Point};
use crate::sys::Shared;
use crate::{
    attempt::ReasonCode,
    execution::Execution,
    job::RunFailure,
    phase1::{InputPhase, JobKind},
    spool::OwnedFile,
    stage::{self, Failure, StageEnd},
    worker::Dispatch,
};
use std::time::Instant;

impl Controller {
    pub(super) fn stage(&mut self, execution: &mut Execution) -> Result<(), super::Halt> {
        let mut next = 0;
        loop {
            self.check()?;
            for slot in 0..self.prepared.stages_active.len() {
                if self
                    .prepared
                    .stages_active
                    .get(slot)
                    .is_some_and(|s| s.task.is_some())
                {
                    continue;
                }
                if next >= self.prepared.bundles.len() {
                    break;
                }
                self.check()?;
                let bundle = u32::try_from(next).map_err(|_| self.internal())?;
                next += 1;
                self.launch_stage(execution, slot, bundle)?;
            }
            let mut active = false;
            let mut completed = false;
            for slot in 0..self.prepared.stages_active.len() {
                if self
                    .prepared
                    .stages_active
                    .get(slot)
                    .and_then(|s| s.task.as_ref())
                    .is_some_and(crate::execution::Task::is_finished)
                {
                    // Stop freezes even completed-but-unreceived pool work.
                    self.check()?;
                    let task = self
                        .prepared
                        .stages_active
                        .get_mut(slot)
                        .and_then(|s| s.task.take())
                        .ok_or(())?;
                    let (finish, mut file) =
                        execution.finish(&task).map_err(|()| self.internal())?;
                    let bundle = self
                        .prepared
                        .stages_active
                        .get_mut(slot)
                        .and_then(|s| s.bundle.take())
                        .ok_or(())?;
                    let hooks = StageHooks {
                        hooks: &*self.hooks,
                        bundle,
                    };
                    let end = finish.finish(&mut file, &|| self.control.stopped(), &hooks);
                    if end.bundle != bundle {
                        return Err(self.internal());
                    }
                    self.stage_end(end, Some(file))?;
                    completed = true;
                }
                active |= self
                    .prepared
                    .stages_active
                    .get(slot)
                    .is_some_and(|s| s.task.is_some());
            }
            if !active && next == self.prepared.bundles.len() {
                return self.check();
            }
            if !active {
                continue;
            }
            if !completed {
                self.wait(execution)?;
            }
        }
    }
    fn launch_stage(
        &mut self,
        execution: &Execution,
        slot: usize,
        bundle: u32,
    ) -> Result<(), super::Halt> {
        let input = self.prepared.spec.bundle(bundle as usize).ok_or(())?;
        self.hooks.event(Event::StageDispatched(bundle));
        match stage::local::open(input, self.prepared.spec.max_decoded_bytes) {
            Err(failure) => self.stage_end(
                StageEnd {
                    bundle,
                    failure: Some(failure),
                    integrity: None,
                    facts: None,
                    claimed_bytes: 0,
                },
                None,
            )?,
            Ok(copy) => {
                let directory = self.spool.input().map_err(|e| self.io(e))?;
                let path =
                    stage::try_path(&directory, bundle, self.prepared.spec.input_count(), false)
                        .map_err(|e| self.io(e))?;
                let mut file =
                    OwnedFile::create(path, self.spool.counter.clone(), self.spool.fs.clone())
                        .map_err(|e| self.io(e))?;
                let control = self.control.clone();
                let hooks = self.hooks.clone();
                let lease = self.spool.clone();
                let task = execution
                    .spawn(move || {
                        // Returning on hand-over must not unlock beside late I/O.
                        let _lease = lease;
                        let adapter = StageHooks {
                            hooks: &*hooks,
                            bundle,
                        };
                        let finish = copy.run(bundle, &mut file, &|| control.stopped(), &adapter);
                        (finish, file)
                    })
                    .map_err(|e| self.io(e.into()))?;
                self.prepared.stages_active.get_mut(slot).ok_or(())?.bundle = Some(bundle);
                self.prepared.stages_active.get_mut(slot).ok_or(())?.task = Some(task);
            }
        }
        Ok(())
    }
    fn stage_end(&mut self, end: StageEnd, file: Option<OwnedFile>) -> Result<(), super::Halt> {
        let state = self
            .prepared
            .bundles
            .get_mut(end.bundle as usize)
            .ok_or(())?;
        if state.stage.is_some() {
            return Err(self.internal());
        }
        state.stage = Some(end);
        self.prepared
            .files
            .get_mut(end.bundle as usize)
            .ok_or(())?
            .staged = file.map(OwnedFile::into_claim);
        if let Some(Failure::Run(f)) = end.failure {
            RunFailure::reduce(&mut self.failure, f);
            self.control
                .request_stop(f, crate::control::Culprit::Bundle(end.bundle));
        }
        self.hooks.event(Event::StageReceived(end.bundle));
        match end.failure {
            Some(Failure::Run(_) | Failure::Stopped) => Err(super::Halt),
            _ => self.check(),
        }
    }
    pub(super) fn stage_barrier(&mut self, execution: &mut Execution) -> Result<(), super::Halt> {
        self.check()?;
        self.mark(false)?;
        self.barrier_failures(execution, false)
    }
    pub(super) fn peek_barrier(&mut self, execution: &mut Execution) -> Result<(), super::Halt> {
        self.check()?;
        self.mark(true)?;
        self.barrier_failures(execution, true)?;
        self.ready_queue();
        Ok(())
    }
    fn barrier_failures(
        &mut self,
        execution: &mut Execution,
        peeks: bool,
    ) -> Result<(), super::Halt> {
        for at in 0..self.prepared.failures.len() {
            self.check()?;
            let input = *self.prepared.failures.get(at).ok_or(())?;
            self.write_failure(execution, input, peeks, None)?;
        }
        Ok(())
    }
    pub(super) fn jobs(
        &mut self,
        execution: &mut Execution,
        peeks: bool,
    ) -> Result<(), super::Halt> {
        let mut next = 0;
        loop {
            self.check()?;
            while self.reserved_slots() < self.admission.slots() {
                let queue = if peeks {
                    &self.prepared.peek_queue
                } else {
                    &self.prepared.ready
                };
                let Some(&id) = queue.get(next) else {
                    break;
                };
                self.check()?;
                self.dispatch(execution, id, peeks)?;
                next += 1;
            }
            let count = if peeks {
                self.prepared.peek_queue.len()
            } else {
                self.prepared.ready.len()
            };
            if self.reserved_slots() == 0 && next == count {
                return self.check();
            }
            self.wait(execution)?;
        }
    }
    fn dispatch(&mut self, execution: &Execution, id: u32, peek: bool) -> Result<(), super::Halt> {
        let at = self
            .prepared
            .active
            .iter()
            .position(|s| s.job.is_none())
            .ok_or_else(|| self.internal())?;
        let kind = if peek {
            JobKind::Peek
        } else {
            JobKind::Attempt
        };
        let directory = if peek {
            None
        } else {
            Some(
                Shared::new(
                    self.spool
                        .create_directory("attempts", id)
                        .map_err(|e| self.io(e))?,
                )
                .map_err(|e| self.io(e.into()))?,
            )
        };
        let peek_file = if peek {
            Some(Shared::new(crate::sys::Lock::new(None)).map_err(|e| self.io(e.into()))?)
        } else {
            None
        };
        let slot = self.prepared.active.get_mut(at).ok_or(())?;
        // Reserve before sending, independent of current measured use.
        slot.job = Some((kind, id));
        slot.dispatched = Some(Instant::now());
        slot.directory.clone_from(&directory);
        slot.peek_file.clone_from(&peek_file);
        if !peek {
            self.prepared.uncommitted.set(id as usize, true).ok_or(())?;
            self.prepared.inputs.get_mut(id as usize).ok_or(())?.phase = InputPhase::Running;
        }
        // Publish before the dispatch can let a worker acquire budget charges.
        self.sample_reservations();
        self.hooks.event(if peek {
            Event::PeekDispatched(id)
        } else {
            Event::AttemptDispatched(id)
        });
        execution
            .dispatch(Dispatch {
                kind,
                id,
                directory,
                peek_file,
            })
            .map_err(|()| self.fail(RunFailure::new(ReasonCode::Internal)))
    }
}
struct StageHooks<'a> {
    hooks: &'a dyn super::Hooks,
    bundle: u32,
}
impl stage::local::Hooks for StageHooks<'_> {
    fn point(&self, point: stage::local::Point) {
        self.hooks.point(Point::Stage(self.bundle, point));
    }
}
