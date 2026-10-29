//! Gate G6's script (`scripts/stub-check.sh`, plan section 6) passes a tree
//! with no stubs and fails one with any sign of a stub.
//!
//! Each case writes a small tree under the system temp directory, runs the
//! script on it and removes it. The stub markers are assembled at run time
//! so this file itself never matches the script.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

/// The workspace root, two levels above this crate.
fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A temporary tree holding `files` (path, text), removed on drop.
struct Tree(PathBuf);

impl Tree {
    fn new(files: &[(&str, String)]) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "unityrs-stub-check-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        for (path, text) in files {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        Self(root)
    }

    /// The script's exit code and output on this tree.
    fn check(&self) -> (i32, String) {
        let out = Command::new("sh")
            .arg(workspace().join("scripts/stub-check.sh"))
            .arg(&self.0)
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout).into_owned()
            + &String::from_utf8_lossy(&out.stderr);
        (out.status.code().unwrap(), text)
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn marker(owner: char) -> String {
    format!("{}({owner})", "STUB")
}

fn todo_call() -> String {
    format!("{}!()", "todo")
}

fn lint(name: &str) -> String {
    format!("clippy::{name}")
}

/// A crate file with no stub: the core's real `lib.rs` (its doc names the
/// macro, and it denies both lints) beside an implemented function.
fn clean() -> Vec<(&'static str, String)> {
    let lib = std::fs::read_to_string(workspace().join("crates/unityrs-core/src/lib.rs")).unwrap();
    vec![
        ("crates/unityrs-core/src/lib.rs", lib),
        (
            "crates/a/src/lib.rs",
            format!(
                "// Never {}: a comment may name it.\npub fn f() -> u8 {{\n    1\n}}\n",
                todo_call()
            ),
        ),
    ]
}

fn with(extra: (&'static str, String)) -> Vec<(&'static str, String)> {
    let mut files = clean();
    files.push(extra);
    files
}

#[test]
fn a_tree_without_stubs_passes() {
    let (code, text) = Tree::new(&clean()).check();
    assert_eq!(code, 0, "{text}");
}

#[test]
fn the_vendored_rabex_crate_is_not_checked() {
    let rabex = format!("pub fn upstream() {{\n    {}\n}}\n", todo_call());
    let (code, text) = Tree::new(&with(("crates/unityrs-rabex/src/de.rs", rabex))).check();
    assert_eq!(code, 0, "{text}");
}

#[test]
fn every_sign_of_a_stub_fails() {
    let cases = [
        (
            "a marker comment",
            format!("// {}: delete this.\npub fn f() {{}}\n", marker('C')),
        ),
        (
            "a blocked test",
            format!(
                "#[test]\n#[ignore = \"{}: waits on the walker\"]\nfn t() {{}}\n",
                marker('B')
            ),
        ),
        (
            "a stub body",
            format!("pub fn f() -> u8 {{\n    {}\n}}\n", todo_call()),
        ),
        (
            "an allow over several lines, with no marker",
            format!(
                "#![allow(\n    {},\n    unused_variables\n)]\npub fn f() {{}}\n",
                lint("todo")
            ),
        ),
        (
            "an allow inside cfg_attr",
            format!(
                "#![cfg_attr(\n    not(test),\n    allow({})\n)]\n",
                lint("unimplemented")
            ),
        ),
        (
            "an expect on one line",
            format!(
                "#[expect({}, reason = \"later\")]\nfn f() {{}}\n",
                lint("todo")
            ),
        ),
    ];
    for (what, text) in cases {
        let (code, out) = Tree::new(&with(("crates/b/src/m.rs", text))).check();
        assert_eq!(code, 1, "{what} passed:\n{out}");
        assert!(out.contains("crates/b/src/m.rs"), "{what}: {out}");
    }
}

#[test]
fn every_hit_names_its_owning_package() {
    // Round 5: the script listed stubs with no owner, and the empty tables
    // package A generates were not listed at all. A hit names the package
    // its own line's marker names (a blocked test: the package it waits
    // on), else the file's first marker's, else `?`.
    let module = format!(
        "// {}: delete this allow.\n#![allow({})]\npub fn f() -> u8 {{\n    {}\n}}\n#[test]\n#[ignore = \"{}: waits on the walker\"]\nfn t() {{}}\n",
        marker('C'),
        lint("todo"),
        todo_call(),
        marker('B')
    );
    let table = format!(
        "/// Pairs.\n// {}: empty until the generator fills it.\npub static T: &[u8] = &[];\n",
        marker('A')
    );
    let unowned = format!("pub fn g() {{\n    {}\n}}\n", todo_call());
    let mut files = clean();
    files.push(("crates/b/src/m.rs", module));
    files.push(("crates/b/src/table.rs", table));
    files.push(("crates/b/src/z.rs", unowned));
    let (code, out) = Tree::new(&files).check();
    assert_eq!(code, 1, "{out}");
    let hits: Vec<&str> = out
        .lines()
        .filter(|l| l.starts_with('['))
        .map(|l| l.split_once(": ").map_or(l, |(head, _)| head))
        .collect();
    assert_eq!(
        hits,
        [
            "[C] crates/b/src/m.rs:1",
            "[C] crates/b/src/m.rs:2",
            "[C] crates/b/src/m.rs:4",
            "[B] crates/b/src/m.rs:7",
            "[A] crates/b/src/table.rs:2",
            "[?] crates/b/src/z.rs:2",
        ],
        "{out}"
    );
    assert!(
        out.contains("hits per package: ? 1, A 1, B 1, C 3"),
        "{out}"
    );
}
