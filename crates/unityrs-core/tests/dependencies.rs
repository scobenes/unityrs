//! Every crate's production dependencies are its code's: each `[dependencies]`
//! entry of every workspace member is named by some file under that member's
//! `src/`. A dependency only a test uses is a dev-dependency, so no consumer
//! builds code nothing calls (review round 10: `lz4_flex` stayed a production
//! dependency of unityrs-core after `codec/lz4.rs` replaced it with a port of
//! liblz4's decoder, and reached every crate built on the core).

use std::path::{Path, PathBuf};
use std::process::Command;

use unityrs_testkit::child;

/// Every file under `dir`, recursively.
fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// Whether `text` names the crate `ident` as code does: a path through it
/// (`ident::`), a `use` of it, or one of its macros (`ident!`).
fn names(text: &str, ident: &str) -> bool {
    text.contains(&format!("{ident}::"))
        || text.contains(&format!("use {ident}"))
        || text.contains(&format!("{ident}!"))
}

#[test]
fn every_production_dependency_is_named_by_its_crates_code() {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    // `--no-deps` lists the workspace's members with their declared
    // dependencies and resolves nothing, so it needs no other package.
    let out = child::output(
        Command::new(cargo)
            .args([
                "metadata",
                "--format-version",
                "1",
                "--no-deps",
                "--offline",
            ])
            .current_dir(env!("CARGO_MANIFEST_DIR")),
    )
    .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let meta: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let packages = meta["packages"].as_array().unwrap();
    assert!(
        packages
            .iter()
            .any(|p| p["name"].as_str() == Some("unityrs-core")),
        "the metadata is the workspace's"
    );
    let mut checked = 0;
    let mut unused = Vec::new();
    for package in packages {
        let manifest = Path::new(package["manifest_path"].as_str().unwrap());
        let mut sources = Vec::new();
        files(&manifest.parent().unwrap().join("src"), &mut sources);
        let texts: Vec<String> = sources
            .iter()
            .filter(|p| p.extension().is_some_and(|e| e == "rs"))
            .map(|p| std::fs::read_to_string(p).unwrap())
            .collect();
        assert!(!texts.is_empty(), "{}: no source", manifest.display());
        for dep in package["dependencies"].as_array().unwrap() {
            // A normal dependency has no kind; "dev" and "build" are others.
            if !dep["kind"].is_null() {
                continue;
            }
            let name = dep["rename"]
                .as_str()
                .unwrap_or_else(|| dep["name"].as_str().unwrap());
            let ident = name.replace('-', "_");
            checked += 1;
            if !texts.iter().any(|t| names(t, &ident)) {
                unused.push(format!("{} -> {name}", package["name"].as_str().unwrap()));
            }
        }
    }
    assert!(checked >= 10, "only {checked} dependencies checked");
    assert!(
        unused.is_empty(),
        "production dependencies no source file names (make them \
         dev-dependencies, or remove them): {unused:?}"
    );
}
