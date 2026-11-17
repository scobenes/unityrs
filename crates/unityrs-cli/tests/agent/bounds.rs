use super::support::{fixture, json, run};
use unityrs_core::env::{EnvOptions, Environment, Source};
use unityrs_testkit::bundle::{self, BundleSpec, EntrySpec};
use unityrs_testkit::ngui::{self, Object, s, st};

#[test]
fn class_lookups_are_charged_and_repeated_filters_are_deduplicated() {
    let tmp = fixture();
    let objects = (0..5000)
        .map(|id| Object::new(id, 49, st("TextAsset", vec![])))
        .collect::<Vec<_>>();
    std::fs::write(
        tmp.join("many.assets"),
        ngui::file(&[], &objects, true).unwrap(),
    )
    .unwrap();
    // Establish the source hashing/loading work independently of filtering.
    let env = Environment::new(EnvOptions::unitypy());
    let file = env
        .load_observed(Source::Path(tmp.join("many.assets")), |data, meter| {
            meter.count_decode(data.len() as u64)?;
            let _ = unityrs_dump::json::sha256(data);
            Ok(())
        })
        .unwrap();
    let cap = (file.meter().decoded() + 10_000 + 1024).to_string();
    for repeated in [true, false] {
        let filters = (0..5000)
            .map(|i| {
                if repeated {
                    "1".to_owned()
                } else {
                    (1000 + i).to_string()
                }
            })
            .collect::<Vec<_>>();
        let mut args = vec!["inspect", "many.assets", "--max-decode-work", &cap];
        for filter in &filters {
            args.extend(["--class", filter]);
        }
        let doc = json(&tmp, &args, 0);
        assert_eq!(doc["total_objects"], 0);
        if repeated {
            assert!(doc["inputs"][0].get("error").is_none(), "{doc}");
        } else {
            assert_eq!(doc["inputs"][0]["error"]["code"], "budget", "{doc}");
        }
    }
}

#[test]
fn explicit_input_selector_skips_unrelated_failures_before_and_after() {
    let tmp = fixture();
    std::fs::write(tmp.join("data.resS"), b"resource bytes").unwrap();
    for unrelated in ["broken.bundle", "data.resS", "missing.assets"] {
        for selected_first in [false, true] {
            let (inputs, address, index) = if selected_first {
                (["synthetic.assets", unrelated], "0:0", 0)
            } else {
                ([unrelated, "synthetic.assets"], "1:0", 1)
            };
            let doc = json(
                &tmp,
                &[
                    "show",
                    inputs[0],
                    inputs[1],
                    "--path-id",
                    "-2",
                    "--file-index",
                    address,
                ],
                0,
            );
            assert_eq!(doc["object"]["input"], index);
            assert_eq!(doc["object"]["file_index"], 0);
            assert_eq!(doc["object"]["path_id"], -2);
            assert_eq!(doc["value"]["m_Name"], "alpha");
        }
    }
}

#[test]
fn inspect_huge_names_have_bounded_previews_and_exact_addresses() {
    let tmp = fixture();
    std::fs::write(
        tmp.join("huge.assets"),
        ngui::file(
            &[],
            &[Object::new(
                7,
                49,
                st("TextAsset", vec![("m_Name", s(&"é".repeat(1 << 19)))]),
            )],
            true,
        )
        .unwrap(),
    )
    .unwrap();
    let output = run(&tmp, &["inspect", "huge.assets", "--limit", "1"]);
    assert!(output.status.success(), "{output:?}");
    assert!(
        output.stdout.len() <= 65_536,
        "{} bytes",
        output.stdout.len()
    );
    let doc = json(&tmp, &["inspect", "huge.assets", "--limit", "1"], 0);
    let object = &doc["objects"][0];
    assert_eq!(object["input"], 0);
    assert_eq!(object["file_index"], 0);
    assert_eq!(object["path_id"], 7);
    assert_eq!(object["name"]["$unityrs"], "preview");
    assert_eq!(object["name"]["length"], 1 << 20);
    assert!(object["name"]["prefix"].as_str().unwrap().len() < 1024);
    assert_eq!(doc["total_objects"], 1);
    assert!(doc["next_cursor"].is_null());
}

#[test]
fn inspect_byte_cut_resumes_after_the_last_emitted_object() {
    let tmp = fixture();
    let args = [
        "inspect",
        "synthetic.assets",
        "--limit",
        "3",
        "--max-bytes",
        "700",
    ];
    let output = run(&tmp, &args);
    assert!(output.status.success(), "{output:?}");
    assert!(output.stdout.len() <= 700);
    let page = json(&tmp, &args, 0);
    let objects = page["objects"].as_array().unwrap();
    assert_eq!(objects.len(), 1);
    assert_eq!(objects[0]["path_id"], -2);
    assert_eq!(page["total_objects"], 3);
    let token = page["next_cursor"].as_str().unwrap();
    // Display byte/page limits may change without changing listing identity.
    let next = json(
        &tmp,
        &[
            "inspect",
            "synthetic.assets",
            "--max-bytes",
            "2000",
            "--cursor",
            token,
        ],
        0,
    );
    assert_eq!(next["objects"].as_array().unwrap().len(), 2);
    assert_eq!(next["objects"][0]["path_id"], 10);
    assert_eq!(next["objects"][1]["path_id"], 30);
    assert!(next["next_cursor"].is_null());
}

#[test]
fn inspect_refuses_a_cap_that_cannot_fit_an_object_address() {
    let tmp = fixture();
    let error = json(
        &tmp,
        &["inspect", "synthetic.assets", "--max-bytes", "64"],
        1,
    );
    assert_eq!(error["code"], "budget");
    assert!(
        error["message"].as_str().unwrap().contains("address"),
        "{error}"
    );
}

#[test]
fn inspect_metadata_previews_keep_long_file_names_pageable() {
    let tmp = fixture();
    let asset = std::fs::read(tmp.join("synthetic.assets")).unwrap();
    let stored_name = [vec![b'x'; 127], "é".as_bytes().to_vec(), vec![b'y'; 20_000]].concat();
    let bundle = bundle::build(&BundleSpec {
        entries: vec![EntrySpec {
            name: stored_name,
            data: asset,
            flags: 4,
        }],
        ..BundleSpec::default()
    });
    let directory = format!("{}/{}/{}", "a".repeat(60), "b".repeat(60), "c".repeat(60));
    std::fs::create_dir_all(tmp.join(&directory)).unwrap();
    let input = format!("{directory}/long.bundle");
    std::fs::write(tmp.join(&input), bundle).unwrap();
    let first = json(
        &tmp,
        &["inspect", &input, "--limit", "1", "--max-bytes", "4096"],
        0,
    );
    assert_eq!(first["inputs"][0]["path"]["$unityrs"], "preview");
    let file = &first["inputs"][0]["files"][1]["file"];
    assert_eq!(file["length"], 20_129);
    assert_eq!(file["prefix"], "x".repeat(127));
    assert_eq!(file["omitted"], 20_002);
    assert_eq!(first["objects"][0]["file"], *file);
    assert_eq!(first["objects"][0]["file_index"], 1);
    let token = first["next_cursor"].as_str().unwrap();
    assert!(token.len() < 300, "filename must not expand the cursor");
    let next = json(
        &tmp,
        &["inspect", &input, "--cursor", token, "--max-bytes", "4096"],
        0,
    );
    assert_eq!(next["objects"][0]["path_id"], 10);
    assert_eq!(next["objects"][1]["path_id"], 30);
    assert!(next["next_cursor"].is_null());
}

#[test]
fn editing_and_rechecksumming_a_cursor_selects_the_callers_own_page() {
    let tmp = fixture();
    let first = json(&tmp, &["inspect", "synthetic.assets", "--limit", "1"], 0);
    let (payload, _) = first["next_cursor"]
        .as_str()
        .unwrap()
        .rsplit_once('.')
        .unwrap();
    let mut parts = payload.split('.').collect::<Vec<_>>();
    parts[3] = "10";
    let edited = parts.join(".");
    let token = format!(
        "{edited}.{}",
        unityrs_dump::json::sha256_hex(edited.as_bytes())
    );
    let page = json(
        &tmp,
        &["inspect", "synthetic.assets", "--cursor", &token],
        0,
    );
    assert_eq!(page["objects"].as_array().unwrap().len(), 1);
    assert_eq!(page["objects"][0]["path_id"], 30);
    assert!(page["next_cursor"].is_null());
}
