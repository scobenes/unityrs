use crate::{mcp_support::*, support};
use serde_json::{Value, json};
use std::{
    ffi::CString,
    io::Write,
    os::unix::{ffi::OsStrExt, fs::symlink, net::UnixListener},
    process::{Command, Stdio},
    time::Duration,
};
use unityrs_testkit::child;

#[test]
fn output_startup_rejects_symlink_ancestors_before_creating_children() {
    let tmp = support::fixture();
    std::fs::create_dir(tmp.join("outside")).unwrap();
    symlink(tmp.join("outside"), tmp.join("alias")).unwrap();
    for output in [
        "alias/new/out".to_owned(),
        tmp.join("alias/new/out").to_str().unwrap().to_owned(),
    ] {
        let result = support::run(&tmp, &["mcp", "--root", ".", "--out", &output]);
        assert_eq!(result.status.code(), Some(2), "{output}: {result:?}");
        assert_eq!(result.stdout, [] as [u8; 0]);
        assert!(!tmp.join("outside/new").exists());
    }
}

#[test]
fn startup_roots_reject_symlinks_and_output_parent_components() {
    let tmp = support::fixture();
    std::fs::create_dir(tmp.join("real")).unwrap();
    std::fs::create_dir(tmp.join("real/child")).unwrap();
    symlink("real", tmp.join("alias")).unwrap();
    for root in ["alias", "alias/child"] {
        let result = support::run(&tmp, &["mcp", "--root", root]);
        assert_eq!(result.status.code(), Some(2), "{root}: {result:?}");
    }
    for output in ["alias", "real/../escaped", "alias/../escaped"] {
        let result = support::run(&tmp, &["mcp", "--root", ".", "--out", output]);
        assert_eq!(result.status.code(), Some(2), "{output}: {result:?}");
        assert!(!tmp.join("escaped").exists());
    }
}

#[test]
fn nul_long_names_and_parent_after_symlink_are_refused() {
    session(&["--out", "out"], |s| {
        s.ready();
        std::fs::create_dir(s.directory.join("real")).unwrap();
        symlink("real", s.directory.join("alias")).unwrap();
        symlink("../real", s.directory.join("out/alias")).unwrap();
        for path in [
            "synthetic.assets\0tail".to_owned(),
            "x".repeat(1024),
            "alias/../synthetic.assets".to_owned(),
        ] {
            let result = s.request(
                "tools/call",
                json!({"name":"unityrs_show", "arguments":{"inputs":[path], "path_id":-2}}),
            );
            assert_eq!(result["result"]["isError"], true, "{result}");
        }
        for output in [
            "bad\0tail".to_owned(),
            "x".repeat(1024),
            "alias/../escaped".to_owned(),
        ] {
            let result = s.request("tools/call", json!({"name":"unityrs_extract", "arguments":{"inputs":["synthetic.assets"], "output":output}}));
            assert_eq!(result["result"]["isError"], true, "{result}");
        }
        assert!(!s.directory.join("out/escaped").exists());
        assert_eq!(s.request("ping", json!({}))["result"], json!({}));
    });
}

#[test]
fn output_keeps_held_directory_when_an_ancestor_is_replaced() {
    session(&["--out", "ancestor/out"], |s| {
        s.ready();
        std::fs::create_dir(s.directory.join("replacement")).unwrap();
        std::fs::rename(
            s.directory.join("ancestor"),
            s.directory.join("held-ancestor"),
        )
        .unwrap();
        symlink("replacement", s.directory.join("ancestor")).unwrap();
        let result = s.request("tools/call", json!({"name":"unityrs_extract", "arguments":{"inputs":["synthetic.assets"], "output":"held", "asset_filter":["TextAsset"], "no_container":true}}));
        assert_eq!(result["result"]["isError"], false, "{result}");
        assert_eq!(
            std::fs::read(s.directory.join("held-ancestor/out/held/alpha.txt")).unwrap(),
            b"hello"
        );
        assert_eq!(
            std::fs::read_dir(s.directory.join("replacement"))
                .unwrap()
                .count(),
            0
        );
    });
}

#[test]
fn special_inputs_are_refused_without_blocking() {
    child::scene(|scene| {
        let tmp = support::fixture();
        let fifo = CString::new(tmp.join("pipe").as_os_str().as_bytes()).unwrap();
        // SAFETY: the terminated pathname names a new leaf in owned scratch.
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        let _socket = UnixListener::bind(tmp.join("socket"))?;
        let mut paths = vec!["pipe", "socket", "."];
        let device = CString::new(tmp.join("device").as_os_str().as_bytes()).unwrap();
        // SAFETY: an optional synthetic device node is created only in scratch.
        // Unprivileged hosts commonly refuse mknod; no host device is accessed.
        let made_device = unsafe { libc::mknod(device.as_ptr(), libc::S_IFCHR | 0o600, 0) };
        if made_device == 0 {
            eprintln!("synthetic device node exercised");
            paths.push("device");
        } else {
            assert_eq!(std::io::Error::last_os_error().kind(), std::io::ErrorKind::PermissionDenied);
        }
        let mut command = Command::new(support::BIN);
        command.current_dir(tmp.path()).args(["mcp", "--root", "."])
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let process = child::live(scene, &mut command, false)?;
        let stdout = child::drain(scene.take_stdout(process).unwrap());
        let stderr = child::drain(scene.take_stderr(process).unwrap());
        let mut input = scene.take_stdin(process).unwrap();
        let mut messages = vec![
            json!({"jsonrpc":"2.0", "id":0, "method":"initialize", "params":initialize("2025-11-25")}),
            json!({"jsonrpc":"2.0", "method":"notifications/initialized"}),
        ];
        for (index, path) in paths.iter().enumerate() {
            messages.push(json!({"jsonrpc":"2.0", "id":index+1, "method":"tools/call", "params":{"name":"unityrs_show", "arguments":{"inputs":[path], "path_id":-2}}}));
        }
        messages.push(json!({"jsonrpc":"2.0", "id":99, "method":"ping"}));
        for message in messages {
            writeln!(input, "{message}")?;
        }
        drop(input);
        // EOF follows every refused special file. A blocking FIFO open cannot
        // hide behind the test's own blocking read; scene cleanup owns the child.
        let mut bytes = Vec::new();
        while let Ok(chunk) = stdout.0.recv_timeout(Duration::from_secs(10)) {
            bytes.extend_from_slice(&chunk);
        }
        let responses: Vec<Value> = bytes.split(|&b| b == b'\n').filter(|line| !line.is_empty())
            .map(|line| serde_json::from_slice(line).unwrap()).collect();
        assert_eq!(responses.len(), paths.len()+2, "{responses:?}");
        for response in &responses[1..responses.len()-1] {
            assert_eq!(response["result"]["isError"], true, "{response}");
        }
        assert_eq!(responses.last().unwrap()["id"], 99);
        assert!(scene.wait(process)?.success());
        stdout.1.join().unwrap();
        stderr.1.join().unwrap();
        assert_eq!(stderr.0.try_iter().flatten().collect::<Vec<_>>(), [] as [u8; 0]);
        Ok(())
    }).unwrap();
}
