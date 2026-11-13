use super::{
    Hooks, PeekArgs, PeekEnd, Point, State,
    word::{FailureCode, Outcome},
};
use crate::{
    attempt::{DetailCode, ReasonCode, classify},
    budget::SealedBudget,
    job::RunFailure,
    spool::{self, OwnedFile},
};
use std::{
    cell::Cell,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Arc,
};
use unityrs_core::{FileContext, budget::Reservation, limits::Meter};
use unityrs_export::ngui::{self, Data, Failure};

struct Engine<'a> {
    budget: Arc<SealedBudget>,
    stop: &'a dyn crate::control::Control,
    hooks: &'a dyn Hooks,
    decoded: Cell<u64>,
    fault: Cell<bool>,
}
impl ngui::Hooks for Engine<'_> {
    fn stopped(&self) -> bool {
        self.stop.stopped() || self.fault.get() || self.hooks.stopped()
    }
    fn refusals(&self) -> Option<u64> {
        Some(self.budget.snapshot().refusals())
    }
    fn meter(&self, meter: &Meter) {
        self.decoded.set(self.decoded.get().max(meter.decoded()));
        // This also runs from a drop guard during unwind. Never double-panic.
        if !self.fault.get() && catch_unwind(AssertUnwindSafe(|| self.hooks.meter(meter))).is_err()
        {
            self.fault.set(true);
        }
    }
}
impl Engine<'_> {
    fn check(&self) -> Result<(), Failure> {
        if self.fault.get() {
            Err(classify::internal())
        } else if ngui::Hooks::stopped(self) {
            Err(Failure::Stopped)
        } else {
            Ok(())
        }
    }
    fn point(&self, point: Point) -> Result<(), Failure> {
        self.hooks.point(point);
        self.check()
    }
}
struct Sample<'a, 'b>(&'a Engine<'b>, &'a Meter);
impl Drop for Sample<'_, '_> {
    fn drop(&mut self) {
        ngui::Hooks::meter(self.0, self.1);
    }
}

#[must_use]
pub fn run_peek(mut args: PeekArgs<'_>) -> PeekEnd {
    let before = crate::alloc::refusals();
    let budget = Arc::new(SealedBudget::input_only(args.m));
    let engine = Engine {
        budget: budget.clone(),
        stop: args.stop,
        hooks: args.hooks,
        decoded: Cell::new(0),
        fault: Cell::new(false),
    };
    let mut end = PeekEnd {
        bundle: args.bundle,
        state: State::Done,
        outcome: None,
        failure: None,
        input_peak: 0,
        decode_work: 0,
        claimed_bytes: 0,
        accounts: crate::budget::Snapshot::default(),
        slot_releasable: true,
    };
    match catch_unwind(AssertUnwindSafe(|| execute(&mut args, &engine))) {
        Ok(Ok(outcome)) => end.outcome = Some(outcome),
        Ok(Err(failure)) => failed(&mut end, failure, args.m),
        Err(_) => run_failure(&mut end, RunFailure::new(ReasonCode::Internal)),
    }
    // BeforeWord is a fault/hold seam too: run it before the final audit so
    // an observer's late charge or absorbed allocation refusal cannot publish.
    if end.state == State::Done {
        match catch_unwind(AssertUnwindSafe(|| engine.point(Point::BeforeWord))) {
            Ok(Ok(())) => {}
            Ok(Err(f)) => failed(&mut end, f, args.m),
            Err(_) => run_failure(&mut end, RunFailure::new(ReasonCode::Internal)),
        }
    }
    audit(&mut end, &engine, before);
    end.claimed_bytes = args.file.as_ref().map_or(0, OwnedFile::claimed);
    if end.state == State::Done {
        let publication = catch_unwind(AssertUnwindSafe(|| {
            let outcome = end.outcome.ok_or_else(classify::internal)?;
            args.word
                .publish(outcome)
                .map_err(|_| classify::internal())?;
            engine.point(Point::AfterWord)
        }));
        match publication {
            Ok(Ok(())) => {}
            Ok(Err(f)) => failed(&mut end, f, args.m),
            Err(_) => run_failure(&mut end, RunFailure::new(ReasonCode::Internal)),
        }
    }
    // A post-publication stop/fault keeps the word and file, but no successful
    // end. Resample for hooks that retained a meter or refused an allocation.
    audit(&mut end, &engine, before);
    if end.state == State::RunFailure {
        args.stop.request_stop(
            end.failure.unwrap_or(RunFailure::new(ReasonCode::Internal)),
            crate::control::Culprit::Bundle(end.bundle),
        );
    }
    if end.state == State::RunFailure
        && catch_unwind(AssertUnwindSafe(|| args.hooks.request_stop(&end))).is_err()
    {
        run_failure(&mut end, RunFailure::new(ReasonCode::Internal));
    }
    end
}
fn audit(end: &mut PeekEnd, engine: &Engine<'_>, before: u64) {
    end.accounts = engine.budget.snapshot();
    end.slot_releasable = end.accounts.is_clear();
    end.input_peak = end.accounts.input.peak;
    end.decode_work = engine.decoded.get();
    if engine.fault.get()
        || !end.slot_releasable
        || end.accounts.dependency.refusals != 0
        || end.accounts.provider_refusals != 0
    {
        run_failure(end, RunFailure::new(ReasonCode::Internal));
    }
    if crate::alloc::refusals() != before {
        run_failure(end, RunFailure::new(ReasonCode::HostMemory));
    }
}

fn execute(args: &mut PeekArgs<'_>, engine: &Engine<'_>) -> Result<Outcome, Failure> {
    if args.file.is_some() || args.word.load() != Ok(Outcome::Pending) {
        return Err(classify::internal());
    }
    engine.check()?;
    let mut ctx = FileContext::unitypy();
    ctx.budget = engine.budget.clone();
    ctx.limits.max_input_memory = args.m;
    ctx.limits.max_input_bytes = args.m;
    ctx.limits.max_decode_work = args.m.saturating_mul(4);
    // Exactly one meter; the core receives the same one for every charge.
    let meter = Meter::for_input(&ctx.limits, engine.budget.clone(), Reservation::Input);
    let _sample = Sample(engine, &meter);
    ngui::Hooks::meter(engine, &meter);
    engine.point(Point::Open)?;
    let bits = ngui::peek(
        args.name,
        Data::Path(args.staged),
        &Arc::new(ctx),
        &meter,
        engine,
    )?;
    engine.check()?;
    if !bits.bytes().iter().any(|b| *b != 0) {
        return Ok(Outcome::Clear(bits.rows()));
    }
    let path = crate::stage::try_path(args.directory, args.bundle, args.inputs, true)
        .map_err(spool_failure)?;
    *args.file = Some(
        OwnedFile::create(path, args.counter.clone(), args.fs.clone()).map_err(spool_failure)?,
    );
    let file = args.file.as_mut().ok_or_else(classify::internal)?;
    file.reserve(bits.bytes().len() as u64)
        .map_err(spool_failure)?;
    file.append(bits.bytes(), &|| ngui::Hooks::stopped(engine))
        .map_err(spool_failure)?;
    engine.point(Point::BeforeSeal)?;
    file.seal(&|| ngui::Hooks::stopped(engine))
        .map_err(spool_failure)?;
    Ok(Outcome::Disk(bits.rows()))
}
fn spool_failure(error: spool::Error) -> Failure {
    classify::record(crate::records::Error::Spool(error))
}
fn failed(end: &mut PeekEnd, failure: Failure, m: u64) {
    match failure {
        Failure::Stopped => {
            end.state = State::Stopped;
            end.outcome = None;
        }
        Failure::Run(reason) => {
            let code = match reason.reason.as_str() {
                "host_io" => ReasonCode::HostIo,
                "host_memory" => ReasonCode::HostMemory,
                "spool_full" => ReasonCode::SpoolFull,
                _ => ReasonCode::Internal,
            };
            let detail = reason.detail.as_deref().and_then(DetailCode::parse);
            if reason.detail.is_some() && detail.is_none() {
                run_failure(end, RunFailure::new(ReasonCode::Internal));
            } else {
                run_failure(
                    end,
                    RunFailure {
                        reason: code,
                        detail,
                    },
                );
            }
        }
        Failure::Input(reason) => {
            let code = match reason.reason.as_str() {
                "not_a_bundle" if reason.detail.is_none() => Some(FailureCode::NotABundle),
                "container_error" => reason
                    .detail
                    .as_deref()
                    .and_then(DetailCode::parse)
                    .map(FailureCode::Container),
                text if reason.detail.is_none() => classify::limit(text).and_then(|limit| {
                    let code = FailureCode::Limit(limit.kind);
                    (code.limit(m) == Some(limit)).then_some(code)
                }),
                _ => None,
            };
            if let Some(code) = code.filter(|c| Outcome::Failed(*c).encode().is_ok()) {
                end.outcome = Some(Outcome::Failed(code));
            } else {
                run_failure(end, RunFailure::new(ReasonCode::Internal));
            }
        }
    }
}
fn run_failure(end: &mut PeekEnd, failure: RunFailure) {
    RunFailure::reduce(&mut end.failure, failure);
    end.state = State::RunFailure;
    end.outcome = None;
}
