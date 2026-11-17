//! Run each inspect/show fixture invocation within the shared path and budget
//! contracts through the stdio transport. This keeps newly added agent fixtures covered without a second list.
use super::support::BIN;
use serde_json::{Map, Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
};
use unityrs_testkit::{child, scratch::Scratch};

fn arguments(args: &[&str]) -> Option<Value> {
    let mut object = Map::new();
    let mut inputs = Vec::new();
    let mut args = args.iter().skip(1);
    let mut positional = false;
    while let Some(&arg) = args.next() {
        if arg == "--" && !positional {
            positional = true;
            continue;
        }
        if positional || !arg.starts_with('-') {
            inputs.push(json!(arg));
            continue;
        }
        let value = *args.next()?;
        let key = arg.strip_prefix("--")?.replace('-', "_");
        let value = match arg {
            "--path-id" => json!(value.parse::<i64>().ok()?),
            "--limit" | "--max-bytes" | "--max-items" | "--max-depth" | "--max-input-memory"
            | "--max-input-bytes" | "--max-decode-work" | "--max-tree-depth" => {
                json!(value.parse::<u64>().ok()?)
            }
            "--class" | "--name" => {
                let values = object
                    .entry(key)
                    .or_insert_with(|| json!([]))
                    .as_array_mut()?;
                values.push(json!(value));
                continue;
            }
            "--file" | "--file-index" | "--cursor" | "--tpk" | "--bytes" | "--value" => {
                json!(value)
            }
            _ => return None,
        };
        if object.insert(key, value).is_some() {
            return None;
        }
    }
    object.insert("inputs".into(), json!(inputs));
    Some(Value::Object(object))
}

pub fn check(tmp: &Scratch, args: &[&str], expected: &Value) {
    if !matches!(args.first(), Some(&"inspect" | &"show")) || expected["code"] == "usage" {
        return;
    }
    // Acquisition can fail at different allocations under tiny resource
    // caps. Still compare those fixtures, with the native default resources.
    if expected["code"] == "budget"
        || expected["inputs"].as_array().is_some_and(|inputs| {
            inputs
                .iter()
                .any(|input| input["error"]["code"] == "budget")
        })
    {
        let mut relaxed = Vec::new();
        let mut iter = args.iter();
        let mut positional = false;
        while let Some(&arg) = iter.next() {
            if arg == "--" {
                positional = true;
            }
            if !positional
                && matches!(
                    arg,
                    "--max-input-memory"
                        | "--max-input-bytes"
                        | "--max-decode-work"
                        | "--max-tree-depth"
                )
            {
                iter.next();
            } else {
                relaxed.push(arg);
            }
        }
        let output =
            child::output(Command::new(BIN).current_dir(tmp.path()).args(&relaxed)).unwrap();
        let document = serde_json::from_slice(&output.stdout).unwrap();
        compare(tmp, &relaxed, &document);
    } else {
        compare(tmp, args, expected);
    }
}
fn compare(tmp: &Scratch, args: &[&str], expected: &Value) {
    if !matches!(args.first(), Some(&"inspect" | &"show")) || expected["code"] == "usage" {
        return;
    }
    let Some(arguments) = arguments(args) else {
        return;
    };
    // CLI-only parent/symlink spellings lie outside the shared root contract.
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let paths = arguments["inputs"]
        .as_array()
        .unwrap()
        .iter()
        .chain(arguments.get("tpk"));
    if paths.filter_map(Value::as_str).any(|path| {
        let path = std::path::Path::new(path);
        path.components()
            .any(|part| part == std::path::Component::ParentDir)
            || (path.is_absolute() && !path.starts_with(&root))
    }) {
        return;
    }
    child::scene(|scene| {
        let mut cmd = Command::new(BIN);
        cmd.current_dir(tmp.path()).args(["mcp", "--root", ".", "--max-request-bytes", "16777216", "--max-response-bytes", "67108864", "--limit", "10000", "--max-depth", "128", "--max-tree-depth", "4294967295"]);
        for key in ["--max-bytes", "--max-items", "--max-input-memory", "--max-input-bytes", "--max-decode-work"] {
            cmd.args([key, "18446744073709551615"]);
        }
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let process = child::live(scene, &mut cmd, false)?;
        let stderr = child::drain(scene.take_stderr(process).unwrap());
        let mut input = scene.take_stdin(process).unwrap();
        let mut output = BufReader::new(scene.take_stdout(process).unwrap());
        for message in [
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"synthetic","version":"1"}}}),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":format!("unityrs_{}",args[0]),"arguments":arguments}}),
        ] { writeln!(input, "{message}")?; }
        drop(input);
        let mut line = String::new();
        output.read_line(&mut line)?;
        let init: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(init["id"], 1);
        line.clear();
        output.read_line(&mut line)?;
        let response: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(response["id"], 2);
        assert_eq!(&response["result"]["structuredContent"], expected, "{args:?}: {response}");
        let text: Value = serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(&text, expected, "{args:?}");
        assert!(scene.wait(process)?.success());
        stderr.1.join().unwrap();
        assert_eq!(stderr.0.try_iter().flatten().collect::<Vec<_>>(), [] as [u8; 0]);
        Ok(())
    }).unwrap();
}
