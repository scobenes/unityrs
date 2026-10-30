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

/// Round 6: the script read only `todo!(` and `unimplemented!(`, so a
/// stub body with brackets or braces, or a space before its delimiter,
/// passed G6. Every delimiter fails, with spaces or line breaks (comment
/// lines too) between the macro's tokens, and the hit is the line that
/// names the macro.
#[test]
fn every_form_of_the_macros_fails() {
    let mut forms = Vec::new();
    for name in ["todo", "unimplemented"] {
        for (open, close) in [("(", ")"), ("[", "]"), ("{", "}")] {
            forms.push(format!("{name}!{open}{close}"));
            forms.push(format!("{name} ! {open} {close}"));
            forms.push(format!("core::{name}!{open}{close}"));
            forms.push(format!("{name}!\n    {open}{close}"));
            forms.push(format!("{name}\n\n    // why\n    !\n    {open}{close}"));
            // Round 7: a comment between the tokens on one line, a trailing
            // one, a block comment over lines and a nested one.
            forms.push(format!("{name} /* why */ !{open}{close}"));
            forms.push(format!("{name}!/* why */{open}{close}"));
            forms.push(format!("{name} // why\n    !{open}{close}"));
            forms.push(format!("{name} /* why\n    and why */ ! {open}{close}"));
            forms.push(format!("{name} /* a /* b */ c */ !{open}{close}"));
        }
    }
    for form in forms {
        let text = format!("pub fn f() -> u8 {{\n    {form}\n}}\n");
        let (code, out) = Tree::new(&with(("crates/b/src/m.rs", text))).check();
        assert_eq!(code, 1, "{form:?} passed:\n{out}");
        assert!(out.contains("crates/b/src/m.rs:2: "), "{form:?}: {out}");
    }
}

/// What only looks like the macros passes: another macro whose name ends
/// in one, a variable compared or assigned on the next line, a name with no
/// delimiter after it, a comment.
#[test]
fn what_only_looks_like_the_macros_passes() {
    let todo = "todo";
    let text = format!(
        "pub fn f() -> bool {{\n    my_{todo}!();\n    let {todo} = 1;\n    let x = {todo}\n        != 2;\n             let y = {todo}\n        + 1;\n    // {todo}![]\n    x\n}}\n"
    );
    let (code, out) = Tree::new(&with(("crates/b/src/m.rs", text))).check();
    assert_eq!(code, 0, "{out}");
}

/// Round 7: a lint allowed under a form the line scan did not read (spaces
/// or a comment inside the path, the `restriction` group, `warn`, `#` and
/// `[` apart) passed G6. Each fails, at the line of the lint's path.
#[test]
fn every_form_of_an_allowed_lint_fails() {
    let path = |name: &str| lint(name);
    let cases = [
        ("#[allow(clippy :: todo)]".to_owned(), 1),
        (format!("#[allow({})]", path("restriction")), 1),
        ("#![allow(clippy /* why */ :: unimplemented)]".to_owned(), 1),
        (
            "#[allow(\n    unused,\n    clippy // why\n    ::todo\n)]".to_owned(),
            3,
        ),
        (format!("#[warn({})]", path("todo")), 1),
        (
            format!(
                "#![cfg_attr(\n    all(),\n    expect({}, reason = \"later\")\n)]",
                path("restriction")
            ),
            3,
        ),
        (format!("# ! [ allow ( {} ) ]", path("unimplemented")), 1),
    ];
    for (attr, line) in cases {
        let text = format!("{attr}\npub fn f() -> u8 {{\n    1\n}}\n");
        let (code, out) = Tree::new(&with(("crates/b/src/m.rs", text))).check();
        assert_eq!(code, 1, "{attr:?} passed:\n{out}");
        assert!(
            out.contains(&format!("crates/b/src/m.rs:{line}: ")),
            "{attr:?}: {out}"
        );
    }
}

/// Round 7: either macro imported under another name was called by a name
/// no scan read. The import fails, on one line or several.
#[test]
fn a_macro_imported_under_another_name_fails() {
    let todo = "todo";
    let cases = [
        (format!("use core::{todo} as later;"), 1),
        (format!("pub use std::{todo} /* why */ as _;"), 1),
        ("use core::{\n    unimplemented as later,\n};".to_owned(), 2),
    ];
    for (import, line) in cases {
        let text = format!("{import}\npub fn f() -> u8 {{\n    later!()\n}}\n");
        let (code, out) = Tree::new(&with(("crates/b/src/m.rs", text))).check();
        assert_eq!(code, 1, "{import:?} passed:\n{out}");
        assert!(
            out.contains(&format!("crates/b/src/m.rs:{line}: ")),
            "{import:?}: {out}"
        );
    }
}

/// The script reads Rust's comments, strings and character literals as
/// such: what they hold passes, and the code after them is still read
/// (a lifetime is no character literal, an escaped quote ends no string).
#[test]
fn comments_strings_and_characters_are_read_as_rust_reads_them() {
    let (todo, call, allow) = ("todo", todo_call(), lint("todo"));
    let clean = format!(
        "/* {call} /* nested {call} */ still a comment {call} */\n\
         /// use core::{todo} as later;\n\
         #[doc = \"#[allow({allow})] {call}\"]\n\
         #[cfg_attr(not(test), deny({allow}, {}))]\n\
         #[forbid({})]\n\
         pub fn f<'a>(x: &'a u8) -> (&'a u8, [&'static str; 6], char, u8) {{\n\
             let quotes = ['\"', '\\'', '\\\\'];\n\
             let s = [\"{call}\", \"\\\"{call}\\\"\", r#\"{call} \" \"#, \"a\n    {call}\n\", \"\\\\\", \"\"];\n\
             let b = b\"{call}\";\n\
             (x, s, quotes[0], b[0] + b'\\'')\n\
         }}\n",
        lint("unimplemented"),
        lint("restriction"),
    );
    let (code, out) = Tree::new(&with(("crates/b/src/m.rs", clean.clone()))).check();
    assert_eq!(code, 0, "{clean}\n{out}");
    // A stub body after each literal is still read.
    for after in [
        "let _: &'static str = \"\"; ",
        "let _ = '\"'; ",
        "let _ = b'\\''; ",
        "let _ = \"\\\\\"; ",
        "let _ = r#\"a\"#; ",
        "/* a */ ",
        "let _ = 'a'; ",
    ] {
        let text = format!("pub fn f<'a>(x: &'a u8) -> u8 {{\n    {after}{call}\n}}\n");
        let (code, out) = Tree::new(&with(("crates/b/src/m.rs", text.clone()))).check();
        assert_eq!(code, 1, "{text} passed:\n{out}");
        assert!(out.contains("crates/b/src/m.rs:2: "), "{text}: {out}");
    }
}

/// Rounds 6 and 7: clippy denied the macros only in the core and the tpk
/// crate, and then `deny` let an attribute allow them again. The
/// workspace's lints forbid both, and every crate inherits them, so no
/// attribute allows them (`allow(clippy::restriction)` included: E0453)
/// and a reachable call fails `cargo clippy` in any crate.
#[test]
fn every_crate_forbids_the_macros_through_the_workspace_lints() {
    let root = std::fs::read_to_string(workspace().join("Cargo.toml")).unwrap();
    let clippy = root
        .split("[workspace.lints.clippy]")
        .nth(1)
        .and_then(|rest| rest.split("\n[").next())
        .unwrap();
    for name in ["todo", "unimplemented"] {
        assert!(
            clippy
                .lines()
                .any(|l| l.trim() == format!("{name} = \"forbid\"")),
            "{name} is not forbidden:\n{clippy}"
        );
    }
    let mut crates = 0;
    for entry in std::fs::read_dir(workspace().join("crates")).unwrap() {
        let manifest = entry.unwrap().path().join("Cargo.toml");
        let Ok(text) = std::fs::read_to_string(&manifest) else {
            continue;
        };
        crates += 1;
        assert!(
            text.contains("[lints]\nworkspace = true\n"),
            "{} does not inherit the workspace's lints",
            manifest.display()
        );
    }
    assert!(crates >= 6, "{crates} crates");
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
