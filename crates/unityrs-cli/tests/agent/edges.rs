use super::support::{assert_cuts, fixture, json, run};
use unityrs_testkit::ngui::{self, Object, V, s, st, vector};
#[test]
fn byte_summary_fallback_and_raw_strings() {
    let tmp = fixture();
    let raw = Object::new(1, 114, V::Bytes((0..=255).cycle().take(4096).collect()));
    std::fs::write(
        tmp.join("raw.assets"),
        ngui::file(&[], &[raw], true).unwrap(),
    )
    .unwrap();
    let full = json(
        &tmp,
        &[
            "show",
            "raw.assets",
            "--path-id",
            "1",
            "--bytes",
            "full",
            "--max-bytes",
            "20000",
        ],
        0,
    );
    assert_eq!(full["value"]["hex"].as_str().unwrap().len(), 8192);
    assert_cuts(&full);
    let summary = json(
        &tmp,
        &[
            "show",
            "raw.assets",
            "--path-id",
            "1",
            "--bytes",
            "full",
            "--max-bytes",
            "900",
        ],
        0,
    );
    assert_eq!(summary["value"]["$unityrs"], "bytes");
    assert_eq!(summary["truncated"]["binary"], 1);
    assert_cuts(&summary);
    let raw_name = Object::new(
        2,
        49,
        st(
            "TextAsset",
            vec![
                ("m_Name", V::Str(vec![0xff, b'x'])),
                ("m_Script", s("text")),
            ],
        ),
    );
    std::fs::write(
        tmp.join("name.assets"),
        ngui::file(&[], &[raw_name], true).unwrap(),
    )
    .unwrap();
    let inspect = json(&tmp, &["inspect", "name.assets", "--name", "?x"], 0);
    assert_eq!(
        inspect["objects"][0]["name"],
        serde_json::json!({"t":"str","hex":"ff78"})
    );
    let show = json(&tmp, &["show", "name.assets", "--path-id", "2"], 0);
    assert_eq!(show["object"]["name"], inspect["objects"][0]["name"]);
    for cap in [1, 256, 400, 512, 900, 2000, 10000] {
        let output = run(
            &tmp,
            &[
                "show",
                "raw.assets",
                "--path-id",
                "1",
                "--max-bytes",
                &cap.to_string(),
            ],
        );
        let v: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        if output.status.success() {
            assert!(output.stdout.len() <= cap);
            assert_cuts(&v);
        } else {
            assert_eq!(v["code"], "budget");
        }
    }
}
#[test]
fn container_path_unnamed_objects_and_repeated_inputs() {
    let tmp = fixture();
    let pointer = |id| ngui::pptr("Object", (0, id));
    let info = st(
        "AssetInfo",
        vec![
            ("preloadIndex", V::Int(0)),
            ("preloadSize", V::Int(1)),
            ("asset", pointer(5)),
        ],
    );
    let shape = ngui::pair(s(""), info.clone());
    let bundle = Object::new(
        1,
        142,
        st(
            "AssetBundle",
            vec![
                ("m_Name", s("synthetic")),
                ("m_PreloadTable", vector(pointer(0), vec![pointer(5)])),
                (
                    "m_Container",
                    V::Vector(
                        "map".into(),
                        Box::new(shape),
                        vec![ngui::pair(s("assets/alpha.txt"), info)],
                    ),
                ),
            ],
        ),
    );
    let text = Object::new(
        5,
        49,
        st(
            "TextAsset",
            vec![("m_Name", s("alpha")), ("m_Script", s("text"))],
        ),
    );
    let unnamed = Object::new(
        8,
        2_000_000_000,
        st("Synthetic", vec![("value", V::Int(8))]),
    );
    std::fs::write(
        tmp.join("paths.assets"),
        ngui::file(&[], &[bundle, text, unnamed], true).unwrap(),
    )
    .unwrap();
    let v = json(&tmp, &["inspect", "paths.assets"], 0);
    assert_eq!(v["objects"][1]["container_path"], "assets/alpha.txt");
    assert_eq!(v["objects"][2]["name"], serde_json::Value::Null);
    assert_eq!(v["objects"][2]["class_name"], "Unknown");
    let s = json(&tmp, &["show", "paths.assets", "--path-id", "5"], 0);
    assert_eq!(s["object"], v["objects"][1]);
}

#[test]
fn repeated_input_addresses_select_the_exact_object() {
    let tmp = fixture();
    let repeats = json(
        &tmp,
        &["inspect", "synthetic.assets", "synthetic.assets"],
        0,
    );
    assert_eq!(repeats["total_objects"], 6);
    assert_eq!(repeats["objects"][3]["input"], 1);
    let selected = json(
        &tmp,
        &[
            "show",
            "synthetic.assets",
            "synthetic.assets",
            "--path-id",
            "-2",
            "--file-index",
            "1:0",
        ],
        0,
    );
    assert_eq!(selected["object"], repeats["objects"][3]);
    for selector in ["0:99", "2:0"] {
        let missing = json(
            &tmp,
            &[
                "show",
                "synthetic.assets",
                "--path-id",
                "-2",
                "--file-index",
                selector,
            ],
            1,
        );
        assert_eq!(missing["code"], "not_found");
    }
    assert_eq!(
        json(
            &tmp,
            &[
                "show",
                "synthetic.assets",
                "synthetic.assets",
                "--path-id",
                "-2"
            ],
            1
        )["code"],
        "ambiguous"
    );
}
#[test]
fn stripped_and_truncated_objects_stay_bounded() {
    let tmp = fixture();
    let stripped = Object::new(1, 49, st("TextAsset", vec![("m_Name", s("alpha"))]));
    std::fs::write(
        tmp.join("stripped.assets"),
        ngui::file(&[], &[stripped], false).unwrap(),
    )
    .unwrap();
    let v = json(&tmp, &["inspect", "stripped.assets"], 0);
    assert_eq!(v["total_objects"], 1);
    assert!(v["objects"][0]["name"].is_null());
    assert_eq!(
        json(&tmp, &["show", "stripped.assets", "--path-id", "1"], 1)["code"],
        "input"
    );
    std::fs::write(tmp.join("bad.tpk"), [0; 8]).unwrap();
    assert_eq!(
        json(
            &tmp,
            &[
                "show",
                "stripped.assets",
                "--path-id",
                "1",
                "--tpk",
                "bad.tpk"
            ],
            1
        )["code"],
        "input"
    );
    let broken = Object::new(2, 49, st("TextAsset", vec![("m_Name", s("alpha"))])).cut_to(2);
    std::fs::write(
        tmp.join("object.assets"),
        ngui::file(&[], &[broken], true).unwrap(),
    )
    .unwrap();
    assert_eq!(
        json(&tmp, &["show", "object.assets", "--path-id", "2"], 1)["code"],
        "input"
    );
    assert_eq!(
        json(&tmp, &["inspect", "object.assets"], 0)["total_objects"],
        1
    );
}

fn exceptional_object() -> Object {
    Object::new(
        1,
        114,
        st(
            "Exceptional",
            vec![
                ("wide_signed", V::Int64(i64::MIN)),
                ("wide_unsigned", V::UInt64(u64::MAX)),
                ("safe", V::Int64(9_007_199_254_740_992)),
                ("nan", V::Float(f32::from_bits(0x7fc0_0042))),
                ("infinity", V::Double(f64::INFINITY)),
                ("negative_zero", V::Double(-0.0)),
                (
                    "float_array",
                    vector(
                        V::Float(0.0),
                        vec![V::Float(1.5), V::Float(f32::NEG_INFINITY)],
                    ),
                ),
                (
                    "non_string_keys",
                    V::Vector(
                        "map".into(),
                        Box::new(ngui::pair(V::Int(0), s(""))),
                        vec![
                            ngui::pair(V::Int(1), s("one")),
                            ngui::pair(V::Int(1), s("again")),
                        ],
                    ),
                ),
                (
                    "collision",
                    st(
                        "Marker",
                        vec![
                            ("$unityrs", s("cut")),
                            ("reason", s("depth")),
                            ("omitted", V::Int(1)),
                        ],
                    ),
                ),
                (
                    "tag_collision",
                    st("Tag", vec![("t", s("dict")), ("v", s("data"))]),
                ),
            ],
        ),
    )
}

#[test]
fn natural_values_and_exact_exceptions() {
    let tmp = fixture();
    let rich = json(&tmp, &["show", "rich.assets", "--path-id", "7"], 0);
    assert_eq!(rich["value"]["m_Name"], "rich");
    assert_eq!(rich["value"]["list"], serde_json::json!([1, 2, 3]));
    assert_eq!(rich["value"]["nested"]["child"]["value"], 42);
    assert_eq!(rich["value"]["pair"], serde_json::json!([1, "two"]));
    assert_eq!(rich["value"]["float"], 1.5);
    let object = exceptional_object();
    std::fs::write(
        tmp.join("exceptions.assets"),
        ngui::file(&[], &[object], true).unwrap(),
    )
    .unwrap();
    let v = json(&tmp, &["show", "exceptions.assets", "--path-id", "1"], 0);
    let value = &v["value"];
    assert_eq!(
        value["wide_signed"],
        serde_json::json!({"t":"i64","v":"-9223372036854775808"})
    );
    assert_eq!(
        value["wide_unsigned"],
        serde_json::json!({"t":"u64","v":"18446744073709551615"})
    );
    assert_eq!(value["safe"], 9_007_199_254_740_992_i64);
    assert_eq!(value["nan"], serde_json::json!({"t":"f32","v":"7fc00042"}));
    assert_eq!(
        value["infinity"],
        serde_json::json!({"t":"f64","v":"7ff0000000000000"})
    );
    assert!(value["negative_zero"].as_f64().unwrap().is_sign_negative());
    assert_eq!(value["collision"]["t"], "dict");
    assert_eq!(value["tag_collision"]["t"], "dict");
    assert_eq!(
        value["float_array"],
        serde_json::json!([1.5,{"t":"f32","v":"ff800000"}])
    );
    assert_eq!(
        value["non_string_keys"],
        serde_json::json!({"t":"map","v":[[1, "one"], [1, "again"]]})
    );
    assert_cuts(&v);
    assert_eq!(v["truncated"]["cuts"], 0);
    let canonical = json(
        &tmp,
        &[
            "show",
            "rich.assets",
            "--path-id",
            "7",
            "--value",
            "canonical",
        ],
        0,
    );
    assert_eq!(canonical["value"]["t"], "dict");
    let canonical_arrays = json(
        &tmp,
        &[
            "show",
            "exceptions.assets",
            "--path-id",
            "1",
            "--value",
            "canonical",
        ],
        0,
    );
    let fields = canonical_arrays["value"]["v"].as_array().unwrap();
    let floats = fields.iter().find(|row| row[0] == "float_array").unwrap();
    assert_eq!(
        floats[1],
        serde_json::json!({"t":"f32s","v":"3fc00000ff800000"})
    );
    assert_eq!(
        canonical["value"]["v"][6][1],
        serde_json::json!({"t":"f32","v":"3fc00000"})
    );
}

#[test]
fn schema_rejects_malformed_recursive_values() {
    let tmp = fixture();
    let schema: serde_json::Value =
        serde_json::from_slice(&run(&tmp, &["describe", "--schema", "show"]).stdout).unwrap();
    let mut document = json(&tmp, &["show", "synthetic.assets", "--path-id", "-2"], 0);
    for bad in [
        serde_json::json!({"t":"dict"}),
        serde_json::json!({"t":"tuple"}),
        serde_json::json!({"t":"map"}),
        serde_json::json!({"t":"map","v":[[1]]}),
        serde_json::json!({"t":"map","v":[[1,2,3]]}),
        serde_json::json!({"t":"map","v":[[{"t":"bytes"},1]]}),
        serde_json::json!({"t":"map","v":[[1,{"t":"tuple"}]]}),
        serde_json::json!({"t":"f32","v":[]}),
        serde_json::json!({"t":"f32","v":"not_hex!"}),
        serde_json::json!({"t":"f64","v":"3ff00000"}),
        serde_json::json!({"t":"str","hex":"f"}),
        serde_json::json!({"t":"u64","v":"-1"}),
        serde_json::json!({"t":"i64","v":123}),
        serde_json::json!({"t":"bytes"}),
        serde_json::json!({"t":"dict","v":[["key"]]}),
        serde_json::json!({"t":"dict","v":[["key",1,2]]}),
        serde_json::json!({"t":"dict","v":[[false,1]]}),
        serde_json::json!({"t":"dict","v":[["key",{"t":"tuple","v":[{"t":"bytes"}]}]]}),
        serde_json::json!({"ordinary":[{"t":"str"}]}),
        serde_json::json!({"$unityrs":"cut","reason":"items","omitted":-1}),
        serde_json::json!({"$unityrs":"bytes","kind":"bytes","length":-1,"sha256":"","prefix_hex":""}),
    ] {
        document["value"] = bad.clone();
        assert!(
            !super::support::valid(&document, &schema, &schema),
            "accepted malformed value: {bad}"
        );
    }
}

fn unity_map(key: V, value: V, entries: Vec<V>) -> V {
    V::Vector("map".into(), Box::new(ngui::pair(key, value)), entries)
}
fn string_pairs() -> Vec<V> {
    vec![ngui::pair(s("z"), V::Int(1)), ngui::pair(s("a"), V::Int(2))]
}
fn logical_map_object() -> Object {
    let inner = unity_map(s(""), V::Int(0), vec![ngui::pair(s("inner"), V::Int(5))]);
    Object::new(
        1,
        114,
        st(
            "Maps",
            vec![
                ("unique", unity_map(s(""), V::Int(0), string_pairs())),
                (
                    "duplicate",
                    unity_map(
                        s(""),
                        V::Int(0),
                        vec![ngui::pair(s("x"), V::Int(1)), ngui::pair(s("x"), V::Int(2))],
                    ),
                ),
                (
                    "raw",
                    unity_map(
                        s(""),
                        V::Int(0),
                        vec![ngui::pair(V::Str(vec![0xff]), V::Int(3))],
                    ),
                ),
                (
                    "reserved",
                    unity_map(s(""), s(""), vec![ngui::pair(s("$unityrs"), s("cut"))]),
                ),
                (
                    "ordinary_pairs",
                    vector(ngui::pair(s(""), V::Int(0)), string_pairs()),
                ),
                (
                    "nested",
                    unity_map(
                        s(""),
                        inner.clone(),
                        vec![ngui::pair(s("outer"), inner.clone())],
                    ),
                ),
                ("array_maps", vector(inner.clone(), vec![inner])),
                ("empty", unity_map(s(""), V::Int(0), vec![])),
            ],
        ),
    )
}
#[test]
fn logical_maps_are_distinguished_from_pair_arrays() {
    let tmp = fixture();
    let maps = logical_map_object();
    std::fs::write(
        tmp.join("maps.assets"),
        ngui::file(&[], &[maps], true).unwrap(),
    )
    .unwrap();
    let doc = json(&tmp, &["show", "maps.assets", "--path-id", "1"], 0);
    let value = &doc["value"];
    assert_eq!(value["unique"], serde_json::json!({"z":1,"a":2}));
    assert_eq!(
        value["duplicate"],
        serde_json::json!({"t":"map","v":[["x",1],["x",2]]})
    );
    assert_eq!(
        value["raw"],
        serde_json::json!({"t":"map","v":[[{"t":"str","hex":"ff"},3]]})
    );
    assert_eq!(
        value["reserved"],
        serde_json::json!({"t":"map","v":[["$unityrs","cut"]]})
    );
    assert_eq!(
        value["ordinary_pairs"],
        serde_json::json!([["z", 1], ["a", 2]])
    );
    assert_eq!(value["nested"], serde_json::json!({"outer":{"inner":5}}));
    assert_eq!(value["array_maps"], serde_json::json!([{"inner":5}]));
    assert_eq!(value["empty"], serde_json::json!({}));
    assert_cuts(&doc);
    let output = run(&tmp, &["show", "maps.assets", "--path-id", "1"]);
    let output = String::from_utf8(output.stdout).unwrap();
    assert!(output.contains("\"unique\":{\"z\":1,\"a\":2}"), "{output}");
    let root = Object::new(2, 114, unity_map(s(""), V::Int(0), string_pairs()));
    std::fs::write(
        tmp.join("rootmap.assets"),
        ngui::file(&[], &[root], true).unwrap(),
    )
    .unwrap();
    let root = json(&tmp, &["show", "rootmap.assets", "--path-id", "2"], 0);
    assert_eq!(root["value"], serde_json::json!({"z":1,"a":2}));
    let clipped = json(
        &tmp,
        &[
            "show",
            "rootmap.assets",
            "--path-id",
            "2",
            "--max-items",
            "1",
        ],
        0,
    );
    assert_eq!(
        clipped["value"],
        serde_json::json!({"t":"map","v":[["z",1]],"cut":{"$unityrs":"cut","reason":"items","omitted":1}})
    );
    assert_cuts(&clipped);
    let canonical = json(
        &tmp,
        &[
            "show",
            "rootmap.assets",
            "--path-id",
            "2",
            "--value",
            "canonical",
        ],
        0,
    );
    assert_eq!(
        canonical["value"],
        serde_json::json!([{"t":"tuple","v":["z",1]},{"t":"tuple","v":["a",2]}])
    );
}
