//! The crate's `unsafe` policy, held by a scan of its sources: `lib.rs`
//! denies `unsafe_code`, allows it for exactly two modules, `alloc` (the
//! counting allocator) and `sys` (the C library's calls std lacks), and no
//! other file of `src/` holds `unsafe` or allows it.

use std::path::{Path, PathBuf};

/// The modules `lib.rs` lets hold `unsafe` code, by file.
const ALLOWED: [&str; 3] = ["alloc.rs", "sys.rs", "sys/entry.rs"];

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

/// A file's code lines, comments left out, with their numbers.
fn code(path: &Path) -> Vec<(usize, String)> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim_start().starts_with("//"))
        .map(|(i, l)| (i + 1, l.to_owned()))
        .collect()
}

/// Rust word boundaries, including punctuation such as `unsafe(...)`
/// and every lint level that can weaken the crate's `unsafe_code` deny.
fn unsafe_tokens(line: &str) -> bool {
    line.split(|c: char| c != '_' && !c.is_alphanumeric())
        .any(|word| word == "unsafe" || word == "unsafe_code")
}

#[test]
fn the_scan_catches_unsafe_words_and_all_lint_levels() {
    for line in [
        "#[allow(unsafe_code)]",
        "#[expect(unsafe_code)]",
        "#[warn(unsafe_code)]",
        "#[warn(dead_code, unsafe_code)]",
        "    unsafe_code,", // A multi-line lint attribute.
        "#[unsafe(no_mangle)]",
        "unsafe{ f() }",
        "unsafe fn f() {}",
    ] {
        assert!(unsafe_tokens(line), "missed {line}");
    }
    for line in [
        "fn unsafe_words() {}",
        "let safe = 1;",
        "unsafe_code_suffix",
    ] {
        assert!(!unsafe_tokens(line), "false positive: {line}");
    }
}

#[test]
fn unsafe_code_is_allowed_for_alloc_and_sys_alone() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let lib = code(&src.join("lib.rs"));
    assert!(
        lib.iter().any(|(_, l)| l.trim() == "#![deny(unsafe_code)]"),
        "lib.rs denies unsafe_code"
    );
    // Each allow in lib.rs, and the item it stands on.
    let allows: Vec<String> = lib
        .iter()
        .enumerate()
        .filter(|(_, (_, l))| l.contains("allow(unsafe_code)"))
        .map(|(i, _)| {
            lib.get(i + 1)
                .map(|(_, next)| next.trim().to_owned())
                .unwrap_or_default()
        })
        .collect();
    assert_eq!(allows, ["pub mod alloc;", "pub mod sys;"]);

    let mut files = Vec::new();
    rust_files(&src, &mut files);
    files.sort();
    let mut found = Vec::new();
    for path in &files {
        let name = path.strip_prefix(&src).unwrap();
        let allowed = ALLOWED.iter().any(|a| name == Path::new(a));
        let lines = code(path);
        for (i, (n, l)) in lines.iter().enumerate() {
            let lib_rule = name == Path::new("lib.rs")
                && (l.trim() == "#![deny(unsafe_code)]"
                    || (l.trim() == "#[allow(unsafe_code)]"
                        && lines.get(i + 1).is_some_and(|(_, next)| {
                            ["pub mod alloc;", "pub mod sys;"].contains(&next.trim())
                        })));
            if !allowed && !lib_rule && unsafe_tokens(l) {
                found.push(format!("{}:{n}: {}", name.display(), l.trim()));
            }
        }
    }
    assert!(found.is_empty(), "unsafe outside alloc and sys: {found:#?}");
    for a in ALLOWED {
        assert!(
            code(&src.join(a)).iter().any(|(_, l)| l.contains("unsafe")),
            "{a} holds the unsafe code it is allowed"
        );
    }
}
