//! Legacy crunch through spec validation, selection, decode and publication.

mod common;
mod publication_support;

use std::{fs, sync::Arc};

use serde_json::{Value, json};
use unityrs_batch::{
    phase1,
    progress::{Snapshot, live::Service},
    reason::Code,
    run,
};
use unityrs_testkit::{
    ByteWriter, ngui,
    scratch::Scratch,
    serialized::{self, ObjectSpec, SerializedSpec, TypeSpec},
};

struct Space;
impl unityrs_batch::start::spool::FileSystem for Space {
    fn in_memory(&self, _: &fs::File) -> std::io::Result<bool> {
        Ok(false)
    }
    fn available(&self, _: &fs::File) -> std::io::Result<u64> {
        Ok(u64::MAX)
    }
}

fn crn(format: i32) -> Vec<u8> {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../unityrs-texture/tests/fixtures/crunch_native.json"
    ))
    .unwrap();
    let case = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["format"] == format && c["width"] == 1 && c["height"] == 1)
        .unwrap();
    let hex = case["data"]["hex"].as_str().unwrap();
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect()
}

fn file(version: &str, externals: &[&str], items: &[ngui::Object]) -> Vec<u8> {
    let mut types = Vec::new();
    let mut objects = Vec::new();
    for object in items {
        let mut writer = ByteWriter::le();
        ngui::write(&mut writer, &object.value).unwrap();
        let type_id = i32::try_from(types.len()).unwrap();
        types.push(TypeSpec {
            class_id: object.class_id,
            nodes: ngui::nodes(0, "Base", &object.value).unwrap(),
            ..TypeSpec::default()
        });
        objects.push(ObjectSpec {
            path_id: object.path_id,
            type_id,
            data: writer.into_inner(),
            ..ObjectSpec::default()
        });
    }
    serialized::build(&SerializedSpec {
        unity_version: version.as_bytes().to_vec(),
        externals: externals
            .iter()
            .map(|p| serialized::ExternalSpec {
                path: p.as_bytes().to_vec(),
                ..serialized::ExternalSpec::default()
            })
            .collect(),
        types,
        objects,
        ..SerializedSpec::default()
    })
}

fn texture_file(version: &str, malformed: bool) -> Vec<u8> {
    let items: Vec<_> = [
        (
            1,
            "dxt1",
            28,
            if malformed { vec![0; 128] } else { crn(28) },
        ),
        (
            2,
            "dxt5",
            29,
            if malformed { vec![0; 128] } else { crn(29) },
        ),
        (3, "plain", 4, vec![255; 4]),
    ]
    .into_iter()
    .map(|(id, name, format, data)| ngui::texture(id, name.as_bytes(), format, (1, 1), data))
    .collect();
    file(version, &[], &items)
}

fn input(version: &str, malformed: bool) -> Vec<u8> {
    unityrs_testkit::env::bundle(&[("CAB-crunch", texture_file(version, malformed))])
}

fn run_case(input: Vec<u8>, want: Code) -> Scratch {
    let scratch = Scratch::new("crunch-v2");
    fs::create_dir(scratch.join("spool")).unwrap();
    fs::create_dir(scratch.join("sink")).unwrap();
    fs::write(scratch.join("input.bundle"), input).unwrap();
    let mut spec = common::base();
    spec["spec_version"] = json!(2);
    spec["inputs"] = json!([{"id": "a", "path": scratch.join("input.bundle")}]);
    spec["dependencies"] = json!([]);
    spec["spool"]["dir"] = json!(scratch.join("spool"));
    spec["sink"]["root"] = json!(scratch.join("sink"));
    spec["outputs"]["sprites"] = json!({"key": "sprites/{name}.png"});
    spec["outputs"]["formats"] = json!(["DXT1Crunched", "DXT5Crunched", "RGBA32"]);
    let prepared = phase1::prepare(common::accepted(&spec)).unwrap();
    let service = Service::start(
        fs::File::create(scratch.join("progress")).unwrap(),
        Snapshot::validate(0),
    )
    .unwrap();
    let result = run::drive(
        prepared,
        123,
        &service.sender(),
        &Space,
        Arc::new(publication_support::Portable),
    )
    .unwrap();
    assert_eq!(result.code, want);
    assert!(scratch.join("sink/a/plain.png").exists());
    scratch
}

fn skips(scratch: &Scratch) -> Vec<Value> {
    let path = scratch.join("sink/_unityrs/r1/1/skips.0.ndjson");
    if !path.exists() {
        return Vec::new();
    }
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn pre_2017_3_crunched_textures_export() {
    let scratch = run_case(input("2017.2.0f1", false), Code::Complete);
    assert!(scratch.join("sink/a/dxt1.png").exists());
    assert!(scratch.join("sink/a/dxt5.png").exists());
    assert_eq!(skips(&scratch), [] as [Value; 0]);
}

#[test]
fn unity_2017_3_and_later_crunched_textures_skip_as_unsupported() {
    for version in ["2017.3.0f1", "2022.3.0f1"] {
        let scratch = run_case(input(version, false), Code::Complete);
        assert!(!scratch.join("sink/a/dxt1.png").exists());
        assert!(!scratch.join("sink/a/dxt5.png").exists());
        let rows = skips(&scratch);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["reason"], "unsupported_format(DXT1Crunched)");
        assert_eq!(rows[1]["reason"], "unsupported_format(DXT5Crunched)");
    }
}

#[test]
fn malformed_legacy_crunch_skips_the_objects_and_continues_the_batch() {
    let scratch = run_case(input("2017.2.0f1", true), Code::CompleteWithSkips);
    let rows = skips(&scratch);
    assert_eq!(rows.len(), 2);
    for row in rows {
        assert_eq!(row["reason"], "decode_error");
        assert_eq!(row["detail"], "crunch");
    }
}

/// A Sprite's version must not choose the decoder for either referenced sheet.
#[test]
fn sprites_use_each_referenced_sheets_version_including_alpha() {
    use ngui::SpriteSpec;
    for (texture_version, sprite_version, exported) in [
        ("2017.2.0f1", "2022.3.0f1", true),
        ("2017.3.0f1", "2017.2.0f1", false),
    ] {
        let sprites = file(
            sprite_version,
            &["archive:/CAB-crunch/CAB-crunch"],
            &[
                SpriteSpec::rect("color", [0.0, 0.0, 1.0, 1.0], (1, 1)).object(4),
                SpriteSpec::rect("alpha", [0.0, 0.0, 1.0, 1.0], (1, 3))
                    .alpha((1, 2))
                    .object(5),
            ],
        );
        let bundle = unityrs_testkit::env::bundle(&[
            ("CAB-sprites", sprites),
            ("CAB-crunch", texture_file(texture_version, false)),
        ]);
        let scratch = run_case(bundle, Code::Complete);
        for name in ["color", "alpha"] {
            assert_eq!(
                scratch.join(format!("sink/sprites/{name}.png")).exists(),
                exported
            );
        }
        let rows = skips(&scratch);
        if exported {
            assert_eq!(rows, [] as [Value; 0]);
        } else {
            assert_eq!(rows.len(), 4);
            assert!(rows.iter().all(|r| {
                r["reason"]
                    .as_str()
                    .unwrap()
                    .starts_with("unsupported_format(")
            }));
        }
    }
}
