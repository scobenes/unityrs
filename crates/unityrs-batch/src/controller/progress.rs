use super::Controller;
use crate::{phase1::InputPhase, stage::Failure};
impl Controller {
    fn reserved_bytes(&self) -> u64 {
        self.prepared
            .active
            .iter()
            .map(|s| match s.job {
                Some(_) => self.admission.reservation(),
                None => 0,
            })
            .sum::<u64>()
            + self.failure_reservation
    }
    pub(super) fn sample_reservations(&self) {
        if let Some(progress) = &self.progress {
            progress.update(|s| s.memory.slots_reserved = self.reserved_bytes());
        }
    }
    pub(super) fn sample(&self) {
        let Some(progress) = &self.progress else {
            return;
        };
        if !progress.sample_due() {
            return;
        }
        self.sample_now();
    }
    pub(super) fn sample_now(&self) {
        let Some(progress) = &self.progress else {
            return;
        };
        progress.update(|snapshot| {
            let p = &self.prepared;
            snapshot.inputs = crate::progress::Inputs {
                total: p.inputs.len() as u64,
                ..crate::progress::Inputs::default()
            };
            for input in &p.inputs {
                match input.phase {
                    InputPhase::Committed => {
                        if input.failure_bundle.is_some()
                            || input
                                .received
                                .is_some_and(|r| r.state == crate::attempt::State::Failed)
                        {
                            snapshot.inputs.failed += 1;
                        } else {
                            snapshot.inputs.complete += 1;
                        }
                    }
                    InputPhase::Running => snapshot.inputs.running += 1,
                    InputPhase::Pending | InputPhase::Ready => snapshot.inputs.queued += 1,
                }
            }
            snapshot.transfers.stages_active = p
                .stages_active
                .iter()
                .filter(|s| s.bundle.is_some())
                .count() as u64;
            snapshot.transfers.stages_done = p
                .bundles
                .iter()
                .filter(|b| b.stage.is_some_and(|s| s.failure.is_none()))
                .count() as u64;
            snapshot.transfers.stages_failed = p
                .bundles
                .iter()
                .filter(|b| {
                    b.stage
                        .is_some_and(|s| matches!(s.failure, Some(Failure::Data(_))))
                })
                .count() as u64;
            snapshot.memory.slots_reserved = self.reserved_bytes();
            snapshot.outputs.candidates = p
                .inputs
                .iter()
                .filter_map(|s| s.received)
                .map(|r| r.outputs)
                .sum();
        });
    }
}
