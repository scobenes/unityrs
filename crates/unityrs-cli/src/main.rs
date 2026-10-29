//! `unityrs`: the canonical dump and the value-case protocol (plan section 5).
//!
//! ```text
//! unityrs dump INPUT... -o OUT.jsonl[.gz] [--index] [--kind K] [--build B]
//!     [--allow-index-mismatch] [--name GLOB]... [--sample N] [--limit N]
//!     [--levels L0,L1,L2,L3] [-j N] [--stats PATH] [-q] [--tpk PATH]
//! unityrs values CASES.json
//! ```
//!
//! `dump` takes `tools/parity/unitypy_dump.py`'s options, parsed as its
//! parser parses them ([`args`]), and runs [`unityrs_dump::run::run`]: status
//! 0 when the dump is written, 1 when it fails (an index mismatch, an input
//! that cannot be read, a dumper panic), 2 on a bad argument or path, as the
//! oracle exits. `values` runs the spike's value-case protocol
//! ([`unityrs_dump::values`]), which `tools/parity/value_cases.py` drives.

mod args;

use std::ffi::OsStr;
use std::io::Write;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::Path;
use std::process::ExitCode;

use unityrs_dump::DumpOptions;
use unityrs_dump::run::{RunError, RunOptions};
use unityrs_dump::select::Selection;

const USAGE: &str = "usage: unityrs {dump,values} ...\n\n\
  unityrs dump INPUT... -o OUT.jsonl[.gz] [options]   the canonical dump (dump -h)\n\
  unityrs values CASES.json                           the value-case protocol";

const VALUES_USAGE: &str = "usage: unityrs values CASES.json";

/// `ap.error`: the usage line, `prog: error: msg`, status 2.
fn usage_error(usage: &str, prog: &str, msg: &str) -> ExitCode {
    eprintln!("{usage}\n{prog}: error: {msg}");
    ExitCode::from(2)
}

fn run_options(a: args::DumpArgs) -> Result<RunOptions, String> {
    if a.tpk.is_some() {
        // STUB(A): the TPK provider is package A's `unityrs-tpk`, not in the
        // workspace yet. Until it is, a dump asking for one is refused
        // rather than written without it.
        return Err("--tpk: the TPK typetree provider (unityrs-tpk) is not built yet".into());
    }
    Ok(RunOptions {
        selection: Selection {
            inputs: a.inputs,
            index: a.index,
            kinds: a.kind,
            builds: a.build,
            names: a.name,
            sample: a.sample,
            limit: a.limit,
        },
        output: a.output,
        dump: DumpOptions {
            levels: a.levels,
            // `run_jobs`: `workers = max(1, workers)`.
            jobs: usize::try_from(a.jobs.max(1)).unwrap_or(usize::MAX),
            tpk_path: a.tpk,
            ..DumpOptions::default()
        },
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
        Ok(_) => ExitCode::SUCCESS,
        Err(RunError::Usage(msg)) => err(&msg),
        Err(e) => {
            eprintln!("unityrs dump: error: {e}");
            ExitCode::from(u8::try_from(e.status()).unwrap_or(1))
        }
    }
}

fn values(path: &Path) -> ExitCode {
    let text = match std::fs::read(path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("unityrs values: {}: {e}", path.display());
            return ExitCode::from(1);
        }
    };
    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());
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

fn main() -> ExitCode {
    let argv: Vec<Vec<u8>> = std::env::args_os()
        .skip(1)
        .map(OsStringExt::into_vec)
        .collect();
    match argv.first().map(Vec::as_slice) {
        Some(b"dump") => dump(&argv[1..]),
        Some(b"values") => match &argv[1..] {
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
                "argument command: invalid choice: {} (choose from 'dump', 'values')",
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
        ])
        .unwrap();
        assert_eq!(o.selection.inputs, [std::path::PathBuf::from("idx.json")]);
        assert!(o.selection.index && o.quiet && o.allow_index_mismatch);
        assert_eq!(o.selection.kinds, ["bundle"]);
        assert_eq!(o.selection.builds, Some(vec!["4100".to_owned()]));
        assert_eq!(o.selection.names, [b"a*".to_vec()]);
        assert_eq!((o.selection.sample, o.selection.limit), (Some(5), Some(3)));
        assert_eq!(o.dump.levels.names(), ["L0", "L1"]);
        assert_eq!(o.output, std::path::PathBuf::from("out.jsonl.gz"));
        assert_eq!(o.stats, Some(std::path::PathBuf::from("s.json")));
    }

    #[test]
    fn tpk_waits_for_its_provider() {
        let e = opts(&["x", "-o", "y", "--tpk", "t.tpk"]).unwrap_err();
        assert!(e.contains("unityrs-tpk"), "{e}");
    }
}
