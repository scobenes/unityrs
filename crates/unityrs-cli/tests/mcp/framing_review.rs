use crate::support;
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    process::{Command, Stdio},
};
use unityrs_testkit::child;

#[test]
fn final_lines_at_eof_are_bounded_and_served_once() {
    for (payload, expected) in [
        (
            r#"{"jsonrpc":"2.0","id":9,"method":"ping"}"#.to_owned(),
            json!({"jsonrpc":"2.0","id":9,"result":{}}),
        ),
        (
            "x".repeat(257),
            json!({"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":"Request line exceeds --max-request-bytes"}}),
        ),
    ] {
        child::scene(|scene| {
            let tmp = support::fixture();
            let mut cmd = Command::new(support::BIN);
            cmd.current_dir(tmp.path())
                .args([
                    "mcp",
                    "--root",
                    ".",
                    "--max-request-bytes",
                    "256",
                    "--max-response-bytes",
                    "4096",
                ])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let process = child::live(scene, &mut cmd, false)?;
            let stderr = child::drain(scene.take_stderr(process).unwrap());
            let mut input = scene.take_stdin(process).unwrap();
            input.write_all(payload.as_bytes())?;
            drop(input);
            let mut output = String::new();
            scene
                .take_stdout(process)
                .unwrap()
                .read_to_string(&mut output)?;
            assert_eq!(output.lines().count(), 1);
            assert!(output.ends_with('\n'));
            assert_eq!(serde_json::from_str::<Value>(&output).unwrap(), expected);
            assert!(scene.wait(process)?.success());
            stderr.1.join().unwrap();
            assert_eq!(
                stderr.0.try_iter().flatten().collect::<Vec<_>>(),
                [] as [u8; 0]
            );
            Ok(())
        })
        .unwrap();
    }
}

#[test]
fn closed_stdout_exits_quietly_without_writing_elsewhere() {
    child::scene(|scene| {
        let tmp = support::fixture();
        let before: std::collections::BTreeSet<_> = std::fs::read_dir(tmp.path())?
            .map(|e| e.unwrap().file_name())
            .collect();
        let mut cmd = Command::new(support::BIN);
        cmd.current_dir(tmp.path())
            .args(["mcp", "--root", "."])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let process = child::live(scene, &mut cmd, false)?;
        let stderr = child::drain(scene.take_stderr(process).unwrap());
        drop(scene.take_stdout(process).unwrap());
        let mut input = scene.take_stdin(process).unwrap();
        input.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":42,\"method\":\"ping\"}\n")?;
        drop(input);
        assert!(scene.wait(process)?.success());
        stderr.1.join().unwrap();
        assert_eq!(
            stderr.0.try_iter().flatten().collect::<Vec<_>>(),
            [] as [u8; 0]
        );
        let after = std::fs::read_dir(tmp.path())?
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(before, after);
        Ok(())
    })
    .unwrap();
}
