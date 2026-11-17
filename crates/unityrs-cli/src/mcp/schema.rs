//! Parser metadata is the source of option names, help, defaults and value forms.
use crate::agent::options::{self, OptionSpec};
use serde_json::{Map, Value, json};
pub fn specs(command: &str) -> Vec<OptionSpec> {
    let specs = if command == "extract" {
        crate::extract::options()
    } else {
        options::specs(command)
    };
    specs
        .into_iter()
        // MCP switches are booleans that only add a flag, so a false value
        // cannot undo a default: extract offers `no_container` alone, not the
        // CLI's `--use-container`/`--no-use-container` aliases.
        .filter(|s| {
            !matches!(
                s.name,
                "--help" | "-o" | "--use-container" | "--no-use-container"
            )
        })
        .collect()
}
pub fn key(name: &str) -> String {
    name.trim_start_matches('-').replace('-', "_")
}
fn property(spec: OptionSpec) -> Value {
    let mut schema = match spec.value {
        None => json!({"type":"boolean"}),
        Some("N") => {
            let (min, max) = match spec.name {
                "--path-id" | "--ignore-first-container-dirs" => (json!(i64::MIN), json!(i64::MAX)),
                "--limit" => (json!(1), json!(10_000)),
                "--max-depth" => (json!(0), json!(128)),
                "--max-tree-depth" => (json!(0), json!(u32::MAX)),
                _ => (json!(0), json!(u64::MAX)),
            };
            json!({"type":"integer","minimum":min,"maximum":max})
        }
        Some(form) if form.contains('|') => {
            json!({"type":"string","enum":form.split('|').collect::<Vec<_>>()})
        }
        _ => json!({"type":"string"}),
    };
    if matches!(spec.name, "--class" | "--name" | "--asset-filter") {
        schema = json!({"type":"array","items":schema});
    }
    if let Some(map) = schema.as_object_mut() {
        if spec.name == "--asset-filter" {
            map.insert("maxItems".into(), json!(1024));
        }
        let mut description = spec.help.to_owned();
        if let Some(default) = spec.default {
            description.push_str(" Default: ");
            description.push_str(default);
            description.push('.');
        }
        if matches!(
            spec.name,
            "--max-input-memory"
                | "--max-input-bytes"
                | "--max-decode-work"
                | "--max-tree-depth"
                | "--max-bytes"
                | "--max-items"
                | "--max-depth"
                | "--limit"
        ) {
            description.push_str(" Omitted requests use the smaller of this default and the server ceiling; explicit values cannot exceed that ceiling.");
        }
        map.insert("description".into(), json!(description));
        if let Some(default) = spec.default {
            let value = match schema_type(spec) {
                "integer" => default.parse::<u64>().ok().map(Value::from),
                "boolean" => default.parse::<bool>().ok().map(Value::from),
                _ => Some(json!(default)),
            };
            if let Some(value) = value {
                map.insert("default".into(), value);
            }
        }
    }
    schema
}
fn schema_type(spec: OptionSpec) -> &'static str {
    match spec.value {
        None => "boolean",
        Some("N") => "integer",
        _ => "string",
    }
}
pub fn input(command: &str) -> Value {
    let mut properties = Map::new();
    let mut required = Vec::new();
    if command == "batch_check" {
        properties.insert("job".into(), json!({"type":"object","description":"Job specification; validated by the batch --check parser and phase-one preparation without execution or file access."}));
        required.push("job");
    } else {
        if command != "describe" {
            properties.insert(
                "inputs".into(),
                json!({"type":"array","minItems":1,"items":{"type":"string"},"description":"Input file paths in caller order, inside the directories the server was started with; relative paths resolve from the server's working directory. Use the same order when reusing inspect addresses or cursors."}),
            );
            required.push("inputs");
        }
        for spec in specs(command) {
            properties.insert(key(spec.name), property(spec));
        }
        if command == "show" {
            required.push("path_id");
        }
        if command == "extract" {
            properties.insert("output".into(), json!({"type":"string","minLength":1,"description":"Relative subdirectory beneath the server --out root; at most 64 directory components and 4096 UTF-8 bytes; no symlinks or parent components."}));
            required.push("output");
        }
    }
    json!({"type":"object","properties":properties,"required":required,"additionalProperties":false})
}
fn agent_schema(name: &str) -> Value {
    crate::agent::execute(
        "describe",
        &[b"--schema".to_vec(), name.as_bytes().to_vec()],
    )
    .ok()
    .and_then(|d| serde_json::from_str(&d.json).ok())
    .unwrap_or_else(|| json!({"type":"object"}))
}
fn extract_schema() -> Value {
    let rawstr = agent_schema("show")
        .pointer("/$defs/rawstr")
        .cloned()
        .unwrap_or(Value::Null);
    let common = json!({"path":rawstr,"serialized_file":rawstr,"path_id":{"type":"integer"},"class_id":{"type":"integer"}});
    let entry = |extra: &str, schema: Value| {
        let mut props = common.as_object().cloned().unwrap_or_default();
        props.insert(extra.into(), schema);
        json!({"type":"object","properties":props,"required":["path","serialized_file","path_id","class_id",extra]})
    };
    json!({"type":"object","required":["written","failures"],"properties":{"written":{"type":"array","items":entry("size",json!({"type":"integer","minimum":0}))},"failures":{"type":"array","items":entry("reason",json!({"type":"string"}))}}})
}
pub fn output(command: &str) -> Value {
    let mut success = match command {
        "extract" => extract_schema(),
        "batch_check" => {
            json!({"type":"object","required":["type","code","detail"],"properties":{"type":{"const":"exit"},"code":{"const":0},"detail":{"type":"string"}}})
        }
        _ => agent_schema(command),
    };
    let mut error = agent_schema("error");
    let mut defs = Map::new();
    for schema in [&mut success, &mut error] {
        if let Some(map) = schema.as_object_mut() {
            if let Some(Value::Object(found)) = map.remove("$defs") {
                defs.extend(found);
            }
            map.remove("$schema");
            map.remove("$id");
        }
    }
    let mut variants = vec![success, error];
    if command == "describe" {
        variants.push(json!({"type":"object","required":["$schema"]}));
    }
    json!({"$schema":"https://json-schema.org/draft/2020-12/schema","type":"object","anyOf":variants,"$defs":defs})
}
pub fn tools(extract: bool) -> Vec<Value> {
    ["describe", "inspect", "show", "batch_check", "extract"]
        .into_iter()
        .filter(|c| *c != "extract" || extract)
        .map(|command| {
            let description = match command {
                "describe" => "Look up unityrs commands, options, default limits and output schemas when a field or error is unclear. Omit schema for the command reference, or select an output JSON Schema; request limits remain bounded by the server's startup ceilings.",
                "inspect" => "Start here to list what the input files contain, including each object's address (input, file_index, path_id), class, name and container path. Filter by class and name, and pass each non-null next_cursor back as cursor until next_cursor is null. Names and paths may be absent or shortened to previews; numeric addresses remain complete, and input failures are reported per input. Use these addresses with unityrs_show, or use unityrs_extract to export assets.",
                "show" => "Read one object's fields as JSON using its path_id from unityrs_inspect. If several files share that ID, pass file_index as INPUT:FILE from inspect's input and file_index fields to select the exact occurrence. Large values are cut and marked; raise max_bytes, max_items or max_depth within server ceilings, or use bytes: full for more natural-view binary data.",
                "extract" => "Write exportable assets (textures, audio, text, meshes, fonts and others) beneath the server's output root and return written files and per-asset failures. Use unityrs_inspect first to discover classes and container paths; asset_filter limits the classes to export. By default, as in UnityPy, only objects with a container path are exported, so an empty written list is expected for a file without container paths. Set no_container to true to consider every object for export by name; only supported types are exported unless export_unknown_as_typetree is enabled.",
                _ => "Validate a batch job document before running it elsewhere. Uses the batch check parser and phase-one preparation under server budgets, without executing the job or accessing its input and output files.",
            };
            json!({"name":format!("unityrs_{command}"),"description":description,"inputSchema":input(command),"outputSchema":output(command)})
        })
        .collect()
}
/// Only the small input-schema vocabulary above is accepted here. Semantic
/// errors (bad class, cursor, job, or file selector) remain operation failures.
pub fn valid(value: &Value, schema: &Value) -> bool {
    let kind = schema.get("type").and_then(Value::as_str);
    if !match kind {
        Some("object") => value.is_object(),
        Some("array") => value.is_array(),
        Some("string") => value.is_string(),
        Some("boolean") => value.is_boolean(),
        Some("integer") => super::integer::parse(value).is_some(),
        _ => true,
    } {
        return false;
    }
    if let Some(values) = schema.get("enum").and_then(Value::as_array)
        && !values.contains(value)
    {
        return false;
    }
    if let Some(n) = value.as_number() {
        // Compare integers exactly, including signed path IDs and u64 budgets.
        let integer = super::integer::parse(&Value::Number(n.clone()));
        for (key, lower) in [("minimum", true), ("maximum", false)] {
            let bound = schema.get(key).and_then(|v| {
                v.as_i64()
                    .map(i128::from)
                    .or_else(|| v.as_u64().map(i128::from))
            });
            if let (Some(n), Some(bound)) = (integer, bound)
                && ((lower && n < bound) || (!lower && n > bound))
            {
                return false;
            }
        }
    }
    if let Some(values) = value.as_array() {
        if schema
            .get("minItems")
            .and_then(Value::as_u64)
            .is_some_and(|min| (values.len() as u64) < min)
        {
            return false;
        }
        if let Some(items) = schema.get("items")
            && !values.iter().all(|v| valid(v, items))
        {
            return false;
        }
    }
    if let Some(text) = value.as_str()
        && schema
            .get("minLength")
            .and_then(Value::as_u64)
            .is_some_and(|n| (text.chars().count() as u64) < n)
    {
        return false;
    }
    if let Some(values) = value.as_object() {
        if let Some(required) = schema.get("required").and_then(Value::as_array)
            && required
                .iter()
                .any(|key| !key.as_str().is_some_and(|k| values.contains_key(k)))
        {
            return false;
        }
        if let Some(props) = schema.get("properties").and_then(Value::as_object) {
            for (key, value) in values {
                match props.get(key) {
                    Some(s) if !valid(value, s) => return false,
                    None if schema.get("additionalProperties") == Some(&Value::Bool(false)) => {
                        return false;
                    }
                    _ => {}
                }
            }
        }
    }
    true
}
