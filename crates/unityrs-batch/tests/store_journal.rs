//! Journal commits survive rewrites and bound recovery of damaged input.
use std::fs;
use unityrs_batch::spool::journal::{Entry, Journal};
use unityrs_testkit::scratch::Scratch;

fn entry(key: &str) -> Entry<'_> {
    Entry {
        endpoint: None,
        bucket: "bucket",
        key,
        upload_id: "upload",
    }
}

#[test]
fn debug_never_renders_reflected_credentials() {
    let scratch = Scratch::new("journal-debug");
    let journal = Journal::new(&scratch).unwrap();
    let secret = "credential-sentinel";
    let entry = Entry {
        endpoint: Some(secret),
        bucket: secret,
        key: secret,
        upload_id: secret,
    };
    journal.append(entry).unwrap();
    let record = journal.reader().unwrap().next().unwrap().unwrap();
    assert!(!format!("{entry:?} {record:?}").contains(secret));
}

#[test]
fn round_trip_escapes_and_exact_endpoint_identity() {
    let scratch = Scratch::new("journal-round-trip");
    let journal = Journal::new(&scratch).unwrap();
    let first = Entry {
        endpoint: Some("HTTPS://Host:443/"),
        bucket: "bucket",
        key: "a\"\\\n\u{0}雪",
        upload_id: "id\t😀",
    };
    journal.append(first).unwrap();
    journal.append(entry("second")).unwrap();
    let stored = fs::read_to_string(scratch.join("journal")).unwrap();
    let first_json: serde_json::Value =
        serde_json::from_str(stored.lines().next().unwrap()).unwrap();
    assert_eq!(
        first_json,
        serde_json::json!(["HTTPS://Host:443/", "bucket", "a\"\\\n\u{0}雪", "id\t😀"])
    );
    let mut reader = journal.reader().unwrap();
    assert_eq!(reader.next().unwrap().unwrap().entry(), first);
    assert_eq!(reader.next().unwrap().unwrap().entry(), entry("second"));
    assert!(reader.next().unwrap().is_none());
    assert!(fs::read(scratch.join("journal")).unwrap().ends_with(b"\n"));
}

#[test]
fn line_ceiling_counts_bytes_before_the_newline() {
    let scratch = Scratch::new("journal-line-ceiling");
    let valid = b"[null,\"bucket\",\"at-limit\",\"upload\"]";
    let mut bytes = valid.to_vec();
    bytes.resize(16 * 1024, b' ');
    bytes.push(b'\n');
    bytes.extend_from_slice(valid);
    bytes.resize(16 * 1024 + 1 + 16 * 1024 + 1, b' ');
    bytes.extend_from_slice(b"\n[null,\"bucket\",\"after\",\"upload\"]");
    fs::write(scratch.join("journal"), bytes).unwrap();
    let mut reader = Journal::new(&scratch).unwrap().reader().unwrap();
    assert_eq!(reader.next().unwrap().unwrap().entry(), entry("at-limit"));
    assert_eq!(reader.next().unwrap().unwrap().entry(), entry("after"));
    assert!(reader.next().unwrap().is_none());
}

#[test]
fn concurrent_mutations_do_not_lose_unrelated_uploads() {
    let scratch = Scratch::new("journal-concurrent");
    let journal = Journal::new(&scratch).unwrap();
    std::thread::scope(|scope| {
        for (key, remove) in [
            ("first", true),
            ("second", false),
            ("third", true),
            ("fourth", false),
        ] {
            let journal = &journal;
            scope.spawn(move || {
                journal.append(entry(key)).unwrap();
                if remove {
                    journal.remove(entry(key)).unwrap();
                }
            });
        }
    });
    let mut reader = journal.reader().unwrap();
    let mut keys = Vec::new();
    while let Some(record) = reader.next().unwrap() {
        keys.push(record.entry().key.to_owned());
    }
    keys.sort();
    assert_eq!(keys, ["fourth", "second"]);
}

#[test]
fn journal_paths_do_not_follow_symlinks() {
    let scratch = Scratch::new("journal-symlink");
    fs::write(scratch.join("outside"), b"untouched").unwrap();
    std::os::unix::fs::symlink(scratch.join("outside"), scratch.join("journal")).unwrap();
    let journal = Journal::new(&scratch).unwrap();
    assert!(journal.append(entry("key")).is_err());
    assert!(journal.remove(entry("key")).is_err());
    assert!(journal.reader().is_err());
    assert_eq!(fs::read(scratch.join("outside")).unwrap(), b"untouched");
}

#[test]
fn removal_and_later_append_use_the_replaced_path() {
    let scratch = Scratch::new("journal-rewrite");
    let journal = Journal::new(&scratch).unwrap();
    for key in ["first", "second", "first"] {
        journal.append(entry(key)).unwrap();
    }
    journal.remove(entry("first")).unwrap();
    journal.append(entry("third")).unwrap();
    let mut reader = journal.reader().unwrap();
    assert_eq!(reader.next().unwrap().unwrap().entry(), entry("second"));
    assert_eq!(reader.next().unwrap().unwrap().entry(), entry("third"));
    assert!(reader.next().unwrap().is_none());
    assert!(!scratch.join("journal.tmp").exists());
}

#[test]
fn recovery_skips_empty_malformed_overlong_and_torn_lines() {
    let scratch = Scratch::new("journal-recovery");
    let mut bytes = b"\n[null,\"b\",\"k\"]\n[42,\"b\",\"k\",\"id\"]\n".to_vec();
    bytes.resize(bytes.len() + 16 * 1024 + 1, b'x');
    bytes.extend_from_slice(b"\n[null,\"bucket\",\"valid\",\"upload\"]\n[null,\"torn");
    fs::write(scratch.join("journal"), bytes).unwrap();
    let journal = Journal::new(&scratch).unwrap();
    let mut reader = journal.reader().unwrap();
    assert_eq!(reader.next().unwrap().unwrap().entry(), entry("valid"));
    assert!(reader.next().unwrap().is_none());
    journal.append(entry("after")).unwrap();
    let mut reader = journal.reader().unwrap();
    assert_eq!(reader.next().unwrap().unwrap().entry(), entry("valid"));
    assert_eq!(reader.next().unwrap().unwrap().entry(), entry("after"));
    assert!(reader.next().unwrap().is_none());
}

#[test]
fn removal_preserves_unmatched_raw_lines_and_endpoint_distinctions() {
    let scratch = Scratch::new("journal-exact");
    let mut bytes = b"broken\n".to_vec();
    bytes.resize(bytes.len() + 32 * 1024, b'x');
    bytes.extend_from_slice(
        b"\n[null,\"bucket\",\"key\",\"upload\"]\n[\"\",\"bucket\",\"key\",\"upload\"]\ntorn",
    );
    fs::write(scratch.join("journal"), &bytes).unwrap();
    let journal = Journal::new(&scratch).unwrap();
    journal.remove(entry("key")).unwrap();
    let mut expected = b"broken\n".to_vec();
    expected.resize(expected.len() + 32 * 1024, b'x');
    expected.extend_from_slice(b"\n[\"\",\"bucket\",\"key\",\"upload\"]\ntorn");
    assert_eq!(fs::read(scratch.join("journal")).unwrap(), expected);
}

#[test]
fn bounded_fields_unicode_escapes_and_absent_file() {
    let scratch = Scratch::new("journal-bounds");
    let journal = Journal::new(&scratch).unwrap();
    assert!(journal.reader().unwrap().next().unwrap().is_none());
    journal.remove(entry("absent")).unwrap();
    let key = "k".repeat(1024);
    let upload = "i".repeat(1024);
    let bucket = "b".repeat(255);
    let endpoint = "e".repeat(1024);
    let maximal = Entry {
        endpoint: Some(&endpoint),
        bucket: &bucket,
        key: &key,
        upload_id: &upload,
    };
    journal.append(maximal).unwrap();
    assert_eq!(
        journal.reader().unwrap().next().unwrap().unwrap().entry(),
        maximal
    );
    let oversized = "k".repeat(1025);
    assert!(journal.append(entry(&oversized)).is_err());
    fs::write(scratch.join("journal"), b"[null,\"bucket\",\"\\u96ea\\ud83d\\ude00\",\"upload\"]\n[null,\"bucket\",\"\\ud800\",\"upload\"]\n").unwrap();
    let mut reader = journal.reader().unwrap();
    assert_eq!(reader.next().unwrap().unwrap().entry(), entry("雪😀"));
    assert!(reader.next().unwrap().is_none());
}
