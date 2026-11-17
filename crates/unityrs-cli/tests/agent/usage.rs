//! Usage errors identify the argument to fix and its accepted spelling.
use super::support::{fixture, json};

#[test]
fn missing_and_duplicate_values_identify_the_option() {
    let tmp = fixture();
    for (command, option, value) in [
        ("inspect", "--limit", "2"),
        ("inspect", "--cursor", "token"),
        ("inspect", "--class", "49"),
        ("inspect", "--name", "alpha"),
        ("inspect", "--max-input-memory", "10000"),
        ("inspect", "--max-input-bytes", "10000"),
        ("inspect", "--max-decode-work", "10000"),
        ("inspect", "--max-tree-depth", "100"),
        ("inspect", "--tpk", "types.tpk"),
        ("show", "--path-id", "7"),
        ("show", "--file", "rich.assets"),
        ("show", "--file-index", "0:0"),
        ("show", "--max-bytes", "65536"),
        ("show", "--max-items", "256"),
        ("show", "--max-depth", "32"),
        ("show", "--bytes", "summary"),
        ("show", "--value", "natural"),
        ("describe", "--schema", "show"),
    ] {
        let args = [command, option];
        let error = json(&tmp, &args, 2);
        let message = error["message"].as_str().unwrap();
        assert!(message.contains(option), "{args:?}: {message}");
        assert!(message.contains("missing"), "{args:?}: {message}");
        if !matches!(option, "--class" | "--name") {
            let args = [command, option, value, option, value];
            let error = json(&tmp, &args, 2);
            let message = error["message"].as_str().unwrap();
            assert!(message.contains(option), "{args:?}: {message}");
            assert!(message.contains("duplicate"), "{args:?}: {message}");
        }
    }
    let error = json(&tmp, &["describe", "--help", "-h"], 2);
    assert!(error["message"].as_str().unwrap().contains("--help"));
}

#[test]
fn bad_values_identify_the_option_value_and_accepted_form() {
    let tmp = fixture();
    for (command, option, value, accepted) in [
        (
            "inspect",
            "--class",
            "NotAClass",
            "a class name or signed 32-bit integer ID",
        ),
        (
            "inspect",
            "--class",
            "2147483648",
            "a class name or signed 32-bit integer ID",
        ),
        ("inspect", "--limit", "many", "10000"),
        ("inspect", "--limit", "0", "10000"),
        ("inspect", "--limit", "10001", "10000"),
        ("inspect", "--max-input-memory", "lots", "integer"),
        ("inspect", "--max-input-bytes", "-1", "integer"),
        (
            "inspect",
            "--max-input-bytes",
            "18446744073709551616",
            "18446744073709551615",
        ),
        ("inspect", "--max-decode-work", "1.5", "integer"),
        ("inspect", "--max-tree-depth", "deep", "integer"),
        ("inspect", "--max-tree-depth", "4294967296", "4294967295"),
        ("show", "--path-id", "oops", "9223372036854775807"),
        (
            "show",
            "--path-id",
            "9223372036854775808",
            "9223372036854775807",
        ),
        ("show", "--max-bytes", "large", "integer"),
        ("show", "--file-index", "0", "INPUT:FILE"),
        ("show", "--file-index", "-1:0", "zero-based"),
        ("show", "--file-index", "0:x", "zero-based"),
        ("show", "--file-index", "0:1:2", "INPUT:FILE"),
        ("show", "--file-index", "18446744073709551616:0", "integer"),
        ("show", "--max-items", "many", "integer"),
        ("show", "--max-depth", "deep", "128"),
        ("show", "--max-depth", "129", "128"),
        ("show", "--bytes", "raw", "summary or full"),
        ("show", "--value", "plain", "natural or canonical"),
        (
            "describe",
            "--schema",
            "batch",
            "inspect, show, describe or error",
        ),
    ] {
        let mut args = vec![command];
        if command != "describe" {
            args.push("synthetic.assets");
        }
        if command == "show" && option != "--path-id" {
            args.extend(["--path-id", "-2"]);
        }
        args.extend([option, value]);
        let error = json(&tmp, &args, 2);
        let message = error["message"].as_str().unwrap();
        for part in [option, value, accepted] {
            assert!(
                message.contains(part),
                "missing {part:?}: {args:?}: {message}"
            );
        }
    }
    let error = json(&tmp, &["show", "synthetic.assets"], 2);
    let message = error["message"].as_str().unwrap();
    assert!(message.contains("--path-id"), "{message}");
    assert!(message.contains("required"), "{message}");
    let error = json(&tmp, &["describe", "unexpected-input"], 2);
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("unexpected-input")
    );
}

#[test]
fn hostile_spellings_are_bounded_and_keep_the_accepted_form() {
    let tmp = fixture();
    for (option, accepted) in [
        ("--limit", "10000"),
        ("--max-input-bytes", "integer"),
        ("--class", "a class name or signed 32-bit integer ID"),
    ] {
        let value = format!("bad-value-{}", "é".repeat(10000));
        let error = json(&tmp, &["inspect", "synthetic.assets", option, &value], 2);
        let message = error["message"].as_str().unwrap();
        assert!(message.contains(option), "{message}");
        assert!(message.contains("bad-value-"), "{message}");
        assert!(message.contains(accepted), "{message}");
        assert!(message.len() < 1024, "{}", message.len());
        assert!(!message.contains(&value));
    }
    let option = format!("--unknown-{}", "x".repeat(20000));
    let error = json(&tmp, &["inspect", "synthetic.assets", &option], 2);
    let message = error["message"].as_str().unwrap();
    assert!(message.contains("--unknown-"), "{message}");
    assert!(message.len() < 1024, "{}", message.len());
}

#[cfg(unix)]
#[test]
fn invalid_utf8_values_are_named_in_usage_errors() {
    use super::support::BIN;
    use std::{ffi::OsString, os::unix::ffi::OsStringExt, process::Command};
    use unityrs_testkit::child;

    let tmp = fixture();
    for (command, option, accepted) in [
        ("inspect", "--limit", "10000"),
        ("inspect", "--max-input-memory", "integer"),
        (
            "inspect",
            "--class",
            "a class name or signed 32-bit integer ID",
        ),
        ("show", "--path-id", "signed integer"),
        ("show", "--bytes", "summary or full"),
        ("show", "--value", "natural or canonical"),
        ("describe", "--schema", "inspect, show, describe or error"),
    ] {
        let mut invocation = Command::new(BIN);
        invocation.current_dir(tmp.path()).arg(command);
        if command != "describe" {
            invocation.arg("synthetic.assets");
        }
        if command == "show" && option != "--path-id" {
            invocation.args(["--path-id", "-2"]);
        }
        invocation
            .arg(option)
            .arg(OsString::from_vec(vec![0xff, b'x']));
        let output = child::output(&mut invocation).unwrap();
        assert_eq!(output.status.code(), Some(2));
        let error: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(error["code"], "usage");
        let message = error["message"].as_str().unwrap();
        for part in [option, "\\xffx", accepted] {
            assert!(message.contains(part), "missing {part:?}: {message}");
        }
        assert_eq!(String::from_utf8_lossy(&output.stderr).lines().count(), 1);
    }
}
