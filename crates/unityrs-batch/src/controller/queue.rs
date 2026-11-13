use super::{Controller, SetupError};
use crate::{peek::word::Outcome, phase1::InputPhase};

#[derive(Clone, Copy, Debug)]
pub struct Admission {
    reservation: u64,
    slots: usize,
}
impl Admission {
    pub fn new(threads: u32, m: u64, d: u64, i: u64) -> Result<Self, SetupError> {
        let reservation = m
            .checked_add(d)
            .filter(|s| *s > 0)
            .ok_or(SetupError::Admission)?;
        let slots = u64::from(threads).min(i / reservation);
        if slots == 0 {
            return Err(SetupError::Admission);
        }
        Ok(Self {
            reservation,
            slots: usize::try_from(slots).map_err(|_| SetupError::Admission)?,
        })
    }
    #[must_use]
    pub fn slots(self) -> usize {
        self.slots
    }
    #[must_use]
    pub fn reservation(self) -> u64 {
        self.reservation
    }
}
impl Controller {
    pub(super) fn peek_queue(&mut self) -> Result<(), super::Halt> {
        let p = &mut self.prepared;
        for (i, input) in p.inputs.iter().enumerate() {
            if input.phase != InputPhase::Pending {
                continue;
            }
            let index = u32::try_from(i).map_err(|_| ())?;
            let members = p
                .walker
                .view(&p.graph, index, p.spec.max_view_bundles)
                .map_err(|_| ())?;
            for &bundle in members {
                p.peek_members.set(bundle as usize, true).ok_or(())?;
            }
        }
        for i in 0..p.bundles.len() {
            if p.peek_members.get(i) == Some(true) {
                p.peek_queue.push(u32::try_from(i).map_err(|_| ())?);
            }
        }
        let lengths = &p.bundles;
        p.peek_queue.sort_unstable_by_key(|&i| {
            (
                std::cmp::Reverse(
                    lengths
                        .get(i as usize)
                        .and_then(|b| b.stage)
                        .and_then(|s| s.facts)
                        .map_or(0, |f| f.size),
                ),
                p.graph.rank(i),
            )
        });
        Ok(())
    }
    pub(super) fn ready_queue(&mut self) {
        let p = &mut self.prepared;
        for (i, input) in p.inputs.iter_mut().enumerate() {
            if input.phase == InputPhase::Pending {
                input.phase = InputPhase::Ready;
                if let Ok(i) = u32::try_from(i) {
                    p.ready.push(i);
                }
            }
        }
        p.ready.sort_unstable_by_key(|&i| {
            (
                std::cmp::Reverse(
                    p.bundles
                        .get(i as usize)
                        .and_then(|b| b.stage)
                        .and_then(|s| s.facts)
                        .map_or(0, |f| f.size),
                ),
                p.graph.rank(i),
            )
        });
    }
    /// Select all holders first. Own failure wins; otherwise the least id.
    pub(super) fn mark(&mut self, peeks: bool) -> Result<(), super::Halt> {
        let p = &mut self.prepared;
        p.failures.clear();
        for (i, input) in p.inputs.iter_mut().enumerate() {
            if input.phase != InputPhase::Pending {
                continue;
            }
            let i = u32::try_from(i).map_err(|_| ())?;
            let failed = |j: u32| {
                p.bundles.get(j as usize).is_some_and(|b| {
                    if peeks {
                        b.peek
                            .is_some_and(|f| matches!(f.outcome, Some(Outcome::Failed(_))))
                    } else {
                        b.stage.is_some_and(|s| {
                            matches!(s.failure, Some(crate::stage::Failure::Data(_)))
                        })
                    }
                })
            };
            let selected = if failed(i) {
                Some(i)
            } else {
                p.walker
                    .view(&p.graph, i, p.spec.max_view_bundles)
                    .map_err(|_| ())?
                    .iter()
                    .copied()
                    .filter(|&j| failed(j))
                    .min_by_key(|&j| p.graph.rank(j))
            };
            if let Some(bundle) = selected {
                input.failure_bundle = Some(bundle);
                p.uncommitted.set(i as usize, true).ok_or(())?;
                p.failures.push(i);
            }
        }
        p.failures.sort_unstable_by_key(|&i| p.graph.rank(i));
        Ok(())
    }
}
