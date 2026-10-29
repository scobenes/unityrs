//! `rabex-dump`: the P0 rabex spike (docs/spikes/rabex.md).
//!
//! ```text
//! rabex-dump dump  INPUT -o OUT.jsonl[.gz] [--index] [selection] [--levels L0,L1,L2,L3] [-j N] [--stats PATH] [--shim-fs6] [--tpk PATH]
//! rabex-dump bench INPUT [--index] [selection] [-j N] [--repeat R] [--phase serial|parallel|both] [--shim-fs6] [--tpk PATH]
//! rabex-dump probe INDEX.json --index --build B --tpk PATH
//! rabex-dump values CASES.json
//! rabex-dump save  INPUT -o DIR [--index] [selection] [--shim-fs6] [--tpk PATH]
//! rabex-dump lzma  INPUT [--index] [selection] [--shim-fs6]
//! ```
//!
//! `dump` writes the canonical dump (docs/CANONICAL_DUMP.md) with rabex as
//! the reader, for `tools/parity/compare.py` to diff against the UnityPy
//! oracle. Selection matches `tools/parity/unitypy_dump.py`: `--kind`,
//! `--build`, `--name GLOB`, `--sample N`, `--limit N`.
//!
//! Every subcommand reads bundles as rabex 0.2.1 publishes them unless
//! `--shim-fs6` is given (see `dump::shim_fs_header`). No TPK is compiled in:
//! `--tpk` loads one from a path outside the repository, and without it an
//! object with no embedded typetree has none.

// The prose names UnityPy, SerializedFile, UnityFS and their kin on every
// line; backticking each one would only make the docs harder to read.
#![allow(clippy::doc_markdown)]

mod detect;
mod dump;
mod encode;
mod json;
mod lzma;
mod probe;
mod tpk;
mod value;

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use rayon::prelude::*;

use crate::dump::{Ctx, Levels, Stats, dump_input};
use crate::json::J;

const USAGE: &str = "usage: rabex-dump dump|bench|probe|values|save|lzma ... (see src/main.rs)";

/// Command-line options shared by the subcommands.
struct Args {
    cmd: String,
    input: PathBuf,
    output: Option<PathBuf>,
    index: bool,
    kinds: Vec<String>,
    builds: Option<Vec<String>>,
    names: Vec<String>,
    sample: Option<usize>,
    limit: Option<usize>,
    levels: Vec<String>,
    jobs: usize,
    stats: Option<PathBuf>,
    repeat: usize,
    shim: bool,
    tpk: Option<PathBuf>,
    phase: String,
}

impl Args {
    fn new(cmd: &str) -> Args {
        Args {
            cmd: cmd.to_owned(),
            input: PathBuf::new(),
            output: None,
            index: false,
            kinds: vec!["bundle".into(), "unitypy-sample".into()],
            builds: None,
            names: Vec::new(),
            sample: None,
            limit: None,
            levels: ["L0", "L1", "L2", "L3"].map(String::from).to_vec(),
            jobs: 1,
            stats: None,
            repeat: 1,
            shim: false,
            tpk: None,
            phase: "both".into(),
        }
    }
}

fn parse_args() -> Result<Args, String> {
    let mut it = std::env::args().skip(1);
    let mut a = Args::new(&it.next().ok_or(USAGE)?);
    let mut positional = Vec::new();
    while let Some(arg) = it.next() {
        let mut val = || it.next().ok_or_else(|| format!("{arg} needs a value"));
        let num = |s: String| s.parse::<usize>().map_err(|e| format!("{s}: {e}"));
        match arg.as_str() {
            "-o" | "--output" => a.output = Some(val()?.into()),
            "--index" => a.index = true,
            "--kind" => a.kinds = val()?.split(',').map(String::from).collect(),
            "--build" => a.builds = Some(val()?.split(',').map(String::from).collect()),
            "--name" => a.names.push(val()?),
            "--sample" => a.sample = Some(num(val()?)?),
            "--limit" => a.limit = Some(num(val()?)?),
            "--levels" => a.levels = val()?.split(',').map(String::from).collect(),
            "-j" | "--jobs" => a.jobs = num(val()?)?,
            "--stats" => a.stats = Some(val()?.into()),
            "--repeat" => a.repeat = num(val()?)?,
            "--shim-fs6" => a.shim = true,
            "--tpk" => a.tpk = Some(val()?.into()),
            "--phase" => a.phase = val()?,
            s if s.starts_with('-') => return Err(format!("unknown option {s}\n{USAGE}")),
            _ => positional.push(arg),
        }
    }
    a.input = positional.pop().ok_or(USAGE)?.into();
    Ok(a)
}

/// fnmatch-style `*` and `?` (what `--name` means in unitypy_dump.py).
fn glob(pat: &[u8], s: &[u8]) -> bool {
    match (pat.first(), s.first()) {
        (None, None) => true,
        (Some(b'*'), _) => glob(&pat[1..], s) || (!s.is_empty() && glob(pat, &s[1..])),
        (Some(b'?'), Some(_)) => glob(&pat[1..], &s[1..]),
        (Some(p), Some(c)) if p == c => glob(&pat[1..], &s[1..]),
        _ => false,
    }
}

/// (absolute path, dump-relative path, expected sha256), in canonical order.
fn collect_inputs(a: &Args) -> Result<Vec<(PathBuf, String, Option<String>)>, String> {
    let mut items = Vec::new();
    if a.index {
        let text =
            std::fs::read_to_string(&a.input).map_err(|e| format!("{}: {e}", a.input.display()))?;
        let index: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        let root = a.input.parent().unwrap_or(Path::new("."));
        for e in index["files"].as_array().ok_or("index has no files")? {
            let kind = e["kind"].as_str().unwrap_or_default();
            if !a.kinds.iter().any(|k| k == kind) {
                continue;
            }
            let build = match &e["build"] {
                serde_json::Value::Null => "None".to_owned(),
                v => v.to_string(),
            };
            if a.builds.as_ref().is_some_and(|b| !b.contains(&build)) {
                continue;
            }
            let rel = e["path"]
                .as_str()
                .ok_or("index entry without path")?
                .to_owned();
            items.push((root.join(&rel), rel, e["sha256"].as_str().map(String::from)));
        }
    } else if a.input.is_dir() {
        let mut stack = vec![a.input.clone()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).map_err(|e| e.to_string())? {
                let p = entry.map_err(|e| e.to_string())?.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.is_file() {
                    let rel = p
                        .strip_prefix(&a.input)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/");
                    items.push((p, rel, None));
                }
            }
        }
    } else {
        let name = a
            .input
            .file_name()
            .ok_or("no file name")?
            .to_string_lossy()
            .into_owned();
        items.push((a.input.clone(), name, None));
    }
    if !a.names.is_empty() {
        items.retain(|i| a.names.iter().any(|p| glob(p.as_bytes(), i.1.as_bytes())));
    }
    items.sort_by(|x, y| x.1.as_bytes().cmp(y.1.as_bytes()));
    if let Some(n) = a.sample.filter(|&n| items.len() > n) {
        let len = items.len();
        items = (0..n).map(|k| items[k * len / n].clone()).collect();
    }
    if let Some(n) = a.limit {
        items.truncate(n);
    }
    Ok(items)
}

/// Peak resident set size (VmHWM) of this process, in bytes.
#[must_use]
pub fn peak_rss() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    status
        .lines()
        .find_map(|l| l.strip_prefix("VmHWM:"))
        .and_then(|v| v.trim().trim_end_matches("kB").trim().parse::<u64>().ok())
        .map_or(0, |kb| kb * 1024)
}

fn pool(jobs: usize) -> rayon::ThreadPool {
    rayon::ThreadPoolBuilder::new()
        .num_threads(jobs.max(1))
        .build()
        .unwrap()
}

fn levels_of(a: &Args) -> Levels {
    let has = |l: &str| a.levels.iter().any(|x| x == l);
    Levels {
        l1: has("L1"),
        l2: has("L2"),
        l3: has("L3"),
    }
}

/// The path `path` names once `..` and symlinks are resolved. Each existing
/// prefix is canonicalised (following symlinks, a dangling one included);
/// components that do not exist yet are applied as written, which is how
/// `create_dir_all` and `File::create` will create them.
fn resolve(path: &Path, depth: u32) -> Result<PathBuf, String> {
    use std::path::Component;
    if depth > 40 {
        return Err(format!("{}: too many levels of symlinks", path.display()));
    }
    let abs = std::path::absolute(path).map_err(|e| e.to_string())?;
    let mut p = PathBuf::new();
    let mut rest = abs.components();
    while let Some(c) = rest.next() {
        match c {
            Component::Prefix(_) | Component::RootDir => p.push(c),
            Component::CurDir => {}
            Component::ParentDir => {
                p.pop();
            }
            Component::Normal(name) => {
                p.push(name);
                if let Ok(real) = std::fs::canonicalize(&p) {
                    p = real;
                } else if let Ok(target) = std::fs::read_link(&p) {
                    // A dangling symlink: carry on from where it points.
                    p.pop();
                    let mut next = p.join(target);
                    next.extend(rest);
                    return resolve(&next, depth + 1);
                }
            }
        }
    }
    Ok(p)
}

/// Refuses an output path inside a git repository, this one above all:
/// `dump` and `save` write corpus-derived data (whole re-saved game bundles,
/// for `save`), which must never be committed to this public repository.
/// The check runs on the resolved path, so neither `..` nor a symlink gets
/// past it. Returns the resolved path, which the caller writes to.
fn outside_repo(out: &Path) -> Result<PathBuf, String> {
    let real = resolve(out, 0)?;
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let ours = std::fs::canonicalize(manifest).ok().and_then(|m| {
        m.ancestors()
            .find(|r| r.join(".git").exists())
            .map(Path::to_path_buf)
    });
    for root in real.ancestors() {
        if root.join(".git").exists() || ours.as_deref() == Some(root) {
            return Err(format!(
                "refusing to write into the repository ({})",
                root.display()
            ));
        }
    }
    Ok(real)
}

#[allow(clippy::too_many_lines)] // one linear pass: select, header, chunks, end, summary
fn cmd_dump(a: &Args) -> Result<(), String> {
    let out_abs = outside_repo(&a.output.clone().ok_or("dump needs -o")?)?;
    let inputs = collect_inputs(a)?;
    if inputs.is_empty() {
        return Err("no inputs matched".into());
    }
    let levels = levels_of(a);
    let order = ["L0", "L1", "L2", "L3"];
    let level_list: Vec<J> = order
        .iter()
        .filter(|l| a.levels.iter().any(|x| x == *l))
        .map(|l| J::s(l))
        .collect();
    let header = J::O(vec![
        ("rec", J::s("dump")),
        ("format", J::s("unityrs-canonical-dump")),
        ("version", J::U(1)),
        ("levels", J::A(level_list)),
        (
            "producer",
            J::O(vec![
                ("name", J::s("rabex-dump")),
                ("rabex", J::s("0.2.1")),
                ("shim_fs6", J::Bool(a.shim)),
            ]),
        ),
    ]);
    let tmp = out_abs.with_file_name(format!(
        "{}.partial",
        out_abs.file_name().unwrap().to_string_lossy()
    ));
    if let Some(dir) = tmp.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let file = BufWriter::with_capacity(1 << 20, File::create(&tmp).map_err(|e| e.to_string())?);
    let mut w: Box<dyn Write> = if out_abs.extension().is_some_and(|e| e == "gz") {
        Box::new(
            flate2::GzBuilder::new()
                .mtime(0)
                .write(file, flate2::Compression::default()),
        )
    } else {
        Box::new(file)
    };
    w.write_all(header.line().as_bytes())
        .map_err(|e| e.to_string())?;
    let tpk = tpk::load(a.tpk.as_deref())?;
    let ctx = Ctx {
        tpk: &tpk,
        levels,
        shim: a.shim,
    };
    let t0 = Instant::now();
    let mut totals = Stats::default();
    let pool = pool(a.jobs);
    let mut mismatches = 0;
    // Chunks keep memory bounded; within a chunk inputs run in parallel and
    // are written in order, so the output equals a serial run.
    for chunk in inputs.chunks(a.jobs.max(1) * 4) {
        let results: Vec<_> = pool.install(|| {
            chunk
                .par_iter()
                .map(|(abs, rel, sha)| {
                    let data = std::fs::read(abs).map_err(|e| format!("{}: {e}", abs.display()))?;
                    let ok = sha.as_ref().is_none_or(|s| *s == json::sha256_hex(&data));
                    let (text, stats) = dump_input(&data, rel, &abs.to_string_lossy(), &ctx);
                    Ok::<_, String>((text, stats, ok))
                })
                .collect()
        });
        for r in results {
            let (text, stats, ok) = r?;
            w.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
            totals.merge(&stats);
            mismatches += usize::from(!ok);
        }
    }
    let end = J::O(vec![
        ("rec", J::s("end")),
        ("files", J::U(totals.files)),
        ("objects", J::U(totals.objects)),
    ]);
    w.write_all(end.line().as_bytes())
        .map_err(|e| e.to_string())?;
    drop(w);
    std::fs::rename(&tmp, &out_abs).map_err(|e| e.to_string())?;
    let summary = summary_json(
        &totals,
        inputs.len(),
        t0.elapsed().as_secs_f64(),
        mismatches,
    );
    eprintln!("{summary}");
    if let Some(p) = &a.stats {
        std::fs::write(p, summary).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn summary_json(t: &Stats, inputs: usize, wall: f64, mismatches: usize) -> String {
    let v = serde_json::json!({
        "inputs": inputs,
        "files": t.files,
        "objects": t.objects,
        "load_errors": t.load_errors,
        "bytes_in": t.bytes_in,
        "bytes_out": t.bytes_out,
        "index_sha256_mismatch": mismatches,
        "wall_s": wall,
        "t_load": t.t_load,
        "t_parse": t.t_parse,
        "t_encode": t.t_encode,
        "t_resave": t.t_resave,
        "peak_rss_bytes": peak_rss(),
        "counts": t.counts,
    });
    serde_json::to_string_pretty(&v).unwrap_or_default()
}

fn main() {
    // rabex panics on some inputs (todo!, unimplemented!, indexing); the
    // dumper catches each one and records it, so keep stderr quiet.
    std::panic::set_hook(Box::new(|_| {}));
    let result = parse_args().and_then(|a| match a.cmd.as_str() {
        "dump" => cmd_dump(&a),
        "bench" => probe::bench(&a),
        "probe" => probe::probe(&a),
        "values" => probe::values(&a.input),
        "save" => probe::save(&a),
        "lzma" => lzma::lzma(&a),
        _ => Err(USAGE.into()),
    });
    if let Err(e) = result {
        eprintln!("rabex-dump: {e}");
        std::process::exit(2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> PathBuf {
        std::fs::canonicalize(env!("CARGO_MANIFEST_DIR")).unwrap()
    }

    #[test]
    fn outside_repo_refuses_the_repository() {
        assert!(outside_repo(&repo().join("out")).is_err());
        assert!(outside_repo(&std::env::temp_dir().join("unityrs-spike-out")).is_ok());
    }

    /// `/tmp/<missing>/../../<repo>/leak-dir`: lexically outside, really inside.
    #[test]
    fn outside_repo_resolves_dot_dot() {
        let tmp = std::fs::canonicalize(std::env::temp_dir()).unwrap();
        let mut p = tmp.join("unityrs-spike-no-such-dir");
        p.push("..");
        for _ in tmp.components().skip(1) {
            p.push("..");
        }
        p.push(repo().strip_prefix("/").unwrap());
        p.push("leak-dir");
        let err = outside_repo(&p).unwrap_err();
        assert!(err.contains("refusing"), "{}: {err}", p.display());
    }

    /// A symlink outside the repository that points into it, live or dangling.
    #[test]
    fn outside_repo_resolves_symlinks() {
        let dir = std::env::temp_dir().join(format!("unityrs-spike-links-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let link = dir.join("link");
        std::os::unix::fs::symlink(repo(), &link).unwrap();
        let dangling = dir.join("dangling");
        std::os::unix::fs::symlink(repo().join("no-such-out"), &dangling).unwrap();
        let through = outside_repo(&link.join("leak-dir"));
        let to_missing = outside_repo(&dangling);
        let plain = outside_repo(&dir.join("plain"));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(through.is_err(), "{through:?}");
        assert!(to_missing.is_err(), "{to_missing:?}");
        assert!(plain.is_ok(), "{plain:?}");
    }
}
