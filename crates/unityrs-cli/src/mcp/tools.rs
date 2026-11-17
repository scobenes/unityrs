use super::{
    config::Config,
    reply::{Document, Tool},
    schema,
    value::Value,
};
use serde_json::{json, value::RawValue};
use std::{collections::BTreeMap, path::Path};
use unityrs_mesh::obj::ChargedBytes;
pub fn failure(code: &str, message: &str) -> Tool {
    Tool {
        document: Document::Fixed(
            json!({"format":"unityrs.error","version":1,"code":code,"message":message}).to_string(),
        ),
        failed: true,
    }
}
fn agent_error(error: &crate::agent::Error) -> Tool {
    crate::agent::error_document(error).map_or_else(
        |_| failure("budget", "Error document refused"),
        |document| Tool {
            document: Document::Agent(document),
            failed: true,
        },
    )
}
pub fn call(params: &Value, config: &Config) -> Result<Tool, String> {
    let command = params
        .get("name")
        .and_then(Value::as_str)
        .and_then(|s| s.strip_prefix("unityrs_"))
        .ok_or("Missing tool name")?;
    if !matches!(
        command,
        "describe" | "inspect" | "show" | "batch_check" | "extract"
    ) || (command == "extract" && config.output.is_none())
    {
        return Err("Unknown tool".into());
    }
    let empty = Value::Object(BTreeMap::new());
    let arguments = params.get("arguments").unwrap_or(&empty);
    if !arguments.is_object() {
        return Err("tools/call arguments must be an object".into());
    }
    if let Err(message) = super::arguments::check(arguments, &schema::input(command), "arguments") {
        return Ok(failure("usage", &message));
    }
    if command == "batch_check" {
        return Ok(batch_check(arguments, config));
    }
    if command == "extract"
        && let Err(message) = super::arguments::extraction(arguments)
    {
        return Ok(failure("usage", &message));
    }
    let mut raw = match options(command, arguments, config) {
        Ok(raw) => raw,
        Err(message) => return Ok(failure("budget", &message)),
    };
    let inputs = arguments.get("inputs").and_then(Value::as_array);
    // Validate every supplied path, even when --file-index will skip a load.
    if let Some(inputs) = inputs {
        for value in inputs {
            let path = Path::new(value.as_str().unwrap_or_default());
            // Opening through source handles also supports missing unsuffixed
            // split names; the actual load checks every companion again.
            if let Err(e) = config.roots.open(path)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                return Ok(failure("input", "Input refused by root confinement"));
            }
        }
    }
    for spec in ["--tpk", "--vorbis-headers"] {
        if let Some(value) = arguments.get(schema::key(spec)).and_then(Value::as_str)
            && let Err(error) = config.roots.open(Path::new(value))
            && error.kind() != std::io::ErrorKind::NotFound
        {
            return Ok(failure("input", "Provider refused by root confinement"));
        }
    }
    if command == "extract" {
        raw.extend([b"-o".to_vec(), b"confined-output".to_vec()]);
    }
    if let Some(inputs) = inputs {
        raw.push(b"--".to_vec());
        raw.extend(
            inputs
                .iter()
                .map(|v| v.as_str().unwrap_or_default().as_bytes().to_vec()),
        );
    }
    if command == "extract" {
        return Ok(extract(&raw, arguments, config));
    }
    Ok(
        match crate::agent::execute_confined(command, &raw, Some(config.roots.clone())) {
            Ok(doc) if doc.json.len() >= config.response_bytes => {
                failure("budget", "Response exceeds --max-response-bytes")
            }
            Ok(doc) if command == "describe" => Tool {
                document: Document::Fixed(doc.json),
                failed: false,
            },
            Ok(doc) => Tool {
                document: Document::Agent(doc),
                failed: false,
            },
            Err(e) => agent_error(&e),
        },
    )
}

fn options(command: &str, arguments: &Value, config: &Config) -> Result<Vec<Vec<u8>>, String> {
    let mut raw = Vec::<Vec<u8>>::new();
    for spec in schema::specs(command) {
        let key = schema::key(spec.name);
        let supplied = arguments.get(&key);
        let ceiling = config.maxima.get(spec.name).copied();
        let owned;
        let value = if let Some(max) = ceiling {
            // Omitted defaults must stay omitted: inventory cursors bind
            // explicitly supplied budget options as well as their values.
            if supplied.is_none()
                && spec
                    .default
                    .and_then(|s| s.parse::<u64>().ok())
                    .is_some_and(|default| default <= max)
            {
                continue;
            }
            if supplied
                .and_then(super::integer::unsigned)
                .is_some_and(|n| n > max)
            {
                return Err(format!(
                    "Argument {key}={}; allowed range 0..={max} (server ceiling {})",
                    supplied.unwrap_or(&Value::Null),
                    spec.name
                ));
            }
            owned = Value::Number(
                RawValue::from_string(
                    supplied
                        .and_then(super::integer::unsigned)
                        .unwrap_or_else(|| {
                            spec.default
                                .and_then(|s| s.parse::<u64>().ok())
                                .unwrap_or(max)
                                .min(max)
                        })
                        .to_string(),
                )
                .map_err(|e| e.to_string())?,
            );
            Some(&owned)
        } else {
            supplied
        };
        let Some(value) = value else {
            continue;
        };
        if spec.value.is_none() {
            if matches!(value, Value::Bool(true)) {
                raw.push(spec.name.as_bytes().to_vec());
            }
            continue;
        }
        let values: Vec<&Value> = value
            .as_array()
            .map_or_else(|| vec![value], |values| values.iter().collect());
        for value in values {
            raw.push(spec.name.as_bytes().to_vec());
            raw.push(value.as_str().map_or_else(
                || {
                    super::integer::parse(value)
                        .map_or_else(|| value.to_string(), |n| n.to_string())
                        .into_bytes()
                },
                |s| s.as_bytes().to_vec(),
            ));
        }
    }
    Ok(raw)
}

fn batch_check(arguments: &Value, config: &Config) -> Tool {
    let bytes = arguments
        .get("job")
        .unwrap_or(&Value::Null)
        .to_string()
        .into_bytes();
    let memory = config
        .maxima
        .get("--max-input-memory")
        .copied()
        .unwrap_or(0);
    let work = config.maxima.get("--max-decode-work").copied().unwrap_or(0);
    let input = config.maxima.get("--max-input-bytes").copied().unwrap_or(0);
    if bytes.len() as u64 > input.min(memory / 64).min(work / 64).min(64 << 20) {
        return failure(
            "budget",
            "Job exceeds server input byte or memory ceiling (64 bytes reserved per job byte)",
        );
    }
    match crate::batch::check_document(bytes, memory, work) {
        Ok(doc) => Tool {
            document: Document::Fixed(doc),
            failed: false,
        },
        Err((code, message)) => failure(code, &message),
    }
}

fn resource_refusal(error: &(dyn std::error::Error + 'static)) -> bool {
    if error
        .downcast_ref::<unityrs_core::limits::ChargeError>()
        .is_some()
    {
        return true;
    }
    if let Some(e) = error.downcast_ref::<unityrs_core::Error>() {
        return unityrs_export::extractor::Error::Core(e.clone()).resource_refusal();
    }
    if let Some(e) = error.downcast_ref::<unityrs_export::extractor::Error>() {
        return e.resource_refusal();
    }
    error.source().is_some_and(resource_refusal)
}

fn extract(raw: &[Vec<u8>], arguments: &Value, config: &Config) -> Tool {
    let output = arguments
        .get("output")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if output.is_empty()
        || Path::new(output).is_absolute()
        || output.split('/').any(|c| c == "..")
        || output.as_bytes().contains(&0)
    {
        return failure("input", "Unsafe extraction subdirectory");
    }
    let operation = || -> Result<ChargedBytes, Box<dyn std::error::Error>> {
        let files = config.output.as_ref().ok_or("extract disabled")?;
        let doc =
            crate::extract::execute(raw, Some(&config.roots), Some((files, output.as_bytes())))?;
        Ok(doc)
    };
    match operation() {
        Ok(doc) => Tool {
            document: Document::Extract(doc),
            failed: false,
        },
        Err(e) => {
            if let Some(tpk) = e.downcast_ref::<unityrs_tpk::TpkError>() {
                let error = crate::agent::Error::tpk(tpk);
                return failure(error.code, &error.message);
            }
            failure(
                if resource_refusal(e.as_ref()) {
                    "budget"
                } else {
                    "input"
                },
                "Extraction failed under the configured limits and confinement",
            )
        }
    }
}
