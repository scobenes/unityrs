//! The perf bench (docs/plans/core.md sections 6, 8 and 10): unityrs's
//! side of the throughput run, and the counts plan section 8 asks for. The
//! corpus driver, `tools/perf/perf.py`, runs it beside UnityPyBoost's side
//! (`tools/perf/bench_unitypy.py`), one pass per process, each under the
//! summed-PSS sampler (`tools/perf/mem_peak.py`):
//!
//! ```text
//! cargo run --release -p unityrs-dump --example bench -- INDEX.json [-j N]
//!     [--budget unbounded|counting|tracked] [--load eager|lazy]
//!     [--kind KIND]... [--sample N] [--pin] [--per-input FILE]
//! cargo run --release -p unityrs-dump --example bench -- --many-blocks [--reads N]
//! ```
//!
//! `--many-blocks` reads no index: it times runs of many blocks, which the
//! corpus lacks, built by the testkit (`runs`).
//!
//! The inputs are the dump's (`unityrs_dump::select`, as `unityrs dump
//! --index` selects them: the index's bundle and UnityPy-sample entries by
//! default, ordered by name; `--sample N` keeps N spread evenly over that
//! order, as `unitypy_dump.py --sample` does). Each is read once before the
//! pass, to warm the page cache. Then, serially on this thread (`-j 1`, the
//! default) or on N threads that take the inputs in order, each input is
//! read from disk (charged to its meter), detected, parsed and loaded, and
//! every object of every serialized file (UnityPy's `files` dicts, in
//! order) is read with `parse_as_dict(true)`: the work of UnityPy's
//! `Environment().load_file(path)` and `obj.parse_as_dict()` on each object.
//!
//! - `--load eager` (default) loads as UnityPy does (`load_deep`); `lazy`
//!   opens each bundle and reads every directory row's entry in directory
//!   order (plan section 8, R2), then the values.
//! - `--budget unbounded` (default) books with `Unbounded`, the default
//!   context's; `counting` with one shared `Counting`, a batch engine's;
//!   `tracked` with a tracker per input that adds to the process's, which
//!   gives each input's peak `Meter::used()` and the peak of the sum of
//!   live charges (R3, R10).
//! - `--pin` keeps every value, drops the input's handles, and measures
//!   what the values keep alive beyond their own charges, then what one
//!   value holding a slice keeps alive (R3).
//!
//! It prints one JSON object: throughput, CPU time (each thread's, from
//! `/proc/thread-self/schedstat`) with the codecs' share, peak RSS, the
//! bundles' counts (`BundleFile::counts`), the serialized files' copies out
//! of their runs (`SerializedFile::copies`, R3), the version sources (R5),
//! and, as the flags ask, meter peaks and pinning. `--per-input` writes a
//! JSON line per input, by its position in the order, never its name.

mod budget;
mod clock;
mod runs;
mod summary;
#[cfg(test)]
mod tests;
mod work;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use unityrs_core::FileContext;
use unityrs_dump::select::{PyStr, Selection, collect};

use budget::{BudgetKind, Tracker};
use clock::{Clock, DecodeTime, timed_codecs};
use work::{Done, Load};

/// The command line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Args {
    pub index: PathBuf,
    pub jobs: usize,
    pub budget: BudgetKind,
    pub load: Load,
    pub kinds: Vec<String>,
    pub sample: Option<i64>,
    pub pin: bool,
    pub per_input: Option<PathBuf>,
}

const USAGE: &str = "usage: bench INDEX.json [-j N] [--budget unbounded|counting|tracked] \
                     [--load eager|lazy] [--kind KIND]... [--sample N] [--pin] [--per-input FILE]\n       \
                     bench --many-blocks [--reads N]";

impl Args {
    pub fn parse(args: &[String]) -> Result<Self, String> {
        let mut index = None;
        let mut out = Self {
            index: PathBuf::new(),
            jobs: 1,
            budget: BudgetKind::Unbounded,
            load: Load::Eager,
            kinds: Vec::new(),
            sample: None,
            pin: false,
            per_input: None,
        };
        let mut it = args.iter();
        while let Some(a) = it.next() {
            let mut value = || it.next().ok_or_else(|| format!("{a} needs a value"));
            match a.as_str() {
                "-j" | "--jobs" => {
                    out.jobs = value()?
                        .parse()
                        .ok()
                        .filter(|n| *n > 0)
                        .ok_or("-j takes a count of at least 1")?;
                }
                "--budget" => {
                    let v = value()?;
                    out.budget = BudgetKind::parse(v).ok_or(format!("no budget {v}"))?;
                }
                "--load" => {
                    out.load = match value()?.as_str() {
                        "eager" => Load::Eager,
                        "lazy" => Load::Lazy,
                        v => return Err(format!("no load {v}")),
                    };
                }
                "--kind" => out.kinds.push(value()?.clone()),
                "--sample" => {
                    out.sample = Some(value()?.parse().map_err(|_| "--sample takes a count")?);
                }
                "--pin" => out.pin = true,
                "--per-input" => out.per_input = Some(PathBuf::from(value()?)),
                s if s.starts_with('-') => return Err(format!("unknown flag {s}")),
                s if index.is_none() => index = Some(PathBuf::from(s)),
                s => return Err(format!("a second index {s}")),
            }
        }
        out.index = index.ok_or("no index")?;
        Ok(out)
    }

    /// The dump's selection of the index's inputs.
    #[must_use]
    pub fn selection(&self) -> Selection {
        let mut sel = Selection {
            inputs: vec![self.index.clone()],
            index: true,
            sample: self.sample,
            ..Selection::default()
        };
        if !self.kinds.is_empty() {
            sel.kinds = self.kinds.iter().map(|k| PyStr::from(k.as_str())).collect();
        }
        sel
    }
}

/// One pass's results, in input order.
pub struct Pass {
    pub done: Vec<Done>,
    /// Each input's peak `Meter::used()` (a tracked budget only).
    pub meter_peaks: Vec<Option<u64>>,
    pub wall_s: f64,
    /// The worker threads' CPU (or wall) time, summed.
    pub threads_ns: u64,
    pub clock: Clock,
    pub decode: Arc<DecodeTime>,
    pub process: Arc<Tracker>,
}

/// An input's results and its peak `Meter::used()` (tracked only).
type Slot = (Done, Option<u64>);

/// Runs the pass `args` names over `paths`.
pub fn run(args: &Args, paths: &[PathBuf]) -> Pass {
    let clock = Clock::new();
    let decode = Arc::new(DecodeTime::default());
    let process = Arc::new(Tracker::default());
    let mut base = FileContext::unitypy();
    base.codecs = Arc::new(timed_codecs(clock, &decode));
    let shared = args.budget.shared();
    if let Some(b) = &shared {
        base.budget = b.clone();
    }
    let base = Arc::new(base);
    let one = |i: usize| {
        let (ctx, tracker) = if shared.is_some() {
            (base.clone(), None)
        } else {
            let t = Tracker::child(&process);
            let mut c = (*base).clone();
            c.budget = t.budget();
            (Arc::new(c), Some(t))
        };
        let path = paths.get(i).cloned().unwrap_or_default();
        let done = work::work(&path, &ctx, args.load, args.pin, clock);
        (done, tracker.map(|t| t.peak()))
    };
    let n = paths.len();
    let slots: Mutex<Vec<Option<Slot>>> = Mutex::new(vec![None; n]);
    let next = AtomicUsize::new(0);
    let threads_ns = Mutex::new(0u64);
    let worker = || {
        let t0 = clock.now();
        loop {
            let i = next.fetch_add(1, Ordering::Relaxed);
            if i >= n {
                break;
            }
            let r = one(i);
            if let Some(s) = slots
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get_mut(i)
            {
                *s = Some(r);
            }
        }
        let spent = clock.now().saturating_sub(t0);
        *threads_ns.lock().unwrap_or_else(PoisonError::into_inner) += spent;
    };
    let start = Instant::now();
    if args.jobs == 1 {
        worker();
    } else {
        std::thread::scope(|s| {
            for _ in 0..args.jobs {
                s.spawn(worker);
            }
        });
    }
    let wall_s = start.elapsed().as_secs_f64();
    let (done, meter_peaks) = slots
        .into_inner()
        .unwrap_or_else(PoisonError::into_inner)
        .into_iter()
        .map(Option::unwrap_or_default)
        .unzip();
    Pass {
        done,
        meter_peaks,
        wall_s,
        threads_ns: threads_ns
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner),
        clock,
        decode,
        process,
    }
}

fn main() -> ExitCode {
    let given: Vec<String> = std::env::args().skip(1).collect();
    if let Some((first, rest)) = given.split_first()
        && first == "--many-blocks"
    {
        return match runs::reads(rest).and_then(|n| runs::many_blocks(&runs::CASES, n)) {
            Ok(v) => {
                println!("{v}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("bench: {e}\n{USAGE}");
                ExitCode::from(2)
            }
        };
    }
    let args = match Args::parse(&given) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("bench: {e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let inputs = match collect(&args.selection()) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("bench: {e}");
            return ExitCode::from(2);
        }
    };
    let paths: Vec<PathBuf> = inputs.into_iter().map(|i| i.path).collect();
    let mut input_bytes = 0u64;
    for (i, p) in paths.iter().enumerate() {
        match std::fs::read(p) {
            Ok(b) => input_bytes += u64::try_from(b.len()).unwrap_or(u64::MAX),
            Err(e) => {
                eprintln!("bench: input {i} of the selection: {e}");
                return ExitCode::from(1);
            }
        }
    }
    let pass = run(&args, &paths);
    if let Some(path) = &args.per_input
        && let Err(e) = std::fs::write(path, summary::per_input(&pass))
    {
        eprintln!("bench: {}: {e}", path.display());
        return ExitCode::from(1);
    }
    println!("{}", summary::summary(&args, &pass, input_bytes));
    ExitCode::SUCCESS
}
