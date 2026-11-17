#![allow(dead_code)]
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    process::{ChildStdin, ChildStdout, Command, Stdio},
};
use unityrs_testkit::child;
pub struct Session {
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    id: u64,
    pub directory: std::path::PathBuf,
}
pub fn initialize(version: &str) -> Value {
    json!({"protocolVersion": version,"capabilities":{},"clientInfo":{"name":"synthetic","version":"1"}})
}
pub fn session(options: &[&str], run: impl Fn(&mut Session)) {
    child::scene(|scene| {
        let tmp = crate::support::fixture();
        let mut cmd = Command::new(crate::support::BIN);
        cmd.current_dir(tmp.path())
            .args(["mcp", "--root", "."])
            .args(options)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let process = child::live(scene, &mut cmd, false)?;
        let stderr = child::drain(scene.take_stderr(process).unwrap());
        let mut s = Session {
            input: scene.take_stdin(process).unwrap(),
            output: BufReader::new(scene.take_stdout(process).unwrap()),
            id: 0,
            directory: std::fs::canonicalize(tmp.path()).unwrap(),
        };
        run(&mut s);
        drop(s.input);
        let mut rest = String::new();
        s.output.read_line(&mut rest)?;
        assert!(rest.is_empty(), "unexpected response: {rest}");
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
impl Session {
    pub fn send(&mut self, line: &str) {
        self.input.write_all(line.as_bytes()).unwrap();
        self.input.flush().unwrap();
    }
    pub fn read(&mut self) -> Value {
        let mut line = String::new();
        self.output.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {line:?}"))
    }
    pub fn request(&mut self, method: &str, params: Value) -> Value {
        self.id += 1;
        let message = serde_json::Value::Object(serde_json::Map::from_iter([
            ("jsonrpc".into(), json!("2.0")),
            ("id".into(), json!(self.id)),
            ("method".into(), json!(method)),
            ("params".into(), params),
        ]));
        self.send(&format!("{message}\n"));
        let v = self.read();
        assert_eq!(v["id"], self.id);
        v
    }
    pub fn notify(&mut self, method: &str) {
        self.send(&format!("{}\n", json!({"jsonrpc":"2.0","method":method})));
    }
    pub fn ready(&mut self) {
        self.request("initialize", initialize("2025-11-25"));
        self.notify("notifications/initialized");
    }
}
