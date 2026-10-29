//! No TPK in the repository or a binary (`docs/plans/core.md` section 2.5,
//! decision 10): the TPK is loaded from a path the user supplies.
//!
//! 1. no `include_bytes!` or `include_str!` in unityrs-core (its generated
//!    tables are literals, so nothing is exempt);
//! 2. no crate the workspace resolves has a feature matching `*tpk*`
//!    enabled (such as rabex's `embed-tpk`, on by default in rabex 0.2.1,
//!    whose TPK reader unityrs-tpk replaced with its port of `Tpk.py`);
//! 3. no file under the repository matches `*.tpk`.

use std::path::{Path, PathBuf};
use std::process::Command;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn walk(dir: &Path, skip: &[&str], out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if path.is_dir() {
            // A directory with its own .git is another checkout (a worktree).
            if !skip.contains(&name.as_str()) && !path.join(".git").exists() {
                walk(&path, skip, out);
            }
        } else {
            out.push(path);
        }
    }
}

#[test]
fn the_core_includes_no_file() {
    let mut files = Vec::new();
    walk(&repo().join("crates/unityrs-core/src"), &[], &mut files);
    assert!(files.len() > 20);
    for path in files {
        let text = std::fs::read_to_string(&path).unwrap();
        for macro_name in ["include_bytes!", "include_str!"] {
            assert!(
                !text.contains(macro_name),
                "{}: {macro_name}",
                path.display()
            );
        }
    }
}

#[test]
fn no_resolved_crate_enables_a_tpk_feature() {
    // Offline, `cargo metadata` needs every platform's packages unless it is
    // filtered to one; a fresh CI runner holds only the host's.
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let version = Command::new(rustc).arg("-vV").output().unwrap();
    let version = String::from_utf8(version.stdout).unwrap();
    let host = version
        .lines()
        .find_map(|l| l.strip_prefix("host: "))
        .unwrap();
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let out = Command::new(cargo)
        .args([
            "metadata",
            "--format-version",
            "1",
            "--offline",
            "--filter-platform",
            host,
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let meta: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let nodes = meta["resolve"]["nodes"].as_array().unwrap();
    // The resolve holds this crate: the metadata is the workspace's.
    assert!(nodes.iter().any(|n| {
        n["id"]
            .as_str()
            .is_some_and(|id| id.contains("unityrs-tpk"))
    }));
    for node in nodes {
        for feature in node["features"].as_array().unwrap() {
            let feature = feature.as_str().unwrap();
            assert!(
                !feature.contains("tpk"),
                "{}: feature {feature}",
                node["id"]
            );
        }
    }
}

#[test]
fn no_file_in_the_repository_is_a_tpk() {
    // The tracked and untracked files git sees, where git works; a walk of
    // the tree otherwise (a worktree in the dev container, whose .git file
    // names a host path), skipping what .gitignore leaves out.
    let git = Command::new("git")
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .current_dir(repo())
        .output();
    let files: Vec<PathBuf> = match git {
        Ok(out) if out.status.success() => out
            .stdout
            .split(|b| *b == 0)
            .filter(|p| !p.is_empty())
            .map(|p| PathBuf::from(String::from_utf8_lossy(p).into_owned()))
            .collect(),
        _ => {
            let mut files = Vec::new();
            walk(
                &repo(),
                &[".git", "target", ".wt", "corpus", "out", "__pycache__"],
                &mut files,
            );
            files
        }
    };
    assert!(files.len() > 50, "{} files", files.len());
    let tpks: Vec<_> = files
        .iter()
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("tpk")))
        .collect();
    assert!(tpks.is_empty(), "TPK files in the repository: {tpks:?}");
}
