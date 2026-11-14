//! Every store client is built in `src/store/client.rs`: no
//! other file of `src/` names `AmazonS3Builder`, `from_env` or `reqwest`, so
//! no client can fall back to `object_store`'s pooled `reqwest` connector
//! (which reads proxy variables; the crate is built without it, and this
//! keeps it so) or to the environment's `AWS_*` settings.
//! Comments are left out; the builder file itself never calls `from_env`
//! and builds exactly one `AmazonS3Builder::new()`.
//!
//! The store's code (`src/store/`, `src/http/shared.rs`) also spawns
//! nothing and uses none of `object_store`'s helpers that spawn: a task
//! spawned on the controller's runtime runs outside every try, and the
//! connector refuses it (`docs/plans/batch.md` 2.9).

use std::path::{Path, PathBuf};

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// A file's code lines, comments left out.
fn code(path: &Path) -> Vec<(usize, String)> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim_start().starts_with("//"))
        .map(|(i, l)| (i + 1, l.split("//").next().unwrap_or_default().to_owned()))
        .collect()
}

fn words(line: &str) -> impl Iterator<Item = &str> {
    line.split(|c: char| c != '_' && !c.is_alphanumeric())
}

/// The forbidden names on a line: the builder, `from_env` as a word, and
/// `reqwest` in any spelling (`ReqwestConnector` among them).
fn forbidden(line: &str) -> Vec<&'static str> {
    let mut found = Vec::new();
    if words(line).any(|w| w == "AmazonS3Builder") {
        found.push("AmazonS3Builder");
    }
    if words(line).any(|w| w == "from_env") {
        found.push("from_env");
    }
    if line.to_ascii_lowercase().contains("reqwest") {
        found.push("reqwest");
    }
    found
}

/// What the store's own code may not name: the helpers that spawn
/// (`WriteMultipart`'s `JoinSet`, `buffered`'s `BufWriter`) and spawning.
const SPAWNING: [&str; 5] = [
    "WriteMultipart",
    "BufWriter",
    "JoinSet",
    "spawn",
    "spawn_blocking",
];

fn spawning(line: &str) -> Vec<&'static str> {
    SPAWNING
        .into_iter()
        .filter(|name| words(line).any(|w| w == *name))
        .collect()
}

#[test]
fn the_spawn_scan_sees_each_spelling() {
    assert_eq!(spawning("tokio::spawn(async {});"), ["spawn"]);
    assert_eq!(spawning("let mut set = JoinSet::new();"), ["JoinSet"]);
    assert_eq!(
        spawning("tokio::task::spawn_blocking(f)"),
        ["spawn_blocking"]
    );
    assert_eq!(
        spawning("use object_store::{buffered::BufWriter, WriteMultipart};"),
        ["WriteMultipart", "BufWriter"]
    );
    assert_eq!(spawning("fn spawned() {}"), [""; 0]);
}

#[test]
fn the_store_code_spawns_nothing() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src.join("store"), &mut files);
    files.push(src.join("http").join("shared.rs"));
    assert!(files.len() > 2);
    let mut bad = Vec::new();
    for file in &files {
        for (n, line) in code(file) {
            for name in spawning(&line) {
                bad.push(format!("{}:{n}: {name}", file.display()));
            }
        }
    }
    assert!(bad.is_empty(), "the store code spawns:\n{}", bad.join("\n"));
}

#[test]
fn the_scan_sees_each_spelling() {
    assert_eq!(
        forbidden("let b = AmazonS3Builder::from_env();"),
        ["AmazonS3Builder", "from_env"]
    );
    assert_eq!(
        forbidden("use object_store::client::ReqwestConnector;"),
        ["reqwest"]
    );
    assert_eq!(forbidden("reqwest::Client::new()"), ["reqwest"]);
    assert_eq!(forbidden("fn from_environment() {}"), [""; 0]);
    assert_eq!(forbidden("AmazonS3BuilderX"), [""; 0]);
}

#[test]
fn only_the_client_file_builds_a_store_client() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    let client = src.join("store").join("client.rs");
    let mut bad = Vec::new();
    let mut builders = 0;
    for file in &files {
        for (n, line) in code(file) {
            for name in forbidden(&line) {
                if file == &client && name == "AmazonS3Builder" {
                    builders += usize::from(line.contains("AmazonS3Builder::new()"));
                } else {
                    bad.push(format!("{}:{n}: {name}", file.display()));
                }
            }
        }
    }
    assert!(
        bad.is_empty(),
        "store clients outside store/client.rs:\n{}",
        bad.join("\n")
    );
    assert_eq!(
        builders, 1,
        "store/client.rs builds one AmazonS3Builder::new()"
    );
}
