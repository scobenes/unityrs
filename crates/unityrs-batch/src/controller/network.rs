//! A networked run's staging pass: local copies and URL stages together,
//! in spec order, at most `download_concurrency` at once, each holding one
//! of the controller's request permits, so each permit has at most one
//! blocking task outstanding (a copy, a lookup or a chunk write) and the
//! pool's `download_concurrency + 1` bound holds with the spare's task.
//!
//! The pass runs inside one `block_on` on the Execution's runtime. Each
//! turn registers the stop and end wakes, then tests the stop word before
//! anything else; it polls the URL slots' futures, receives finished local
//! copies and launches what permits allow. On a stop it returns at once:
//! nothing is received, no slot is polled again and none is dropped, so a
//! frozen request keeps its socket and its file until the host's exit (or
//! the library's drop after its finisher). It never awaits a slot after a
//! stop: a stopped network wait never returns.
use super::{Controller, Halt, Point};
use crate::{
    attempt::ReasonCode,
    execution::Execution,
    http::permit::PermitPool,
    job::RunFailure,
    stage::{
        Failure,
        slot::{Ended, Launch},
    },
    sys::RegisteredWake,
};
use std::{
    future::poll_fn,
    sync::Arc,
    task::{Context, Poll},
};

/// A networked run's URL launcher and the controller's request permits.
pub(crate) struct Staging {
    pub launch: Arc<dyn Launch>,
    pub permits: PermitPool,
    /// The Execution's stop and end wakes, registered with its runtime
    /// before the running head, since registration allocates.
    /// Out of here only during the pass, and put back after it.
    pub wakes: Option<[RegisteredWake; 2]>,
}
impl Controller {
    pub(super) fn stage_network(&mut self, execution: &Execution) -> Result<(), Halt> {
        let Some(runtime) = execution.runtime() else {
            // The host's lent runtime is driven by its controller thread.
            return Err(self.internal());
        };
        // Taken for the pass and put back, so the owners keep it parked:
        // dropping a registration would deregister it after the head.
        let Some(wakes) = self.network.as_mut().and_then(|n| n.wakes.take()) else {
            return Err(self.internal());
        };
        let mut next = 0;
        let result = runtime.block_on(poll_fn(|cx| {
            match self.network_turn(cx, execution, &wakes, &mut next) {
                Ok(true) => Poll::Ready(Ok(())),
                Ok(false) => Poll::Pending,
                Err(halt) => Poll::Ready(Err(halt)),
            }
        }));
        if let Some(network) = self.network.as_mut() {
            network.wakes = Some(wakes);
        }
        result
    }
    /// `Ok(true)` once every bundle is received and every permit's blocking
    /// task has returned (a quarantined lookup's included), `Ok(false)`
    /// with `cx` registered for whatever can change that.
    fn network_turn(
        &mut self,
        cx: &mut Context<'_>,
        execution: &Execution,
        wakes: &[RegisteredWake; 2],
        next: &mut usize,
    ) -> Result<bool, Halt> {
        loop {
            // Register before testing the stop word: a later notify wakes.
            for wake in wakes {
                while let Poll::Ready(result) = wake.poll_wait(cx) {
                    if result.is_err() {
                        return Err(self.fail(RunFailure::new(ReasonCode::HostIo)));
                    }
                }
            }
            self.check()?;
            let mut progressed = false;
            for slot in 0..self.prepared.stages_active.len() {
                progressed |= self.turn_slot(cx, execution, slot)?;
            }
            progressed |= self.launch_network(cx, execution, next)?;
            let busy = self
                .prepared
                .stages_active
                .iter()
                .any(|s| s.task.is_some() || s.run.is_some() || s.held.is_some());
            if !busy && *next == self.prepared.bundles.len() {
                let permits = &self.network.as_ref().ok_or(())?.permits;
                return Ok(permits.poll_idle(cx).is_ready());
            }
            if !progressed {
                return Ok(false);
            }
        }
    }
    /// One slot's turn: receive its finished copy, or poll its future once.
    fn turn_slot(
        &mut self,
        cx: &mut Context<'_>,
        execution: &Execution,
        slot: usize,
    ) -> Result<bool, Halt> {
        let state = self.prepared.stages_active.get_mut(slot).ok_or(())?;
        if state
            .task
            .as_ref()
            .is_some_and(crate::execution::Task::is_finished)
        {
            self.check()?;
            self.receive_local(execution, slot)?;
            return Ok(true);
        }
        let Some(bundle) = state.bundle.filter(|_| state.run.is_some()) else {
            return Ok(false);
        };
        // No slot is polled once the stop word is set.
        self.check()?;
        self.hooks.point(Point::StagePoll(bundle));
        let state = self.prepared.stages_active.get_mut(slot).ok_or(())?;
        let Some(run) = state.run.as_mut() else {
            return Ok(false);
        };
        let Poll::Ready(ended) = run.as_mut().poll(cx) else {
            return Ok(false);
        };
        state.run = None;
        if ended.end.bundle != bundle {
            return Err(self.internal());
        }
        self.receive_url(slot, ended)?;
        Ok(true)
    }
    /// A run failure is received: the stage already requested its stop, and
    /// its end names the reason. Any other end the stop word (or, for a
    /// stopped end, the cutoff before the word) overtook stays unreceived.
    fn receive_url(&mut self, slot: usize, mut ended: Ended) -> Result<(), Halt> {
        let run_failure = matches!(ended.end.failure, Some(Failure::Run(_)));
        let cut = matches!(ended.end.failure, Some(Failure::Stopped));
        if !run_failure && (cut || self.control.stopped()) {
            self.prepared.stages_active.get_mut(slot).ok_or(())?.held = Some(ended);
            return self.check();
        }
        let state = self.prepared.stages_active.get_mut(slot).ok_or(())?;
        state.bundle = None;
        // A run failure's stage stays parked here with its connection.
        state.residual = ended.residual.take();
        self.stage_end(ended.end, ended.file)
    }
    /// Launch in spec order while a slot and a permit are free. A held end
    /// means the cutoff passed: nothing more starts.
    fn launch_network(
        &mut self,
        cx: &mut Context<'_>,
        execution: &Execution,
        next: &mut usize,
    ) -> Result<bool, Halt> {
        let mut launched = false;
        if self.prepared.stages_active.iter().any(|s| s.held.is_some()) {
            return Ok(false);
        }
        for slot in 0..self.prepared.stages_active.len() {
            if *next >= self.prepared.bundles.len() {
                break;
            }
            let free = self.prepared.stages_active.get(slot).is_some_and(|s| {
                s.bundle.is_none() && s.task.is_none() && s.run.is_none() && s.residual.is_none()
            });
            if !free {
                continue;
            }
            self.check()?;
            let Poll::Ready(permit) = self.network.as_ref().ok_or(())?.permits.poll_acquire(cx)
            else {
                // Every permit is out, a quarantined lookup's included: the
                // next bundle, local or not, waits for one, and the drop
                // that frees one wakes this pass.
                break;
            };
            let bundle = u32::try_from(*next).map_err(|_| self.internal())?;
            *next += 1;
            launched = true;
            let spec = self.prepared.spec.clone();
            if spec.bundle(bundle as usize).ok_or(())?.url().is_none() {
                self.launch_stage(execution, slot, bundle)?;
                let state = self.prepared.stages_active.get_mut(slot).ok_or(())?;
                if state.task.is_some() {
                    state.permit = Some(permit);
                }
                continue;
            }
            self.hooks.event(super::Event::StageDispatched(bundle));
            let file = self.destination(bundle)?;
            let launch = self.network.as_ref().ok_or(())?.launch.clone();
            match launch.launch(&spec, bundle, file, permit) {
                Ok(run) => {
                    let state = self.prepared.stages_active.get_mut(slot).ok_or(())?;
                    state.bundle = Some(bundle);
                    state.run = Some(run);
                }
                Err(failure) => {
                    self.stage_end(crate::stage::slot::ended(bundle, failure), None)?;
                }
            }
        }
        Ok(launched)
    }
}
