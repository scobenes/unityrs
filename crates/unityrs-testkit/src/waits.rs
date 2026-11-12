//! The source scans behind plan section 4.1's rules that the core never
//! waits and never starts a thread, and that it computes no lazy value
//! inside its cell.
//!
//! **Waits.** A batch engine runs core calls on its own pool threads, so a
//! core call that parks, sleeps, spins, waits on a condition or a channel,
//! or hands work to a thread of its own would let the pool starve itself, or
//! escape the engine's bound on threads. [`waits`] lists every line outside
//! test code that names one of [`FORBIDDEN`]: a thread function by its path
//! or bare (as a grouped or aliased import calls it), any use of
//! `std::thread` but `available_parallelism`, an alias of it, a spin hint
//! (`std::hint::spin_loop`, the older `spin_loop_hint`), a condition
//! variable, a barrier, a channel or a thread pool. A call token (one that
//! ends in `(`) matches with a turbofish and with spaces before its
//! parenthesis too (`spawn::<F, T>(`, `sleep (`).
//!
//! **Cells.** A value computed inside `OnceLock::get_or_init` (or a
//! `LazyLock`, a `Once`) runs caller code with the cell locked, which a
//! callback that reaches the same cell, or a thread holding Python's GIL,
//! deadlocks on (plan 3.16). [`cell_fills`] lists every line outside test
//! code and outside the crate's own `src/once.rs` (that one file: a `once.rs`
//! elsewhere in the tree is scanned) that names a cell-filling method or
//! type, by any form of call, a turbofish's included, or as a path, so
//! fills a cell any other way than through
//! `once::get_or_publish` or `get_or_publish_result`, whose tests pin that
//! only a move runs inside the cell. So every lazy value of the core, the
//! stubs' included once they are written, goes through the tested helper.
//!
//! **What neither scan checks: locks.** The scan cannot tell a lock held for
//! a lookup from one held across a call into caller code (a codec, a
//! provider, a resolver, a fetcher). That no core lock is held across caller
//! code is checked by tests that re-enter each public path that takes one:
//! the provider chain's (`provider::tests`), and packages C's and D's for
//! the bundle and the Environment (plan 4.1, 7.4, 7.5).
//!
//! **Test code** is an item, field, match arm or statement under
//! `#[cfg(test)]`, found by bracket depth, not by line endings: an item
//! (`fn`, `mod`, `impl`, `use`, a macro such as `thread_local!`) ends at the
//! `;` or the closing `}` of its first block at its own depth; anything
//! else (a struct field, a struct literal's field, a match arm, a statement)
//! also ends at a `,` at its own depth, and at the `}` or `)` that closes
//! what encloses it. String and character literals and comments do not
//! count toward the depth; comments are never scanned.
//!
//! Owner: the scaffold (implemented).

use std::path::Path;

/// What the core never names outside tests: thread starts, sleeps, parks,
/// spins and scopes (by path, or bare as an import calls them), `std::thread`
/// itself and its aliases (only `available_parallelism` is allowed), spin
/// hints, condition variables, barriers, channels, a thread pool.
pub const FORBIDDEN: &[&str] = &[
    "thread::spawn",
    "thread::scope",
    "thread::Builder",
    "thread::sleep",
    "thread::park",
    "thread::yield_now",
    "std::thread",
    "thread::",
    "thread as",
    "spawn(",
    "sleep(",
    "park(",
    "park_timeout(",
    "yield_now(",
    "scope(",
    "spin_loop",
    "Condvar",
    "Barrier",
    "mpsc",
    "rayon",
    ".wait(",
    ".wait_timeout",
    ".recv(",
];

/// The one `std::thread` item the core may name: it starts nothing.
const ALLOWED_THREAD: &str = "thread::available_parallelism";

/// What fills a cell with caller code inside it, outside `once.rs`: each
/// named at the start of a word, whatever follows (a call, a turbofish, a
/// path), so `call_once` names `call_once_force` too.
pub const CELL_FILLS: &[&str] = &[
    "get_or_init",
    "get_or_try_init",
    "get_mut_or_init",
    "LazyLock",
    "LazyCell",
    "call_once",
    "OnceCell",
];

/// The file that holds the one cell helper, relative to `src`.
const ONCE_FILE: &str = "once.rs";

/// Every `file:line: token` under `src` where non-test code names a
/// [`FORBIDDEN`] token (the first on the line).
///
/// # Panics
/// When `src` cannot be read (a test helper).
#[must_use]
pub fn waits(src: &Path) -> Vec<String> {
    scan_tree(src, |_| true, FORBIDDEN)
}

/// Every `file:line: token` under `src`, `src/once.rs` itself excepted,
/// where non-test code fills a cell other than through the `once` helper
/// ([`CELL_FILLS`]).
///
/// # Panics
/// When `src` cannot be read (a test helper).
#[must_use]
pub fn cell_fills(src: &Path) -> Vec<String> {
    scan_tree(src, |rel| rel != Path::new(ONCE_FILE), CELL_FILLS)
}

/// Scans every `.rs` file under `src` whose path relative to `src` `keep`
/// accepts.
fn scan_tree(src: &Path, keep: impl Fn(&Path) -> bool, tokens: &[&str]) -> Vec<String> {
    let mut files = Vec::new();
    collect(src, &mut files);
    files.sort();
    let mut found = Vec::new();
    for (name, text) in &files {
        let path = Path::new(name);
        if keep(path.strip_prefix(src).unwrap_or(path)) {
            scan(name, text, tokens, &mut found);
        }
    }
    found
}

fn collect(dir: &Path, out: &mut Vec<(String, String)>) {
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
        let path = entry
            .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
            .path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            out.push((path.display().to_string(), text));
        }
    }
}

fn scan(name: &str, text: &str, tokens: &[&str], found: &mut Vec<String>) {
    let code = live_text(text);
    for (i, line) in code.lines().enumerate() {
        let line = line.replace(ALLOWED_THREAD, "");
        if let Some(token) = tokens.iter().find(|t| names(&line, t)) {
            found.push(format!("{name}:{}: {token}", i + 1));
        }
    }
}

/// Whether `line` names `token`: a token that starts with an identifier
/// character must start a word there (`registry_scope(` is not `scope(`,
/// `t::spawn(` is `spawn(`), and a call token (one ending in `(`) matches
/// its name followed by spaces and a turbofish before the parenthesis too
/// (`spawn::<F, T>(`, `sleep (`), but not a longer name (`spawned(`).
fn names(line: &str, token: &str) -> bool {
    let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let word = token.bytes().next().is_some_and(ident);
    let (stem, call) = match token.strip_suffix('(') {
        Some(stem) => (stem, true),
        None => (token, false),
    };
    line.match_indices(stem).any(|(at, _)| {
        let starts = !word || at == 0 || !ident(line.as_bytes()[at - 1]);
        starts && (!call || opens_call(&line[at + stem.len()..]))
    })
}

/// Whether `rest` (what follows a name) opens its call: spaces, an optional
/// turbofish `::<…>` (nested angle brackets counted), spaces, then `(`.
fn opens_call(rest: &str) -> bool {
    let mut rest = rest.trim_start();
    if let Some(generics) = rest.strip_prefix("::") {
        let generics = generics.trim_start();
        if !generics.starts_with('<') {
            return false;
        }
        let mut depth = 0usize;
        let mut end = None;
        for (i, c) in generics.char_indices() {
            match c {
                '<' => depth += 1,
                '>' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(i + 1);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(end) = end else {
            // The turbofish runs past the line: a call split over lines.
            return true;
        };
        rest = generics[end..].trim_start();
    }
    rest.starts_with('(')
}

/// What a byte of the source is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    /// Code: counted toward depth, scanned.
    Code,
    /// Inside a string or character literal: scanned, not counted.
    Literal,
    /// A comment: neither.
    Comment,
}

/// The source's live code: every comment, every test item and every
/// string and character literal blanked to spaces (newlines kept, so line
/// numbers hold), for a scan that matches code forms and counts braces
/// (the core's CR8 refusal scan, `counters_scan_tests.rs`).
#[must_use]
pub fn live_code(text: &str) -> String {
    let live = live_text(text);
    let class = classify(live.as_bytes());
    let kept: Vec<u8> = live
        .bytes()
        .zip(class)
        .map(|(b, c)| {
            if c == Class::Literal && b != b'\n' {
                b' '
            } else {
                b
            }
        })
        .collect();
    String::from_utf8_lossy(&kept).into_owned()
}

/// The source with every comment and every test item blanked to spaces
/// (newlines kept, so line numbers hold).
fn live_text(text: &str) -> String {
    let bytes = text.as_bytes();
    let class = classify(bytes);
    let mut dead: Vec<bool> = class.iter().map(|c| *c == Class::Comment).collect();
    let attr = b"#[cfg(test)]";
    let mut i = 0;
    while i < bytes.len() {
        if class[i] == Class::Code && bytes[i..].starts_with(attr) {
            let end = test_item_end(bytes, &class, i + attr.len());
            for d in &mut dead[i..end] {
                *d = true;
            }
            i = end;
        } else {
            i += 1;
        }
    }
    let kept: Vec<u8> = bytes
        .iter()
        .zip(&dead)
        .map(|(b, d)| if *d && *b != b'\n' { b' ' } else { *b })
        .collect();
    String::from_utf8_lossy(&kept).into_owned()
}

/// Classifies every byte: code, literal or comment.
fn classify(b: &[u8]) -> Vec<Class> {
    let mut class = vec![Class::Code; b.len()];
    let mark = |class: &mut Vec<Class>, from: usize, to: usize, c: Class| {
        for x in &mut class[from..to.min(b.len())] {
            *x = c;
        }
    };
    let mut i = 0;
    while i < b.len() {
        let rest = &b[i..];
        let ident_before = i > 0 && (b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_');
        if rest.starts_with(b"//") {
            let end = rest
                .iter()
                .position(|c| *c == b'\n')
                .map_or(b.len(), |n| i + n);
            mark(&mut class, i, end, Class::Comment);
            i = end;
        } else if rest.starts_with(b"/*") {
            let end = block_comment_end(b, i);
            mark(&mut class, i, end, Class::Comment);
            i = end;
        } else if let Some((open, hashes)) = raw_string_start(rest).filter(|_| !ident_before) {
            let mut close = vec![b'"'];
            close.extend(std::iter::repeat_n(b'#', hashes));
            let body = i + open;
            let end = b[body..]
                .windows(close.len())
                .position(|w| w == close.as_slice())
                .map_or(b.len(), |n| body + n + close.len());
            mark(&mut class, i, end, Class::Literal);
            i = end;
        } else if b[i] == b'"' {
            let mut j = i + 1;
            while j < b.len() && b[j] != b'"' {
                j += if b[j] == b'\\' { 2 } else { 1 };
            }
            let end = (j + 1).min(b.len());
            mark(&mut class, i, end, Class::Literal);
            i = end;
        } else if b[i] == b'\'' {
            if let Some(n) = char_literal_len(rest) {
                mark(&mut class, i, i + n, Class::Literal);
                i += n;
            } else {
                i += 1; // a lifetime
            }
        } else {
            i += 1;
        }
    }
    class
}

/// Where the (nested) block comment that opens at `from` ends.
fn block_comment_end(b: &[u8], from: usize) -> usize {
    let mut depth = 0;
    let mut j = from;
    while j < b.len() {
        if b[j..].starts_with(b"/*") {
            depth += 1;
            j += 2;
        } else if b[j..].starts_with(b"*/") {
            depth -= 1;
            j += 2;
            if depth == 0 {
                return j;
            }
        } else {
            j += 1;
        }
    }
    b.len()
}

/// `r"`, `r#"`, `br##"`, `cr#"`…: the bytes up to the body, and the `#`
/// count (round 3 of CR8: a raw C string was read as two plain strings, so
/// the code after it was blanked as a literal).
fn raw_string_start(rest: &[u8]) -> Option<(usize, usize)> {
    let after_b = usize::from(matches!(rest.first(), Some(b'b' | b'c')));
    if rest.get(after_b) != Some(&b'r') {
        return None;
    }
    let hashes = rest[after_b + 1..]
        .iter()
        .take_while(|c| **c == b'#')
        .count();
    let quote = after_b + 1 + hashes;
    (rest.get(quote) == Some(&b'"')).then_some((quote + 1, hashes))
}

/// The length of a character literal starting at `rest[0] == '\''`, or
/// `None` for a lifetime.
fn char_literal_len(rest: &[u8]) -> Option<usize> {
    if rest.get(1) == Some(&b'\\') {
        // An escape: the closing quote comes after the escaped character
        // (which may itself be a quote, `'\''`), within a `\u{10FFFF}`.
        return rest
            .iter()
            .skip(3)
            .take(10)
            .position(|c| *c == b'\'')
            .map(|n| n + 4);
    }
    let lead = *rest.get(1)?;
    let width = match lead {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        _ => 4,
    };
    (rest.get(1 + width) == Some(&b'\'')).then_some(width + 2)
}

/// Words that start an item, which ends at its `;` or its first block's
/// closing `}`, never at a `,`.
const ITEM_WORDS: &[&str] = &[
    "fn",
    "mod",
    "impl",
    "use",
    "struct",
    "enum",
    "union",
    "trait",
    "type",
    "const",
    "static",
    "extern",
    "unsafe",
    "async",
    "macro_rules!",
    "thread_local!",
];

/// Where the test item after `#[cfg(test)]` (which ends at `from`) ends:
/// the index just past its last byte (module docs).
fn test_item_end(b: &[u8], class: &[Class], from: usize) -> usize {
    let item = starts_item(b, class, from);
    let mut depth: i32 = 0;
    for i in from..b.len() {
        if class[i] != Class::Code {
            continue;
        }
        match b[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth -= 1;
                if depth < 0 {
                    // It closed what encloses the item: the item ended before it.
                    return i;
                }
                if depth == 0 && b[i] == b'}' {
                    return i + 1;
                }
            }
            b';' if depth == 0 => return i + 1,
            b',' if depth == 0 && !item => return i + 1,
            _ => {}
        }
    }
    b.len()
}

/// Whether the code after `from` (past further attributes, comments and a
/// visibility) starts with an item word.
fn starts_item(b: &[u8], class: &[Class], from: usize) -> bool {
    let mut i = from;
    loop {
        while i < b.len() && (b[i].is_ascii_whitespace() || class[i] == Class::Comment) {
            i += 1;
        }
        if b[i..].starts_with(b"#[") {
            // Another attribute: skip to its closing bracket.
            let mut depth = 0;
            while i < b.len() {
                if class[i] == Class::Code {
                    if b[i] == b'[' {
                        depth += 1;
                    } else if b[i] == b']' {
                        depth -= 1;
                        if depth == 0 {
                            i += 1;
                            break;
                        }
                    }
                }
                i += 1;
            }
            continue;
        }
        if b[i..].starts_with(b"pub") {
            i += 3;
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            if b.get(i) == Some(&b'(') {
                i += b[i..].iter().position(|c| *c == b')').map_or(0, |n| n + 1);
            }
            continue;
        }
        break;
    }
    let end = b[i..]
        .iter()
        .position(|c| !(c.is_ascii_alphanumeric() || *c == b'_' || *c == b'!'))
        .map_or(b.len(), |n| i + n);
    let word = &b[i..end];
    ITEM_WORDS.iter().any(|w| w.as_bytes() == word)
}

#[cfg(test)]
mod tests {
    use super::{CELL_FILLS, FORBIDDEN, scan};

    fn run(text: &str) -> Vec<String> {
        let mut found = Vec::new();
        scan("f.rs", text, FORBIDDEN, &mut found);
        found
    }

    #[test]
    fn code_outside_tests_is_flagged() {
        assert_eq!(
            run("fn go() {\n    std::thread::spawn(|| {});\n    cv.wait(g);\n}\n"),
            ["f.rs:2: thread::spawn", "f.rs:3: .wait("]
        );
    }

    #[test]
    fn test_items_and_comments_are_not() {
        let text = "\
            // std::thread::sleep in a comment\n\
            #[cfg(test)]\n\
            thread_local! {\n    static X: u8 = 0;\n}\n\
            #[cfg(test)]\n\
            use std::sync::Barrier;\n\
            fn real() {\n    #[cfg(test)]\n    COUNT.with(|n| n.set(1));\n    let x = rx.recv();\n}\n\
            #[cfg(test)]\n\
            mod tests {\n    fn t() {\n        std::thread::spawn(|| {});\n    }\n}\n\
            fn after() { std::thread::sleep(d); }\n";
        assert_eq!(run(text), ["f.rs:11: .recv(", "f.rs:19: thread::sleep"]);
    }

    #[test]
    fn a_test_field_arm_or_literal_field_skips_only_itself() {
        // Round 4: a test item that ended with `,` made the scan skip the
        // next block as test code, so the code after a `#[cfg(test)]` match
        // arm or struct field went unseen.
        let arm = "fn go(k: Kind) -> u8 {\n    match k {\n        #[cfg(test)]\n        Kind::Probe => 0,\n        Kind::Real => {\n            std::thread::sleep(D);\n            1\n        }\n    }\n}\n";
        assert_eq!(run(arm), ["f.rs:6: thread::sleep"]);
        let literal = "fn make() -> Pool {\n    Pool {\n        #[cfg(test)]\n        probes: 0,\n        worker: std::thread::spawn(move || run()),\n    }\n}\n";
        assert_eq!(run(literal), ["f.rs:5: thread::spawn"]);
        let last_field =
            "struct S {\n    #[cfg(test)]\n    probe: u8\n}\nfn go() { std::thread::park(); }\n";
        assert_eq!(run(last_field), ["f.rs:5: thread::park"]);
        let block_arm = "fn go(k: Kind) {\n    match k {\n        #[cfg(test)]\n        Kind::Probe => { let s = \"}\"; },\n        Kind::Real => std::thread::yield_now(),\n    }\n}\n";
        assert_eq!(run(block_arm), ["f.rs:5: thread::yield_now"]);
        let where_fn = "#[cfg(test)]\nfn t<A, B>() where A: X, B: Y {\n    std::thread::spawn(|| {});\n}\nfn go() { rx.recv(); }\n";
        assert_eq!(run(where_fn), ["f.rs:5: .recv("]);
    }

    #[test]
    fn grouped_and_aliased_thread_imports_and_spins_are_flagged() {
        // Round 4: these named no listed token.
        assert_eq!(
            run(
                "use std::thread::{sleep, spawn};\nfn go() {\n    spawn(|| {});\n    sleep(D);\n}\n"
            ),
            ["f.rs:1: std::thread", "f.rs:3: spawn(", "f.rs:4: sleep("]
        );
        assert_eq!(
            run("use std::thread as t;\nfn go() {\n    t::spawn(|| {});\n}\n"),
            ["f.rs:1: std::thread", "f.rs:3: spawn("]
        );
        assert_eq!(
            run("use std::{sync::Arc, thread as t};\n"),
            ["f.rs:1: thread as"]
        );
        assert_eq!(
            run(
                "fn go(f: &AtomicBool) {\n    while !f.load(Acquire) {\n        std::thread::yield_now();\n    }\n}\n"
            ),
            ["f.rs:3: thread::yield_now"]
        );
        let none: [&str; 0] = [];
        assert_eq!(
            run(
                "fn n() -> usize { std::thread::available_parallelism().map_or(1, usize::from) }\n"
            ),
            none
        );
        // A name that only ends in one is not one.
        assert_eq!(
            run("fn go() { registry_scope(x); spark(y); my_spawn(z); }\n"),
            none
        );
    }

    #[test]
    fn calls_with_a_turbofish_or_spaces_and_spin_hints_are_flagged() {
        // Round 5: `spawn::<F, T>(` and `std::hint::spin_loop()` named no
        // listed token.
        assert_eq!(
            run(
                "use std::hint;\nfn go() {\n    spawn::<F, T>(f);\n    sleep (D);\n    hint::spin_loop();\n    rx.recv::<>();\n    x.wait :: < Vec<u8> > (g);\n}\n"
            ),
            [
                "f.rs:3: spawn(",
                "f.rs:4: sleep(",
                "f.rs:5: spin_loop",
                "f.rs:6: .recv(",
                "f.rs:7: .wait(",
            ]
        );
        assert_eq!(
            run("fn go() {\n    core::sync::atomic::spin_loop_hint();\n}\n"),
            ["f.rs:2: spin_loop"]
        );
        let none: [&str; 0] = [];
        // A longer name, or a path that is not a call, is not a call.
        assert_eq!(
            run("fn go() { spawned(x); let s = sleep_ms; my_sleep::<u8>(1); }\n"),
            none
        );
    }

    #[test]
    fn a_cell_filled_by_turbofish_path_or_force_is_flagged() {
        // Round 5: `get_or_init::<_>(` and `call_once_force` passed the scan.
        let mut found = Vec::new();
        let text = "fn a(&self) -> &u8 {\n    self.cell.get_or_init::<_>(|| 1)\n}\nfn b(&self) -> &u8 {\n    self.cell.get_or_try_init::<_, E>(|| Ok(1)).unwrap()\n}\nfn c(o: &Once) {\n    o.call_once_force(|_| ());\n}\nfn d(c: &OnceLock<u8>) -> &u8 {\n    let f = OnceLock::get_or_init;\n    f(c, || 1)\n}\n";
        scan("f.rs", text, CELL_FILLS, &mut found);
        assert_eq!(
            found,
            [
                "f.rs:2: get_or_init",
                "f.rs:5: get_or_try_init",
                "f.rs:8: call_once",
                "f.rs:11: get_or_init",
            ]
        );
    }

    #[test]
    fn only_the_crates_own_once_rs_is_exempt_from_the_cell_scan() {
        // Round 5: the scan exempted every file whose path ended in
        // `once.rs` (`src/files/bonce.rs`, `src/env/once.rs`).
        let scratch = crate::scratch::Scratch::new("waits");
        let dir = scratch.to_path_buf();
        let fill = "pub fn f(c: &std::sync::OnceLock<u8>) -> &u8 {\n    c.get_or_init(|| 1)\n}\n";
        std::fs::create_dir_all(dir.join("env")).unwrap();
        std::fs::create_dir_all(dir.join("files")).unwrap();
        for f in ["once.rs", "env/once.rs", "files/bonce.rs", "lib.rs"] {
            std::fs::write(dir.join(f), fill).unwrap();
        }
        let found = super::cell_fills(&dir);
        let rel: Vec<String> = found
            .iter()
            .map(|f| {
                let f = f.strip_prefix(&dir.display().to_string()).unwrap();
                f.trim_start_matches(std::path::MAIN_SEPARATOR).to_owned()
            })
            .collect();
        let sep = std::path::MAIN_SEPARATOR;
        assert_eq!(
            rel,
            [
                format!("env{sep}once.rs:2: get_or_init"),
                format!("files{sep}bonce.rs:2: get_or_init"),
                "lib.rs:2: get_or_init".to_owned(),
            ]
        );
    }

    #[test]
    fn a_cell_filled_with_its_value_inside_is_flagged_and_its_helper_is_not() {
        // Plan 4.1 (round 4): a lazy value computed inside its cell runs
        // caller code with the cell locked; only `once.rs` may call
        // `get_or_init`, and only to move a computed value in.
        let mut found = Vec::new();
        let text = "fn entry(&self) -> &File {\n    self.cell.get_or_init(|| parse(self))\n}\nstatic T: LazyLock<u8> = LazyLock::new(|| 1);\n#[cfg(test)]\nmod tests { fn t() { c.get_or_init(|| 1); } }\n";
        scan("f.rs", text, CELL_FILLS, &mut found);
        assert_eq!(found, ["f.rs:2: get_or_init", "f.rs:4: LazyLock"]);
    }

    /// `live_code` blanks every kind of literal, its braces and quotes
    /// with it, and every comment, nested ones too, and keeps the code
    /// around them (round 3 of CR8: a raw C string and `'\''` were
    /// misread, so the code after them was blanked).
    #[test]
    fn live_code_blanks_each_literal_and_keeps_the_code_after_it() {
        let text = concat!(
            "let a = \"{\\\"}\";\n",
            "let b = r#\"{\"}\"#;\n",
            "let c = cr#\"a\"b{\"#;\n",
            "let d = c\"{\\\"}\";\n",
            "let e = br\"}\";\n",
            "let f = '\\'';\n",
            "let g = b'{';\n",
            "let h = b'\\'';\n",
            "let i = '\\u{7B}';\n",
            "/* /* nested } */ { */\n",
            "fn live<'a>(m: &'a Meter) { m.work_used_up(); }\n",
        );
        let code = super::live_code(text);
        let lines: Vec<&str> = code.lines().collect();
        assert_eq!(lines.len(), 11, "{code}");
        for (i, l) in lines.iter().take(9).enumerate() {
            assert!(!l.contains(['{', '}', '"', '\'', '\\']), "line {i}: {l:?}");
            assert!(l.trim_end().ends_with(';'), "line {i}: {l:?}");
        }
        assert_eq!(lines[9].trim(), "");
        assert_eq!(lines[10], "fn live<'a>(m: &'a Meter) { m.work_used_up(); }");
    }

    /// A test item mid-file is blanked, an attribute on its item's line
    /// included, and the live code after it is kept; a raw identifier is
    /// code.
    #[test]
    fn live_code_blanks_test_items_and_keeps_what_follows() {
        let text = "fn a() { x(); }\n#[cfg(test)]\nfn t() { y(); }\nfn b() { z(); }\n\
            #[cfg(test)] impl S { fn u() { w(); } }\n#[allow(dead_code)] impl T { fn v() {} }\n\
            let r#fn = r#work_used_up();\n";
        let code = super::live_code(text);
        let lines: Vec<&str> = code.lines().map(str::trim).collect();
        assert_eq!(
            lines,
            [
                "fn a() { x(); }",
                "",
                "",
                "fn b() { z(); }",
                "",
                "#[allow(dead_code)] impl T { fn v() {} }",
                "let r#fn = r#work_used_up();",
            ]
        );
    }

    #[test]
    fn literals_and_lifetimes_do_not_move_the_depth() {
        let text = "#[cfg(test)]\nfn t<'a>(x: &'a str) {\n    let _ = ('{', \"{{\", r#\"}\"#, b'(');\n}\nfn go() { rx.recv(); }\n";
        assert_eq!(run(text), ["f.rs:5: .recv("]);
    }
}
