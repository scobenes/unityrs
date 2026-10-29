//! The source scan behind plan section 4.1's rule that the core never waits
//! and never starts a thread: a batch engine runs core calls on its own pool
//! threads, so a core call that parks, sleeps, waits on a condition or a
//! channel, or hands work to a thread of its own would let the pool starve
//! itself, or escape the engine's bound on threads.
//!
//! [`waits`] lists every line outside test code that names one of
//! [`FORBIDDEN`]. Test code is an item under `#[cfg(test)]`: the block that
//! follows the attribute (a `mod tests { … }`, a `fn`, a `thread_local!`),
//! up to the `}` at the attribute's indent, or the one line when it ends
//! with `;`. Comments are skipped. Locks are not on the list: a core lock is
//! held only for a lookup or a publish (plan 3.16), which the `once` tests
//! check.
//!
//! Owner: the scaffold (implemented).

use std::path::Path;

/// What the core never calls outside tests: thread starts, sleeps, parks,
/// condition variables, barriers, channels, a thread pool.
pub const FORBIDDEN: &[&str] = &[
    "thread::spawn",
    "thread::scope",
    "thread::Builder",
    "thread::sleep",
    "thread::park",
    "Condvar",
    "Barrier",
    "mpsc",
    "rayon",
    ".wait(",
    ".wait_timeout",
    ".recv(",
];

/// Every `file:line: token` under `src` where non-test code names a
/// [`FORBIDDEN`] token.
///
/// # Panics
/// When `src` cannot be read (a test helper).
#[must_use]
pub fn waits(src: &Path) -> Vec<String> {
    let mut files = Vec::new();
    collect(src, &mut files);
    files.sort();
    let mut found = Vec::new();
    for (name, text) in &files {
        scan(name, text, &mut found);
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

/// Where a test item that started at `indent` ends.
enum Skip {
    None,
    /// The next code line is the item; it may open a block.
    Item(usize),
    /// Inside a block that closes with a `}` at this indent.
    Block(usize),
}

fn scan(name: &str, text: &str, found: &mut Vec<String>) {
    let mut skip = Skip::None;
    for (i, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        if trimmed.starts_with("//") || trimmed.is_empty() {
            continue;
        }
        match skip {
            Skip::Block(at) => {
                if indent == at && trimmed.starts_with('}') {
                    skip = Skip::None;
                }
                continue;
            }
            Skip::Item(at) => {
                if trimmed.starts_with("#[") {
                    continue;
                }
                skip = if trimmed.ends_with('{') {
                    Skip::Block(at)
                } else if trimmed.ends_with(';') || trimmed.ends_with('}') {
                    Skip::None
                } else {
                    // A signature split over lines: wait for its `{`.
                    Skip::Item(at)
                };
                continue;
            }
            Skip::None => {}
        }
        if trimmed.starts_with("#[cfg(test)]") {
            skip = Skip::Item(indent);
            continue;
        }
        for token in FORBIDDEN {
            if line.contains(token) {
                found.push(format!("{name}:{}: {token}", i + 1));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::scan;

    fn run(text: &str) -> Vec<String> {
        let mut found = Vec::new();
        scan("f.rs", text, &mut found);
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
}
