mod common;
#[cfg(target_os = "linux")]
#[path = "../../unityrs-batch/tests/run_support/mod.rs"]
mod run_support;
use serde_json::{Value, json};
use std::{fs, process::Command};
use unityrs_testkit::{child, scratch::Scratch};

#[test]
fn unavailable_capabilities_are_refused_before_mutation_but_check_accepts_them() {
    for kind in ["url", "unused_url", "store", "deadline", "stdin", "parent"] {
        let scratch = Scratch::new("batch-refusals");
        fs::create_dir(scratch.join("spool")).unwrap();
        fs::create_dir(scratch.join("sink")).unwrap();
        fs::write(scratch.join("spool/sentinel"), b"keep").unwrap();
        let mut spec: Value = serde_json::from_str(&common::job(common::INPUTS)).unwrap();
        spec["spool"]["dir"] = json!(scratch.join("spool"));
        spec["sink"]["root"] = json!(scratch.join("sink"));
        match kind {
            "url" => spec["inputs"] = json!([{"id":"a","url":"https://example.invalid/input"}]),
            "unused_url" => {
                spec["dependencies"] =
                    json!([{"id":"unused","url":"https://example.invalid/input"}]);
            }
            "store" => {
                spec["sink"] =
                    json!({"kind":"object_store","bucket":"bucket","region":"region","prefix":""});
                spec["deadline"] = "2099-01-01T00:00:00Z".into();
            }
            "deadline" => spec["deadline"] = "2099-01-01T00:00:00Z".into(),
            "stdin" => spec["stop_on_stdin_eof"] = true.into(),
            "parent" => spec["parent_pid"] = 2.into(),
            _ => unreachable!(),
        }
        let path = scratch.join("job.json");
        fs::write(&path, serde_json::to_vec(&spec).unwrap()).unwrap();
        let check = child::output(
            Command::new(common::BIN)
                .args(["batch", "--check"])
                .arg(&path),
        )
        .unwrap();
        assert_eq!(check.status.code(), Some(0), "{kind} {check:?}");
        let output = child::output(Command::new(common::BIN).arg("batch").arg(&path)).unwrap();
        assert_eq!(output.status.code(), Some(78), "{kind} {output:?}");
        let (_, line) = common::line_of(&output);
        assert_eq!(line["reason"], "bad_spec");
        assert_eq!(line["detail"], "usage");
        assert!(String::from_utf8_lossy(&output.stderr).contains("temporary normal-run subset"));
        assert_eq!(fs::read(scratch.join("spool/sentinel")).unwrap(), b"keep");
        assert_eq!(fs::read_dir(scratch.join("sink")).unwrap().count(), 0);
        assert_eq!(fs::read_dir(scratch.join("spool")).unwrap().count(), 1);
    }
}
#[cfg(target_os = "linux")]
#[test]
fn normal_cli_replays_the_versioned_synthetic_goldens() {
    for case in run_support::cases() {
        let scene = run_support::scene(&case);
        let mut baseline = None;
        for threads in [1, 2, 8] {
            let scratch = Scratch::new("cli-golden");
            fs::create_dir(scratch.join("spool")).unwrap();
            fs::create_dir(scratch.join("sink")).unwrap();
            let (mut spec, expected) = run_support::instantiate(&case, &scene, &scratch);
            spec["threads"] = threads.into();
            spec["download_concurrency"] = threads.into();
            let path = scratch.join("job.json");
            fs::write(&path, serde_json::to_vec(&spec).unwrap()).unwrap();
            let checked = child::output(
                Command::new(common::BIN)
                    .args(["batch", "--check"])
                    .arg(&path),
            )
            .unwrap();
            assert!(checked.status.success(), "{checked:?}");
            let (_, report) = common::line_of(&checked);
            let bound: u64 = report["detail"]
                .as_str()
                .unwrap()
                .strip_prefix("memory_bound ")
                .unwrap()
                .parse()
                .unwrap();
            // A case's `env` (the UnityCN golden's key) is the child's.
            let vars = case["env"].as_object().into_iter().flatten();
            let output = child::output(
                Command::new(common::BIN)
                    .arg("batch")
                    .arg(&path)
                    .envs(vars.map(|(k, v)| (k, v.as_str().unwrap()))),
            )
            .unwrap();
            assert_eq!(
                output.status.code().unwrap(),
                i32::try_from(case["exit_code"].as_u64().unwrap()).unwrap(),
                "{} {output:?}",
                case["name"]
            );
            let lines: Vec<Value> = std::str::from_utf8(&output.stdout)
                .unwrap()
                .lines()
                .map(|l| serde_json::from_str(l).unwrap())
                .collect();
            assert_ne!(lines.len(), 0);
            for (index, line) in lines.iter().enumerate() {
                assert_eq!(line["seq"], index);
                assert_eq!(line["p"], 1);
                let inputs = &line["inputs"];
                let sum: u64 = ["complete", "failed", "running", "queued", "not_run"]
                    .iter()
                    .map(|k| inputs[k].as_u64().unwrap())
                    .sum();
                assert_eq!(inputs["total"], sum);
            }
            assert_eq!(lines.last().unwrap()["type"], "exit");
            assert_eq!(lines.last().unwrap()["code"], case["exit_code"]);
            let actual = run_support::check(&scratch, &spec, &case, expected, bound);
            if let Some(baseline) = &baseline {
                assert_eq!(&actual, baseline);
            } else {
                baseline = Some(actual);
            }
        }
    }
}

/// A version 2 spec's UnityCN key variable, unset or malformed: a 69
/// naming the variable at preflight, before the spool or sink is touched,
/// the value nowhere in the output. `--check` reads no environment.
#[test]
fn a_missing_or_malformed_unitycn_key_is_a_69_naming_the_variable() {
    const VALUE: &str = "made-up-key-0001";
    for (value, reason) in [
        (None, "header_env_missing"),
        (Some(VALUE), "header_env_invalid"),
    ] {
        let scratch = Scratch::new("batch-unitycn-key");
        fs::create_dir(scratch.join("spool")).unwrap();
        fs::create_dir(scratch.join("sink")).unwrap();
        fs::write(scratch.join("a"), b"not a bundle").unwrap();
        let mut spec: Value = serde_json::from_str(&common::job(common::INPUTS)).unwrap();
        spec["spec_version"] = 2.into();
        spec["inputs"] = json!([{"id": "a", "path": scratch.join("a")}]);
        spec["unitycn_key"] = json!({"env": "UNITYRS_TEST_KEY"});
        spec["spool"]["dir"] = json!(scratch.join("spool"));
        spec["sink"]["root"] = json!(scratch.join("sink"));
        let path = scratch.join("job.json");
        fs::write(&path, serde_json::to_vec(&spec).unwrap()).unwrap();
        let checked = child::output(
            Command::new(common::BIN)
                .args(["batch", "--check"])
                .arg(&path)
                .env_remove("UNITYRS_TEST_KEY"),
        )
        .unwrap();
        assert_eq!(checked.status.code(), Some(0), "{checked:?}");
        let mut cmd = Command::new(common::BIN);
        cmd.arg("batch").arg(&path).env_remove("UNITYRS_TEST_KEY");
        if let Some(value) = value {
            cmd.env("UNITYRS_TEST_KEY", value);
        }
        let output = child::output(&mut cmd).unwrap();
        assert_eq!(output.status.code(), Some(69), "{output:?}");
        let (_, line) = common::line_of(&output);
        assert_eq!(line["reason"], reason);
        assert_eq!(line["detail"], "UNITYRS_TEST_KEY");
        for stream in [&output.stdout, &output.stderr] {
            assert!(!String::from_utf8_lossy(stream).contains(VALUE));
        }
        assert_eq!(fs::read_dir(scratch.join("sink")).unwrap().count(), 0);
        assert_eq!(fs::read_dir(scratch.join("spool")).unwrap().count(), 0);
    }
}

/// The key is read before the host checks, as a header's variable is
/// (`docs/plans/batch.md` 2.11 step 6, then 2.7): under a file-size limit,
/// which the host checks refuse (`rlimit_too_low`), an unset key is still
/// `header_env_missing`.
#[cfg(target_os = "linux")]
#[test]
fn the_unitycn_key_is_read_before_the_host_checks() {
    for (value, reason) in [
        (None, "header_env_missing"),
        (Some("000102030405060708090a0b0c0d0e0f"), "rlimit_too_low"),
    ] {
        let scratch = Scratch::new("batch-unitycn-order");
        fs::create_dir(scratch.join("spool")).unwrap();
        fs::create_dir(scratch.join("sink")).unwrap();
        fs::write(scratch.join("a"), b"not a bundle").unwrap();
        let mut spec: Value = serde_json::from_str(&common::job(common::INPUTS)).unwrap();
        spec["spec_version"] = 2.into();
        spec["inputs"] = json!([{"id": "a", "path": scratch.join("a")}]);
        spec["unitycn_key"] = json!({"env": "UNITYRS_TEST_KEY"});
        spec["spool"]["dir"] = json!(scratch.join("spool"));
        spec["sink"]["root"] = json!(scratch.join("sink"));
        let path = scratch.join("job.json");
        fs::write(&path, serde_json::to_vec(&spec).unwrap()).unwrap();
        // `ulimit -f` sets the hard file-size limit the host checks refuse.
        let mut cmd = Command::new("sh");
        cmd.args([
            "-c",
            r#"ulimit -f 1048576 && exec "$0" batch "$1""#,
            common::BIN,
            path.to_str().unwrap(),
        ])
        .env_remove("UNITYRS_TEST_KEY");
        if let Some(value) = value {
            cmd.env("UNITYRS_TEST_KEY", value);
        }
        let output = child::output(&mut cmd).unwrap();
        assert_eq!(output.status.code(), Some(69), "{output:?}");
        let (_, line) = common::line_of(&output);
        assert_eq!(line["reason"], reason, "{output:?}");
    }
}
