//! `unityrs`: the canonical dump and the value-case protocol (plan section 5).
//!
//! ```text
//! unityrs dump INPUT... -o OUT.jsonl[.gz] [--index] [--kind K] [--build B]
//!     [--allow-index-mismatch] [--name GLOB]... [--sample N] [--limit N]
//!     [--levels L0,L1,L2,L3] [-j N] [--stats PATH] [-q] [--tpk PATH] [--reverse-prepass]
//!     [--unbounded KIND[,KIND...]]
//! unityrs values CASES.json
//! unityrs batch --check JOB.json | --schema | JOB.json
//! unityrs --version
//! ```
//!
//! `dump` takes `tools/parity/unitypy_dump.py`'s options, parsed as its
//! parser parses them ([`args`]), and runs [`unityrs_dump::run::run`],
//! exiting as the oracle exits: status 0 when the dump is written, 1 when it
//! fails (an index Python cannot read, an index mismatch, an input that
//! cannot be read, a dumper panic), 2 on a bad argument or path, and 3 when
//! the dump is written but every input in it crashed (ran its dumper out of
//! memory), which says more about the machine than the inputs.
//! `values` runs the spike's value-case protocol
//! ([`unityrs_dump::values`]), which `tools/parity/value_cases.py` drives.
//! `batch` is the engine's surface ([`batch`], `docs/BATCH_SPEC.md`), its
//! exit codes section 9's; `--version` prints `unityrs <version> spec
//! <highest spec_version>` (section 10).
//!
//! The binary's allocator is the engine's counting allocator
//! ([`unityrs_batch::alloc`]), which counts each refusal and each thread's
//! live bytes, and otherwise is `System`.

// No panics on untrusted input (plan section 3.15): the CLI parses its
// arguments, which a caller chooses. Tests may opt out.
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::todo,
        clippy::unimplemented,
        clippy::unreachable,
        clippy::indexing_slicing
    )
)]

mod args;
mod batch;

use std::ffi::OsStr;
use std::io::Write;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

#[cfg(not(test))]
use unityrs_batch::alloc::Counting;
use unityrs_batch::progress::clock::Clock;
use unityrs_batch::reason::Code;

use unityrs_dump::DumpOptions;
use unityrs_dump::run::{RunError, RunOptions, Summary};
use unityrs_dump::select::{PyStr, Selection};

#[cfg(test)]
mod test_alloc;

#[cfg(not(test))]
#[global_allocator]
static ALLOCATOR: Counting = Counting::new();

const USAGE: &str = "usage: unityrs {dump,values,batch} ...\n\n\
  unityrs dump INPUT... -o OUT.jsonl[.gz] [options]   the canonical dump (dump -h)\n\
  unityrs values CASES.json                           the value-case protocol\n\
  unityrs batch --check JOB.json | --schema           the batch engine\n\
  unityrs --version                                   the version and spec version";

const VALUES_USAGE: &str = "usage: unityrs values CASES.json";

/// `ap.error`: the usage line, `prog: error: msg`, status 2.
fn usage_error(usage: &str, prog: &str, msg: &str) -> ExitCode {
    eprintln!("{usage}\n{prog}: error: {msg}");
    ExitCode::from(2)
}

fn run_options(a: args::DumpArgs) -> Result<RunOptions, String> {
    let mut dump = DumpOptions {
        levels: a.levels,
        // `run_jobs`: `workers = max(1, workers)`.
        jobs: usize::try_from(a.jobs.max(1)).unwrap_or(usize::MAX),
        lazy_reverse: a.lazy_reverse,
        ..DumpOptions::default()
    };
    // Before `--tpk`, whose provider is opened under the context's limits.
    dump = dump
        .with_unbounded(&a.unbounded)
        .map_err(|k| format!("--unbounded: {k:?} bounds no input"))?;
    if let Some(path) = &a.tpk {
        // A TPK that does not load is refused, never dumped without it.
        dump = dump
            .with_tpk(path)
            .map_err(|e| format!("--tpk {}: {e}", path.display()))?;
    }
    Ok(RunOptions {
        selection: Selection {
            inputs: a.inputs,
            index: a.index,
            kinds: a.kind.iter().map(|k| PyStr::from_os_bytes(k)).collect(),
            builds: a
                .build
                .map(|b| b.iter().map(|v| PyStr::from_os_bytes(v)).collect()),
            names: a.name,
            sample: a.sample,
            limit: a.limit,
        },
        output: a.output,
        dump,
        stats: a.stats,
        quiet: a.quiet,
        allow_index_mismatch: a.allow_index_mismatch,
    })
}

fn dump(rest: &[Vec<u8>]) -> ExitCode {
    let err = |msg: &str| usage_error(args::USAGE, "unityrs dump", msg);
    let parsed = match args::parse(rest) {
        Ok(args::Parsed::Help) => {
            println!("{}\n\n{}", args::USAGE, args::HELP);
            return ExitCode::SUCCESS;
        }
        Ok(args::Parsed::Dump(a)) => a,
        Err(args::Usage(msg)) => return err(&msg),
    };
    let opts = match run_options(*parsed) {
        Ok(o) => o,
        Err(msg) => return err(&msg),
    };
    match unityrs_dump::run::run(&opts) {
        Ok(summary) => written(&summary),
        Err(RunError::Usage(msg)) => err(&msg),
        Err(e) => {
            eprintln!("unityrs dump: error: {e}");
            ExitCode::from(u8::try_from(e.status()).unwrap_or(1))
        }
    }
}

/// A written dump's status: 3 when every input crashed, after the
/// oracle's message (`unitypy_dump.py` `main`), else 0.
fn written(summary: &Summary) -> ExitCode {
    if summary.every_input_crashed() {
        eprintln!(
            "unityrs dump: every input ({}) crashed; the dump is written, but check the machine",
            summary.inputs
        );
        return ExitCode::from(3);
    }
    ExitCode::SUCCESS
}

fn values(path: &Path) -> ExitCode {
    let text = match std::fs::read(path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("unityrs values: {}: {e}", path.display());
            return ExitCode::from(1);
        }
    };
    // `Stdout`, not its lock, which no other thread may hold: the cases run
    // on a thread of their own, and each line is written as its case ends.
    let mut out = std::io::BufWriter::new(std::io::stdout());
    let done = unityrs_dump::values::run(&text, &mut out)
        .and_then(|()| out.flush().map_err(|e| e.to_string()));
    match done {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("unityrs values: {e}");
            ExitCode::from(1)
        }
    }
}

/// The engine's surfaces, `batch` and `--version`: a panic on their path
/// exits 70, with no `exit` line (`docs/BATCH_SPEC.md` section 10), and
/// writes only the panic's message.
fn engine(main_start: Instant, run: impl FnOnce(&Clock) -> ExitCode) -> ExitCode {
    let clock = Clock::process(main_start);
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&clock)))
        .unwrap_or_else(|_| ExitCode::from(Code::Internal.value()))
}

fn main() -> ExitCode {
    // `ms`'s origin: the process's start on the boot clock, else this.
    let main_start = Instant::now();
    let argv: Vec<Vec<u8>> = std::env::args_os()
        .skip(1)
        .map(OsStringExt::into_vec)
        .collect();
    let rest = argv.get(1..).unwrap_or_default();
    match argv.first().map(Vec::as_slice) {
        Some(b"dump") => dump(rest),
        Some(b"batch") => engine(main_start, |c| batch::main(c, rest)),
        Some(b"--version") => engine(main_start, |c| batch::version(c, rest)),
        Some(b"values") => match rest {
            [h] if h == b"-h" || h == b"--help" => {
                println!("{VALUES_USAGE}");
                ExitCode::SUCCESS
            }
            [p] if !p.starts_with(b"-") || p == b"-" => values(Path::new(OsStr::from_bytes(p))),
            _ => usage_error(
                VALUES_USAGE,
                "unityrs values",
                "expected one argument, the cases file",
            ),
        },
        Some(b"-h" | b"--help") => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some(other) => usage_error(
            USAGE,
            "unityrs",
            &format!(
                "argument command: invalid choice: {} (choose from 'dump', 'values', 'batch')",
                args::py_repr(other)
            ),
        ),
        None => usage_error(
            USAGE,
            "unityrs",
            "the following arguments are required: command",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(s: &[&str]) -> Result<RunOptions, String> {
        let argv: Vec<Vec<u8>> = s.iter().map(|a| a.as_bytes().to_vec()).collect();
        match args::parse(&argv) {
            Ok(args::Parsed::Dump(a)) => run_options(*a),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn jobs_below_one_mean_one() {
        assert_eq!(opts(&["x", "-o", "y", "-j", "0"]).unwrap().dump.jobs, 1);
        assert_eq!(opts(&["x", "-o", "y", "-j", "-5"]).unwrap().dump.jobs, 1);
        assert_eq!(opts(&["x", "-o", "y", "-j", "8"]).unwrap().dump.jobs, 8);
    }

    #[test]
    fn the_options_reach_the_run() {
        let o = opts(&[
            "idx.json",
            "-o",
            "out.jsonl.gz",
            "--index",
            "--kind",
            "bundle",
            "--build",
            "4100",
            "--name",
            "a*",
            "--sample",
            "5",
            "--limit",
            "3",
            "--levels",
            "L0,L1",
            "--stats",
            "s.json",
            "-q",
            "--allow-index-mismatch",
            "-j",
            "3",
            "--reverse-prepass",
            "--unbounded",
            "TreeDepth",
        ])
        .unwrap();
        assert_eq!(o.selection.inputs, [std::path::PathBuf::from("idx.json")]);
        assert!(o.selection.index && o.quiet && o.allow_index_mismatch);
        assert_eq!(o.selection.kinds, [PyStr::from("bundle")]);
        assert_eq!(o.selection.builds, Some(vec![PyStr::from("4100")]));
        assert_eq!(o.selection.names, [b"a*".to_vec()]);
        assert_eq!((o.selection.sample, o.selection.limit), (Some(5), Some(3)));
        assert_eq!(o.dump.levels.names(), ["L0", "L1"]);
        assert_eq!(o.output, std::path::PathBuf::from("out.jsonl.gz"));
        assert_eq!(o.stats, Some(std::path::PathBuf::from("s.json")));
        // Review round 14: `--reverse-prepass` and `--unbounded` could be
        // dropped here with every test passing (`tests/flags.rs` runs each
        // through the binary).
        assert_eq!(o.dump.jobs, 3);
        assert!(o.dump.lazy_reverse);
        assert_eq!(o.dump.unbounded, [unityrs_core::LimitKind::TreeDepth]);
        let mut lifted = DumpOptions::default().context.limits;
        assert!(unityrs_dump::lift(
            &mut lifted,
            unityrs_core::LimitKind::TreeDepth
        ));
        assert_eq!(o.dump.context.limits, lifted);
        assert_ne!(lifted, DumpOptions::default().context.limits);
    }

    /// `--tpk`'s provider answers through the run's context, and is opened
    /// under the limits `--unbounded` lifts, wherever either stands on the
    /// command line: UnityPy's chain 300 deep (`tools/fixtures/tpk.py`'s
    /// `deep_chain_300`) is a `TreeDepth` refusal at the default depth of 256
    /// and a tree with `TreeDepth` lifted (review round 14: no test held
    /// `run_options` to opening the TPK after lifting the bounds).
    #[test]
    fn the_tpk_answers_under_the_lifted_limits() {
        use unityrs_core::provider::TreeRequest;
        use unityrs_core::{LimitKind, UnityVersion};

        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../unityrs-tpk/tests/fixtures/tpk/synthetic.json");
        let all: serde_json::Value =
            serde_json::from_slice(&std::fs::read(fixture).unwrap()).unwrap();
        let case = all["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == "deep_chain_300")
            .unwrap();
        let hex = case["tpk"].as_str().unwrap();
        let bytes: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect();
        let dir = unityrs_testkit::scratch::Scratch::new("cli-tpk");
        let tpk = dir.join("deep.tpk");
        std::fs::write(&tpk, bytes).unwrap();
        let tpk = tpk.to_str().unwrap();
        let version =
            UnityVersion::from_packed(case["versions"][0].as_str().unwrap().parse().unwrap());
        let lookup = |args: &[&str]| {
            let o = opts(&[&["x", "-o", "y"], args].concat()).unwrap();
            o.dump
                .context
                .providers
                .lookup(&TreeRequest {
                    class_id: 1,
                    version: &version,
                    script: None,
                })
                .map(|t| t.map(|t| t.source))
                .map_err(|e| e.limit.map(|l| l.kind()))
        };
        let tree = Ok(Some(unityrs_core::provider::TreeSource::Provider("tpk")));
        assert_eq!(
            lookup(&["--tpk", tpk]),
            Err(Some(LimitKind::TreeDepth)),
            "the default depth"
        );
        assert_eq!(lookup(&["--tpk", tpk, "--unbounded", "TreeDepth"]), tree);
        assert_eq!(lookup(&["--unbounded", "TreeDepth", "--tpk", tpk]), tree);
        assert_eq!(lookup(&["--unbounded", "TreeDepth"]), Ok(None), "no TPK");
    }

    /// Review round 7: when every input crashed the oracle exits 3, and
    /// `unityrs dump` exited 0.
    #[test]
    fn a_dump_in_which_every_input_crashed_exits_3() {
        let summary = |inputs, crashes| Summary {
            inputs,
            crashes,
            ..Summary::default()
        };
        assert_eq!(written(&summary(2, 2)), ExitCode::from(3));
        assert_eq!(written(&summary(1, 1)), ExitCode::from(3));
        assert_eq!(written(&summary(2, 1)), ExitCode::SUCCESS);
        assert_eq!(written(&summary(3, 0)), ExitCode::SUCCESS);
    }

    #[test]
    fn a_tpk_that_does_not_load_is_refused() {
        let e = opts(&["x", "-o", "y", "--tpk", "/nonexistent/t.tpk"]).unwrap_err();
        assert!(e.starts_with("--tpk /nonexistent/t.tpk: "), "{e}");
        let o = opts(&["x", "-o", "y"]).unwrap();
        assert!(o.dump.context.providers.is_empty());
    }
}
