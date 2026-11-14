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
            let output = child::output(Command::new(common::BIN).arg("batch").arg(&path)).unwrap();
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
