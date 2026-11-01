//! The `unityrs` binary's exit statuses and streams, for what runs before
//! any input is read (the dump itself is `unityrs-dump`'s tests): status 2
//! with argparse's usage and error lines for a bad argument or path, 0 for
//! `-h`, 1 for a cases file that cannot be read.

use std::path::PathBuf;
use std::process::{Command, Output};

use unityrs_testkit::child;

/// A run of the binary; one the system kills from outside is run again
/// (`unityrs_testkit::child`).
fn unityrs(args: &[&str]) -> Output {
    child::output(Command::new(env!("CARGO_BIN_EXE_unityrs")).args(args))
        .expect("the unityrs binary runs")
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// A folder outside the repository, removed when dropped.
struct Tmp(PathBuf);

impl Tmp {
    fn new(what: &str) -> Self {
        let base = std::env::temp_dir().join(format!("unityrs-cli-{what}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        Self(std::fs::canonicalize(&base).unwrap())
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_bad_argument_exits_2_with_the_usage_line() {
    let o = unityrs(&["dump", "x"]);
    assert_eq!(o.status.code(), Some(2));
    let e = stderr(&o);
    assert!(e.starts_with("usage: unityrs dump "), "{e}");
    assert!(
        e.ends_with("unityrs dump: error: the following arguments are required: -o/--output\n"),
        "{e}"
    );
    for args in [
        &["dump", "x", "-o", "y", "--sample", "x"][..],
        &["dump", "x", "-o", "y", "--levels", "L1"],
        &["dump", "x", "-o", "y", "--tpk", "t.tpk"],
        &["what"],
        &[],
        &["values"],
    ] {
        assert_eq!(unityrs(args).status.code(), Some(2), "{args:?}");
    }
}

#[test]
fn help_exits_0() {
    for args in [&["dump", "-h"][..], &["-h"], &["values", "--help"]] {
        let o = unityrs(args);
        assert_eq!(o.status.code(), Some(0), "{args:?}");
        assert!(String::from_utf8_lossy(&o.stdout).starts_with("usage: unityrs"));
    }
}

#[test]
fn paths_the_oracle_refuses_exit_2_and_write_nothing() {
    let tmp = Tmp::new("paths");
    let input = tmp.0.join("in");
    std::fs::create_dir_all(&input).unwrap();
    std::fs::write(input.join("a.bin"), b"a").unwrap();
    let input = input.to_str().unwrap();
    // Inside a repository: one the test makes (a folder holding `.git`, as
    // a clone is), so this holds wherever the tree is, a checkout or not.
    let repo = tmp.0.join("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    let inside = repo.join("never-written.jsonl");
    let o = unityrs(&["dump", input, "-o", inside.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(2), "{}", stderr(&o));
    assert!(stderr(&o).contains("repository"), "{}", stderr(&o));
    assert!(!inside.exists());
    // No input matched.
    let out = tmp.0.join("out.jsonl");
    let o = unityrs(&[
        "dump",
        input,
        "-o",
        out.to_str().unwrap(),
        "--name",
        "nothing*",
    ]);
    assert_eq!(o.status.code(), Some(2));
    assert!(
        stderr(&o).ends_with("unityrs dump: error: no inputs matched\n"),
        "{}",
        stderr(&o)
    );
    assert!(!out.exists());
}

#[test]
fn values_reports_an_unreadable_cases_file_with_status_1() {
    let tmp = Tmp::new("values");
    let o = unityrs(&["values", tmp.0.join("missing.json").to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(1));
    let bad = tmp.0.join("bad.json");
    std::fs::write(&bad, b"{").unwrap();
    let o = unityrs(&["values", bad.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).starts_with("unityrs values: "), "{}", stderr(&o));
    // An empty case list is a run with no output.
    let empty = tmp.0.join("empty.json");
    std::fs::write(&empty, b"[]").unwrap();
    let o = unityrs(&["values", empty.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert_eq!(String::from_utf8_lossy(&o.stdout), "");
}
