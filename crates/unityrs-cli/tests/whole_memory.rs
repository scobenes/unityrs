//! The actual CLI measures its own process only when explicitly requested.
mod common;
#[cfg(target_os = "linux")]
#[path = "../../unityrs-batch/tests/run_support/mod.rs"]
mod run_support;

#[cfg(target_os = "linux")]
#[test]
fn whole_memory_is_opt_in_and_the_unfeatured_binary_ignores_it() {
    use serde_json::json;
    use std::{fs, process::Command};
    use unityrs_testkit::{child, scratch::Scratch};
    let case = run_support::cases().into_iter().next().unwrap();
    let scene = run_support::scene(&case);
    for (setting, workers, dc) in [
        (None, 1, 1),
        (Some("0"), 1, 1),
        (Some("1"), 1, 1),
        (Some("1"), 8, 4),
    ] {
        let scratch = Scratch::new("cli-whole-memory");
        fs::create_dir(scratch.join("spool")).unwrap();
        fs::create_dir(scratch.join("sink")).unwrap();
        let (mut spec, _) = run_support::instantiate(&case, &scene, &scratch);
        spec["threads"] = json!(workers);
        spec["download_concurrency"] = json!(dc);
        let path = scratch.join("job.json");
        fs::write(&path, serde_json::to_vec(&spec).unwrap()).unwrap();
        let mut cmd = Command::new(common::BIN);
        cmd.arg("batch")
            .arg(&path)
            .env_remove("UNITYRS_TEST_WHOLE_MEMORY");
        if let Some(value) = setting {
            cmd.env("UNITYRS_TEST_WHOLE_MEMORY", value);
        }
        let vars = case["env"].as_object().into_iter().flatten();
        cmd.envs(vars.map(|(k, v)| (k, v.as_str().unwrap())));
        let out = child::output(&mut cmd).unwrap();
        assert_eq!(
            out.status.code(),
            Some(i32::try_from(case["exit_code"].as_u64().unwrap()).unwrap()),
            "{out:?}"
        );
        let stderr = String::from_utf8(out.stderr).unwrap();
        let lines: Vec<_> = stderr.lines().collect();
        if !cfg!(feature = "test-hooks") || setting != Some("1") {
            assert!(stderr.is_empty(), "{stderr}");
            continue;
        }
        let whole: Vec<_> = lines
            .iter()
            .filter(|s| s.starts_with("WHOLE_MEMORY"))
            .collect();
        assert_eq!(whole.len(), 1, "{stderr}");
        let words: Vec<_> = whole[0].split(' ').collect();
        assert_eq!(words.len(), 13, "{stderr}");
        assert_eq!(words[0], "WHOLE_MEMORY_V1");
        let numbers: Vec<u64> = words[1..].iter().map(|n| n.parse().unwrap()).collect();
        assert!(numbers[0] > 0 && numbers[1] > 0, "{stderr}");
        assert_eq!(&numbers[2..5], &[255, workers, dc], "{stderr}");
        assert_eq!(numbers[6], 0, "negative heap: {stderr}");
        assert_eq!(numbers[8], 0, "negative held charge: {stderr}");
        let base: Vec<_> = lines.iter().filter(|s| s.starts_with("B12_")).collect();
        if workers == 1 && dc == 1 {
            assert_eq!(base.len(), 1, "{stderr}");
            assert_eq!(*base[0], format!("B12_COEFFICIENT c_base {}", numbers[0]));
        } else {
            assert!(base.is_empty(), "{stderr}");
        }
        assert_eq!(lines.len(), whole.len() + base.len(), "{stderr}");
    }
}
