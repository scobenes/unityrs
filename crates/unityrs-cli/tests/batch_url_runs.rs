//! Plain HTTP URL execution through the production binary, without hooks.
#![cfg(target_os = "linux")]
mod common;
#[path = "../../unityrs-batch/tests/run_support/mod.rs"]
mod run_support;
#[path = "../../unityrs-batch/tests/url_run_support/mod.rs"]
mod url_run_support;
use serde_json::{Value, json};
use std::{fs, process::Command};
use unityrs_testkit::{child, scratch::Scratch};
use url_run_support::{Origin, Reply};

#[test]
fn http_ip_literal_runs_match_local_synthetic_outputs_and_manifest_rows() {
    let case = run_support::cases()
        .into_iter()
        .find(|c| c["name"] == "ngui-material_in_dependency")
        .unwrap();
    let fixture = run_support::scene(&case);
    let mut baseline = None;
    for remote in [false, true] {
        let scratch = Scratch::new("cli-url-run");
        fs::create_dir(scratch.join("spool")).unwrap();
        fs::create_dir(scratch.join("sink")).unwrap();
        let (mut spec, expected) = run_support::instantiate(&case, &fixture, &scratch);
        let origin = Origin::start();
        if remote {
            url_run_support::onto(&origin, &mut spec, |_| true);
        }
        let path = common::write_job(&scratch, &spec.to_string());
        let check = child::output(
            Command::new(common::BIN)
                .args(["batch", "--check"])
                .arg(&path),
        )
        .unwrap();
        assert!(check.status.success(), "{check:?}");
        let (_, report) = common::line_of(&check);
        let bound = report["detail"]
            .as_str()
            .unwrap()
            .strip_prefix("memory_bound ")
            .unwrap()
            .parse()
            .unwrap();
        let output = child::output(
            Command::new(common::BIN)
                .arg("batch")
                .arg(&path)
                .env_remove("UNITYRS_TEST_RESOLVE")
                .env_remove("UNITYRS_TEST_EXTRA_ROOTS"),
        )
        .unwrap();
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        let checked = run_support::check(&scratch, &spec, &case, expected, bound);
        if let Some(local) = &baseline {
            assert_eq!(&checked, local);
        } else {
            baseline = Some(checked);
        }
        if remote {
            assert_eq!(origin.total(), fixture.bundles.len());
            for head in origin.heads() {
                let head = head.to_ascii_lowercase();
                assert!(head.contains("accept: */*\r\n"), "{head}");
                assert!(head.contains("accept-encoding: identity\r\n"), "{head}");
                assert!(head.contains("user-agent: unityrs/"), "{head}");
            }
        }
    }
}

#[test]
fn an_unused_url_dependency_is_staged_and_its_failure_sets_exit_4() {
    let scratch = Scratch::new("cli-unused-url");
    let origin = Origin::start();
    for dir in ["spool", "sink"] {
        fs::create_dir(scratch.join(dir)).unwrap();
    }
    let case = run_support::cases()
        .into_iter()
        .find(|c| c["name"] == "ngui-material_in_dependency")
        .unwrap();
    let fixture = run_support::scene(&case);
    let (mut spec, _) = run_support::instantiate(&case, &fixture, &scratch);
    spec["run_id"] = json!("r");
    spec["dependencies"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id":"unused", "url":origin.url("/unused?query_secret=hidden")}));
    let path = common::write_job(&scratch, &spec.to_string());
    let output = child::output(Command::new(common::BIN).arg("batch").arg(&path)).unwrap();
    assert_eq!(output.status.code(), Some(4), "{output:?}");
    assert_eq!(origin.requests("/unused"), 1);
    let head: Value =
        serde_json::from_slice(&fs::read(scratch.join("sink/_unityrs/r/manifest.json")).unwrap())
            .unwrap();
    assert_eq!(head["counts"]["inputs_complete"], 1);
    assert_eq!(head["counts"]["inputs_failed"], 0);
    assert_eq!(head["counts"]["bundles_failed"], 1);
    let rows = fs::read_to_string(scratch.join("sink/_unityrs/r/1/bundles.0.ndjson")).unwrap();
    let rows: Vec<Value> = rows
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let unused = rows.iter().find(|r| r["id"] == "unused").unwrap();
    assert_eq!(unused["status"], "failed");
    assert_eq!(unused["reason"], "download_failed");
    assert_eq!(unused["detail"], "status 404");
    for bytes in [&output.stdout, &output.stderr] {
        assert!(!String::from_utf8_lossy(bytes).contains("query_secret"));
    }
}

#[cfg(not(feature = "test-hooks"))]
#[test]
fn a_build_without_hooks_never_reads_resolver_or_root_variables() {
    let scratch = Scratch::new("cli-url-no-hooks");
    let origin = Origin::start();
    origin.body("/input", b"not a bundle".to_vec());
    let path = scenes_job(&scratch, |spec| {
        spec["inputs"] = json!([{"id":"a", "url":origin.url("/input")}]);
    });
    let output = child::output(
        Command::new(common::BIN)
            .arg("batch")
            .arg(path)
            .env("UNITYRS_TEST_RESOLVE", "malformed hook")
            .env("UNITYRS_TEST_EXTRA_ROOTS", "missing-root-file"),
    )
    .unwrap();
    assert_eq!(output.status.code(), Some(4), "{output:?}");
    assert_eq!(origin.requests("/input"), 1);
}

#[test]
fn redirects_retries_and_literal_and_environment_headers_reach_the_origin() {
    let scratch = Scratch::new("cli-url-retries");
    let origin = Origin::start();
    origin.body("/final", b"not a bundle".to_vec());
    origin.script(
        "/start",
        [
            Reply::Status(503, String::new()),
            Reply::Redirect("/final?redirect_secret=hidden".into()),
        ],
    );
    let path = scenes_job(&scratch, |spec| {
        spec["inputs"] = json!([{"id":"a", "url":origin.url("/start?query_secret=hidden"),
            "headers":{"Accept":"application/octet-stream", "X-Literal":"literal_secret", "X-Env":{"env":"UNITYRS_TEST_HEADER"}}}]);
        spec["http"] = json!({"attempts":2,"backoff_initial_ms":0,"backoff_max_ms":0});
    });
    let output = child::output(
        Command::new(common::BIN)
            .arg("batch")
            .arg(&path)
            .env("UNITYRS_TEST_HEADER", "environment_secret"),
    )
    .unwrap();
    assert_eq!(output.status.code(), Some(4), "{output:?}");
    assert_eq!(origin.requests("/start"), 2);
    assert_eq!(origin.requests("/final"), 1);
    for head in origin.heads() {
        let head = head.to_ascii_lowercase();
        for field in [
            "accept: application/octet-stream\r\n",
            "x-literal: literal_secret\r\n",
            "x-env: environment_secret\r\n",
        ] {
            assert!(head.contains(field), "{head}");
        }
    }
    let manifest = fs::read(scratch.join("sink/_unityrs/r/manifest.json")).unwrap();
    let rows = fs::read(scratch.join("sink/_unityrs/r/1/bundles.0.ndjson")).unwrap();
    for bytes in [&output.stdout, &output.stderr, &manifest, &rows] {
        let text = String::from_utf8_lossy(bytes);
        for secret in [
            "query_secret",
            "redirect_secret",
            "literal_secret",
            "environment_secret",
        ] {
            assert!(!text.contains(secret), "{text}");
        }
    }
}

fn scenes_job(scratch: &Scratch, edit: impl FnOnce(&mut Value)) -> std::path::PathBuf {
    for name in ["spool", "sink"] {
        fs::create_dir(scratch.join(name)).unwrap();
    }
    fs::write(scratch.join("a"), b"not a bundle").unwrap();
    let mut spec: Value = serde_json::from_str(&common::job(common::INPUTS)).unwrap();
    spec["spool"]["dir"] = json!(scratch.join("spool"));
    spec["sink"]["root"] = json!(scratch.join("sink"));
    spec["inputs"] = json!([{"id":"a", "path":scratch.join("a")}]);
    edit(&mut spec);
    common::write_job(scratch, &spec.to_string())
}

#[test]
fn preflight_stderr_masks_queries_literal_headers_and_previously_resolved_values() {
    for source in ["query", "literal", "resolved"] {
        let scratch = Scratch::new("cli-preflight-secret");
        let path = scenes_job(&scratch, |spec| {
            let url = if source == "query" {
                "http://127.0.0.1:9/input?MASKED_SECRET"
            } else {
                "http://127.0.0.1:9/input"
            };
            let mut headers = json!({"X-Second":{"env":"MASKED_SECRET"}});
            if source == "literal" {
                headers["X-First"] = json!("MASKED_SECRET");
            } else if source == "resolved" {
                headers["X-First"] = json!({"env":"UNITYRS_FIRST_SECRET"});
            }
            spec["inputs"] = json!([{"id":"a", "url":url, "headers":headers}]);
        });
        let output = child::output(
            Command::new(common::BIN)
                .arg("batch")
                .arg(path)
                .env_remove("MASKED_SECRET")
                .env("UNITYRS_FIRST_SECRET", "MASKED_SECRET"),
        )
        .unwrap();
        assert_eq!(output.status.code(), Some(69), "{output:?}");
        let err = String::from_utf8(output.stderr).unwrap();
        assert!(err.contains("header_env_missing ***"), "{source}: {err}");
        assert!(!err.contains("MASKED_SECRET"), "{source}: {err}");
        // Structured details keep the variable's name, as section 10
        // requires; no header value or URI-bearing error is rendered.
        let line: Value = serde_json::from_slice(
            output
                .stdout
                .split(|c| *c == b'\n')
                .rfind(|l| !l.is_empty())
                .unwrap(),
        )
        .unwrap();
        assert_eq!(line["detail"], "MASKED_SECRET");
    }
}
