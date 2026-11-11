//! Every crate's production dependencies are its code's: each `[dependencies]`
//! entry of every workspace member is named by the code of some file under
//! that member's `src/`. A dependency only a test uses is a dev-dependency, so
//! no consumer builds code nothing calls (review round 10: `lz4_flex` stayed a
//! production dependency of unityrs-core after `codec/lz4.rs` replaced it with
//! a port of liblz4's decoder, and reached every crate built on the core).
//!
//! Only code counts: comments, doc comments and the text of string literals
//! are dropped before a file is searched, so a dependency that a doc comment
//! names (`` `lz4_flex::block::decompress` decodes ``) and no code calls is
//! still unused (review round 11).
//!
//! The same holds for the platform: every C function a file under `crates/`
//! declares is one every glibc the workspace builds on has
//! ([`C_FUNCTIONS`]), so no test target fails to link on an older one
//! (review round 13), or one a crate's own vendored C defines
//! ([`VENDORED_C_FUNCTIONS`]).

use std::collections::BTreeMap;
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

/// Whether `code` (a Rust source through [`code_only`]) names the crate
/// `ident`: a path through it (`ident::`), a `use` of it, or one of its
/// macros (`ident!`). `ident` is a whole identifier there, which no
/// identifier character touches, and a path's first segment (`::ident::`,
/// never `x::ident::`, a module of `x`): `serde_json::`, `liblzma::`,
/// `my_dep::`, `crate::dep::`, `use depot` and `dep != 0` name no crate
/// `serde`, `lzma` or `dep` (review round 12). In a `use` group the path is
/// the group's (`use_group`): `use crate::{codec, dep::x}` names a module
/// `dep` of the crate, `use {dep::x}` the crate `dep` (review round 13: a
/// group's item was counted as a crate whatever path the group was in).
fn names(code: &str, ident: &str) -> bool {
    let b = code.as_bytes();
    let word = |c: &u8| c.is_ascii_alphanumeric() || *c == b'_';
    code.match_indices(ident).any(|(at, _)| {
        let (before, after) = (&b[..at], &b[at + ident.len()..]);
        if before.last().is_some_and(word) || after.first().is_some_and(word) {
            return false;
        }
        if let Some(path) = before.strip_suffix(b"::")
            && path.last().is_some_and(|c| word(c) || *c == b'>')
        {
            return false;
        }
        if let Some(first) = use_group(before) {
            return first;
        }
        let spaced = before.trim_ascii_end();
        let used = spaced.len() < before.len()
            && spaced
                .strip_suffix(b"use")
                .is_some_and(|u| !u.last().is_some_and(word));
        used || after.starts_with(b"::") || (after.starts_with(b"!") && !after.starts_with(b"!="))
    })
}

/// Whether the item that `before` (code up to an identifier) leaves open
/// is in a `use` group, and if so whether that identifier is its path's
/// first segment: `Some(true)` when no group around it has a path before
/// its brace (`use {dep::x}`, `use ::{a, dep}`, `use {a::b, {dep}}`),
/// `Some(false)` when one has (`use crate::{a, dep::x}`, `use
/// super::{dep::x}`, `use x::{a::{dep}}`: a module of that path), and `None`
/// when `before` leaves no group's item open: it does not end in `{` or `,`
/// (an item's first segment follows one), or the brace it reaches is not a
/// group's (a block's, after `)` or `=`; a struct literal's, after a type),
/// or it reaches code no `use` tree holds (`;`, `(`, `.`, a lone `:`).
fn use_group(before: &[u8]) -> Option<bool> {
    let word = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let ends_with_word = |b: &[u8], w: &[u8]| {
        b.strip_suffix(w)
            .is_some_and(|rest| !rest.last().is_some_and(|c| word(*c)))
    };
    let opens_item = |b: &[u8]| matches!(b.trim_ascii_end().last(), Some(b'{' | b','));
    if !opens_item(before) {
        return None;
    }
    let mut i = before.len();
    // Groups closed between here and the brace being looked for.
    let mut closed = 0usize;
    while i > 0 {
        i -= 1;
        match before[i] {
            c if c.is_ascii_whitespace() || word(c) || c == b',' || c == b'*' => {}
            b':' if i > 0 && before[i - 1] == b':' => i -= 1,
            b'}' => closed += 1,
            b'{' if closed > 0 => closed -= 1,
            b'{' => {
                let head = before[..i].trim_ascii_end();
                if let Some(path) = head.strip_suffix(b"::") {
                    let path = path.trim_ascii_end();
                    // `use ::{` is the root; any other path is a module's.
                    return Some(
                        ends_with_word(path, b"use") || !path.last().is_some_and(|c| word(*c)),
                    );
                }
                if ends_with_word(head, b"use") {
                    return Some(true);
                }
                if !opens_item(head) {
                    // A block's or a literal's brace.
                    return None;
                }
                // A group with no path of its own, inside another: go on to
                // the brace around it.
            }
            _ => return None,
        }
    }
    None
}

/// `text` with its comments (`//` to the end of the line, doc comments
/// included, and `/* */`, nested) and the contents of its string literals
/// dropped, so that only code is left to search. A string or a character
/// literal is skipped whole, so a `//` or a `"` inside one starts nothing; a
/// lifetime's `'` is code.
fn code_only(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let ident_before = i > 0 && (b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_');
        match b[i] {
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                let mut depth = 0usize;
                while i < b.len() {
                    if b[i..].starts_with(b"/*") {
                        depth += 1;
                        i += 2;
                    } else if b[i..].starts_with(b"*/") {
                        depth -= 1;
                        i += 2;
                        if depth == 0 {
                            break;
                        }
                    } else {
                        i += 1;
                    }
                }
                out.push(b' ');
            }
            b'"' => {
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
                i += 1;
                out.extend_from_slice(b"\"\"");
            }
            b'r' | b'b' | b'c' if !ident_before && raw_string(&b[i..]).is_some() => {
                let (hashes, open) = raw_string(&b[i..]).unwrap_or_default();
                let close: Vec<u8> = std::iter::once(b'"')
                    .chain(std::iter::repeat_n(b'#', hashes))
                    .collect();
                i += open;
                while i < b.len() && !b[i..].starts_with(&close) {
                    i += 1;
                }
                i += close.len();
                out.extend_from_slice(b"\"\"");
            }
            b'\'' => {
                // A character literal ('x', '\n', '\u{..}', '"'), or a
                // lifetime or label ('a), which is code.
                let len = text[i + 1..].chars().next().map_or(0, char::len_utf8);
                if b.get(i + 1) == Some(&b'\\') {
                    i += 2;
                    while i < b.len() && b[i] != b'\'' {
                        i += 1;
                    }
                    i += 1;
                    out.extend_from_slice(b"' '");
                } else if len > 0 && b.get(i + 1 + len) == Some(&b'\'') {
                    i += len + 2;
                    out.extend_from_slice(b"' '");
                } else {
                    out.push(b'\'');
                    i += 1;
                }
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The `#`s and the length of the opening of a raw string literal at the
/// start of `b` (`r"`, `r#"`, `br##"`, `cr"`), if one starts there.
fn raw_string(b: &[u8]) -> Option<(usize, usize)> {
    let mut i = usize::from(matches!(b.first(), Some(b'b' | b'c')));
    if b.get(i) != Some(&b'r') {
        return None;
    }
    i += 1;
    let hashes = b[i..].iter().take_while(|&&c| c == b'#').count();
    (b.get(i + hashes) == Some(&b'"')).then_some((hashes, i + hashes + 1))
}

/// The C functions the workspace's Rust may declare (in an `extern` block),
/// each with the glibc version that first had it on x86-64: its oldest
/// symbol version in the dev container's glibc 2.36 (`nm -D
/// --with-symbol-versions /lib/x86_64-linux-gnu/libc.so.6`). Each is older
/// than [`OLDEST_GLIBC`], so a target that declares it links on every glibc
/// the workspace builds on. A newer function is looked up when the test
/// runs: `lzma_decoder_heap.rs` finds `mallinfo2` (2.33) with `dlsym`
/// (review round 13: it declared it, and its test target did not link on
/// glibc 2.28 to 2.32).
const C_FUNCTIONS: [(&str, &str); 6] = [
    ("dlsym", "2.2.5"),
    ("getrlimit", "2.2.5"),
    ("gnu_get_libc_version", "2.2.5"),
    ("malloc_usable_size", "2.2.5"),
    ("mallopt", "2.2.5"),
    ("setrlimit", "2.2.5"),
];

/// The C functions a crate compiles from vendored source, which link
/// whatever the glibc: zlib 1.2.13's, which unityrs-imageio's `build.rs`
/// compiles under `Z_PREFIX`, each declared by the `z_` name that
/// `zconf.h` gives it there, and only in the file named.
const VENDORED_C_FUNCTIONS: [(&str, &str); 6] = [
    ("z_crc32", "unityrs-imageio/src/zlib.rs"),
    ("z_deflate", "unityrs-imageio/src/zlib.rs"),
    ("z_deflateBound", "unityrs-imageio/src/zlib.rs"),
    ("z_deflateEnd", "unityrs-imageio/src/zlib.rs"),
    ("z_deflateInit2_", "unityrs-imageio/src/zlib.rs"),
    ("z_zlibVersion", "unityrs-imageio/src/zlib.rs"),
];

/// The oldest glibc Rust's `*-linux-gnu` targets run on, which the
/// workspace builds on with them.
const OLDEST_GLIBC: [u32; 3] = [2, 17, 0];

/// The names `code` (a Rust source through [`code_only`]) declares in its
/// `extern` blocks (`extern "C" { .. }`, `unsafe extern "C" { .. }`,
/// `extern { .. }`): each `fn` and `static`. A function with an ABI of its
/// own (`extern "C" fn f() {}`), a function pointer type and `extern crate`
/// declare none.
fn c_functions(code: &str) -> Vec<String> {
    let b = code.as_bytes();
    let word = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let ident = |at: usize| {
        let end = b[at..]
            .iter()
            .position(|c| !word(*c))
            .map_or(b.len(), |n| at + n);
        &code[at..end]
    };
    let skip_space = |mut at: usize| {
        while b.get(at).is_some_and(u8::is_ascii_whitespace) {
            at += 1;
        }
        at
    };
    let mut out = Vec::new();
    for (at, _) in code.match_indices("extern") {
        if (at > 0 && word(b[at - 1])) || b.get(at + 6).is_some_and(|c| word(*c)) {
            continue;
        }
        let mut i = skip_space(at + 6);
        if b[i..].starts_with(b"\"\"") {
            i = skip_space(i + 2);
        }
        if b.get(i) != Some(&b'{') {
            continue;
        }
        let mut depth = 0usize;
        let mut block = i;
        while let Some(&c) = b.get(block) {
            match c {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            block += 1;
        }
        let body = &code[i..block];
        for kw in ["fn", "static"] {
            for (k, _) in body.match_indices(kw) {
                let start = i + k;
                if (start > 0 && word(b[start - 1]))
                    || b.get(start + kw.len()).is_some_and(|c| word(*c))
                {
                    continue;
                }
                let mut name = skip_space(start + kw.len());
                if kw == "static"
                    && b[name..].starts_with(b"mut")
                    && !b.get(name + 3).is_some_and(|c| word(*c))
                {
                    name = skip_space(name + 3);
                }
                let name = ident(name);
                if !name.is_empty() {
                    out.push(name.to_owned());
                }
            }
        }
    }
    out
}

fn glibc_version(v: &str) -> [u32; 3] {
    let mut parts = v.split('.').map(|p| p.parse::<u32>().unwrap());
    [0; 3].map(|_| parts.next().unwrap_or(0))
}

#[test]
fn an_extern_block_is_read_for_its_declarations_only() {
    let code = code_only(
        "unsafe extern \"C\" {\n    fn a(x: *mut u8) -> usize;\n    pub fn b();\n    \
         static C: u8;\n    static mut D: u8;\n}\nextern {\n    fn e();\n}\n\
         extern \"C\" fn not_declared() {}\nextern crate f;\n\
         type G = unsafe extern \"C\" fn() -> u8;\nfn h() { let _ = std::mem::transmute::<u8, \
         extern \"C\" fn()>(0); }\n// extern \"C\" { fn commented(); }\n",
    );
    assert_eq!(c_functions(&code), ["a", "b", "C", "D", "e"]);
}

/// Review round 13: `tests/lzma_decoder_heap.rs` declared `mallinfo2`,
/// which glibc has only since 2.33, so `cargo test` failed to link on an
/// older glibc. Every C function a file under `crates/` declares is in
/// [`C_FUNCTIONS`], each older than [`OLDEST_GLIBC`], and each there is
/// declared somewhere, so the list says what the workspace links.
#[test]
fn every_c_function_a_crate_declares_links_on_every_glibc_it_builds_on() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut sources = Vec::new();
    files(&crates, &mut sources);
    let known: BTreeMap<&str, &str> = C_FUNCTIONS.into_iter().collect();
    let vendored: BTreeMap<&str, &str> = VENDORED_C_FUNCTIONS.into_iter().collect();
    let mut declared = BTreeMap::new();
    for file in sources
        .iter()
        .filter(|p| p.extension().is_some_and(|e| e == "rs"))
    {
        let code = code_only(&std::fs::read_to_string(file).unwrap());
        let path = file.strip_prefix(&crates).unwrap_or(file);
        assert!(
            !code.contains("link_name"),
            "{}: a `link_name` hides which C symbol links; declare it by its own name",
            path.display()
        );
        for name in c_functions(&code) {
            if let Some(file) = vendored.get(name.as_str()) {
                assert_eq!(
                    path,
                    Path::new(file),
                    "{name} is the vendored C of {file} alone"
                );
                declared.entry(name).or_insert_with(|| path.to_path_buf());
                continue;
            }
            let Some(since) = known.get(name.as_str()) else {
                panic!(
                    "{}: declares the C function {name}, which C_FUNCTIONS does not list: \
                     list it with the glibc that first had it, if that is {}.{} or older, \
                     or look it up when the code runs (dlsym)",
                    path.display(),
                    OLDEST_GLIBC[0],
                    OLDEST_GLIBC[1]
                );
            };
            assert!(
                glibc_version(since) <= OLDEST_GLIBC,
                "{name}: glibc {since} is newer than the oldest the workspace builds on"
            );
            declared.entry(name).or_insert_with(|| path.to_path_buf());
        }
    }
    for (name, _) in C_FUNCTIONS {
        assert!(
            declared.contains_key(name),
            "C_FUNCTIONS lists {name}, which no file declares"
        );
    }
    // Each vendored function is declared, and is one zlib's `zconf.h`
    // renames to it under `Z_PREFIX` (`#  define deflate  z_deflate`).
    let zconf =
        std::fs::read_to_string(crates.join("unityrs-imageio/vendor/zlib-1.2.13/zconf.h")).unwrap();
    let renames: Vec<Vec<&str>> = zconf
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>())
        .filter(|w| w.len() == 4 && w[0] == "#" && w[1] == "define")
        .collect();
    for (name, _) in VENDORED_C_FUNCTIONS {
        assert!(
            declared.contains_key(name),
            "VENDORED_C_FUNCTIONS lists {name}, which no file declares"
        );
        assert!(
            renames
                .iter()
                .any(|w| Some(w[2]) == name.strip_prefix("z_") && w[3] == name),
            "{name} is no function zconf.h renames under Z_PREFIX"
        );
    }
}

#[test]
fn only_code_names_a_crate() {
    // Named only in comments, doc comments or strings: not a use.
    for text in [
        "/// `dep::decode` decodes a block.\nfn f() {}\n",
        "//! Uses `dep!` and `use dep`.\n",
        "fn f() {} // dep::x\n",
        "/* dep::x /* nested */ dep::y */ fn f() {}\n",
        "const S: &str = \"use dep; dep::x\";\n",
        "const S: &str = r#\"dep::x \"quoted\" \"#;\n",
        "const B: &[u8] = br\"dep::x\";\n",
        "const S: &str = \"a \\\" dep::x\";\n",
        "const Q: char = '\"'; const S: &str = \"dep::x\";\n",
    ] {
        assert!(!names(&code_only(text), "dep"), "counted a use in {text:?}");
    }
    // Named as part of another identifier, or as a module of another path:
    // not a use (review round 12).
    for (text, ident) in [
        ("use serde_json::Value;\n", "serde"),
        ("fn f() { liblzma::decode(); }\n", "lzma"),
        ("fn f() { my_dep::x(); }\n", "dep"),
        ("fn f() { crate::dep::x(); }\n", "dep"),
        ("fn f() { <T as Tr>::dep::x(); }\n", "dep"),
        ("use depot;\n", "dep"),
        ("fn f() { reuse dep(); }\n", "dep"),
        ("fn f() { undep!(); }\n", "dep"),
        ("fn f(dep: u8) -> bool { dep != 0 }\n", "dep"),
        ("fn f(dep: u8) -> bool { dep!=0 }\n", "dep"),
        // A module in a `use` group under another path (review round 13).
        ("use crate::{codec, dep::x};\n", "dep"),
        ("use crate::{dep::x};\n", "dep"),
        ("use super::{dep::x, y};\n", "dep"),
        (
            "use crate::{\n    budget::DecodeBuf,\n    dep::x,\n};\n",
            "dep",
        ),
        ("use self::{a::{b, c}, dep::x};\n", "dep"),
        ("use x::{a::{dep::y}};\n", "dep"),
        ("use x::{{dep::y}};\n", "dep"),
        ("pub(crate) use std::{dep};\n", "dep"),
    ] {
        assert!(
            !names(&code_only(text), ident),
            "counted {ident} as used in {text:?}"
        );
    }
    // Named by code, around comments, strings, characters and lifetimes.
    for text in [
        "use dep::Thing;\n",
        "fn f() { dep::decode(&[]); }\n",
        "fn f() { dep!(); }\n",
        "/// Doc.\nfn f() { let x = \"//\"; dep::g(x); }\n",
        "fn f<'a>(x: &'a str) -> char { let _ = x; dep::g(); '/' }\n",
        "const Q: char = '\\''; fn f() { dep::g(); }\n",
        "/* a */ fn f() { dep::g(); } // b\n",
        "fn f() -> char { 'é' } fn g() { dep::h(); }\n",
        "pub use dep as d;\n",
        "use\tdep;\n",
        "fn f() { ::dep::g(); }\n",
        "fn f() { (dep::g)(); }\n",
        "fn f() -> Vec<u8> { dep![1] }\n",
        // The crate, in a `use` group with no path before it.
        "use {dep::x};\n",
        "use ::{a, dep::x};\n",
        "use {a::b, {dep::x}};\n",
        "pub use {dep};\n",
        // A path through it in a block or a list, after a brace or a comma.
        "fn f() { dep::g(); }\n",
        "fn f() {{ dep::g() }}\n",
        "fn f() -> u8 { match 1 { _ => {}, } ; g(a, dep::h()) }\n",
        "const A: [u8; 2] = [1, dep::B];\n",
        "fn f() -> S { S { a: 1, b: dep::g() } }\n",
        "fn f() { if x {} else { dep::g() } }\n",
    ] {
        assert!(names(&code_only(text), "dep"), "missed the use in {text:?}");
    }
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
        let codes: Vec<String> = sources
            .iter()
            .filter(|p| p.extension().is_some_and(|e| e == "rs"))
            .map(|p| code_only(&std::fs::read_to_string(p).unwrap()))
            .collect();
        assert!(!codes.is_empty(), "{}: no source", manifest.display());
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
            if !codes.iter().any(|c| names(c, &ident)) {
                unused.push(format!("{} -> {name}", package["name"].as_str().unwrap()));
            }
        }
    }
    assert!(checked >= 10, "only {checked} dependencies checked");
    assert!(
        unused.is_empty(),
        "production dependencies no source file's code names (make them \
         dev-dependencies, or remove them): {unused:?}"
    );
}
