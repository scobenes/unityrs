//! One sealed staged-path attempt. Dispatch, commits and publication are callers'.
pub(crate) mod classify;
mod copy;
mod end;
pub(crate) mod hooks;
mod metadata;
mod options;
mod peek;
use crate::{
    budget::SealedBudget,
    records::{self, Figures, Status, Summary, Writer},
    spool::Directory,
    work::Work,
};
pub use end::{AttemptEnd, DetailCode, EntryDetail, ReasonCode, State};
use hooks::{Engine, probe};
pub use hooks::{Hooks, Point};
pub use options::Prepared;
pub use peek::{PeekEntry, PeekTable};
use std::{
    cell::{Cell, RefCell},
    panic::{AssertUnwindSafe, catch_unwind},
    path::Path,
    sync::{Arc, atomic::AtomicBool},
    time::Instant,
};
use unityrs_core::{
    FileContext, RawStr,
    budget::Reservation,
    env::EnvOptions,
    limits::{Meter, MeteredVec},
};
use unityrs_export::ngui::{
    self, Admit, Data, Failure, KeyTable, MemberTemplate, Options, Registration, Staged,
};

#[derive(Clone, Copy, Debug)]
pub struct StagedBundle<'a> {
    pub id: &'a str,
    pub name: &'a RawStr,
    pub path: &'a Path,
}
impl<'a> StagedBundle<'a> {
    fn staged(self) -> Staged<'a> {
        Staged {
            id: self.id,
            name: self.name,
            data: Data::Path(self.path),
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct AttemptView<'a> {
    pub input_index: u64,
    pub input: StagedBundle<'a>,
    /// Excludes X; strictly ascending bytewise ids.
    pub members: &'a [StagedBundle<'a>],
}
#[derive(Clone, Copy, Debug)]
pub struct AttemptLimits {
    pub m: u64,
    pub d: u64,
    pub max_attempt_evictions: u64,
    pub max_export_work: u64,
}
/// Borrow prepared immutable state. The directory contains its shared spool
/// counter and preflight filesystem facts; its ownership survives this call.
#[derive(Clone, Copy)]
pub struct AttemptArgs<'a> {
    pub view: AttemptView<'a>,
    pub options: &'a Options,
    pub prefix: &'a str,
    pub members: &'a [MemberTemplate],
    /// Final archive keys, indexed by `MemberTemplate.archive`.
    pub archive_keys: &'a [&'a str],
    pub limits: AttemptLimits,
    pub peeks: &'a dyn ngui::PeekBits,
    pub directory: &'a Directory,
    pub stop: &'a AtomicBool,
    pub hooks: &'a dyn Hooks,
}
/// Open, register, extract, copy and seal, then drop and audit, on this thread.
/// `Done` alone is eligible for a later controller commit. This never renames.
#[must_use]
pub fn run_attempt(args: AttemptArgs<'_>) -> AttemptEnd {
    let started = Instant::now();
    let before = crate::alloc::refusals();
    let budget = Arc::new(SealedBudget::new(args.limits.m, args.limits.d));
    let engine = Engine {
        budget: budget.clone(),
        stop: args.stop,
        observer: args.hooks,
        work: RefCell::new(Work::new(args.limits.max_export_work)),
        decode_peak: Cell::new(0),
        evictions: Cell::new(0),
        fault: Cell::new(false),
    };
    let mut end = AttemptEnd::new(args.view.input_index);
    // No charged result escapes this closure, including on unwind.
    let result = catch_unwind(AssertUnwindSafe(|| {
        execute(&args, &engine, &mut end, started)
    }));
    match result {
        Ok(Ok(())) => {}
        Ok(Err(failure)) => classify::failure(&mut end, &failure, args.view),
        Err(_) => end.run_failure(ReasonCode::Internal),
    }
    if engine.fault.get() {
        end.run_failure(ReasonCode::Internal);
    }
    end.accounts = budget.snapshot();
    end.slot_releasable = end.accounts.is_clear();
    if !end.slot_releasable {
        end.run_failure(ReasonCode::Internal);
    }
    if crate::alloc::refusals() != before {
        end.run_failure(ReasonCode::HostMemory);
    }
    end.figures.input_peak = end.accounts.input.peak;
    end.figures.dependency_peak = end.accounts.dependency.peak;
    end.figures.decode_work_peak = engine.decode_peak.get();
    end.figures.evictions = engine.evictions.get();
    end.figures.export_work = engine.work.borrow().export_work();
    end.spooled_part = engine.work.borrow().spooled_part();
    end.claimed_bytes = args.directory.claimed();
    if end.state != State::Done {
        end.figures.ms = elapsed(started);
        end.figures.skips = 0;
        end.figures.unresolved_scripts = 0;
        end.skips_expected = 0;
    }
    if end.state == State::RunFailure {
        // The observer cannot unwind across the public boundary either.
        if catch_unwind(AssertUnwindSafe(|| args.hooks.request_stop(&end))).is_err() {
            end.run_failure(ReasonCode::Internal);
        }
    }
    end
}
fn elapsed(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}
fn execute(
    args: &AttemptArgs<'_>,
    engine: &Engine<'_>,
    end: &mut AttemptEnd,
    started: Instant,
) -> Result<(), Failure> {
    let (env, meter) = context(args, engine);
    let _sample = engine.track(&meter);
    // Member ids are charged before allocation; registration validates ordering.
    let mut ids = MeteredVec::new(&meter);
    for m in args.view.members {
        probe(engine, || {
            ids.push(m.id).map_err(|e| ngui::classify::charge(&e))
        })?;
    }
    let admit = Admit {
        env: &env,
        meter: &meter,
        hooks: engine,
        max_evictions: args.limits.max_attempt_evictions,
    };
    let mut registration = Registration::open(args.view.input.staged(), ids.as_slice(), admit)?;
    engine.point(Point::Opened)?;
    let undeclared = metadata::dependencies(registration.own(), args.view, &meter, engine)?;
    engine.sample(registration.own().meter());
    engine.point(Point::Metadata)?;
    let mut names = metadata::Names::new(&meter);
    let mut loaded = |id: &str, bundle: &unityrs_core::files::BundleFile| {
        let holder = if id == args.view.input.id {
            args.view.members.len()
        } else {
            args.view
                .members
                .binary_search_by(|m| m.id.as_bytes().cmp(id.as_bytes()))
                .ok()
                .ok_or_else(classify::internal)?
        };
        names.loaded(holder, bundle, &meter, engine)
    };
    for m in args.view.members {
        let result = registration.member(&m.staged(), args.peeks, &mut loaded);
        end.figures.evictions = registration.stats().dependencies_evicted;
        result?;
    }
    let registered = registration.finish(args.peeks, &mut loaded)?;
    engine.point(Point::Registered)?;
    let mut writer = engine.records(|_| Writer::new(args.directory, &meter))?;
    end.diagnostics = names.write(args.view, &mut writer, engine)?;
    for name in undeclared.as_slice() {
        metadata::diagnostic(
            &mut writer,
            engine,
            args.view.input.id,
            records::DiagnosticKind::UndeclaredDependency,
            None,
            name,
        )?;
        end.diagnostics += 1;
    }
    drop(names);
    drop(undeclared);
    let mut keys = KeyTable::new(&meter);
    let cut = {
        let mut sink = |output| copy::output(args, engine, &mut writer, end, output);
        let result = ngui::cut(
            ngui::Context {
                registered: &registered,
                meter: &meter,
                input: args.view.input.id,
                hooks: engine,
                max_evictions: args.limits.max_attempt_evictions,
                prefix: args.prefix,
                members: args.members,
                keys: &mut keys,
                sink: &mut sink,
            },
            args.options,
        );
        end.figures.evictions = registered.env.stats().dependencies_evicted;
        engine.sample(registered.own.meter());
        result?
    };
    engine.observer.sheets(&cut.sheets);
    for path in &cut.nested_containers {
        metadata::diagnostic(
            &mut writer,
            engine,
            args.view.input.id,
            records::DiagnosticKind::NestedContainer,
            Some(args.view.input.id),
            path,
        )?;
        end.diagnostics += 1;
    }
    for skip in cut.plan_skips.iter().chain(&cut.export_skips) {
        copy::skip(args.view.input.id, engine, &mut writer, end, skip)?;
    }
    end.figures.unresolved_scripts = cut.unresolved_scripts;
    drop(cut);
    drop(keys);
    drop(registered);
    engine.sample(&meter);
    // Drop the core and cut before recording the terminal peak figures. The
    // records buffer remains charged until its final flush and close.
    engine.point(Point::BeforeSeal)?;
    seal(args, engine, end, started, writer)
}
fn seal(
    args: &AttemptArgs<'_>,
    engine: &Engine<'_>,
    end: &mut AttemptEnd,
    started: Instant,
    writer: Writer,
) -> Result<(), Failure> {
    let snapshot = engine.budget.snapshot();
    end.figures = Figures {
        input_peak: snapshot.input.peak,
        dependency_peak: snapshot.dependency.peak,
        decode_work_peak: engine.decode_peak.get(),
        ms: elapsed(started),
        ..end.figures
    };
    let status = if end.figures.skips == 0 {
        Status::Complete
    } else {
        Status::CompleteWithSkips
    };
    engine.point(Point::BeforeFinish)?;
    let summary = engine.records(|work| {
        writer.finish_stoppable(
            Summary {
                input: args.view.input.id,
                status,
                reason: None,
                detail: None,
                figures: Some(end.figures),
                spooled_part: 0,
                claimed_bytes: 0,
                cut: false,
            },
            work,
            &|| ngui::Hooks::stopped(engine),
        )
    })?;
    end.figures = summary.figures.ok_or_else(classify::internal)?;
    Ok(())
}

fn context(args: &AttemptArgs<'_>, engine: &Engine<'_>) -> (EnvOptions, Arc<Meter>) {
    let mut ctx = FileContext::unitypy();
    ctx.budget = engine.budget.clone();
    ctx.limits.max_input_memory = args.limits.m;
    ctx.limits.max_input_bytes = args.limits.m;
    ctx.limits.max_decode_work = args.limits.m.saturating_mul(4);
    let meter = Meter::for_input(&ctx.limits, engine.budget.clone(), Reservation::Input);
    let mut env = EnvOptions::batch(Arc::new(ctx));
    env.dep_cache_bytes = args.limits.d;
    (env, meter)
}
