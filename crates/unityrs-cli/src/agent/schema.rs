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
fn tagged(tag: &str, field: &str, payload: Value) -> Value {
    let mut properties = serde_json::Map::new();
    properties.insert("t".into(), json!({"const":tag}));
    properties.insert(field.into(), payload);
    let mut value = object(&["t", field], Value::Object(properties));
    if let Some(map) = value.as_object_mut() {
        map.insert("additionalProperties".into(), json!(false));
    }
    value
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
fn value_schema(raw_string: &Value) -> Value {
    let string = json!({"type":"string"});
    let hex = json!({"type":"string","pattern":"^([0-9a-f]{2})*$"});
    let count = json!({"type":"integer","minimum":0});
    let mut dict = tagged("dict", "v", array(reference("pair")));
    if let Some(map) = dict.get_mut("properties").and_then(Value::as_object_mut) {
        map.insert("cut".into(), reference("cut"));
    }
    let mut map = tagged("map", "v", array(reference("map_pair")));
    if let Some(properties) = map.get_mut("properties").and_then(Value::as_object_mut) {
        properties.insert("cut".into(), reference("cut"));
    }
    let tuple = tagged("tuple", "v", array(reference("value")));
    let hashed = |kind| {
        let mut tag = object(
            &["t", "len", "sha256"],
            json!({"t":{"const":kind},"len":count,"sha256":{"type":"string","pattern":"^[0-9a-f]{64}$"}}),
        );
        if let Some(map) = tag.as_object_mut() {
            map.insert("additionalProperties".into(), json!(false));
        }
        tag
    };
    json!({"anyOf":[
        {"type":"boolean"}, {"type":"number"}, string.clone(),
        array(reference("value")),
        {"type":"object","propertyNames":{"not":{"enum":["t","$unityrs"]}},"additionalProperties":reference("value")},
        reference("cut"), reference("binary"), dict, map, tuple, raw_string,
        tagged("bytes", "hex", hex.clone()), tagged("u8s", "hex", hex.clone()),
        hashed("bytes"), hashed("u8s"), hashed("str"),
        tagged("f32s", "v", json!({"type":"string","pattern":"^([0-9a-f]{8})*$"})),
        tagged("f64s", "v", json!({"type":"string","pattern":"^([0-9a-f]{16})*$"})),
        tagged("f32", "v", json!({"type":"string","pattern":"^[0-9a-f]{8}$"})),
        tagged("f64", "v", json!({"type":"string","pattern":"^[0-9a-f]{16}$"})),
        tagged("i64", "v", json!({"type":"string","pattern":"^-?(0|[1-9][0-9]*)$","maxLength":20})),
        tagged("u64", "v", json!({"type":"string","pattern":"^(0|[1-9][0-9]*)$","maxLength":20}))
    ]})
}
pub fn run(name: &str) -> Result<Document> {
    let integer = json!({"type":"integer"});
    let string = json!({"type":"string"});
    let hex = json!({"type":"string","pattern":"^([0-9a-f]{2})*$"});
    let raw_string = tagged("str", "hex", hex.clone());
    let rawstr = json!({"anyOf":[string.clone(),raw_string.clone()]});
    let count = json!({"type":"integer","minimum":0});
    let nullable = |v: Value| json!({"anyOf":[{"type":"null"},v]});
    let header = object(
        &[
            "input",
            "file_index",
            "file",
            "path_id",
            "class_id",
            "class_name",
            "name",
            "container_path",
            "byte_size",
        ],
        json!({"input":count,"file_index":count,"file":reference("rawstr"),"path_id":integer,"class_id":integer,"class_name":string,"name":nullable(reference("rawstr")),"container_path":nullable(reference("rawstr")),"byte_size":integer}),
    );
    let preview = object(
        &["$unityrs", "length", "prefix", "omitted"],
        json!({"$unityrs":{"const":"preview"},"length":count,"prefix":reference("rawstr"),"omitted":{"type":"integer","minimum":1}}),
    );
    let listingstr = json!({"anyOf":[reference("rawstr"), reference("preview")]});
    let mut listing_header = header.clone();
    if let Some(props) = listing_header
        .get_mut("properties")
        .and_then(Value::as_object_mut)
    {
        props.insert("file".into(), reference("listingstr"));
        props.insert("name".into(), nullable(reference("listingstr")));
        props.insert("container_path".into(), nullable(reference("listingstr")));
    }
    let error = base(
        "error",
        &["code", "message"],
        json!({"code":{"enum":["usage","input","budget","not_found","ambiguous","bad_cursor"]},"message":string,"detail":{}}),
    );
    let metadata = object(
        &[
            "file",
            "file_index",
            "kind",
            "unity_version",
            "target_platform",
        ],
        json!({"file":reference("listingstr"),"file_index":count,"kind":string,"unity_version":nullable(reference("listingstr")),"target_platform":nullable(integer.clone()),"external_paths":{"type":"array","items":reference("listingstr"),"description":"Stored external paths in PPtr file-ID order, present when requested with --external-paths and nonempty."}}),
    );
    let cut = object(
        &["$unityrs", "reason", "omitted"],
        json!({"$unityrs":{"const":"cut"},"reason":{"enum":["bytes","items","depth"]},"omitted":{"type":"integer","minimum":1}}),
    );
    let binary = object(
        &["$unityrs", "kind", "length", "sha256", "prefix_hex"],
        json!({"$unityrs":{"const":"bytes"},"kind":{"enum":["bytes","u8s"]},"length":count,"sha256":{"type":"string","pattern":"^[0-9a-f]{64}$"},"prefix_hex":hex}),
    );
    let value = value_schema(&raw_string);
    let pair = json!({"type":"array","prefixItems":[reference("rawstr"),reference("value")],"minItems":2,"maxItems":2});
    let map_pair = json!({"type":"array","prefixItems":[reference("value"),reference("value")],"minItems":2,"maxItems":2});
    let mut schema = match name {
        "error" => error.clone(),
        "inspect" => base(
            "inspect",
            &["inputs", "objects", "total_objects", "next_cursor"],
            json!({
            "inputs":array(object(&["path"],json!({"path":reference("listingstr"),"files":array(reference("metadata")),"error":reference("error")}))),
            "objects":array(reference("listing_header")),"total_objects":count,"next_cursor":nullable(string.clone())}),
        ),
        "show" => base(
            "show",
            &["object", "value", "truncated"],
            json!({"object":reference("header"),"value":reference("value"),"truncated":object(&["cuts","bytes","items","depth","binary"],json!({"cuts":count,"bytes":count,"items":count,"depth":count,"binary":count}))}),
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
        map.insert("$defs".into(), json!({"rawstr":rawstr,"listingstr":listingstr,"preview":preview,"header":header,"listing_header":listing_header,"metadata":metadata,"error":error,"cut":cut,"binary":binary,"value":value,"pair":pair,"map_pair":map_pair}));
    }
    fixed(&schema)
}
