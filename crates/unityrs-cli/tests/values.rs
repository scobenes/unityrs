//! `unityrs values` answers every case, a case too big for the memory the
//! process has included (review round 15: one 16-byte case aborted the
//! binary, and with it every other case's answer).
//!
//! The binary runs with its address space capped (`RLIMIT_AS`, set by the
//! shell's `ulimit -v` before it starts, on one malloc arena, as
//! `unityrs_testkit::address_space` explains), so the allocator refuses at
//! a size that does not depend on the machine. Two cases:
//! - the review's own, 16 bytes that read as 50,331,648 empty structs, under
//!   a cap that holds the read and not the text;
//! - one that reads in a few hundred KiB as 16,384 structs, each an empty
//!   struct named with 64 KiB, so its value's text is 1 GiB.
//!
//! The binary built before the fix aborted on each (status 134, `memory
//! allocation of ... bytes failed`), growing that text in a plain `String`;
//! now the text is a charged [`Text`] grown fallibly, and its refusal is the
//! case's `crash` line.
//!
//! [`Text`]: unityrs_dump::text::Text

use std::path::Path;
use std::process::{Command, Output};

use serde_json::{Value, json};
use unityrs_testkit::child;
use unityrs_testkit::scratch::Scratch;

/// The binary run on `cases` with its address space capped at `cap_kib`
/// KiB (`ulimit -v`); one the system kills from outside is run again
/// (`unityrs_testkit::child`).
fn capped(cases: &Path, cap_kib: u64) -> Output {
    child::output(
        Command::new("sh")
            .args([
                "-c",
                r#"ulimit -v "$1" && exec "$0" values "$2""#,
                env!("CARGO_BIN_EXE_unityrs"),
                &cap_kib.to_string(),
                cases.to_str().unwrap(),
            ])
            .env("MALLOC_ARENA_MAX", "1"),
    )
    .expect("sh runs the unityrs binary")
}

/// 16,384 structs of one field, an empty struct whose name is 64 KiB long:
/// 4 bytes, about 1 GiB of text.
fn too_big() -> Value {
    json!({"name": "too_big", "nodes": [
        [0, "Base", "Base", 0],
        [1, "vector", "f", 0],
        [2, "Array", "Array", 0],
        [3, "int", "size", 0],
        [3, "S", "data", 0],
        [4, "E", "n".repeat(64 << 10), 0]
    ], "hex": "00400000"})
}

/// Review round 15's case: an unaligned `TypelessData` puts the vector's
/// count at byte 5, where it reads `0x03000000`.
fn the_reviews() -> Value {
    json!({"name": "the_reviews", "nodes": [
        [0, "Base", "Base", 0],
        [1, "TypelessData", "f0", 0],
        [2, "int", "size", 0],
        [2, "UInt8", "data", 0],
        [1, "vector", "f1", 0],
        [2, "Array", "Array", 0],
        [3, "int", "size", 0],
        [3, "PPtr<Object>", "data", 0],
        [1, "unsigned short", "f2", 16384]
    ], "hex": "01000000e500000003000000ff7f0000"})
}

fn small() -> Value {
    json!({"name": "small", "nodes": [[0, "Base", "Base", 0], [1, "int", "x", 0]], "hex": "07000000"})
}

/// `case`, then [`small`], with the address space capped at `cap_kib` KiB:
/// the first is its `crash` line, and the second is answered.
fn crashes_and_the_next_case_runs(case: &Value, cap_kib: u64) {
    if !unityrs_testkit::address_space::SUPPORTED {
        eprintln!("skipped: the address space is capped only on Linux x86-64 and 64-bit Arm");
        return;
    }
    let name = case["name"].clone();
    let dir = Scratch::new("cli-values-capped");
    let cases = dir.join("cases.json");
    std::fs::write(&cases, json!([case, small()]).to_string()).unwrap();
    let o = capped(&cases, cap_kib);
    let stderr = String::from_utf8_lossy(&o.stderr);
    assert_eq!(o.status.code(), Some(0), "{:?}: {stderr}", o.status);
    assert_eq!(stderr, "");
    let lines: Vec<Value> = o
        .stdout
        .split(|b| *b == b'\n')
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_slice(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 2, "{lines:?}");
    let crash = &lines[0];
    assert_eq!(
        (&crash["name"], &crash["kind"]),
        (&name, &json!("crash")),
        "{crash}"
    );
    assert_eq!(
        (&crash["value"], &crash["resave"]),
        (&Value::Null, &json!("skipped")),
        "{crash}"
    );
    let note = crash["note"].as_str().unwrap();
    assert!(
        note.starts_with("unityrs ran out of memory on this case ("),
        "{note}"
    );
    assert_eq!(
        lines[1],
        json!({"name": "small", "kind": null, "bytes_read": null,
               "value": r#"{"t":"dict","v":[["x",7]]}"#, "resave": "equal",
               "resave_hex": "07000000", "note": null})
    );
}

/// 192 MiB: room for the binary and the read, not for the text.
#[test]
fn a_case_too_big_for_memory_is_a_crash_and_the_next_case_runs() {
    crashes_and_the_next_case_runs(&too_big(), 192 << 10);
}

/// 2,000,000 KiB: the read fits, and the text, 1.3 GB, does not. (The binary
/// before the fix, on this case: `memory allocation of 35651584 bytes failed`,
/// status 134, at this cap and at 2,500,000 and 3,000,000 KiB.)
#[test]
fn the_reviews_16_byte_case_is_a_crash_and_the_next_case_runs() {
    crashes_and_the_next_case_runs(&the_reviews(), 2_000_000);
}
