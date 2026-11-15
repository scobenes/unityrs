//! Legacy and Unity fork crunch through spec validation, selection, decode and publication.

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

fn crn(format: i32, unity: bool) -> Vec<u8> {
    let text = if unity {
        include_str!("../../unityrs-texture/tests/fixtures/unitycrunch_native.json")
    } else {
        include_str!("../../unityrs-texture/tests/fixtures/crunch_native.json")
    };
    let fixture: Value = serde_json::from_str(text).unwrap();
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
    // Only 2017.2 is legacy among these fixture versions.
    let unity = version != "2017.2.0f1";
    let payload = |format| {
        if malformed {
            vec![0; 128]
        } else {
            crn(format, unity || format >= 64)
        }
    };
    let items: Vec<_> = [
        (1, "dxt1", 28, payload(28)),
        (2, "dxt5", 29, payload(29)),
        (3, "plain", 4, vec![255; 4]),
        (6, "etc1", 64, payload(64)),
        (7, "etc2", 65, payload(65)),
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
    spec["outputs"]["formats"] = json!([
        "DXT1Crunched",
        "DXT5Crunched",
        "ETC_RGB4Crunched",
        "ETC2_RGBA8Crunched",
        "RGBA32"
    ]);
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
    assert!(scratch.join("sink/a/etc1.png").exists());
    assert!(scratch.join("sink/a/etc2.png").exists());
    assert_eq!(skips(&scratch), [] as [Value; 0]);
}

#[test]
fn unity_2017_3_and_later_crunched_textures_export() {
    for version in ["2017.3.0f1", "2022.3.0f1"] {
        let scratch = run_case(input(version, false), Code::Complete);
        for name in ["dxt1", "dxt5", "etc1", "etc2"] {
            assert!(scratch.join(format!("sink/a/{name}.png")).exists());
        }
        assert_eq!(skips(&scratch), [] as [Value; 0]);
    }
}

#[test]
fn malformed_crunch_skips_the_objects_and_continues_the_batch() {
    for version in ["2017.2.0f1", "2017.3.0f1"] {
        let scratch = run_case(input(version, true), Code::CompleteWithSkips);
        let rows = skips(&scratch);
        assert_eq!(rows.len(), 4);
        for row in rows {
            assert_eq!(row["reason"], "decode_error");
            assert_eq!(row["detail"], "crunch");
        }
    }
}

/// A Sprite's version must not choose the decoder for either referenced sheet.
#[test]
fn sprites_use_each_referenced_sheets_version_including_alpha() {
    use ngui::SpriteSpec;
    for (texture_version, sprite_version) in
        [("2017.2.0f1", "2022.3.0f1"), ("2017.3.0f1", "2017.2.0f1")]
    {
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
            assert!(scratch.join(format!("sink/sprites/{name}.png")).exists());
        }
        assert_eq!(skips(&scratch), [] as [Value; 0]);
    }
}

#[test]
fn malformed_packed_versions_skip_textures_and_both_sprite_sheets() {
    use ngui::SpriteSpec;
    // The file parses, but 1536 spills into the packed build-type byte (6).
    let sprites = file(
        "2022.3.0f1",
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
        ("CAB-crunch", texture_file("5.0.0a1536", false)),
    ]);
    let scratch = run_case(bundle, Code::CompleteWithSkips);
    let rows = skips(&scratch);
    assert_eq!(rows.len(), 6);
    for row in rows {
        assert_eq!(row["reason"], "decode_error");
        assert_eq!(row["detail"], "version");
    }
    for name in ["dxt1", "dxt5", "etc1", "etc2"] {
        assert!(!scratch.join(format!("sink/a/{name}.png")).exists());
    }
    for name in ["color", "alpha"] {
        assert!(!scratch.join(format!("sink/sprites/{name}.png")).exists());
    }
}

#[test]
fn missing_data_precedes_malformed_versions_in_batch_textures_and_sprite_sheets() {
    use ngui::SpriteSpec;
    for format in [28, 29, 64, 65, 4] {
        let bundle = unityrs_testkit::env::bundle(&[(
            "CAB-crunch",
            file(
                "5.0.0a1536",
                &[],
                &[
                    ngui::texture(1, b"missing", format, (1, 1), vec![]),
                    ngui::texture(2, b"plain", 4, (1, 1), vec![255; 4]),
                    SpriteSpec::rect("color", [0.0, 0.0, 1.0, 1.0], (0, 1)).object(3),
                    SpriteSpec::rect("alpha", [0.0, 0.0, 1.0, 1.0], (0, 2))
                        .alpha((0, 1))
                        .object(4),
                    SpriteSpec::rect("plain", [0.0, 0.0, 1.0, 1.0], (0, 2)).object(5),
                ],
            ),
        )]);
        let scratch = run_case(bundle, Code::CompleteWithSkips);
        assert!(scratch.join("sink/sprites/plain.png").exists());
        let rows = skips(&scratch);
        assert_eq!(rows.len(), 3, "format {format}");
        for row in rows {
            assert_eq!(row["reason"], "decode_error");
            assert_eq!(row["detail"], "no_image_data");
        }
    }
}
