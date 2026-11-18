use super::support::{fixture, json};
use unityrs_testkit::ngui::{self, Object, s, st};

#[test]
fn cursor_binds_input_roles_and_bytes() {
    let tmp = fixture();
    let first = json(
        &tmp,
        &[
            "inspect",
            "synthetic.assets",
            "--class",
            "49",
            "--limit",
            "1",
        ],
        0,
    );
    let cursor = first["next_cursor"].as_str().unwrap();
    let replay = json(
        &tmp,
        &[
            "inspect",
            "--cursor",
            cursor,
            "--",
            "synthetic.assets",
            "--class",
            "49",
        ],
        1,
    );
    assert_eq!(replay["code"], "bad_cursor");
    assert!(
        replay["message"]
            .as_str()
            .unwrap()
            .contains("inputs changed")
    );
    let replacement = ngui::file(
        &[],
        &[
            Object::new(-2, 49, st("TextAsset", vec![("m_Name", s("replacement"))])),
            Object::new(10, 49, st("TextAsset", vec![("m_Name", s("second"))])),
        ],
        true,
    )
    .unwrap();
    std::fs::write(tmp.join("synthetic.assets"), replacement).unwrap();
    let replay = json(
        &tmp,
        &[
            "inspect",
            "synthetic.assets",
            "--class",
            "49",
            "--cursor",
            cursor,
        ],
        1,
    );
    assert_eq!(replay["code"], "bad_cursor");
    assert!(
        replay["message"]
            .as_str()
            .unwrap()
            .contains("inputs changed")
    );
}

#[test]
fn cursor_replayed_from_another_directory_checks_contents() {
    let first_dir = fixture();
    let second_dir = fixture();
    let page = json(
        &first_dir,
        &["inspect", "synthetic.assets", "--limit", "1"],
        0,
    );
    let cursor = page["next_cursor"].as_str().unwrap();
    std::fs::copy(
        second_dir.join("rich.assets"),
        second_dir.join("synthetic.assets"),
    )
    .unwrap();
    let replay = json(
        &second_dir,
        &["inspect", "synthetic.assets", "--cursor", cursor],
        1,
    );
    assert_eq!(replay["code"], "bad_cursor");
    assert!(
        replay["message"]
            .as_str()
            .unwrap()
            .contains("inputs changed")
    );
}

#[test]
fn input_errors_identify_argument_and_index() {
    let tmp = fixture();
    let page = json(&tmp, &["inspect", "synthetic.assets", "broken.bundle"], 0);
    let e = &page["inputs"][1]["error"];
    assert!(e["message"].as_str().unwrap().contains("broken.bundle"));
    assert_eq!(e["detail"]["input"], 1);
    let e = json(&tmp, &["show", "broken.bundle", "--path-id", "1"], 1);
    assert!(e["message"].as_str().unwrap().contains("broken.bundle"));
    assert_eq!(e["detail"]["input"], 0);
}

#[test]
fn inspect_has_one_metadata_listing() {
    let tmp = fixture();
    let page = json(&tmp, &["inspect", "synthetic.assets"], 0);
    assert!(page["inputs"][0].get("metadata").is_none());
    assert_eq!(page["inputs"][0]["files"].as_array().unwrap().len(), 1);
}

#[test]
fn inspect_lists_external_paths_in_file_id_order() {
    let tmp = fixture();
    let paths = [
        "archive:/cab/cab.sharedasset",
        "resources/unity_builtin_extra",
    ];
    let file = unityrs_testkit::env::serialized(&paths, &[]);
    std::fs::write(tmp.join("scene.assets"), file).unwrap();
    let page = json(&tmp, &["inspect", "scene.assets"], 0);
    assert_eq!(
        page["inputs"][0]["files"][0]["external_paths"],
        serde_json::json!(paths)
    );
}

#[test]
fn large_name_matching_refuses_tight_memory_budget_without_aborting() {
    let tmp = fixture();
    let bytes = ngui::file(
        &[],
        &[Object::new(
            7,
            49,
            st("TextAsset", vec![("m_Name", s(&"x".repeat(1 << 20)))]),
        )],
        true,
    )
    .unwrap();
    std::fs::write(tmp.join("large.assets"), bytes).unwrap();
    let page = json(
        &tmp,
        &[
            "inspect",
            "large.assets",
            "--name",
            "*",
            "--limit",
            "1",
            "--max-input-memory",
            "4500000",
        ],
        0,
    );
    assert_eq!(page["inputs"][0]["error"]["code"], "budget");
}

#[test]
fn nested_files_with_the_same_name_and_path_ids_page_once() {
    use std::collections::BTreeSet;
    use unityrs_testkit::bundle::{self, BundleSpec, EntrySpec};

    let tmp = fixture();
    let inner = bundle::build(&BundleSpec {
        entries: vec![EntrySpec {
            name: b"CAB-a".to_vec(),
            data: std::fs::read(tmp.join("synthetic.assets")).unwrap(),
            flags: 4,
        }],
        ..BundleSpec::default()
    });
    let outer = bundle::build(&BundleSpec {
        entries: [b"left.bundle".as_slice(), b"right.bundle".as_slice()]
            .into_iter()
            .map(|name| EntrySpec {
                name: name.to_vec(),
                data: inner.clone(),
                flags: 4,
            })
            .collect(),
        ..BundleSpec::default()
    });
    std::fs::write(tmp.join("nested.bundle"), outer).unwrap();
    let mut cursor: Option<String> = None;
    let mut tokens = BTreeSet::new();
    let mut ids = Vec::new();
    for _ in 0..6 {
        let mut args = vec!["inspect", "nested.bundle", "--limit", "1"];
        if let Some(token) = &cursor {
            args.extend(["--cursor", token]);
        }
        let page = json(&tmp, &args, 0);
        assert_eq!(page["total_objects"], 6);
        let objects = page["objects"].as_array().unwrap();
        assert_eq!(objects.len(), 1);
        assert_eq!(objects[0]["file"], "CAB-a");
        let file_index = objects[0]["file_index"].as_u64().unwrap();
        let address = format!("0:{file_index}");
        let path_id = objects[0]["path_id"].to_string();
        let shown = json(
            &tmp,
            &[
                "show",
                "nested.bundle",
                "--path-id",
                &path_id,
                "--file",
                "CAB-a",
                "--file-index",
                &address,
            ],
            0,
        );
        assert_eq!(shown["object"], objects[0]);
        let ambiguous = json(
            &tmp,
            &[
                "show",
                "nested.bundle",
                "--path-id",
                &path_id,
                "--file",
                "CAB-a",
            ],
            1,
        );
        assert_eq!(ambiguous["code"], "ambiguous");
        let candidates = ambiguous["detail"].as_array().unwrap();
        assert_eq!(candidates.len(), 2);
        assert_ne!(candidates[0]["file_index"], candidates[1]["file_index"]);
        assert!(candidates.iter().any(|c| c["file_index"] == file_index));
        assert!(
            ambiguous["message"]
                .as_str()
                .unwrap()
                .contains("--file-index")
        );
        ids.push(objects[0]["path_id"].as_i64().unwrap());
        cursor = page["next_cursor"].as_str().map(str::to_owned);
        if let Some(token) = &cursor {
            assert!(
                tokens.insert(token.clone()),
                "cursor repeated a nested file checkpoint"
            );
        }
    }
    assert_eq!(ids, [-2, 10, 30, -2, 10, 30]);
    assert!(
        cursor.is_none(),
        "every object must have been emitted exactly once"
    );
}

#[test]
fn absolute_input_cursor_works_from_two_directories() {
    let tmp = fixture();
    let other = unityrs_testkit::scratch::Scratch::new("agent-other-directory");
    let absolute = tmp.join("synthetic.assets");
    let input = absolute.to_str().unwrap();
    let first = json(&tmp, &["inspect", input, "--limit", "1"], 0);
    let other_first = json(&other, &["inspect", input, "--limit", "1"], 0);
    assert_eq!(first, other_first);
    let cursor = first["next_cursor"].as_str().unwrap();
    let here = json(
        &tmp,
        &["inspect", input, "--limit", "1", "--cursor", cursor],
        0,
    );
    let there = json(
        &other,
        &["inspect", input, "--limit", "1", "--cursor", cursor],
        0,
    );
    assert_eq!(here, there);
    assert_eq!(there["objects"][0]["path_id"], 10);
}

#[test]
fn an_unhashable_refused_input_cannot_mint_a_cursor() {
    let tmp = fixture();
    let bytes = std::fs::read(tmp.join("synthetic.assets")).unwrap().len();
    let bound = (bytes + 128).to_string();
    std::fs::write(tmp.join("oversized.assets"), vec![0; bytes + 4096]).unwrap();
    let error = json(
        &tmp,
        &[
            "inspect",
            "synthetic.assets",
            "oversized.assets",
            "--limit",
            "1",
            "--max-input-bytes",
            &bound,
        ],
        1,
    );
    assert_eq!(error["code"], "budget");
    // When the complete listing fits, the normal per-input budget error stays
    // useful and does not require a content-bound continuation token.
    let complete = json(
        &tmp,
        &[
            "inspect",
            "synthetic.assets",
            "oversized.assets",
            "--max-input-bytes",
            &bound,
        ],
        0,
    );
    assert_eq!(complete["objects"].as_array().unwrap().len(), 3);
    assert_eq!(complete["inputs"][1]["error"]["code"], "budget");
    assert!(complete["next_cursor"].is_null());
}

#[test]
fn cursor_binds_provider_bytes_even_with_embedded_trees() {
    let tmp = fixture();
    // A stored TPK with an empty tree blob: timestamp plus six zero counts.
    let mut provider = b"TPK*".to_vec();
    provider.extend_from_slice(&[1, 0, 0, 0]);
    provider.extend_from_slice(&0_u32.to_le_bytes());
    provider.extend_from_slice(&32_u32.to_le_bytes());
    provider.extend_from_slice(&32_u32.to_le_bytes());
    provider.extend_from_slice(&1_i64.to_le_bytes());
    provider.extend_from_slice(&[0; 24]);
    std::fs::write(tmp.join("types.tpk"), &provider).unwrap();
    let page = json(
        &tmp,
        &[
            "inspect",
            "synthetic.assets",
            "--tpk",
            "types.tpk",
            "--limit",
            "1",
        ],
        0,
    );
    let cursor = page["next_cursor"].as_str().unwrap();
    *provider.get_mut(20).unwrap() = 2;
    std::fs::write(tmp.join("types.tpk"), provider).unwrap();
    let error = json(
        &tmp,
        &[
            "inspect",
            "synthetic.assets",
            "--tpk",
            "types.tpk",
            "--cursor",
            cursor,
        ],
        1,
    );
    assert_eq!(error["code"], "bad_cursor");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("inputs changed")
    );
    std::fs::write(tmp.join("types.tpk"), [0; 8]).unwrap();
    let error = json(
        &tmp,
        &[
            "inspect",
            "synthetic.assets",
            "--tpk",
            "types.tpk",
            "--cursor",
            cursor,
        ],
        1,
    );
    assert_eq!(error["code"], "bad_cursor");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("inputs changed")
    );
}

#[test]
fn unreadable_inputs_cannot_mint_unbound_cursors() {
    let tmp = fixture();
    let error = json(
        &tmp,
        &[
            "inspect",
            "synthetic.assets",
            "missing.assets",
            "--limit",
            "1",
        ],
        1,
    );
    assert_eq!(error["code"], "input");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("missing.assets")
    );
    assert_eq!(error["detail"]["input"], 1);
}

fn split_source_cursor_roundtrip(input: &str) {
    let tmp = fixture();
    let bytes = std::fs::read(tmp.join("synthetic.assets")).unwrap();
    let midpoint = bytes.len() / 2;
    std::fs::write(tmp.join("split.assets.split0"), &bytes[..midpoint]).unwrap();
    let mut companion = bytes[midpoint..].to_vec();
    std::fs::write(tmp.join("split.assets.split1"), &companion).unwrap();

    let inventory = json(&tmp, &["inspect", input], 0);
    assert_eq!(inventory["total_objects"], 3, "{input}: {inventory}");
    assert_eq!(inventory["objects"].as_array().unwrap().len(), 3);
    assert!(inventory["inputs"][0].get("error").is_none());

    let mut cursor: Option<String> = None;
    let mut first_cursor = None;
    let mut ids = Vec::new();
    for page_index in 0..3 {
        let mut args = vec!["inspect", input, "--limit", "1"];
        if let Some(token) = &cursor {
            args.extend(["--cursor", token]);
        }
        let page = json(&tmp, &args, 0);
        assert_eq!(page["total_objects"], 3);
        let objects = page["objects"].as_array().unwrap();
        assert_eq!(objects.len(), 1);
        ids.push(objects[0]["path_id"].as_i64().unwrap());
        cursor = page["next_cursor"].as_str().map(str::to_owned);
        if page_index == 0 {
            first_cursor.clone_from(&cursor);
            assert!(first_cursor.is_some());
        }
    }
    assert_eq!(ids, [-2, 10, 30]);
    assert!(cursor.is_none());

    // Change only a name byte in the companion: object IDs, schema, and all
    // file offsets remain valid, and the first split stays byte-for-byte equal.
    let name = companion
        .windows(4)
        .position(|window| window == b"beta")
        .unwrap();
    *companion.get_mut(name).unwrap() = b'B';
    std::fs::write(tmp.join("split.assets.split1"), companion).unwrap();
    let error = json(
        &tmp,
        &[
            "inspect",
            input,
            "--limit",
            "1",
            "--cursor",
            first_cursor.as_deref().unwrap(),
        ],
        1,
    );
    assert_eq!(error["code"], "bad_cursor");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("inputs changed")
    );
}

#[test]
fn implicit_split_sources_page_and_bind_companion_bytes() {
    split_source_cursor_roundtrip("split.assets");
}

#[test]
fn explicit_split_sources_page_and_bind_companion_bytes() {
    split_source_cursor_roundtrip("split.assets.split0");
}
