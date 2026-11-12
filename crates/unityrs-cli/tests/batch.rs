//! `unityrs batch` and `unityrs --version` through the binary
//! (`docs/BATCH_SPEC.md` sections 9 and 10): `--check`'s exact `exit` line
//! for a valid spec and a refused one, `--schema` byte for byte,
//! `--version`'s line, both 74 to a full stdout, 78 `usage` for a bad
//! command line, a run refused, and `extract.md` X8's rows a check
//! reaches: a `JOB.json` missing, unreadable, a FIFO, over 64 MiB, a
//! `deps` repeat or one past `max_view_bundles`, a view too large, and
//! E42's secrets kept out of a refusal. How the lines are written to a
//! held stdout is `batch_write.rs`'s.

mod common;

use std::fs::File;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};

use common::{
    INPUTS, assert_refused, assert_refused_by, check, enterable, job, line_of, u, unityrs,
    unprivileged, write_job,
};
use unityrs_batch::alloc::Measured;
use unityrs_batch::bound::{CHECK_ENV_BLOCK, PROVISIONAL, memory_bound};
use unityrs_batch::progress::{Exit, Snapshot};
use unityrs_batch::reason::Code;
use unityrs_batch::spec::{self, SCHEMA, SPEC_VERSION};
use unityrs_core::limits::heap_cost;
use unityrs_testkit::child;
use unityrs_testkit::scratch::Scratch;

/// `--check`'s bound for `text`, from its `exit` line, checked whole.
fn bound_of(dir: &Scratch, text: &str, inputs: u64) -> u64 {
    let o = check(&write_job(dir, text));
    assert_eq!(o.status.code(), Some(0), "{o:?}");
    assert!(
        o.stderr.is_empty(),
        "{:?}",
        String::from_utf8_lossy(&o.stderr)
    );
    let (line, v) = line_of(&o);
    let detail = v["detail"].as_str().unwrap();
    let bound: u64 = detail
        .strip_prefix("memory_bound ")
        .unwrap()
        .parse()
        .unwrap();
    let mut s = Snapshot::validate(u(&v["ms"]));
    s.inputs.total = inputs;
    s.inputs.queued = inputs;
    s.memory.inflight_limit = 268_435_456;
    s.memory.rss = u(&v["memory"]["rss"]);
    s.spool.size = 1 << 30;
    let want = s.line(Some(&Exit {
        status: None,
        code: Code::Complete,
        reason: None,
        detail: Some(format!("memory_bound {bound}")),
    }));
    assert_eq!(line, want);
    bound
}

/// The bound with nothing measured.
fn fixed(text: &str) -> u64 {
    let parsed = spec::parse(text.as_bytes().to_vec()).unwrap().spec;
    memory_bound(&parsed, Measured::default(), CHECK_ENV_BLOCK, &PROVISIONAL).total()
}

#[test]
fn check_a_valid_spec_exits_0_with_its_memory_bound() {
    let dir = Scratch::new("cli-batch-valid");
    enterable(&dir);
    let text = job(INPUTS);
    let bound = bound_of(&dir, &text, 2);
    // The measured part, `C_spec`'s and `p`'s, is more than nothing: at
    // least the strings the spec keeps (`batch.md` 2.11 step 2), here
    // `r`, `a`, `b`, `/a`, `/b`, the template, `/spool` and `/out`, which
    // a copy holds whole, and less than the document's few kilobytes'
    // worth.
    let kept = [
        "r",
        "a",
        "b",
        "/a",
        "/b",
        "{input}/{name}.png",
        "/spool",
        "/out",
    ];
    let floor = heap_cost(kept.iter().map(|s| s.len() as u64).sum());
    let base = fixed(&text);
    assert!(bound > base, "{bound} {base}");
    assert!(bound - base >= floor, "{bound} {base} {floor}");
    assert!(bound < base + (1 << 20), "{bound} {base}");
    // A 4 KiB path the spec keeps: its measured part holds it whole.
    let long = format!("/{}", "p".repeat(4095));
    let longer = job(&INPUTS.replace(r#""path":"/a""#, &format!(r#""path":"{long}""#)));
    let grown = bound_of(&dir, &longer, 2);
    assert_eq!(fixed(&longer), base, "no fixed term reads a path");
    assert!(grown - base >= heap_cost(4096), "{grown} {base}");
    assert!(grown > bound, "{grown} {bound}");
}

#[test]
fn check_a_refused_spec_exits_78_with_its_rule_and_pointer() {
    let dir = Scratch::new("cli-batch-refused");
    enterable(&dir);
    let p = write_job(
        &dir,
        &job(INPUTS).replace(r#""threads":2"#, r#""threads":0"#),
    );
    assert_refused(&check(&p), "range /threads");
    let p = write_job(&dir, &job(INPUTS).replace(r#""run_id":"r","#, ""));
    assert_refused(&check(&p), "missing /run_id");
}

#[test]
fn schema_prints_the_schema_byte_for_byte() {
    let o = unityrs(&["batch", "--schema"]);
    assert_eq!(o.status.code(), Some(0));
    let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/batch/spec-v1.schema.json");
    assert_eq!(o.stdout, std::fs::read(file).unwrap());
    assert_eq!(o.stdout, SCHEMA);
    assert_eq!(o.stderr, b"");
}

#[test]
fn version_prints_one_line() {
    let o = unityrs(&["--version"]);
    assert_eq!(o.status.code(), Some(0));
    let want = format!(
        "unityrs {} spec {SPEC_VERSION}\n",
        env!("CARGO_PKG_VERSION")
    );
    assert_eq!(String::from_utf8(o.stdout).unwrap(), want);
    assert_eq!(o.stderr, b"");
}

/// `--version` and `--schema` to a stdout whose write fails (`/dev/full`,
/// `ENOSPC`): 74, and nothing on stderr either (9.1).
#[cfg(target_os = "linux")]
#[test]
fn version_and_schema_exit_74_when_stdout_fails() {
    for args in [&["--version"][..], &["batch", "--schema"]] {
        let full = File::options().write(true).open("/dev/full").unwrap();
        let out = child::output(unprivileged(
            Command::new(common::BIN)
                .args(args)
                .stdout(Stdio::from(full)),
        ))
        .unwrap();
        assert_eq!(out.status.code(), Some(74), "{args:?} {out:?}");
        assert!(out.stdout.is_empty(), "{args:?}");
        assert_eq!(out.stderr, b"", "{args:?}");
    }
}

#[test]
fn a_bad_command_line_exits_78_usage() {
    for args in [
        &["batch"][..],
        &["batch", "--check"],
        &["batch", "--check", "a", "b"],
        &["batch", "--schema", "x"],
        &["batch", "--frobnicate"],
        &["batch", "a", "b"],
    ] {
        let o = unityrs(args);
        assert_refused(&o, "usage");
        let err = String::from_utf8_lossy(&o.stderr);
        assert!(
            err.contains("usage: unityrs batch --check JOB.json"),
            "{err}"
        );
    }
    // `--version`'s own usage, under its own name.
    let o = unityrs(&["--version", "x"]);
    assert_refused_by(&o, "unityrs", "usage");
    assert_eq!(
        String::from_utf8_lossy(&o.stderr),
        "unityrs: bad_spec usage\nusage: unityrs --version\n"
    );
}

/// A run is not built yet (B7): a valid spec's run is 78 `usage`, its
/// usage lines saying so; its spec is checked first.
#[test]
fn a_run_is_refused_78_and_its_spec_checked_first() {
    let dir = Scratch::new("cli-batch-run");
    enterable(&dir);
    let p = write_job(&dir, &job(INPUTS));
    let o = unityrs(&["batch", p.to_str().unwrap()]);
    assert_refused(&o, "usage");
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(
        err.contains("unityrs batch JOB.json   (a run: not built yet, refused)"),
        "{err}"
    );
    let p = write_job(
        &dir,
        &job(INPUTS).replace(r#""threads":2"#, r#""threads":0"#),
    );
    assert_refused(&unityrs(&["batch", p.to_str().unwrap()]), "range /threads");
}

/// X8: the document's own rules, through the binary. A test run as root
/// runs the binary as `nobody`, so the mode-0000 file binds it.
#[test]
fn x8_a_job_file_missing_unreadable_a_fifo_or_too_large() {
    let dir = Scratch::new("cli-batch-file");
    enterable(&dir);
    assert_refused(&check(&dir.join("none.json")), "unreadable");
    assert_refused(&check(dir.path()), "not_regular");
    let fifo = dir.join("fifo");
    let status = child::status(Command::new("mkfifo").arg(&fifo)).unwrap();
    assert!(status.success());
    assert_refused(&check(&fifo), "not_regular");
    let big = dir.join("big.json");
    File::create(&big)
        .unwrap()
        .set_len(spec::MAX_SPEC_BYTES + 1)
        .unwrap();
    std::fs::set_permissions(&big, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_refused(&check(&big), "too_large");
    let locked = write_job(&dir, &job(INPUTS));
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    assert_refused(&check(&locked), "unreadable");
}

/// X8: a `deps` repeat, a `deps` list longer than `max_view_bundles`, and
/// a view over it.
#[test]
fn x8_a_deps_repeat_and_a_view_too_large() {
    let dir = Scratch::new("cli-batch-deps");
    enterable(&dir);
    let deps = r#","inputs":[{"id":"a","path":"/a","deps":["d","d"]}],
        "dependencies":[{"id":"d","path":"/d"}]"#;
    assert_refused(&check(&write_job(&dir, &job(deps))), "deps /inputs/0/deps");
    let five = r#","inputs":[{"id":"a","path":"/a","deps":["d1","d2","d3","d4","d5"]}],
        "dependencies":[{"id":"d1","path":"/1"},{"id":"d2","path":"/2"},
        {"id":"d3","path":"/3"},{"id":"d4","path":"/4"},{"id":"d5","path":"/5"}]"#;
    assert_refused(&check(&write_job(&dir, &job(five))), "deps /inputs/0/deps");
    let view = r#","inputs":[{"id":"a","path":"/a","deps":["d1","d2","d3","d4"]}],
        "dependencies":[{"id":"d1","path":"/1"},{"id":"d2","path":"/2"},
        {"id":"d3","path":"/3"},{"id":"d4","path":"/4"}]"#;
    assert_refused(&check(&write_job(&dir, &job(view))), "view_size /inputs/0");
}

/// X8, E42: a spec refused at validation that holds a header's value, a
/// signed URL's query and userinfo, and a secret-like string where a rule
/// refuses it: none of their bytes reach stdout or stderr, whichever rule
/// refuses the spec, only the rule and pointer do.
#[test]
fn x8_e42_a_refused_spec_s_secrets_reach_no_output() {
    let dir = Scratch::new("cli-batch-e42");
    enterable(&dir);
    let input = |url: &str, headers: &str| {
        format!(
            r#","inputs":[{{"id":"a","url":"{url}","headers":{{{headers}}}}},{{"id":"b","path":"/b"}}]"#
        )
    };
    let signed = "https://h.example/b?X-Amz-Signature=SECRETSIG0&X-Amz-Credential=SECRETCRED0";
    let header = r#""Authorization":"Bearer SECRETHDR0""#;
    let cases = [
        // Refused elsewhere, the secrets valid.
        (
            job(&input(signed, header)).replace(r#""threads":2"#, r#""threads":0"#),
            "range /threads",
        ),
        // The URL itself refused: userinfo.
        (
            job(&input(
                "https://u:SECRETPW1@h.example/b?sig=SECRETSIG1",
                header,
            )),
            "format /inputs/0/url",
        ),
        // A header's value refused: a control character in it.
        (
            job(&input(
                signed,
                r#""Authorization":"Bearer SECRETHDR2\u0001""#,
            )),
            "format /inputs/0/headers/Authorization",
        ),
        // A refused header's name, its value a secret.
        (
            job(&input(signed, r#""Host":"SECRETHOST3""#)),
            "refused_header /inputs/0/headers/Host",
        ),
    ];
    for (text, detail) in cases {
        let o = check(&write_job(&dir, &text));
        assert_refused(&o, detail);
        for (name, bytes) in [("stdout", &o.stdout), ("stderr", &o.stderr)] {
            let s = String::from_utf8_lossy(bytes);
            assert!(!s.contains("SECRET"), "{detail}: {name} {s}");
            assert!(!s.contains("h.example"), "{detail}: {name} {s}");
        }
    }
}
