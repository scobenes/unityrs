//! `unityrs-bench gen --scale full|gate DIR`: writes every fixture and
//! `manifest.json` into DIR (`docs/plans/bench.md` section 2).
//!
//! `unityrs-bench run WORKLOAD FIXTURE`: one run of one Rust workload
//! ([`unityrs_bench::work`]), which prints one JSON line: `workload`,
//! `status` (`ok` or `failed`, the reason on stderr), `scope_s`, `counts`,
//! `digest`, `alloc_bytes` (the bytes the scope asked the allocator for,
//! a `realloc`'s new size included, on the scope's thread), `scopes` (the
//! scope entries of the whole process, which must be 1: the run fails
//! otherwise) and `cpu` (the x86-64 features the process detected: under
//! valgrind, its guest's).

use std::path::Path;
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use serde_json::{Value, json};
use unityrs_bench::{Scale, generate, work};
use unityrs_testkit::counted::{self, Allocator, Tally};

const USAGE: &str =
    "usage: unityrs-bench gen --scale full|gate DIR\n       unityrs-bench run WORKLOAD FIXTURE";

/// The bytes the scope's thread asked for.
struct Requested(AtomicU64);

impl Tally for Requested {
    fn grew(&self, size: usize, _zeroed: bool) {
        self.0
            .fetch_add(u64::try_from(size).unwrap_or(u64::MAX), Ordering::Relaxed);
    }

    fn shrank(&self, _size: usize) {}

    fn shrunk(&self, _old: usize, new: usize) {
        self.grew(new, false);
    }
}

#[global_allocator]
static ALLOC: Allocator<Requested> = Allocator(Requested(AtomicU64::new(0)));

fn gen_into(scale: Scale, dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let manifest = generate(scale, |fixture| {
        std::fs::write(dir.join(&fixture.name), &fixture.bytes)
    })
    .map_err(|e| e.to_string())?;
    let mut text = serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?;
    text.push('\n');
    std::fs::write(dir.join("manifest.json"), text).map_err(|e| e.to_string())
}

/// The x86-64 features this process detects, by name.
fn cpu() -> Vec<&'static str> {
    #[cfg(target_arch = "x86_64")]
    {
        let found = [
            ("sse4.1", std::arch::is_x86_feature_detected!("sse4.1")),
            ("sse4.2", std::arch::is_x86_feature_detected!("sse4.2")),
            ("popcnt", std::arch::is_x86_feature_detected!("popcnt")),
            ("avx", std::arch::is_x86_feature_detected!("avx")),
            ("avx2", std::arch::is_x86_feature_detected!("avx2")),
            ("fma", std::arch::is_x86_feature_detected!("fma")),
            ("bmi2", std::arch::is_x86_feature_detected!("bmi2")),
            ("avx512f", std::arch::is_x86_feature_detected!("avx512f")),
        ];
        found
            .iter()
            .filter(|(_, on)| *on)
            .map(|(n, _)| *n)
            .collect()
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        Vec::new()
    }
}

/// One run: setup, the scope (timed, its allocations counted), the digest.
fn run(name: &str, fixture: &Path) -> Result<Value, String> {
    let (op, scope) = work::find(name).ok_or_else(|| format!("no workload {name:?}"))?;
    let data = std::fs::read(fixture).map_err(|e| format!("{}: {e}", fixture.display()))?;
    let source_bytes = data.len();
    let input = work::setup(op, data.into())?;
    let features = cpu();
    ALLOC.0.0.store(0, Ordering::Relaxed);
    counted::count_this_thread();
    let t0 = Instant::now();
    let out = scope(&input);
    let scope_s = t0.elapsed().as_secs_f64();
    counted::stop_counting_this_thread();
    let alloc_bytes = ALLOC.0.0.load(Ordering::Relaxed);
    let digested = out.and_then(|o| work::digest(&o));
    let entered = work::scopes_entered();
    let digested = if entered == 1 {
        digested
    } else {
        Err(format!("{entered} scope entries in the process, not 1"))
    };
    let (status, digest, mut counts) = match digested {
        Ok((digest, counts)) => ("ok", Value::String(digest), counts),
        Err(why) => {
            eprintln!("unityrs-bench run {name}: {why}");
            ("failed", Value::Null, serde_json::Map::new())
        }
    };
    counts.insert("source_bytes".into(), json!(source_bytes));
    Ok(json!({
        "workload": name,
        "status": status,
        "scope_s": scope_s,
        "counts": counts,
        "digest": digest,
        "alloc_bytes": alloc_bytes,
        "scopes": entered,
        "cpu": features.join(" "),
    }))
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [cmd, flag, scale, dir] if cmd == "gen" && flag == "--scale" => {
            let Some(scale) = Scale::parse(scale) else {
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            };
            match gen_into(scale, Path::new(dir)) {
                Ok(()) => ExitCode::SUCCESS,
                Err(why) => {
                    eprintln!("unityrs-bench gen: {why}");
                    ExitCode::FAILURE
                }
            }
        }
        [cmd, name, fixture] if cmd == "run" => match run(name, Path::new(fixture)) {
            Ok(line) => {
                println!("{line}");
                ExitCode::SUCCESS
            }
            Err(why) => {
                eprintln!("unityrs-bench run: {why}");
                ExitCode::FAILURE
            }
        },
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}
