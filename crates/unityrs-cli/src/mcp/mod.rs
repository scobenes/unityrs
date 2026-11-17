//! MCP 2025-11-25 over bounded newline-delimited JSON-RPC, with no stdout logs.
mod config;
mod reply;
use reply::Response;
pub(crate) mod roots;
mod schema;
mod tools;
use config::Config;
use serde_json::{Value, json};
use std::{
    io::{self, BufRead, Write},
    process::ExitCode,
};
const VERSION: &str = "2025-11-25";
fn error(id: Value, code: i32, message: &str) -> Response {
    Response::Json(Value::Object(serde_json::Map::from_iter([
        ("jsonrpc".into(), json!("2.0")),
        ("id".into(), id),
        ("error".into(), json!({"code":code,"message":message})),
    ])))
}
fn success(id: Value, result: Value) -> Response {
    Response::Json(Value::Object(serde_json::Map::from_iter([
        ("jsonrpc".into(), json!("2.0")),
        ("id".into(), id),
        ("result".into(), result),
    ])))
}
#[derive(Default)]
struct State {
    initialized: bool,
    ready: bool,
}
impl State {
    fn handle(&mut self, request: &Value, config: &Config) -> Option<Response> {
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let valid_id = request
            .get("id")
            .is_none_or(|v| v.is_string() || v.is_i64() || v.is_u64());
        if !request.is_object()
            || request.get("jsonrpc") != Some(&json!("2.0"))
            || !request.get("method").is_some_and(Value::is_string)
            || !valid_id
        {
            return Some(error(
                if valid_id { id } else { Value::Null },
                -32600,
                "Invalid Request",
            ));
        }
        let method = request
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if request.get("id").is_none() {
            if method == "notifications/initialized" && self.initialized {
                self.ready = true;
            }
            return None;
        }
        let empty = json!({});
        let params = request.get("params").unwrap_or(&empty);
        if !params.is_object() {
            return Some(error(id, -32602, "params must be an object"));
        }
        let result = match method {
            "initialize" => {
                if self.initialized {
                    return Some(error(id, -32600, "Already initialized"));
                }
                if !params.get("protocolVersion").is_some_and(Value::is_string)
                    || !params.get("capabilities").is_some_and(Value::is_object)
                    || !params
                        .get("clientInfo")
                        .and_then(|v| v.get("name"))
                        .is_some_and(Value::is_string)
                    || !params
                        .get("clientInfo")
                        .and_then(|v| v.get("version"))
                        .is_some_and(Value::is_string)
                {
                    return Some(error(id, -32602, "Invalid initialize parameters"));
                }
                self.initialized = true;
                json!({"protocolVersion":VERSION,"capabilities":{"tools":{"listChanged":false}},"serverInfo":{"name":"unityrs","version":env!("CARGO_PKG_VERSION")}})
            }
            "ping" => json!({}),
            "tools/list" | "tools/call" if !self.ready => {
                return Some(error(
                    id,
                    -32600,
                    "Initialize and send notifications/initialized first",
                ));
            }
            "tools/list" => {
                if params.get("cursor").is_some_and(|v| !v.is_null()) {
                    return Some(error(id, -32602, "Tool list has no continuation cursor"));
                }
                json!({"tools":schema::tools(config.output.is_some())})
            }
            "tools/call" => match tools::call(params, config) {
                Ok(result) => return Some(Response::Tool { id, result }),
                Err(message) => return Some(error(id, -32602, &message)),
            },
            _ => return Some(error(id, -32601, "Method not found")),
        };
        Some(success(id, result))
    }
}
/// Read and drain exactly one physical line with bounded storage, including
/// when a peer sends an unlimited line. A missing terminal newline is accepted.
fn line(input: &mut impl BufRead, cap: usize) -> io::Result<Option<Result<Vec<u8>, ()>>> {
    let mut bytes = Vec::new();
    let mut oversized = false;
    loop {
        let available = input.fill_buf()?;
        if available.is_empty() {
            return Ok(if bytes.is_empty() && !oversized {
                None
            } else {
                Some(if oversized { Err(()) } else { Ok(bytes) })
            });
        }
        let end = available
            .iter()
            .position(|b| *b == b'\n')
            .map_or(available.len(), |i| i + 1);
        let chunk = available.get(..end).unwrap_or_default();
        let done = chunk.last() == Some(&b'\n');
        if !oversized {
            if bytes.len().saturating_add(end) > cap || bytes.try_reserve(end).is_err() {
                oversized = true;
                bytes.clear();
            } else {
                bytes.extend_from_slice(chunk);
            }
        }
        input.consume(end);
        if done {
            return Ok(Some(if oversized { Err(()) } else { Ok(bytes) }));
        }
    }
}
fn serve(config: &Config, input: &mut impl BufRead, output: &mut impl Write) -> io::Result<()> {
    let mut state = State::default();
    while let Some(line) = line(input, config.request_bytes)? {
        let response = match line {
            Err(()) => Some(error(
                Value::Null,
                -32600,
                "Request line exceeds --max-request-bytes",
            )),
            Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
                Err(_) => Some(error(Value::Null, -32700, "Parse error")),
                Ok(value) => std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    state.handle(&value, config)
                }))
                .unwrap_or_else(|_| {
                    value
                        .get("id")
                        .map(|id| error(id.clone(), -32603, "Internal error"))
                }),
            },
        };
        if let Some(response) = response {
            response.write(config, output)?;
        }
    }
    Ok(())
}
pub(crate) fn main(raw: &[Vec<u8>]) -> ExitCode {
    if matches!(raw, [v] if v == b"--help" || v == b"-h") {
        eprintln!(
            "usage: unityrs mcp --root DIR [--root DIR] [--out DIR] [limits]\n{}",
            config::help()
        );
        return ExitCode::SUCCESS;
    }
    let config = match Config::parse(raw) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("unityrs mcp: {e}");
            return ExitCode::from(2);
        }
    };
    match serve(&config, &mut io::stdin().lock(), &mut io::stdout().lock()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("unityrs mcp: {e}");
            ExitCode::from(1)
        }
    }
}
pub(crate) fn options() -> Vec<crate::agent::options::OptionSpec> {
    config::options()
}
