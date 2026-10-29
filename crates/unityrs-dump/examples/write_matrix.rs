//! G3's and G3b's inputs (plan section 6). G3: `unityrs-testkit`'s synthetic format matrix
//! (`bundle::matrix`: every `SerializedFile` format in both byte orders, a
//! file without type trees, UnityFS 6-8 with every codec, blocks-info at
//! either end, both flag sets, padding, UnityWeb and UnityRaw at format 6,
//! nested bundles), one file per case, written into an empty folder outside
//! the repository so both dumpers can read it:
//!
//! ```text
//! cargo run -p unityrs-dump --example write_matrix -- DIR
//! python tools/parity/unitypy_dump.py DIR -o ORACLE.jsonl.gz
//! unityrs dump DIR --tpk TPK -o UNITYRS.jsonl.gz
//! python tools/parity/compare.py ORACLE.jsonl.gz UNITYRS.jsonl.gz
//! ```
//!
//! G3b: `write_matrix --mutants DIR` writes instead every builder output
//! `unityrs_testkit::mutate::builder_outputs` lists (the matrix and every
//! Environment scene's files), each as `unityrs_testkit::mutate::PER_INPUT`
//! seeded mutants named `<input>~<index>~<label>`, dumped and compared the
//! same way.

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let (mutate, dir) = match args.as_slice() {
        [dir] => (false, dir),
        [flag, dir] if flag.as_os_str() == "--mutants" => (true, dir),
        _ => {
            eprintln!(
                "usage: write_matrix [--mutants] DIR (an empty or new folder outside the repository)"
            );
            return ExitCode::from(2);
        }
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
        use unityrs_testkit::mutate::{builder_outputs, mutants};
        builder_outputs()
            .into_iter()
            .flat_map(|(name, bytes)| {
                mutants(name.as_bytes(), &bytes)
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
