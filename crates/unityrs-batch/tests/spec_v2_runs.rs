//! Version 2's UnityCN key through a whole run (`docs/BATCH_SPEC.md` 1.1,
//! 7.3): read at preflight from the variable the spec names, set in every
//! load's options, and never rendered. The `v2-unitycn` golden pins the
//! keyed run's outputs (`golden_runs.rs`); these runs vary the key.

mod common;
mod publication_support;
mod run_support;

use std::ffi::OsString;
use std::fs;
use std::path::Path;

use serde_json::{Value, json};
use unityrs_batch::{
    phase1,
    progress::{Snapshot, live::Service},
    reason::Code,
    run,
};
use unityrs_testkit::scratch::Scratch;

struct Space;
impl unityrs_batch::start::spool::FileSystem for Space {
    fn in_memory(&self, _: &fs::File) -> std::io::Result<bool> {
        Ok(false)
    }
    fn available(&self, _: &fs::File) -> std::io::Result<u64> {
        Ok(u64::MAX)
    }
}

/// The made-up key the encrypted golden bundle is built with, in hex.
const KEY_HEX: &str = "73796e7468657469632d6b65792d3031";

fn case() -> Value {
    run_support::cases()
        .into_iter()
        .find(|c| c["name"] == "v2-unitycn")
        .expect("the v2-unitycn golden")
}

/// `case`'s spec with `edit`, run with `env` as its environment: the
/// key's read, then the run's end, and the scratch holding its sink and
/// progress lines.
fn run_with(
    edit: impl FnOnce(&mut Value),
    env: impl Fn(&str) -> Option<OsString>,
) -> (Result<Code, run::Error>, Scratch) {
    let case = case();
    let scene = run_support::scene(&case);
    let scratch = Scratch::new("spec-v2-run");
    fs::create_dir(scratch.join("spool")).unwrap();
    fs::create_dir(scratch.join("sink")).unwrap();
    let (mut spec, _) = run_support::instantiate(&case, &scene, &scratch);
    edit(&mut spec);
    let mut prepared =
        phase1::prepare(unityrs_batch::spec::parse(serde_json::to_vec(&spec).unwrap()).unwrap())
            .unwrap();
    if let Err(e) = run::unitycn_key(&mut prepared, env) {
        return (Err(e), scratch);
    }
    let service = Service::start(
        fs::File::create(scratch.join("progress")).unwrap(),
        Snapshot::validate(0),
    )
    .unwrap();
    let result = run::drive(
        prepared,
        123,
        &service.sender(),
        &Space,
        std::sync::Arc::new(publication_support::Portable),
    )
    .map(|end| end.code);
    (result, scratch)
}

fn only(value: &str) -> impl Fn(&str) -> Option<OsString> + '_ {
    move |name| (name == "UNITYRS_GOLDEN_KEY").then(|| value.into())
}

/// The run's `inputs` row.
fn input_row(scratch: &Scratch) -> Value {
    let rows = scratch.join("sink/_unityrs/golden/1/inputs.0.ndjson");
    serde_json::from_slice(&fs::read(rows).unwrap()).unwrap()
}

/// Every byte the run wrote: the sink's files and the progress lines.
fn written(root: &Path, out: &mut Vec<u8>) {
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            written(&path, out);
        } else {
            out.extend(fs::read(&path).unwrap());
        }
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn the_key_opens_the_encrypted_input_and_is_never_written() {
    let (code, scratch) = run_with(|_| {}, only(KEY_HEX));
    assert_eq!(code.unwrap(), Code::Complete);
    let head: Value = serde_json::from_slice(
        &fs::read(scratch.join("sink/_unityrs/golden/manifest.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(head["manifest_version"], 2);
    assert_eq!(head["spec_version"], 2);
    assert_eq!(head["unitycn_key"], true);
    assert_eq!(head["formats"], json!(["RGBA32", "RGB565"]));
    assert_eq!(head["image"], "png");
    let mut bytes = Vec::new();
    written(&scratch.join("sink"), &mut bytes);
    written(&scratch.join("spool"), &mut bytes);
    bytes.extend(fs::read(scratch.join("progress")).unwrap());
    for needle in [
        KEY_HEX.as_bytes(),
        KEY_HEX.to_uppercase().as_bytes(),
        b"synthetic-key-01".as_slice(),
    ] {
        assert!(
            !contains(&bytes, needle),
            "{}",
            String::from_utf8_lossy(needle)
        );
    }
    // The decrypted input's textures were written.
    assert!(scratch.join("sink/RGB565.png").exists());
    assert!(scratch.join("sink/RGBA32.png").exists());
}

#[test]
fn without_the_right_key_the_input_fails_and_the_run_goes_on() {
    // No key: C3's per-input refusal (7.3), `unsupported`.
    let (code, scratch) = run_with(
        |s| {
            s.as_object_mut().unwrap().remove("unitycn_key");
        },
        |_| panic!("a spec naming no key reads no variable"),
    );
    assert_eq!(code.unwrap(), Code::Incomplete);
    let row = input_row(&scratch);
    assert_eq!(row["reason"], "container_error");
    assert_eq!(row["detail"], "unsupported");
    // A wrong key: its signature check fails, `malformed`.
    let (code, scratch) = run_with(|_| {}, only("000102030405060708090a0b0c0d0e0f"));
    assert_eq!(code.unwrap(), Code::Incomplete);
    let row = input_row(&scratch);
    assert_eq!(row["reason"], "container_error");
    assert_eq!(row["detail"], "malformed");
}

#[test]
fn a_missing_or_malformed_variable_is_a_69_naming_the_variable_alone() {
    for (value, reason) in [
        (None, "header_env_missing"),
        (Some(""), "header_env_missing"),
        (Some(&KEY_HEX[..30]), "header_env_invalid"),
        (Some("synthetic-key-01"), "header_env_invalid"),
    ] {
        let (result, _scratch) = run_with(
            |_| {},
            |name| {
                assert_eq!(name, "UNITYRS_GOLDEN_KEY");
                value.map(OsString::from)
            },
        );
        let e = result.unwrap_err();
        assert_eq!(e.code, Code::CouldNotStart);
        assert_eq!(e.reason, reason);
        assert_eq!(e.detail.as_deref(), Some("UNITYRS_GOLDEN_KEY"));
        assert!(!format!("{e:?}").contains("synthetic-key"));
    }
}

/// A host that skips the read never runs a keyed spec without its key.
#[test]
fn drive_refuses_a_named_key_that_was_not_read() {
    let case = case();
    let scene = run_support::scene(&case);
    let scratch = Scratch::new("spec-v2-unread");
    fs::create_dir(scratch.join("spool")).unwrap();
    fs::create_dir(scratch.join("sink")).unwrap();
    let (spec, _) = run_support::instantiate(&case, &scene, &scratch);
    let prepared =
        phase1::prepare(unityrs_batch::spec::parse(serde_json::to_vec(&spec).unwrap()).unwrap())
            .unwrap();
    let service = Service::start(
        fs::File::create(scratch.join("progress")).unwrap(),
        Snapshot::validate(0),
    )
    .unwrap();
    let e = run::drive(
        prepared,
        123,
        &service.sender(),
        &Space,
        std::sync::Arc::new(publication_support::Portable),
    )
    .unwrap_err();
    assert_eq!(e.reason, "internal");
    assert!(!scratch.join("sink/_unityrs").exists());
}
