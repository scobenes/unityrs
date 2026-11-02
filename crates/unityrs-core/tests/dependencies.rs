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
/// `serde`, `lzma` or `dep` (review round 12).
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
        let spaced = before.trim_ascii_end();
        let used = spaced.len() < before.len()
            && spaced
                .strip_suffix(b"use")
                .is_some_and(|u| !u.last().is_some_and(word));
        used || after.starts_with(b"::") || (after.starts_with(b"!") && !after.starts_with(b"!="))
    })
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
