//! Output contracts use an intentionally small JSON Schema subset, checked
//! without introducing a validator dependency.
use super::{Document, Error, Result, fixed};
use serde_json::{Value, json};
fn object(required: &[&str], properties: Value) -> Value {
    let mut result = json!({"type":"object","required":required});
    if let Some(map) = result.as_object_mut() {
        map.insert("properties".into(), properties);
    }
    result
}
fn array(items: Value) -> Value {
    let mut result = json!({"type":"array"});
    if let Some(map) = result.as_object_mut() {
        map.insert("items".into(), items);
    }
    result
}
fn reference(name: &str) -> Value {
    json!({"$ref":format!("#/$defs/{name}")})
}
fn base(name: &str, required: &[&str], mut properties: Value) -> Value {
    if let Some(map) = properties.as_object_mut() {
        map.insert("format".into(), json!({"const":format!("unityrs.{name}")}));
        map.insert("version".into(), json!({"const":1}));
    }
    let mut keys = vec!["format", "version"];
    keys.extend_from_slice(required);
    object(&keys, properties)
}
pub fn run(name: &str) -> Result<Document> {
    let integer = json!({"type":"integer"});
    let string = json!({"type":"string"});
    let rawstr = json!({"anyOf":[{"type":"string"},{"type":"object","required":["t","hex"],"properties":{"t":{"const":"str"},"hex":{"type":"string"}}}]});
    let nullable = |v: Value| json!({"anyOf":[{"type":"null"},v]});
    let header = object(
        &[
            "input",
            "file",
            "path_id",
            "class_id",
            "class_name",
            "name",
            "container_path",
            "byte_size",
        ],
        json!({"input":integer,"file":reference("rawstr"),"path_id":integer,"class_id":integer,"class_name":string,"name":nullable(reference("rawstr")),"container_path":nullable(reference("rawstr")),"byte_size":integer}),
    );
    let error = base(
        "error",
        &["code", "message"],
        json!({"code":{"enum":["usage","input","budget","not_found","ambiguous","bad_cursor"]},"message":string,"detail":{}}),
    );
    let metadata = object(
        &["file", "kind", "unity_version", "target_platform"],
        json!({"file":reference("rawstr"),"kind":string,"unity_version":nullable(reference("rawstr")),"target_platform":nullable(integer.clone())}),
    );
    let cut = object(
        &["$unityrs", "reason", "omitted"],
        json!({"$unityrs":{"const":"cut"},"reason":{"enum":["bytes","items","depth"]},"omitted":integer}),
    );
    let binary = object(
        &["$unityrs", "kind", "length", "sha256", "prefix_hex"],
        json!({"$unityrs":{"const":"bytes"},"kind":{"enum":["bytes","u8s"]},"length":integer,"sha256":string,"prefix_hex":string}),
    );
    let value = json!({"anyOf":[{"type":"boolean"},{"type":"integer"},{"type":"string"},array(reference("value")),reference("cut"),reference("binary"),
        {"type":"object","required":["t"],"properties":{"t":{"enum":["dict","tuple","str","bytes","u8s","f32","f64","f32s","f64s"]},"v":{"anyOf":[{"type":"string"},array(json!({}))]},"hex":string,"len":integer,"sha256":string,"cut":reference("cut")}}]});
    let mut schema = match name {
        "error" => error.clone(),
        "inspect" => base(
            "inspect",
            &["inputs", "objects", "total_objects", "next_cursor"],
            json!({
            "inputs":array(object(&["path"],json!({"path":reference("rawstr"),"metadata":reference("metadata"),"files":array(reference("metadata")),"error":reference("error")}))),
            "objects":array(reference("header")),"total_objects":integer,"next_cursor":nullable(string.clone())}),
        ),
        "show" => base(
            "show",
            &["object", "value", "truncated"],
            json!({"object":reference("header"),"value":reference("value"),"truncated":object(&["cuts","bytes","items","depth","binary"],json!({"cuts":integer,"bytes":integer,"items":integer,"depth":integer,"binary":integer}))}),
        ),
        "describe" => base(
            "describe",
            &["commands", "exit_codes", "schemas", "batch_schema"],
            json!({
            "commands":array(object(&["name","purpose","usage","options","stdout","exit_codes"],json!({"name":string,"purpose":string,"usage":string,"stdout":string,"exit_codes":array(integer.clone()),"options":array(object(&["name","value","default","help"],json!({"name":string,"value":nullable(string.clone()),"default":nullable(string.clone()),"help":string})))}))),
            "exit_codes":{"type":"object"},"schemas":array(string.clone()),"batch_schema":string}),
        ),
        _ => {
            return Err(Error::usage(
                "unknown schema; expected inspect, show, describe or error",
            ));
        }
    };
    if let Some(map) = schema.as_object_mut() {
        map.insert(
            "$schema".into(),
            json!("https://json-schema.org/draft/2020-12/schema"),
        );
        map.insert("title".into(), json!(format!("unityrs.{name} version 1")));
        map.insert("$defs".into(), json!({"rawstr":rawstr,"header":header,"metadata":metadata,"error":error,"cut":cut,"binary":binary,"value":value}));
    }
    fixed(&schema)
}
