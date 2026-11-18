//! K1 L2/L2b: read a synthetic stripped file and both providers in the container.
//! No oracle outputs, hashes or schemas from the image's TPK are persisted.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::{Value as J, json};
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use unityrs_core::env::{EnvOptions, Environment, Source};
use unityrs_core::files::File;
use unityrs_core::provider::ProviderChain;
use unityrs_core::{Bytes, FileContext, Parsed, RawStr, Value};
use unityrs_testkit::serialized::{ObjectSpec, SerializedSpec, TypeSpec, build};
use unityrs_testkit::{ByteWriter, child, scratch::Scratch};
use unityrs_tpk::TpkProvider;

fn string(w: &mut ByteWriter, s: &str) {
    w.i32(i32::try_from(s.len()).unwrap())
        .bytes(s.as_bytes())
        .align(4);
}

// Layout facts from the MIT class definitions/readers, also used by testkit's
// ngui/objects.rs. These bytes are authored without consulting any TPK schema.
fn stripped(big_endian: bool, version: &[u8]) -> Vec<u8> {
    let writer = || {
        if big_endian {
            ByteWriter::be()
        } else {
            ByteWriter::le()
        }
    };
    let mut text = writer();
    string(&mut text, "synthetic");
    string(&mut text, "payload");
    let mut mono = writer();
    mono.i32(0).i64(0).u8(1).align(4).i32(0).i64(3);
    string(&mut mono, "component");
    let mut script = writer();
    string(&mut script, "Example");
    script.i32(0).bytes(&[0; 16]);
    for s in ["Example", "Synthetic", "Assembly.dll"] {
        string(&mut script, s);
    }
    let types = [49, 114, 115, 74]
        .into_iter()
        .map(|class_id| TypeSpec {
            class_id,
            ..TypeSpec::default()
        })
        .collect();
    let animation = unityrs_testkit::scene3d::anim::playback::stripped_position_payload(big_endian);
    let objects = [
        text.into_inner(),
        mono.into_inner(),
        script.into_inner(),
        animation,
    ]
    .into_iter()
    .enumerate()
    .map(|(i, data)| ObjectSpec {
        path_id: i64::try_from(i + 1).unwrap(),
        type_id: i32::try_from(i).unwrap(),
        data,
        ..ObjectSpec::default()
    })
    .collect();
    build(&SerializedSpec {
        type_trees: false,
        big_endian,
        unity_version: version.to_vec(),
        types,
        objects,
        ..SerializedSpec::default()
    })
}

fn value(parsed: &Parsed, v: &Value) -> J {
    match v {
        Value::Bool(v) => json!(v),
        Value::Int(v) => json!(v),
        Value::UInt(v) => json!(v),
        Value::Str(s) => json!(s.to_str().unwrap()),
        Value::F32(bits) => json!(f32::from_bits(*bits)),
        Value::List(values) => J::Array(values.iter().map(|v| value(parsed, v)).collect()),
        Value::Prims(values) if values.is_empty() => json!([]),
        Value::Dict(m) => J::Object(
            parsed
                .entries(m)
                .map(|(k, v)| (std::str::from_utf8(k).unwrap().to_owned(), value(parsed, v)))
                .collect(),
        ),
        _ => panic!("unexpected synthetic value {v:?}"),
    }
}

#[test]
fn stripped_l2_and_l2b_match_the_image_provider_at_test_time() {
    if std::env::var_os("UNITYRS_K1_IMAGE").is_none() {
        assert!(
            std::env::var_os("UNITYRS_ORACLE_TPK").is_none(),
            "K1 must run in the oracle job"
        );
        eprintln!(
            "K1 image comparison requires tools/fixtures/providers.py --check in the container"
        );
        return;
    }
    assert_eq!(std::env::consts::OS, "linux", "never start Python natively");
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let python = std::env::var("UNITYRS_PYTHON").unwrap_or_else(|_| "python".into());
    let locate = child::output(Command::new(&python).args(["-c",
        "from pathlib import Path; import UnityPy; print(Path(UnityPy.__file__).parent / 'resources/lzma.tpk')"])).unwrap();
    assert!(locate.status.success());
    let tpk = PathBuf::from(String::from_utf8(locate.stdout).unwrap().trim());
    assert!(std::fs::metadata(&tpk).unwrap().len() > 0);
    let provider = Arc::new(TpkProvider::open(&tpk).unwrap());
    let scratch = Scratch::new("k1-stripped");
    for version in [b"2021.3.0f1".as_slice(), b"2022.3.0f1".as_slice()] {
        for big_endian in [false, true] {
            let bytes = stripped(big_endian, version);
            eprintln!(
                "K1 synthetic stripped input: {} endian={} {} bytes",
                std::str::from_utf8(version).unwrap(),
                if big_endian { "big" } else { "little" },
                bytes.len()
            );
            let path = scratch.join("stripped.assets");
            std::fs::write(&path, &bytes).unwrap();
            let oracle = child::output(
                Command::new(&python)
                    .arg(root.join("tools/fixtures/providers.py"))
                    .arg("--compare-image")
                    .arg(&path),
            )
            .unwrap();
            assert!(
                oracle.status.success(),
                "{}",
                String::from_utf8_lossy(&oracle.stderr)
            );
            let want: J = serde_json::from_slice(&oracle.stdout).unwrap();
            let mut context = FileContext::default();
            context.providers = Arc::new(ProviderChain::new(vec![provider.clone()]));
            let mut options = EnvOptions::default();
            options.context = Arc::new(context);
            let env = Environment::new(options);
            let File::Serialized(file) = env
                .load(Source::Bytes {
                    name: RawStr::from("stripped.assets"),
                    data: Bytes::from(bytes),
                })
                .unwrap()
            else {
                panic!()
            };
            assert!(file.types().iter().all(|t| t.nodes.is_none()));
            for c in [49, 114, 115, 74] {
                let list = provider
                    .node_list(c, &file.version().unwrap().version)
                    .unwrap()
                    .unwrap();
                let got: Vec<J> = list
                    .rows()
                    .iter()
                    .map(|r| {
                        json!([
                            r.level,
                            r.ty.to_str(),
                            r.name.to_str(),
                            r.byte_size,
                            r.version,
                            r.type_flags,
                            r.variable_count,
                            r.index,
                            r.meta_flag,
                            r.ref_type_hash
                        ])
                    })
                    .collect();
                assert_eq!(json!(got), want["rows"][c.to_string()], "class {c}");
            }
            for id in 1..=4 {
                let obj = file.object(id).unwrap();
                let tree = env.object_tree(&obj, None).unwrap().unwrap();
                let parsed = obj.read_typetree(Some(&tree.tree), true).unwrap();
                assert_eq!(
                    value(&parsed, parsed.value()),
                    want["values"][id.to_string()]
                );
            }
            check_animation(&file.object(4).unwrap());
        }
    }
}

fn check_animation(object: &unityrs_core::ObjectRef) {
    let meter = unityrs_core::limits::Meter::unlimited();
    let bytes = object.raw_data().unwrap();
    let clip = unityrs_anim::read_clip(object, &meter).unwrap();
    let timing = clip.timing().unwrap().unwrap();
    assert_eq!(
        [timing.start(), timing.stop(), timing.rate()],
        [2., 2., 24.]
    );
    assert!(!object.file().is_changed());
    assert_eq!(object.raw_data().unwrap(), bytes);
}
