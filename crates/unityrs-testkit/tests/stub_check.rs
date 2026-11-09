//! Gate G6's script (`scripts/stub-check.sh`, plan section 6) passes a tree
//! with no stubs and fails one with any sign of a stub.
//!
//! Each case writes a small tree under the system temp directory, runs the
//! script on it and removes it. The stub markers are assembled at run time
//! so this file itself never matches the script.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use unityrs_testkit::child;
use unityrs_testkit::scratch::Scratch;

/// The workspace root, two levels above this crate.
fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A temporary tree holding `files` (path, text), removed on drop
/// (`unityrs_testkit::scratch`).
struct Tree(
    PathBuf,
    #[allow(dead_code, reason = "held for its drop, which removes the folder")] Scratch,
);

impl Tree {
    fn new(files: &[(&str, String)]) -> Self {
        let dir = Scratch::new("stub-check");
        let root = dir.to_path_buf();
        for (path, text) in files {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        Self(root, dir)
    }

    /// The same tree with a symlink at `path` to `target`.
    fn link(self, path: &str, target: &Path) -> Self {
        let path = self.0.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(target, path).unwrap();
        self
    }

    /// The script's exit code and output on this tree.
    fn check(&self) -> (i32, String) {
        let out = child::output(
            Command::new("sh")
                .arg(workspace().join("scripts/stub-check.sh"))
                .arg(&self.0),
        )
        .unwrap();
        let text = String::from_utf8_lossy(&out.stdout).into_owned()
            + &String::from_utf8_lossy(&out.stderr);
        (out.status.code().unwrap(), text)
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

/// The two macros' names, held in strings so this file never names them
/// as words of its own.
const NAMES: (&str, &str) = ("todo", "unimplemented");

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

/// Each case (a tree, what it holds, and the text its hit prints) that the
/// script does not fail with that hit, with the script's output, so a
/// failing test lists every form that passed.
fn missed(cases: impl IntoIterator<Item = (Tree, String, String)>) -> String {
    let mut missed = String::new();
    for (tree, what, hit) in cases {
        let (code, out) = tree.check();
        if code != 1 || !out.contains(&hit) {
            writeln!(missed, "--- {what}\nexit {code}, no `{hit}`:\n{out}").unwrap();
        }
    }
    missed
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
            forms.push(format!("r#{name}!{open}{close}"));
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

/// What only looks like the macros passes: a longer word holding either
/// name (another macro whose name ends in one, a variable, a type, a
/// `restriction` that no `::` comes before), a comment, a string, and the
/// lint paths a `deny` or `forbid` names in an inner attribute at the
/// file's top level, inside `cfg_attr`s or not, whatever whitespace or
/// comments lie between their tokens.
#[test]
fn what_only_looks_like_the_macros_passes() {
    let (todo_name, unimpl_name) = NAMES;
    let text = format!(
        "#![deny(clippy::{todo_name})]\n\
         #![cfg_attr(\n    not(test),\n    deny(\n        clippy::unwrap_used,\n        clippy::{todo_name},\n        clippy::{unimpl_name},\n    )\n)]\n\
         # ! [ forbid ( clippy /* why */ :: {unimpl_name} , clippy::restriction ) ]\n\
         #![cfg_attr(a, cfg_attr(not(b), forbid(clippy::{todo_name})))]\n\
         pub fn f(restriction: u8) -> bool {{\n    my_{todo_name}!();\n    let {todo_name}_list = 1;\n    let x: Todo = {unimpl_name}_yet() != restriction;\n    // {todo_name}![] and let {todo_name} = 1;\n    let _ = \"{todo_name}!()\";\n    x\n}}\n"
    );
    let (code, out) = Tree::new(&with(("crates/b/src/m.rs", text))).check();
    assert_eq!(code, 0, "{out}");
}

/// Review round 9: the scan's rule became one sentence. Outside comments,
/// strings and character literals, either name, raw or not, appears only as
/// a `clippy::` lint name directly under `deny(` or `forbid(` in an inner
/// attribute at its file's top level. So any other word of either name
/// fails at its line: a variable, a field, a module or a function of that
/// name (round 9: `let todo` passed outside a macro call), a re-export (round
/// 9: `pub use core::todo;` gave a module a macro `clippy::todo!` could
/// call), and a lint path anywhere else: in an outer attribute (a derive or
/// an attribute macro on the item is handed it), inside a macro call, a
/// module, a function or an impl, under another attribute, or spelled
/// otherwise.
#[test]
fn every_other_word_of_either_name_fails() {
    let (todo_name, unimpl_name) = NAMES;
    let cases = [
        format!("pub fn f() -> u8 {{\n    let {todo_name} = 1;\n    {todo_name}\n}}\n"),
        format!("pub fn f(\n    {unimpl_name}: u8,\n) {{}}\n"),
        format!("pub struct S {{\n    pub {todo_name}: u8,\n}}\n"),
        format!("pub fn f(s: &S) -> u8 {{\n    s.{todo_name}\n}}\n"),
        format!("pub mod\n    r#{todo_name} {{}}\n"),
        format!("pub fn\n    {unimpl_name}() {{}}\n"),
        format!("mod clippy {{\n    pub use core::{todo_name};\n}}\n"),
        format!("pub use core::\n    {unimpl_name};\n"),
        // The lint path inside a module, a function, an impl or a macro call.
        format!("mod m {{\n    #![deny(clippy::{todo_name})]\n}}\n"),
        format!("pub fn f() {{\n    #[forbid(clippy::{unimpl_name})]\n    let _x = 1;\n}}\n"),
        format!("impl S {{\n    #[deny(clippy::{todo_name})]\n    fn g() {{}}\n}}\n"),
        format!("call! {{\n    #[deny(clippy::{todo_name})]\n}}\n"),
        "call!(\n    #![forbid(clippy::restriction)]\n);\n".to_owned(),
        // An outer attribute.
        format!("#[deny(\n    clippy::{todo_name})]\nfn g() {{}}\n"),
        format!("#[cfg_attr(x, forbid(\n    clippy::{unimpl_name}))]\npub struct S;\n"),
        // An inner one, but under another attribute or spelled otherwise.
        format!("#![anything(\n    clippy::{todo_name},\n)]\n"),
        format!("#![tool::deny(\n    clippy::{todo_name})]\n"),
        format!("#![r#deny(\n    clippy::{todo_name})]\n"),
        format!("#![deny(\n    clippy::r#{unimpl_name})]\n"),
        format!("#![deny(\n    r#clippy::{todo_name})]\n"),
        format!("#![deny(\n    tool::clippy::{todo_name})]\n"),
        format!("#![deny(\n    clippy::{todo_name}::more)]\n"),
        format!("#![deny(lint(\n    clippy::{todo_name}))]\n"),
        format!("#![cfg_attr(x, anything(deny(\n    clippy::{unimpl_name})))]\n"),
        format!("#![deny(deny(\n    clippy::{todo_name}))]\n"),
        "#![deny(\n    clippy::restriction::all)]\n".to_owned(),
        // No attribute: a macro call or an expression of the same shape.
        format!("call![deny(\n    clippy::{todo_name})];\n"),
        format!("pub static S: [u8; 0] = [deny(\n    clippy::{todo_name})];\n"),
        format!("pub const C: () = deny(\n    clippy::{todo_name});\n"),
        "#![allow(\n    clippy::r#restriction)]\n".to_owned(),
    ];
    let missed = missed(cases.map(|text| {
        let tree = Tree::new(&with(("crates/b/src/m.rs", text.clone())));
        (tree, text, "] crates/b/src/m.rs:2: ".to_owned())
    }));
    assert!(missed.is_empty(), "{missed}");
}

/// Review round 8: a `macro_rules!` body that puts the `!` after a name it
/// is handed (`$m!()`) or after one it holds (`todo $($t)*`) calls the macro
/// where no name scan reads the call, and clippy, which lints the macro whose
/// expansion the call starts, lints the outer one: both passed G6 with a
/// reachable `todo!()`. Review round 9: so did a raw name (a `macro_rules!
/// r#later` whose body holds the name, a macro called as `r#loop!` or
/// `r#if!`), and a lint path inside an attribute that is itself handed to a
/// macro (`call!(#[deny(clippy::todo)])`, `#[anything(..)]` too), which the
/// macro can make a call of. Each fails, at the line that holds the name.
#[test]
fn a_macro_named_inside_another_macro_fails() {
    let (todo_name, unimpl_name) = NAMES;
    let cases = [
        (
            format!(
                "macro_rules! call {{\n    ($m:ident) => {{ $m!() }};\n}}\npub fn f() -> u8 {{\n    call!({todo_name})\n}}\n"
            ),
            5,
        ),
        (
            format!(
                "macro_rules! call {{\n    ($m:path) => {{ $m!() }};\n}}\npub fn f() -> u8 {{\n    call![\n        core::{unimpl_name}\n    ]\n}}\n"
            ),
            6,
        ),
        (
            format!(
                "macro_rules! bang {{\n    ($($t:tt)*) => {{ {todo_name} $($t)* }};\n}}\npub fn f() -> u8 {{\n    bang!(!())\n}}\n"
            ),
            2,
        ),
        (
            format!(
                "macro_rules! two {{\n    ($a:ident $b:tt) => {{ $a $b () }};\n}}\npub fn f() -> u8 {{\n    two! {{ {unimpl_name} ! }}\n}}\n"
            ),
            5,
        ),
        // A lint path's spelling, inside a macro call: a module's macro.
        (
            format!(
                "mod clippy {{\n    pub use core::{todo_name};\n}}\nmacro_rules! call {{\n    ($($p:ident)::+) => {{ $($p)::+!() }};\n}}\npub fn f() -> u8 {{\n    call!(clippy::{todo_name})\n}}\n"
            ),
            8,
        ),
        (format!("#[stub_attr({todo_name})]\npub fn f() {{}}\n"), 1),
        // Review round 9: a lint path in an attribute handed to a macro,
        // which makes a call of it.
        (
            format!(
                "mod clippy {{\n    pub use core::{todo_name};\n}}\nmacro_rules! call {{\n    (#[deny($($p:ident)::+)]) => {{ $($p)::+!() }};\n}}\npub fn f() -> u8 {{\n    call!(#[deny(clippy::{todo_name})])\n}}\n"
            ),
            8,
        ),
        (
            format!(
                "mod clippy {{\n    pub use core::{unimpl_name};\n}}\nmacro_rules! call {{\n    (#[$a:ident($($p:ident)::+)]) => {{ $($p)::+!() }};\n}}\npub fn f() -> u8 {{\n    call!(#[anything(clippy::{unimpl_name})])\n}}\n"
            ),
            8,
        ),
        // Review round 9: raw names.
        (
            format!(
                "macro_rules! r#later {{\n    ($b:tt) => {{ {todo_name} $b () }};\n}}\npub fn f() -> u8 {{\n    later!(!)\n}}\n"
            ),
            2,
        ),
        (
            format!(
                "macro_rules! call {{\n    ($m:ident) => {{ $m!() }};\n}}\nuse call as r#loop;\npub fn f() -> u8 {{\n    r#loop!({todo_name})\n}}\n"
            ),
            6,
        ),
        (
            format!(
                "macro_rules! r#if {{\n    ($m:ident) => {{ $m!() }};\n}}\npub fn f() -> u8 {{\n    r#if!({todo_name})\n}}\n"
            ),
            5,
        ),
        (
            format!(
                "macro_rules! call {{\n    ($m:ident) => {{ $m!() }};\n}}\npub fn f() -> u8 {{\n    call!(r#{unimpl_name})\n}}\n"
            ),
            5,
        ),
        (format!("#![stub_attr(x, {unimpl_name})]\n"), 1),
    ];
    let missed = missed(cases.map(|(text, line)| {
        let tree = Tree::new(&with(("crates/b/src/m.rs", text.clone())));
        (tree, text, format!("] crates/b/src/m.rs:{line}: "))
    }));
    assert!(missed.is_empty(), "{missed}");
}

/// Round 7: a lint allowed under a form the line scan did not read (spaces
/// or a comment inside the path, the `restriction` group, `warn`, `#` and
/// `[` apart) passed G6. Each fails, at the line of the lint's name.
#[test]
fn every_form_of_an_allowed_lint_fails() {
    let path = |name: &str| lint(name);
    let cases = [
        ("#[allow(clippy :: todo)]".to_owned(), 1),
        (format!("#[allow({})]", path("restriction")), 1),
        ("#![allow(clippy /* why */ :: unimplemented)]".to_owned(), 1),
        (
            "#[allow(\n    unused,\n    clippy // why\n    ::todo\n)]".to_owned(),
            4,
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
    let (todo_name, _) = NAMES;
    let cases = [
        (format!("use core::{todo_name} as later;"), 1),
        (format!("pub use std::{todo_name} /* why */ as _;"), 1),
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

/// Review round 9: the scan read only `crates/`, never through a symlink,
/// so a module a `#[path]` names elsewhere in the tree, or one reached
/// through a symlinked folder or file, went unread, and a file name with a
/// space or a quote was split. Every `.rs` file under the root is read,
/// symlinks followed, but in the root's own `target/` and hidden folders
/// (a build's output, `.git`, worktrees); each hit names its file as given.
#[test]
fn every_rust_file_under_the_root_is_read() {
    let stub = || format!("pub fn f() -> u8 {{\n    {}\n}}\n", todo_call());
    let mut cases = Vec::new();
    for path in [
        "tools/stub.rs",
        "crates/b/target/m.rs",
        "crates/b/.hidden/m.rs",
        "crates/b/src/a b'\"c.rs",
    ] {
        let tree = Tree::new(&with((path, stub())));
        cases.push((tree, path.to_owned(), format!("] {path}:2: ")));
    }
    let outside = Tree::new(&[("m.rs", stub())]);
    for (link, target, shown) in [
        (
            "crates/b/src/sub",
            outside.0.clone(),
            "crates/b/src/sub/m.rs",
        ),
        (
            "crates/b/src/l.rs",
            outside.0.join("m.rs"),
            "crates/b/src/l.rs",
        ),
    ] {
        let tree = Tree::new(&clean()).link(link, &target);
        cases.push((
            tree,
            format!("a symlink at {link}"),
            format!("] {shown}:2: "),
        ));
    }
    let missed = missed(cases);
    assert!(missed.is_empty(), "{missed}");
    let mut files = clean();
    for path in [
        "target/debug/build/x/out/m.rs",
        ".git/m.rs",
        ".wt/w/crates/b/src/m.rs",
    ] {
        files.push((path, stub()));
    }
    let (code, out) = Tree::new(&files).check();
    assert_eq!(code, 0, "{out}");
}

/// The script reads Rust's comments, strings and character literals as
/// such: what they hold passes, and the code after them is still read
/// (a lifetime is no character literal, an escaped quote ends no string).
#[test]
fn comments_strings_and_characters_are_read_as_rust_reads_them() {
    let (todo_name, call, allow) = (NAMES.0, todo_call(), lint("todo"));
    let clean = format!(
        "/* {call} /* nested {call} */ still a comment {call} */\n\
         #![cfg_attr(not(test), deny({allow}, {}))]\n\
         #![forbid({})]\n\
         /// use core::{todo_name} as later;\n\
         #[doc = \"#[allow({allow})] {call}\"]\n\
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

/// Review round 10: rustc skips a first line that starts with `#!` unless
/// the first token after those two characters, past whitespace and
/// comments that are not doc comments, is `[` (`rustc_lexer::strip_shebang`),
/// in every file it compiles. The scan read `#!/*` and `#!"` as code that
/// opens a comment or a string running to the file's end, so a stub after
/// them passed it, and a stub a macro assembles passes clippy too.
#[test]
fn a_first_line_is_a_shebang_exactly_where_rustc_skips_it() {
    let (todo_name, unimpl_name) = NAMES;
    let deny = lint("todo");
    let assembled = |name: &str| {
        format!(
            "macro_rules! call {{\n    ($m:ident) => {{ $m!() }};\n}}\npub fn g() -> u8 {{\n    call!({name})\n}}\n"
        )
    };
    let mut cases = Vec::new();
    // A shebang: the stub five lines below it is read.
    for (first, name) in [
        ("#!/*", todo_name),
        ("#!\"", unimpl_name),
        ("\u{feff}#!/*", todo_name),
        ("#!/**/\"", todo_name),
        ("#!/***/ r#\"", unimpl_name),
        ("#!\u{2028}\"", todo_name),
        ("#!/// \"", todo_name),
        ("#!/*! [ */ \"", unimpl_name),
        ("#!/usr/bin/env some-runner", todo_name),
    ] {
        let text = format!("{first}\n{}", assembled(name));
        cases.push((
            Tree::new(&with(("crates/b/src/m.rs", text.clone()))),
            text,
            "crates/b/src/m.rs:6: ".to_owned(),
        ));
    }
    // An inner attribute: the line is code, and a stub on it is read.
    for (first, line) in [
        ("#! [", 1),
        ("#!\t/* c */[", 1),
        ("#!/**/[", 1),
        ("#!/***/[", 1),
        ("#!/* a /* nested */ comment */[", 1),
        ("#!\n[", 2),
        ("#!// c\n[", 2),
        ("#!/*\n*/[", 2),
    ] {
        let text = format!(
            "{first}deny({deny})] pub fn h() -> u8 {{ {} }}\n",
            todo_call()
        );
        cases.push((
            Tree::new(&with(("crates/b/src/m.rs", text.clone()))),
            text,
            format!("crates/b/src/m.rs:{line}: "),
        ));
        // And without the stub, its lint path is excused as `#![` is.
        let clean = format!("{first}deny({deny})]\npub fn f() -> u8 {{\n    1\n}}\n");
        let (code, out) = Tree::new(&with(("crates/b/src/m.rs", clean.clone()))).check();
        assert_eq!(code, 0, "{clean:?} failed:\n{out}");
    }
    // A doc comment is a token, so a `[` after one starts no attribute:
    // rustc skips the line, the stub on it included.
    for first in ["#!/*! c */[", "#!/** c */["] {
        let text = format!(
            "{first}deny({deny})] pub fn h() -> u8 {{ {} }}\n",
            todo_call()
        );
        let (code, out) = Tree::new(&with(("crates/b/src/m.rs", text.clone()))).check();
        assert_eq!(code, 0, "{text:?} failed:\n{out}");
    }
    let missed = missed(cases);
    assert!(missed.is_empty(), "{missed}");
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
