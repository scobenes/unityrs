use super::{Controller, Halt, Hooks, Point, PreparedSpool};
use crate::sys::Shared;
use crate::{
    attempt::{AttemptEnd, ReasonCode},
    control::Control,
    execution::Execution,
    job::RunFailure,
    peek::word::FailureCode,
    reason::{Limit, LimitKind},
    records::{self, Figures, Reason, Status, Summary, Text},
    spec::model::Spec,
    spool::Directory,
    stage::DataFailure,
};
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, OnceLock},
};

#[derive(Clone, Copy)]
pub(crate) struct FailureEnd {
    figures: Option<Figures>,
    spooled: u64,
    claimed: u64,
    memory_peak: u64,
}
#[allow(clippy::large_enum_variant)] // Bounded inline copy of the fixed end.
#[derive(Clone, Copy)]
enum Selected {
    Stage(DataFailure, Option<crate::stage::Integrity>),
    Peek(FailureCode),
    Attempt(AttemptEnd),
}
struct Write {
    input: u32,
    bundle: u32,
    selected: Selected,
    owner: Shared<OnceLock<Directory>>,
    spec: Arc<Spec>,
    spool: PreparedSpool,
    control: Arc<dyn Control>,
    hooks: Arc<dyn Hooks>,
    memory: Arc<crate::job_memory::Plan>,
}
impl Write {
    fn run(&self) -> Result<Option<FailureEnd>, RunFailure> {
        let internal = || RunFailure::new(ReasonCode::Internal);
        let directory = self
            .spool
            .create_directory("fail", self.input)
            .map_err(RunFailure::from)?;
        self.owner.set(directory).map_err(|_| internal())?;
        self.hooks.point(Point::FailureCreated(self.input));
        if self.control.stopped() {
            return Ok(None);
        }
        let input = self
            .spec
            .bundle(self.input as usize)
            .ok_or_else(internal)?
            .id();
        let bundle = self
            .spec
            .bundle(self.bundle as usize)
            .ok_or_else(internal)?
            .id();
        let mut reason_text = [0; 128];
        let (reason, detail) = text(
            &self.selected,
            self.spec.max_decoded_bytes,
            &mut reason_text,
        )?;
        let reason = if self.bundle == self.input {
            Reason::Text(Text::Prepared(reason, false))
        } else {
            Reason::Dependency {
                id: Text::from(bundle),
                reason: Text::Prepared(reason, false),
            }
        };
        let end = match &self.selected {
            Selected::Attempt(e) => Some(e),
            _ => None,
        };
        // Only this failed slot's M is available to the charged writer.
        let memory = self
            .memory
            .failures
            .get(self.input as usize)
            .ok_or_else(internal)?;
        let budget = &memory.budget;
        let meter = memory.meter.clone();
        let summary = records::write_failure_record_stoppable(
            self.owner.get().ok_or_else(internal)?,
            &meter,
            Summary {
                input,
                status: Status::Failed,
                reason: Some(reason),
                detail: detail.map(|s| Text::Prepared(s, false)),
                figures: end.map(|e| Figures {
                    skips: 0,
                    unresolved_scripts: 0,
                    ..e.figures
                }),
                spooled_part: end.map_or(0, |e| e.spooled_part),
                claimed_bytes: 0,
                cut: end.is_some_and(|e| e.cut),
            },
            &|| self.control.stopped(),
        );
        drop(meter);
        let accounts = budget.snapshot();
        if !accounts.is_clear() {
            return Err(internal());
        }
        match summary {
            Ok(s) => Ok(Some(FailureEnd {
                figures: s.figures,
                spooled: s.spooled_part,
                claimed: s.claimed_bytes,
                memory_peak: accounts.input.peak,
            })),
            Err(records::Error::Spool(crate::spool::Error::Stopped)) => Ok(None),
            Err(records::Error::Spool(e)) => Err(e.into()),
            Err(records::Error::Charge(_)) => Err(RunFailure::new(ReasonCode::HostMemory)),
            Err(_) => Err(internal()),
        }
    }
}
impl Controller {
    fn reserve_failure(&mut self, end: Option<&AttemptEnd>) {
        // A failed attempt retains its slot until this writer completes. A
        // stage/peek failure borrows one otherwise unused M instead.
        self.failure_reservation = if end.is_none() {
            self.prepared.spec.max_decoded_bytes
        } else {
            0
        };
        self.sample_reservations();
    }
    fn release_failure(&mut self) {
        self.failure_reservation = 0;
        self.sample_reservations();
    }
    fn select_failure(
        &mut self,
        input: u32,
        peeks: bool,
        end: Option<&AttemptEnd>,
    ) -> Result<(Selected, u32), Halt> {
        if let Some(end) = end {
            let bundle = if let Some(dependency) = end.dependency {
                let mut walk = crate::view::BoundedWalker::try_new(
                    self.prepared.spec.max_view_bundles as usize,
                )
                .map_err(|e| self.io(e.into()))?;
                *walk
                    .view(
                        &self.prepared.graph,
                        input,
                        self.prepared.spec.max_view_bundles,
                    )
                    .map_err(|_| ())?
                    .get(dependency as usize)
                    .ok_or(())?
            } else {
                input
            };
            return Ok((Selected::Attempt(*end), bundle));
        }
        let bundle = self
            .prepared
            .inputs
            .get(input as usize)
            .and_then(|s| s.failure_bundle)
            .ok_or(())?;
        let state = self.prepared.bundles.get(bundle as usize).ok_or(())?;
        let selected = if peeks {
            let Some(crate::peek::word::Outcome::Failed(f)) = state.peek.and_then(|f| f.outcome)
            else {
                return Err(self.internal());
            };
            Selected::Peek(f)
        } else {
            let Some(crate::stage::Failure::Data(f)) = state.stage.and_then(|s| s.failure) else {
                return Err(self.internal());
            };
            Selected::Stage(f, state.stage.and_then(|s| s.integrity))
        };
        Ok((selected, bundle))
    }
    pub(super) fn write_failure(
        &mut self,
        execution: &mut Execution,
        input: u32,
        peeks: bool,
        end: Option<&AttemptEnd>,
    ) -> Result<(), Halt> {
        self.check()?;
        let (selected, bundle) = self.select_failure(input, peeks, end)?;
        self.reserve_failure(end);
        let owner = Shared::new(OnceLock::new()).map_err(|e| self.io(e.into()))?;
        self.failure_owner = Some(owner.clone());
        let args = Write {
            input,
            bundle,
            selected,
            owner,
            spec: self.prepared.spec.clone(),
            spool: self.spool.clone(),
            control: self.control.clone(),
            hooks: self.hooks.clone(),
            memory: self.prepared.memory.clone(),
        };
        let control = self.control.clone();
        let task = execution
            .spawn(move || {
                let before = crate::alloc::refusals();
                let result = catch_unwind(AssertUnwindSafe(|| args.run()))
                    .unwrap_or_else(|_| Err(RunFailure::new(ReasonCode::Internal)));
                // Drop the pool's owner/reference before publishing completion.
                drop(args);
                let result = if crate::alloc::refusals() == before {
                    result
                } else {
                    Err(RunFailure::new(ReasonCode::HostMemory))
                };
                if let Err(f) = result {
                    control.request_stop(f, crate::control::Culprit::None);
                }
                result.and_then(|s| s.ok_or(RunFailure::new(ReasonCode::Internal)))
            })
            .map_err(|e| self.io(e.into()))?;
        self.failure_task = Some(task);
        while !self
            .failure_task
            .as_ref()
            .is_some_and(crate::execution::Task::is_finished)
        {
            execution
                .wait()
                .map_err(|_| self.fail(RunFailure::new(ReasonCode::HostIo)))?;
            self.hooks.point(Point::PollReturned);
            // Complete this received end before receiving the next one.
            self.check()?;
        }
        self.check()?;
        let task = self.failure_task.take().ok_or(())?;
        let result = execution
            .finish(&task)
            .map_err(|()| self.internal())?
            .map_err(|f| self.fail(f))?;
        self.failure_writer_peak = self.failure_writer_peak.max(result.memory_peak);
        if let Some(facts) = self
            .prepared
            .inputs
            .get_mut(input as usize)
            .and_then(|s| s.received.as_mut())
        {
            facts.figures = result.figures.ok_or(())?;
            facts.spooled_part = result.spooled;
            facts.claimed_bytes = result.claimed;
        }
        let owner = self.failure_owner.take().ok_or(())?;
        self.pending_directory = Shared::try_unwrap(owner).map_err(|_| ())?.into_inner();
        self.commit(input)?;
        self.release_failure();
        Ok(())
    }
}
fn text<'a>(
    selected: &'a Selected,
    m: u64,
    scratch: &'a mut [u8; 128],
) -> Result<(&'a str, Option<&'a str>), RunFailure> {
    let internal = || RunFailure::new(ReasonCode::Internal);
    let mut limit = None;
    let (reason, detail) = match selected {
        Selected::Stage(f, integrity) => (
            match f {
                DataFailure::SourceMissing => "source_missing",
                DataFailure::SourceNotRegular => "source_not_regular",
                DataFailure::SourceChanged => "source_changed",
                DataFailure::IntegrityMismatch => "integrity_mismatch",
                DataFailure::InputBytes(n) => {
                    limit = Some(Limit {
                        kind: LimitKind::InputBytes,
                        value: *n,
                    });
                    ""
                }
            },
            integrity.map(crate::stage::Integrity::name),
        ),
        Selected::Peek(f) => match f {
            FailureCode::NotABundle => ("not_a_bundle", None),
            FailureCode::Container(d) => ("container_error", Some(d.name())),
            FailureCode::Limit(_) => {
                limit = Some(f.limit(m).ok_or_else(internal)?);
                ("", None)
            }
        },
        Selected::Attempt(e) => (
            match e.reason.ok_or_else(internal)? {
                ReasonCode::NotABundle => "not_a_bundle",
                ReasonCode::ContainerError => "container_error",
                ReasonCode::EntryError => "entry_error",
                ReasonCode::BudgetInput => "budget(input)",
                ReasonCode::BudgetDependency => "budget(dependency)",
                ReasonCode::Limit(l) => {
                    limit = Some(l);
                    ""
                }
                _ => return Err(internal()),
            },
            if e.reason == Some(ReasonCode::EntryError) {
                Some(e.entry_detail.as_str())
            } else {
                e.detail.map(crate::attempt::DetailCode::name)
            },
        ),
    };
    if let Some(limit) = limit {
        use std::io::Write;
        let mut cursor = std::io::Cursor::new(&mut scratch[..]);
        write!(cursor, "{limit}").map_err(|_| internal())?;
        let len = usize::try_from(cursor.position()).map_err(|_| internal())?;
        Ok((
            std::str::from_utf8(scratch.get(..len).ok_or_else(internal)?)
                .map_err(|_| internal())?,
            detail,
        ))
    } else {
        Ok((reason, detail))
    }
}
