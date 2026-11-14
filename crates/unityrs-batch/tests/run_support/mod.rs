#![allow(dead_code)]
mod accounting;
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, path::Path};
use unityrs_batch::{
    digest::{Hashes, Hex},
    manifest::Family,
};
use unityrs_testkit::{
    ngui::{self, AtlasScene},
    scratch::Scratch,
};
pub type Checked = (Value, BTreeMap<String, Vec<u8>>);
pub fn cases() -> Vec<Value> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/specs/v1");
    let mut files: Vec<_> = fs::read_dir(root)
        .unwrap()
        .map(|p| p.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    files.sort();
    files
        .iter()
        .map(|p| serde_json::from_slice(&fs::read(p).unwrap()).unwrap())
        .collect()
}
pub fn scene(case: &Value) -> AtlasScene {
    if case["fixture"] == "textures:five"
        || case["fixture"].as_str().unwrap().starts_with("synthetic:")
    {
        return textures(case);
    }
    let (family, name) = case["fixture"].as_str().unwrap().split_once(':').unwrap();
    let scenes = if family == "ngui" {
        ngui::scenes().unwrap()
    } else {
        ngui::sprite_scenes().unwrap()
    };
    scenes.into_iter().find(|(n, _)| *n == name).unwrap().1
}
pub fn instantiate(case: &Value, scene: &AtlasScene, scratch: &Scratch) -> (Value, Value) {
    let mut tokens = BTreeMap::new();
    tokens.insert("$SPOOL".into(), json!(scratch.join("spool")));
    tokens.insert("$SINK".into(), json!(scratch.join("sink")));
    for (index, bundle) in scene.bundles.iter().enumerate() {
        let path = scratch.join(format!("source-{index}"));
        if !case["missing_sources"].as_array().is_some_and(|sources| {
            sources
                .iter()
                .any(|s| s.as_str() == Some(bundle.id.as_str()))
        }) {
            fs::write(&path, &bundle.data).unwrap();
        }
        tokens.insert(format!("$SOURCE:{}", bundle.id), json!(path));
        tokens.insert(
            format!("$BUNDLE_SIZE:{}", bundle.id),
            json!(bundle.data.len()),
        );
        tokens.insert(
            format!("$BUNDLE_SHA256:{}", bundle.id),
            json!(format!(
                "{}",
                Hex(&unityrs_batch::digest::sha256(&bundle.data))
            )),
        );
    }
    let mut spec = case["spec"].clone();
    let mut expected = case["normalized"].clone();
    substitute(&mut spec, &tokens);
    substitute(&mut expected, &tokens);
    (spec, expected)
}
fn substitute(value: &mut Value, tokens: &BTreeMap<String, Value>) {
    match value {
        Value::String(text) => {
            if let Some(replacement) = tokens.get(text) {
                *value = replacement.clone();
            }
        }
        Value::Array(values) => {
            for v in values {
                substitute(v, tokens);
            }
        }
        Value::Object(fields) => {
            for v in fields.values_mut() {
                substitute(v, tokens);
            }
        }
        _ => {}
    }
}
pub fn check(
    scratch: &Scratch,
    spec: &Value,
    case: &Value,
    mut expected: Value,
    memory_bound: u64,
) -> Checked {
    let sink = scratch.join("sink");
    let head_path = sink.join(format!(
        "_unityrs/{}/manifest.json",
        spec["run_id"].as_str().unwrap()
    ));
    let head: Value = serde_json::from_slice(&fs::read(head_path).unwrap()).unwrap();
    validate_head(&head, spec);
    assert_eq!(head["run"]["memory_bound"], memory_bound);
    let mut rows = serde_json::Map::new();
    for family in Family::ALL {
        let mut collected = Vec::<Value>::new();
        for chunk in 0..head["rows"][family.name()].as_u64().unwrap() {
            let path = sink.join(format!(
                "_unityrs/{}/{}/{}.{chunk}.ndjson",
                spec["run_id"].as_str().unwrap(),
                spec["attempt"].as_u64().unwrap(),
                family.name()
            ));
            let bytes = fs::read(path).unwrap();
            assert!(bytes.len() <= 16 * 1024 * 1024);
            assert_eq!(bytes.last(), Some(&b'\n'));
            for line in bytes.split_inclusive(|b| *b == b'\n') {
                assert!(line.len() <= 32 * 1024);
                let row: Value = serde_json::from_slice(line).unwrap();
                collected.push(row);
            }
        }
        rows.insert(family.name().into(), json!(collected));
    }
    accounting::validate(&head, &rows, &expected, spec, case);
    if case["name"] == "textures-five" && spec["attempt"] == 1 {
        accounting::reject_corruption(&head, &rows, &expected, spec, case);
    }
    for family in rows.values_mut() {
        for row in family.as_array_mut().unwrap() {
            normalize_row(row);
        }
    }
    let mut outputs = BTreeMap::new();
    let mut tokens = BTreeMap::new();
    for (key, pinned) in case["outputs"].as_object().unwrap() {
        let bytes = fs::read(sink.join(key)).unwrap();
        assert_eq!(
            format!("{}", Hex(&Hashes::of(&bytes).md5)),
            pinned["md5"].as_str().unwrap(),
            "{} {key}",
            case["name"]
        );
        if let Some(hex) = pinned["hex"].as_str() {
            assert_eq!(format!("{}", Hex(&bytes)), hex);
        }
        if let Some(text) = pinned["text"].as_str() {
            assert_eq!(bytes, text.as_bytes());
        }
        tokens.insert(format!("$OUTPUT_SIZE:{key}"), json!(bytes.len()));
        outputs.insert(key.clone(), bytes);
    }
    assert_eq!(
        rows["outputs"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["status"] == "published")
            .count(),
        outputs.len()
    );
    let mut physical = Vec::new();
    physical_keys(&sink, "", &mut physical);
    physical.sort();
    assert_eq!(physical, outputs.keys().cloned().collect::<Vec<_>>());
    substitute(&mut expected, &tokens);
    let actual = json!({"head":normalize_head(head),"rows":rows});
    compare(&actual, &expected, case["name"].as_str().unwrap());
    (actual, outputs)
}
fn normalize_row(row: &mut Value) {
    let fields = row.as_object_mut().unwrap();
    fields.retain(|name, _| {
        !name.ends_with("_peak") && !["ms", "export_work", "peek_work"].contains(&name.as_str())
    });
    if fields
        .get("status")
        .and_then(Value::as_str)
        .is_some_and(|s| s == "written" || s == "present")
    {
        fields.insert("status".into(), "published".into());
    }
}

pub fn assert_raw_publication(
    scratch: &Scratch,
    spec: &Value,
    count: usize,
    rewritten: Option<&str>,
) {
    let root = scratch.join(format!(
        "sink/_unityrs/{}",
        spec["run_id"].as_str().unwrap()
    ));
    let head: Value =
        serde_json::from_slice(&fs::read(root.join("manifest.json")).unwrap()).unwrap();
    let resume = spec["resume"] == true;
    let written = if resume {
        usize::from(rewritten.is_some())
    } else {
        count
    };
    assert_eq!(head["counts"]["outputs_present"], count - written);
    assert_eq!(head["counts"]["outputs_written"], written);
    let mut published = 0;
    for chunk in 0..head["rows"]["outputs"].as_u64().unwrap() {
        let bytes =
            fs::read(root.join(format!("{}/outputs.{chunk}.ndjson", spec["attempt"]))).unwrap();
        for line in bytes.split(|b| *b == b'\n').filter(|line| !line.is_empty()) {
            let row: Value = serde_json::from_slice(line).unwrap();
            if row["status"] == "written" || row["status"] == "present" {
                let status = if resume && rewritten != row["key"].as_str() {
                    "present"
                } else {
                    "written"
                };
                assert_eq!(
                    row["status"], status,
                    "raw resume outcome for {}",
                    row["key"]
                );
                published += 1;
            }
        }
    }
    assert_eq!(published, count);
}

fn textures(case: &Value) -> AtlasScene {
    let objects: Vec<_> = case["texture_inputs"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .map(|(index, input)| {
            let mut rng = unityrs_testkit::Rng::new(input["data"]["seed"].as_u64().unwrap_or(0));
            let len = usize::try_from(
                input["data"]["len"]
                    .as_u64()
                    .or_else(|| input["data"]["zeros"].as_u64())
                    .unwrap(),
            )
            .unwrap();
            let mut bytes = Vec::new();
            while bytes.len() < len {
                bytes.extend_from_slice(&rng.next_u64().to_le_bytes());
            }
            bytes.truncate(len);
            if input["data"]["zeros"].is_u64() {
                bytes.fill(0);
            }
            if let Some(pinned) = input["source_sha256"].as_str() {
                assert_eq!(
                    format!("{}", Hex(&unityrs_batch::digest::sha256(&bytes))),
                    pinned
                );
            }
            ngui::texture(
                i64::try_from(index + 1).unwrap(),
                input["name"].as_str().unwrap().as_bytes(),
                i32::try_from(input["format"].as_i64().unwrap()).unwrap(),
                input["size"].as_array().map_or((1, 1), |s| {
                    (
                        i32::try_from(s[0].as_i64().unwrap()).unwrap(),
                        i32::try_from(s[1].as_i64().unwrap()).unwrap(),
                    )
                }),
                bytes,
            )
        })
        .collect();
    let file = ngui::file(&[], &objects, true).unwrap();
    let serialized_name = case["serialized_name"].as_str().unwrap_or("CAB-five");
    let body = if case["fixture"] == "synthetic:nested-repeated" {
        let hidden = ngui::file(
            &[],
            &[ngui::texture(1, b"hidden", 4, (1, 1), vec![1, 2, 3, 255])],
            true,
        )
        .unwrap();
        unityrs_testkit::env::bundle(&[
            ("CAB-five", hidden.clone()),
            ("CAB-five", file.clone()),
            (
                "nested",
                unityrs_testkit::env::bundle(&[("CAB-child", hidden)]),
            ),
        ])
    } else {
        unityrs_testkit::env::bundle(&[(serialized_name, file.clone())])
    };
    let bundles = case["spec"]["inputs"]
        .as_array()
        .unwrap()
        .iter()
        .chain(case["spec"]["dependencies"].as_array().unwrap())
        .map(|bundle| ngui::SceneBundle {
            id: bundle["id"].as_str().unwrap().into(),
            data: body.clone(),
            deps: vec![],
        })
        .collect();
    AtlasScene {
        bundles,
        inputs: case["spec"]["inputs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b["id"].as_str().unwrap().into())
            .collect(),
        options: ngui::CutOptions::textures("{name}.png"),
    }
}

fn validate_head(head: &Value, spec: &Value) {
    assert_eq!(head["manifest_version"], 1);
    assert_eq!(head["run_id"], spec["run_id"]);
    assert_eq!(head["spec_version"], 1);
    assert!(head["binary_version"].is_string());
    assert!(head["deadline"].is_null());
    assert!(head["stop_detail"].is_null());
    for (name, value) in head["run"].as_object().unwrap() {
        if name == "spool" {
            assert_eq!(value["size_bytes"], spec["spool"]["size_bytes"]);
            assert_eq!(value["reserve_bytes"], spec["spool"]["reserve_bytes"]);
        } else {
            assert!(value.is_u64(), "run/{name}");
        }
    }
    assert_eq!(head["write_seq"], 1);
    assert_eq!(head["attempt"], spec["attempt"]);
    assert_eq!(head["phase"], "publish");
    assert!(head["verified"].is_null());
    assert_eq!(head["verified_by_size"], 0);
    assert_eq!(head["unsettled_tries"], 0);
    assert_eq!(head["rows"].as_object().unwrap().len(), 8);
    assert_eq!(head["counts"].as_object().unwrap().len(), 19);
    assert_eq!(head["warnings"].as_object().unwrap().len(), 6);
    let parsed = unityrs_batch::spec::parse(serde_json::to_vec(spec).unwrap()).unwrap();
    assert_eq!(head["spec_sha256"], format!("{}", Hex(&parsed.spec_sha256)));
    for field in [
        "max_inflight_uncompressed_bytes",
        "max_decoded_bytes",
        "dep_cache_bytes",
        "max_attempt_evictions",
        "max_export_work",
    ] {
        assert!(head["run"][field].is_u64());
    }
    assert!(head["run"]["spool_peak"].as_u64().unwrap() <= parsed.spec.spool.size_bytes);
    assert!(head["run"]["memory_bound"].is_u64());
    for field in ["started_at", "ended_at"] {
        assert!(head[field].as_str().unwrap().ends_with('Z'));
    }
}

fn physical_keys(root: &Path, prefix: &str, keys: &mut Vec<String>) {
    for entry in fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_str().unwrap().to_owned();
        if (prefix.is_empty() && name == "_unityrs") || name.starts_with(".unityrs-tmp-") {
            continue;
        }
        let key = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        if entry.file_type().unwrap().is_dir() {
            physical_keys(&entry.path(), &key, keys);
        } else {
            keys.push(key);
        }
    }
}

fn compare(actual: &Value, expected: &Value, path: &str) {
    match (actual, expected) {
        (Value::Object(a), Value::Object(b)) => {
            assert_eq!(
                a.keys().collect::<Vec<_>>(),
                b.keys().collect::<Vec<_>>(),
                "{path}"
            );
            for (key, value) in b {
                compare(&a[key], value, &format!("{path}/{key}"));
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            assert_eq!(a.len(), b.len(), "{path}");
            for (index, (a, b)) in a.iter().zip(b).enumerate() {
                compare(a, b, &format!("{path}/{index}"));
            }
        }
        _ => assert_eq!(actual, expected, "{path}"),
    }
}

fn normalize_head(mut head: Value) -> Value {
    let fields = head.as_object_mut().unwrap();
    fields.retain(|name, _| {
        [
            "status",
            "exit_code",
            "stop_reason",
            "counts",
            "warnings",
            "run",
        ]
        .contains(&name.as_str())
    });
    for (written, present) in [
        ("outputs_written", "outputs_present"),
        ("archives_written", "archives_present"),
    ] {
        let counts = fields.get_mut("counts").unwrap().as_object_mut().unwrap();
        counts.insert(
            written.into(),
            json!(counts[written].as_u64().unwrap() + counts[present].as_u64().unwrap()),
        );
        counts.remove(present);
    }
    fields
        .get_mut("warnings")
        .unwrap()
        .as_object_mut()
        .unwrap()
        .retain(|name, _| !["stderr_dropped", "stdin_watch_ended"].contains(&name.as_str()));
    fields
        .get_mut("run")
        .unwrap()
        .as_object_mut()
        .unwrap()
        .retain(|name, _| {
            [
                "same_named_cabs",
                "undeclared_dependencies",
                "nested_containers",
            ]
            .contains(&name.as_str())
        });
    head
}
