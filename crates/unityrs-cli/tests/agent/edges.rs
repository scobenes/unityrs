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
    let repeats = json(
        &tmp,
        &["inspect", "synthetic.assets", "synthetic.assets"],
        0,
    );
    assert_eq!(repeats["total_objects"], 6);
    assert_eq!(repeats["objects"][3]["input"], 1);
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
