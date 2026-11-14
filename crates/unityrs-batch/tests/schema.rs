//! `docs/batch/spec-v1.schema.json` and `docs/batch/spec-v2.schema.json`,
//! written by hand, equal the parser's field tables of their versions
//! (`unityrs_batch::spec::fields`): each object's properties,
//! required members and `additionalProperties: false`, each value's type,
//! nullability, bounds, closed values, patterns and formats, each array's
//! counts and uniqueness, and every default section 1 states as a value
//! (`docs/plans/extract.md` section 3, item 4).
//!
//! The table is rendered to the schema it implies, the file's `$ref`s are
//! inlined and its annotations (`description`, `title`, `$schema`, `$id`)
//! dropped, and the two must be equal.

use std::path::Path;

use serde_json::{Map, Value, json};
use unityrs_batch::spec::fields::{
    self, Array, DIRECTORY, ENV, Field, Format, HEADER_VALUE_PATTERN, MAX_HEADER_VALUE,
    MAX_HEADERS, OBJECT_STORE, PREFER, REFUSED_HEADERS, Str, TOKEN_PATTERN, Ty,
};

fn object(table: &[Field]) -> Value {
    let mut props = Map::new();
    let mut required: Vec<&str> = Vec::new();
    for f in table {
        let mut v = ty(&f.ty);
        if f.nullable {
            let t = v["type"]
                .as_str()
                .expect("a nullable field has a plain type")
                .to_owned();
            v["type"] = json!([t, "null"]);
        }
        if let Some(d) = f.default {
            v["default"] = serde_json::from_str(d).expect("a default is JSON");
        }
        if f.required {
            required.push(f.name);
        }
        props.insert(f.name.to_owned(), v);
    }
    required.sort_unstable();
    json!({
        "type": "object",
        "properties": props,
        "required": required,
        "additionalProperties": false,
    })
}

fn string(s: &Str) -> Value {
    let mut v = json!({"type": "string"});
    if s.min > 0 {
        v["minLength"] = json!(s.min);
    }
    if let Some(m) = s.max {
        v["maxLength"] = json!(m);
    }
    if !s.values.is_empty() {
        v["enum"] = json!(s.values);
    }
    match s.format {
        Some(Format::DateTime) => v["format"] = json!("date-time"),
        Some(f @ (Format::Url | Format::Endpoint)) => {
            v["format"] = json!("uri");
            v["maxLength"] = json!(f.written_cap().expect("a cap"));
            v["pattern"] = json!(f.pattern().expect("a pattern"));
        }
        Some(f) => v["pattern"] = json!(f.pattern().expect("a pattern")),
        None => {}
    }
    v
}

fn array(a: &Array) -> Value {
    let mut v = json!({"type": "array", "items": ty(&a.item)});
    if a.min > 0 {
        v["minItems"] = json!(a.min);
    }
    if let Some(m) = a.max {
        v["maxItems"] = json!(m);
    }
    if a.unique {
        v["uniqueItems"] = json!(true);
    }
    v
}

/// `^(...)$` matching each refused name in any case, as ECMA-262 (no
/// inline flags) writes it.
fn refused_pattern() -> String {
    let names: Vec<String> = REFUSED_HEADERS
        .iter()
        .map(|n| {
            n.chars()
                .map(|c| {
                    if c.is_ascii_alphabetic() {
                        format!("[{}{}]", c.to_ascii_uppercase(), c.to_ascii_lowercase())
                    } else {
                        c.to_string()
                    }
                })
                .collect()
        })
        .collect();
    format!("^({})$", names.join("|"))
}

fn ty(t: &Ty) -> Value {
    match t {
        Ty::Bool => json!({"type": "boolean"}),
        Ty::Int { min, max } => json!({"type": "integer", "minimum": min, "maximum": max}),
        Ty::Str(s) => string(s),
        Ty::Object(table) => object(table),
        Ty::Array(a) | Ty::Bundles(a, _) => array(a),
        Ty::Deps => json!({"type": "array", "items": {"type": "string"}}),
        Ty::Prefer => json!({"type": "object", "additionalProperties": object(&PREFER)}),
        Ty::Headers => json!({
            "type": "object",
            "maxProperties": MAX_HEADERS,
            "propertyNames": {
                "pattern": TOKEN_PATTERN,
                "not": {"pattern": refused_pattern()},
            },
            "additionalProperties": {
                "anyOf": [
                    {"type": "string", "maxLength": MAX_HEADER_VALUE, "pattern": HEADER_VALUE_PATTERN},
                    object(&ENV),
                ]
            },
        }),
        Ty::Sink => json!({"oneOf": [object(&DIRECTORY), object(&OBJECT_STORE)]}),
        Ty::TextureFormat => json!({
            "type": "string",
            "enum": fields::texture_format_names().collect::<Vec<_>>(),
        }),
    }
}

/// Inlines every `$ref` (its siblings merged in) and drops annotations.
fn normalise(v: &Value, defs: &Map<String, Value>) -> Value {
    match v {
        Value::Object(m) => {
            let mut out = Map::new();
            if let Some(r) = m.get("$ref").and_then(Value::as_str) {
                let name = r.strip_prefix("#/$defs/").expect("a local ref");
                let target = normalise(&defs[name], defs);
                out.clone_from(target.as_object().expect("an object"));
            }
            for (k, x) in m {
                if matches!(
                    k.as_str(),
                    "$ref" | "description" | "title" | "$schema" | "$id" | "$defs"
                ) {
                    continue;
                }
                let x = normalise(x, defs);
                let x = match (k.as_str(), x) {
                    ("required", Value::Array(mut a)) => {
                        a.sort_by(|p, q| p.as_str().cmp(&q.as_str()));
                        Value::Array(a)
                    }
                    (_, x) => x,
                };
                out.insert(k.clone(), x);
            }
            Value::Object(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(|x| normalise(x, defs)).collect()),
        x => x.clone(),
    }
}

fn schema(version: u64) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../../docs/batch/spec-v{version}.schema.json"));
    let text = std::fs::read_to_string(path).expect("the schema file");
    serde_json::from_str(&text).expect("the schema is JSON")
}

/// The first place two values differ, for a readable failure.
fn first_difference(want: &Value, got: &Value, at: &str) -> Option<String> {
    match (want, got) {
        (Value::Object(wants), Value::Object(gots)) => {
            for name in wants.keys().chain(gots.keys()) {
                match (wants.get(name), gots.get(name)) {
                    (Some(w), Some(g)) => {
                        if let Some(d) = first_difference(w, g, &format!("{at}/{name}")) {
                            return Some(d);
                        }
                    }
                    (w, g) => return Some(format!("{at}/{name}: {w:?} vs {g:?}")),
                }
            }
            None
        }
        (Value::Array(wants), Value::Array(gots)) if wants.len() == gots.len() => wants
            .iter()
            .zip(gots)
            .enumerate()
            .find_map(|(i, (w, g))| first_difference(w, g, &format!("{at}/{i}"))),
        _ if want == got => None,
        _ => Some(format!("{at}: {want} vs {got}")),
    }
}

#[test]
fn each_schema_equals_its_field_table() {
    for version in 1..=unityrs_batch::spec::SPEC_VERSION {
        let file = schema(version);
        assert_eq!(
            file["$schema"],
            json!("https://json-schema.org/draft/2020-12/schema")
        );
        let defs = file["$defs"].as_object().expect("$defs").clone();
        let got = normalise(&file, &defs);
        let want = object(fields::top(version));
        if let Some(d) = first_difference(&want, &got, "") {
            panic!("version {version}'s schema and table differ at {d}");
        }
        assert_eq!(want, got);
    }
}

/// `--schema` prints the highest version's file.
#[test]
fn the_embedded_schema_is_the_highest_versions() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
        "../../docs/batch/spec-v{}.schema.json",
        unityrs_batch::spec::SPEC_VERSION
    ));
    assert_eq!(std::fs::read(path).unwrap(), unityrs_batch::spec::SCHEMA);
}

/// Version 2's tables are version 1's but for what it adds.
#[test]
fn version_2_adds_three_members_and_changes_none() {
    let (v1, v2) = (object(&fields::TOP), object(&fields::TOP_V2));
    let mut v2_less = v2.clone();
    let props = v2_less["properties"].as_object_mut().unwrap();
    props.remove("unitycn_key");
    props["spec_version"] = v1["properties"]["spec_version"].clone();
    let outputs = props["outputs"]["properties"].as_object_mut().unwrap();
    outputs.remove("formats");
    outputs.remove("image");
    assert_eq!(v1, v2_less);
}

#[test]
fn every_def_is_used() {
    for version in 1..=unityrs_batch::spec::SPEC_VERSION {
        let file = schema(version);
        let text = file.to_string();
        for name in file["$defs"].as_object().expect("$defs").keys() {
            assert!(
                text.contains(&format!("\"#/$defs/{name}\"")),
                "{name} is never referenced"
            );
        }
    }
}
