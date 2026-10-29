//! The fence (`docs/plans/core.md` section 2.3): unityrs-rabex is vendored
//! whole, but only this crate may use it, and only its TPK reader.
//! `clippy.toml` here disallows the superseded types and functions; these
//! tests check what clippy cannot: that no other crate names rabex at all
//! (the P0 spike, which depends on the crates.io release, and the fork itself
//! aside), that the lock file agrees, and that this crate reaches rabex only
//! through `rabex::tpk`'s reader and the F4 accessor.

use std::path::{Path, PathBuf};

fn crates_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if path.is_dir() {
            if name != "target" && name != "fixtures" {
                files(&path, out);
            }
        } else if path.extension().is_some_and(|e| e == "rs") || name == "Cargo.toml" {
            out.push(path);
        }
    }
}

/// Every `.rs` and `Cargo.toml` under `crates/`, but the three crates that
/// may name rabex.
fn other_crates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut dirs: Vec<_> = std::fs::read_dir(crates_dir())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.join("Cargo.toml").is_file())
        .collect();
    dirs.sort();
    for dir in dirs {
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        if !["unityrs-tpk", "unityrs-spike-rabex", "unityrs-rabex"].contains(&name.as_str()) {
            files(&dir, &mut out);
        }
    }
    out
}

/// Whether `text` names rabex as code does: a path (`rabex::`), an import or
/// a dependency key. Prose that mentions rabex is no use of it.
fn uses_rabex(text: &str, toml: bool) -> Vec<String> {
    text.lines()
        .filter(|line| {
            let code = line.trim_start();
            if toml {
                let key = code.split('=').next().unwrap_or("").trim();
                !code.starts_with('#')
                    && (key == "rabex"
                        || key == "unityrs-rabex"
                        || code.contains("\"unityrs-rabex\""))
            } else {
                !code.starts_with("//")
                    && (code.contains("rabex::") || code.contains("extern crate rabex"))
            }
        })
        .map(str::to_owned)
        .collect()
}

#[test]
fn no_crate_but_the_tpk_provider_and_the_spike_names_rabex() {
    let mut found = Vec::new();
    let files = other_crates();
    assert!(files.len() > 20, "{} files scanned", files.len());
    for path in files {
        let text = std::fs::read_to_string(&path).unwrap();
        let toml = path.file_name().is_some_and(|n| n == "Cargo.toml");
        for line in uses_rabex(&text, toml) {
            found.push(format!("{}: {line}", path.display()));
        }
    }
    assert!(
        found.is_empty(),
        "rabex used outside unityrs-tpk: {found:#?}"
    );
}

#[test]
fn the_scan_sees_a_use() {
    assert_eq!(
        uses_rabex("use rabex::tpk::TpkFile;\n// rabex::x in prose\n", false).len(),
        1
    );
    assert_eq!(
        uses_rabex("rabex = \"0.2.1\"\n# rabex = 1\n", true).len(),
        1
    );
    assert_eq!(
        uses_rabex("r = { package = \"unityrs-rabex\", path = \"x\" }\n", true).len(),
        1
    );
    assert_eq!(
        uses_rabex("// see rabex 0.2.1\nlet x = 1;\n", false),
        Vec::<String>::new()
    );
}

/// Cargo.lock: the crates.io rabex has one dependent, the spike, and the
/// fork has one, this crate.
#[test]
fn the_lock_file_links_each_rabex_to_its_one_crate() {
    let lock = std::fs::read_to_string(crates_dir().join("../Cargo.lock")).unwrap();
    let dependents = |dep: &str| {
        let mut names = Vec::new();
        for package in lock.split("[[package]]") {
            let name = package
                .lines()
                .find_map(|l| l.strip_prefix("name = \""))
                .map(|n| n.trim_end_matches('"'));
            let deps = package
                .split("dependencies = [")
                .nth(1)
                .map(|d| d.split(']').next().unwrap_or(""));
            if let (Some(name), Some(deps)) = (name, deps)
                && deps.lines().any(|l| {
                    let l = l.trim().trim_end_matches(',').trim_matches('"');
                    l == dep || l.starts_with(&format!("{dep} "))
                })
            {
                names.push(name.to_owned());
            }
        }
        names
    };
    assert_eq!(dependents("rabex"), ["unityrs-spike-rabex"]);
    assert_eq!(dependents("unityrs-rabex"), ["unityrs-tpk"]);
}

/// This crate's own sources reach rabex only through `rabex::tpk`, and there
/// only through the reader and the F4 accessor's types; never through the
/// blob's raw class map, whose keys are rabex's `ClassId`.
#[test]
fn this_crate_reaches_only_the_tpk_reader() {
    const ALLOWED: [&str; 6] = [
        "TpkFile",
        "TpkNodeRow",
        "TpkTypeTreeBlob",
        "UnityClass",
        "Error",
        "TpkVersion",
    ];
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut sources = Vec::new();
    files(&here.join("src"), &mut sources);
    files(&here.join("tests"), &mut sources);
    let mut found = Vec::new();
    for path in sources {
        if path.ends_with("fence.rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        for line in text.lines().filter(|l| !l.trim_start().starts_with("//")) {
            if line.contains("class_information") {
                found.push(format!("{}: {line}", path.display()));
            }
            let mut rest = line;
            while let Some(at) = rest.find("rabex::") {
                rest = &rest[at + "rabex::".len()..];
                let ok = rest.strip_prefix("tpk::").is_some_and(|after| {
                    let names = after.trim_start_matches('{');
                    let names = names.split('}').next().unwrap_or(names);
                    names
                        .split(',')
                        .map(|n| {
                            n.trim()
                                .split(|c: char| !c.is_alphanumeric() && c != '_')
                                .next()
                                .unwrap_or("")
                        })
                        .all(|n| n.is_empty() || ALLOWED.contains(&n))
                });
                if !ok {
                    found.push(format!("{}: {line}", path.display()));
                }
            }
        }
    }
    assert!(
        found.is_empty(),
        "rabex reached past the TPK reader: {found:#?}"
    );
}
