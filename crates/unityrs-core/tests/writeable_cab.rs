//! UnityPy 1.25.2 CAB dictionary behavior and explicit Tier C repairs for
//! slotted external registration and WebFile's missing writer.save call.

use serde_json::{Value, json};
use unityrs_core::env::{EnvOptions, Environment, Source};
use unityrs_core::files::{
    BundleSaveOptions, File, SaveError, SaveLayout, WebSaveOptions, WriteableCab,
};
use unityrs_core::{Bytes, RawStr};

fn unhex(s: &str) -> Bytes {
    Bytes::from(
        s.as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
            .collect::<Vec<_>>(),
    )
}
fn load(data: Bytes) -> (Environment, File) {
    let env = Environment::new(EnvOptions::unitypy());
    let file = env
        .load(Source::Bytes {
            name: RawStr::from_static(b"oracle"),
            data,
        })
        .unwrap();
    (env, file)
}
fn keys(env: &Environment) -> Value {
    let mut keys: Vec<_> = env
        .registry()
        .keys()
        .map(|s| String::from_utf8(s.as_bytes().to_vec()).unwrap())
        .collect();
    keys.sort();
    json!(keys)
}
fn cab(file: &File, name: Option<&RawStr>) -> Result<WriteableCab, SaveError> {
    match file {
        File::Bundle(b) => b.get_writeable_cab(name),
        File::Web(w) => w.get_writeable_cab(name),
        _ => panic!("container"),
    }
}
fn cabs(file: &File) -> unityrs_core::limits::MeteredVec<WriteableCab> {
    match file {
        File::Bundle(b) => b.writeable_cabs().unwrap(),
        File::Web(w) => w.writeable_cabs().unwrap(),
        _ => panic!("container"),
    }
}
fn save(file: &File) -> Bytes {
    match file {
        File::Bundle(b) => b
            .save(BundleSaveOptions {
                layout: SaveLayout::UnityPy,
                ..Default::default()
            })
            .unwrap(),
        File::Web(w) => w
            .save(WebSaveOptions {
                layout: SaveLayout::UnityPy,
                ..Default::default()
            })
            .unwrap(),
        _ => panic!("container"),
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn replay_cab_creation_reuse_external_registration_and_container_roundtrip() {
    let text = include_str!("fixtures/writeable_cab.json");
    assert!(text.len() < 1_500_000);
    let fixture: Value = serde_json::from_str(text).unwrap();
    let guid: [u8; 16] = unhex(fixture["guid"].as_str().unwrap())
        .as_ref()
        .try_into()
        .unwrap();
    assert_eq!(fixture["cases"].as_array().unwrap().len(), 42);
    let mut actual_errors = 0;
    let mut web_errors = 0;
    for case in fixture["cases"].as_array().unwrap() {
        let (env, parent) = load(unhex(case["input"].as_str().unwrap()));
        if case["conflict"] == true {
            assert!(matches!(
                cab(&parent, Some(&parent.container_files()[0].name)),
                Err(SaveError::CabExists)
            ));
            assert_eq!(cabs(&parent).len(), 0);
            continue;
        }
        assert_eq!(keys(&env), case["cabs_before"]);
        let File::Serialized(file) = parent.container_entry(0).unwrap() else {
            panic!("serialized");
        };
        let name = case["name"]
            .as_str()
            .map(|s| RawStr::new(Bytes::copy_from_slice(s.as_bytes())));
        let serialized = case["caller"] == "serialized";
        let writer = if serialized {
            actual_errors += 1;
            assert_eq!(case["actual_error"], "TypeError");
            file.get_writeable_cab_with_guid(name.as_ref(), guid)
                .unwrap()
                .unwrap()
        } else {
            cab(&parent, name.as_ref()).unwrap()
        };
        let again = if serialized {
            file.get_writeable_cab_with_guid(name.as_ref(), [b'x'; 16])
                .unwrap()
                .unwrap()
        } else {
            cab(&parent, name.as_ref()).unwrap()
        };
        assert_eq!(
            keys(&env),
            case["cabs_before"],
            "creation must not register a CAB"
        );
        assert!(!parent.is_changed() && !file.is_changed());
        assert_eq!(
            writer.name().as_bytes(),
            case["key"].as_str().unwrap().as_bytes()
        );
        assert_eq!(u64::from(writer.flags()), case["flags"].as_u64().unwrap());
        assert_eq!(writer.endian(), unityrs_core::stream::Endian::Big);
        assert_eq!(writer.write_bytes(b"abc").unwrap(), 0);
        again.write_i32(0x1122_3344).unwrap();
        assert_eq!(
            writer.save().unwrap(),
            unhex(case["payload"].as_str().unwrap())
        );
        let added = cabs(&parent);
        assert_eq!(added.len(), 1);
        assert_eq!(added.as_slice()[0].save().unwrap(), writer.save().unwrap());
        let mut live_keys: Vec<_> = parent
            .container_files()
            .iter()
            .map(|s| String::from_utf8(s.name.as_bytes().to_vec()).unwrap())
            .collect();
        live_keys.push(String::from_utf8(writer.name().as_bytes().to_vec()).unwrap());
        assert_eq!(json!(live_keys), case["keys"]);
        if serialized {
            assert_eq!(
                writer.path().unwrap().as_bytes(),
                case["path"].as_str().unwrap().as_bytes()
            );
            let externals = file.writeable_externals().unwrap();
            assert_eq!(externals.len(), 1);
            assert_eq!(
                externals.as_slice()[0].guid.as_ref().unwrap().as_slice(),
                guid
            );
            assert_eq!(externals.as_slice()[0].path, writer.path().unwrap());
            assert_eq!(file.externals(), []);
            file.mark_changed();
            assert!(file.is_changed() && parent.is_changed());
        }
        if case["web_error"] == "AttributeError" {
            web_errors += 1;
        }
        let saved = save(&parent);
        assert_eq!(
            saved,
            unhex(case["output"].as_str().unwrap()),
            "{} {} {:?}",
            case["kind"],
            case["caller"],
            name
        );
        let (back_env, back) = load(saved.clone());
        assert_eq!(keys(&back_env), case["cabs_after"]);
        assert_eq!(save(&back), saved);
        let raw = back
            .container_entry(back.container_files().len() - 1)
            .unwrap();
        let File::Raw(raw) = raw else {
            panic!("raw resource");
        };
        assert_eq!(raw.data, writer.save().unwrap());
        let File::Serialized(back_file) = back.container_entry(0).unwrap() else {
            panic!("serialized");
        };
        assert_eq!(back_file.externals().len(), usize::from(serialized));
    }
    assert_eq!((actual_errors, web_errors), (18, 18));
    let (_, top) = load(unhex(fixture["top"].as_str().unwrap()));
    let File::Serialized(top) = top else {
        panic!("top-level serialized");
    };
    assert!(top.get_writeable_cab(None).unwrap().is_none());
}
