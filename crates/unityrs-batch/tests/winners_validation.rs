//! Corrupt committed keys must never produce publishable artifacts.
mod artifact_support;
mod common;
#[path = "winners_support/harness.rs"]
mod harness;

use std::sync::Arc;
use unityrs_batch::{
    phase::{Account, Error},
    sort::{NoHooks, Options},
    spool::{Counter, Host, workspace::Workspace},
    winners::{self, Context, Prepared},
};
use unityrs_testkit::scratch::Scratch;

fn refuses_key(key: &[u8]) {
    let scratch = Scratch::new("winner-key-validation");
    let case = harness::Case::new(vec![harness::candidate("a", "good", None, 1)]);
    let spec = case.spec().spec;
    let counter = Counter::new(u64::MAX, 0, 4096).unwrap();
    let account = Account::new(64 << 20);
    let (commits, _) = case.commit(&scratch, &counter, &account, &spec);
    let path = &commits.paths[0];
    let original = std::fs::read(path).unwrap();
    let mut lines = original.split_inclusive(|&b| b == b'\n');
    let mut row: serde_json::Value = serde_json::from_slice(lines.next().unwrap()).unwrap();
    row["key"] = "INVALID_KEY".into();
    let encoded = serde_json::to_vec(&row).unwrap();
    let at = encoded
        .windows(11)
        .position(|w| w == b"INVALID_KEY")
        .unwrap();
    let mut corrupt = encoded[..at].to_vec();
    // Encode valid UTF-8 as JSON; retain invalid bytes to exercise not_utf8.
    if let Ok(text) = std::str::from_utf8(key) {
        let quoted = serde_json::to_vec(text).unwrap();
        corrupt.extend_from_slice(&quoted[1..quoted.len() - 1]);
    } else {
        corrupt.extend_from_slice(key);
    }
    corrupt.extend_from_slice(&encoded[at + 11..]);
    corrupt.push(b'\n');
    corrupt.extend(lines.flatten());
    std::fs::write(path, corrupt).unwrap();
    let prepared = Prepared::from_spec(&spec).unwrap();
    let mut workspace = Workspace::create(scratch.join("w"), counter, Arc::new(Host)).unwrap();
    let result = winners::run(&mut Context {
        spec: &spec,
        prepared: &prepared,
        inputs: &commits,
        workspace: &mut workspace,
        account: &account,
        options: Options::default(),
        stopped: &|| false,
        hooks: &NoHooks,
        observer: &winners::NoObserver,
    });
    assert_eq!(result.unwrap_err(), Error::internal(), "key={key:?}");
    assert_eq!(account.snapshot().input.used, 0);
}

#[test]
fn every_non_prefix_key_rule_refuses_committed_candidates() {
    for key in [
        "",
        "a\u{0}",
        "a\u{1f}",
        "a\u{7f}",
        "/a",
        "a/",
        "a//b",
        ".",
        "..",
        "./a",
        "../escape",
        "a/../b",
        "a/.",
        "a/..",
        "_unityrs",
        "_unityrs/a",
        ".unityrs-tmp-a",
        "a/.unityrs-tmp-b",
    ] {
        refuses_key(key.as_bytes());
    }
    refuses_key(&[0xff]);
    refuses_key("a".repeat(256).as_bytes());
    refuses_key(format!("{}/a", vec!["a".repeat(255); 4].join("/")).as_bytes());
}

#[test]
fn valid_escaped_unicode_and_near_reserved_keys_still_publish() {
    for key in [
        "a\"b",
        "a\\b",
        "a\u{85}b",
        "é",
        "_unityrs-more",
        "a/.unityrs-tmp",
    ] {
        let case = harness::Case::new(vec![harness::candidate("a", key, None, 1)]);
        let decisions = case.select(Options::default(), true);
        assert_eq!(decisions.len(), 1);
        assert!(decisions[0].publish);
    }
}
