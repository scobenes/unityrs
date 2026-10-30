//! G3's and G3b's inputs (plan section 6). G3: `unityrs-testkit`'s synthetic format matrix
//! (`bundle::matrix`: every `SerializedFile` format from 1 to 22 in both byte orders, a
//! file without type trees, UnityFS 6-8 with every codec, blocks-info at
//! either end, both flag sets, padding, UnityWeb and UnityRaw at format 6,
//! nested bundles), one file per case, written into an empty folder outside
//! the repository so both dumpers can read it:
//!
//! ```text
//! cargo run -p unityrs-dump --example write_matrix -- DIR
//! python tools/parity/unitypy_dump.py DIR -o ORACLE.jsonl.gz
//! unityrs dump DIR --tpk TPK -o UNITYRS.jsonl.gz
//! python tools/parity/compare.py --oracle ORACLE.jsonl.gz UNITYRS.jsonl.gz
//! ```
//!
//! (`--oracle` refuses a reference that is not the oracle's dump on the
//! pinned stack.)
//!
//! G3b: `write_matrix --mutants [-j N] DIR` writes instead the mutants of every
//! builder output `unityrs_testkit::mutate::builder_outputs` lists (the matrix
//! and every Environment scene's files): two for each field the output's build
//! writes, then `unityrs_testkit::mutate::RANDOM_PER_INPUT` bit flips and
//! truncations, named `<input>~<index>~<label>`. The mutants are made on `N`
//! threads (default 4); they do not depend on `N`. Both sides dump them, the
//! oracle under a memory cap, and `tools/parity/g3b.py` judges the two dumps,
//! not `compare.py`, which fails every difference g3b.py admits (plan section
//! 10):
//!
//! ```text
//! cargo run --release -p unityrs-dump --example write_matrix -- --mutants -j 4 DIR
//! python tools/parity/unitypy_dump.py DIR -j 4 --memory-cap 4294967296 -o ORACLE.jsonl.gz
//! unityrs dump DIR -j 4 --tpk TPK -o UNITYRS.jsonl.gz
//! python tools/parity/g3b.py ORACLE.jsonl.gz UNITYRS.jsonl.gz --inputs DIR \
//!     --unityrs UNITYRS_BINARY --tpk TPK -j 4
//! ```

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let threads = |n: &PathBuf| {
        n.to_str()
            .and_then(|n| n.parse::<usize>().ok())
            .filter(|n| *n > 0)
    };
    let (mutate, threads, dir) = match args.as_slice() {
        [dir] => (false, Some(1), dir),
        [flag, dir] if flag.as_os_str() == "--mutants" => (true, Some(4), dir),
        [flag, j, n, dir] if flag.as_os_str() == "--mutants" && j.as_os_str() == "-j" => {
            (true, threads(n), dir)
        }
        _ => (false, Some(0), &PathBuf::new()),
    };
    let Some(threads) = threads.filter(|_| !dir.as_os_str().is_empty()) else {
        eprintln!(
            "usage: write_matrix [--mutants [-j N]] DIR (an empty or new folder outside the repository)"
        );
        return ExitCode::from(2);
    };
    // The folder as it will be, resolved through its nearest existing
    // ancestor, so nothing is created inside the repository.
    let mut existing = dir.clone();
    let mut rest = Vec::new();
    while !existing.exists() {
        match (existing.file_name(), existing.parent()) {
            (Some(n), Some(p)) => {
                rest.push(n.to_owned());
                existing = if p.as_os_str().is_empty() {
                    PathBuf::from(".")
                } else {
                    p.to_path_buf()
                };
            }
            _ => break,
        }
    }
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    match (repo.canonicalize(), existing.canonicalize()) {
        (Ok(repo), Ok(base)) => {
            let full = rest.iter().rev().fold(base, |p, n| p.join(n));
            if full.starts_with(&repo) {
                eprintln!("write_matrix: {} is inside the repository", full.display());
                return ExitCode::from(2);
            }
        }
        (_, Err(e)) | (Err(e), _) => {
            eprintln!("write_matrix: {}: {e}", dir.display());
            return ExitCode::from(2);
        }
    }
    if let Err(e) = std::fs::create_dir_all(dir) {
        eprintln!("write_matrix: {}: {e}", dir.display());
        return ExitCode::from(2);
    }
    match std::fs::read_dir(dir).map(|mut d| d.next().is_none()) {
        Ok(true) => {}
        Ok(false) => {
            eprintln!("write_matrix: {} is not empty", dir.display());
            return ExitCode::from(2);
        }
        Err(e) => {
            eprintln!("write_matrix: {}: {e}", dir.display());
            return ExitCode::from(2);
        }
    }
    let files: Vec<(String, Vec<u8>)> = if mutate {
        unityrs_testkit::mutate::gate_mutants(threads)
            .into_iter()
            .flat_map(|(name, mutants)| {
                mutants
                    .into_iter()
                    .enumerate()
                    .map(move |(i, m)| (format!("{name}~{i:03}~{}", m.label), m.bytes))
            })
            .collect()
    } else {
        unityrs_testkit::bundle::matrix()
    };
    for (name, bytes) in &files {
        if let Err(e) = std::fs::write(dir.join(name), bytes) {
            eprintln!("write_matrix: {name}: {e}");
            return ExitCode::from(1);
        }
    }
    println!("{} files in {}", files.len(), dir.display());
    ExitCode::SUCCESS
}
