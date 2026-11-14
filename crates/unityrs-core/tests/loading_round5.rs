//! Round five: caller keys, literal opens, empty roots and ZIP None lookup.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use serde_json::{Value as J, json};
use std::{path::Path, process::Command};
use unityrs_core::{
    File, RawStr,
    env::{DerefError, EnvOptions, Environment, PPtr, Source},
};
use unityrs_testkit::{
    child,
    env::{Obj, serialized},
    scratch::Scratch,
    zip::archive,
};

fn environment() -> Environment {
    Environment::new(EnvOptions::unitypy())
}

fn raw(file: File) -> Vec<u8> {
    let File::Raw(raw) = file else {
        panic!("not raw")
    };
    raw.data.to_vec()
}

fn oracle_case(name: &str, actual: &J) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/loading/oracle.json");
    let fixture: J = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let Some(expected) = fixture.get("round5").and_then(|cases| cases.get(name)) else {
        assert!(
            std::env::var_os("CI").is_none(),
            "regenerate loading.py for round five"
        );
        eprintln!("SKIP: round five oracle case {name} awaits coordinator generation");
        return;
    };
    assert_eq!(actual, expected, "round five oracle: {name}");
}

fn raw_answers(env: &Environment) -> J {
    let prefix = format!("{}/", std::env::current_dir().unwrap().display());
    let entries: Vec<_> = env.files().collect();
    json!({
        "keys": entries.iter().map(|e| e.name.to_str().unwrap().strip_prefix(&prefix).unwrap_or(e.name.to_str().unwrap())).collect::<Vec<_>>(),
        "data": entries.iter().map(|e| String::from_utf8(raw(e.file.clone())).unwrap()).collect::<Vec<_>>()
    })
}

fn run_probe(name: &str) {
    let scratch = Scratch::new(name);
    let output = child::output(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", name, "--ignored", "--nocapture"])
            .current_dir(scratch.path())
            .env("HOME", scratch.path().join("home")),
    )
    .unwrap();
    assert!(output.status.success(), "{output:?}");
}

#[test]
fn split_keys_preserve_protocol_spelling() {
    let scratch = Scratch::new("split-key-spellings");
    let path = scratch.path().join("a.split0");
    std::fs::write(&path, b"abc").unwrap();
    let supplied = [
        format!("file:{}", path.display()),
        path.display().to_string(),
    ];
    let expected = [
        format!("file:{}/a", scratch.path().display()),
        format!("{}/a", scratch.path().display()),
    ];
    for constructor in [false, true] {
        let env = environment();
        if constructor {
            for name in &supplied {
                env.load_path(Path::new(name)).unwrap();
            }
        } else {
            env.load_files(&supplied).unwrap();
        }
        assert_eq!(
            env.files().map(|e| e.name.to_string()).collect::<Vec<_>>(),
            expected
        );
        for entry in env.files() {
            assert_eq!(raw(entry.file), b"abc");
        }
        let prefix = format!("{}/", scratch.path().display());
        let keys: Vec<_> = env
            .files()
            .map(|e| e.name.to_string().replace(&prefix, ""))
            .collect();
        oracle_case(
            if constructor {
                "constructor_split_keys"
            } else {
                "split_keys"
            },
            &json!(keys),
        );
    }
}

#[test]
fn literal_opens_and_fsspec_opens_have_separate_boundaries() {
    run_probe("literal_open_probe");
}

#[test]
#[ignore = "runs in a child with a temporary cwd and HOME"]
fn literal_open_probe() {
    std::fs::create_dir("~").unwrap();
    std::fs::create_dir("home").unwrap();
    for (name, bytes) in [
        ("plain", b"physical".as_slice()),
        ("file:plain", b"literal protocol"),
        ("~/plain", b"literal tilde"),
        ("home/plain", b"expanded tilde"),
    ] {
        std::fs::write(name, bytes).unwrap();
    }
    let env = environment();
    env.load_files(["file:plain", "~/plain"]).unwrap();
    let entries: Vec<_> = env.files().collect();
    assert_eq!(entries[0].name.as_bytes(), b"file:plain");
    assert_eq!(raw(entries[0].file.clone()), b"literal protocol");
    assert_eq!(entries[1].name.as_bytes(), b"~/plain");
    assert_eq!(raw(entries[1].file.clone()), b"literal tilde");
    oracle_case("literal_load_files", &raw_answers(&env));
    // load_file's OS existence gate is literal, then fs.open normalises.
    for constructor in [false, true] {
        for (name, expected) in [
            ("file:plain", b"physical".as_slice()),
            ("~/plain", b"expanded tilde"),
        ] {
            let env = environment();
            if constructor {
                env.load_path(Path::new(name)).unwrap();
            } else {
                env.load_file(Source::Path(name.into())).unwrap().unwrap();
            }
            let entry = env.files().next().unwrap();
            assert_eq!(entry.name.as_bytes(), name.as_bytes());
            assert_eq!(raw(entry.file), expected);
            oracle_case(
                &format!(
                    "{}:{name}",
                    if constructor {
                        "constructor"
                    } else {
                        "load_file"
                    }
                ),
                &raw_answers(&env),
            );
        }
    }
    std::fs::remove_file("file:plain").unwrap();
    for constructor in [false, true] {
        let env = environment();
        if constructor {
            env.load_path(Path::new("file:plain")).unwrap();
        } else {
            assert!(
                env.load_file(Source::Path("file:plain".into()))
                    .unwrap()
                    .is_none()
            );
        }
        assert_eq!(env.files().count(), 0);
        oracle_case(
            if constructor {
                "constructor_missing_literal"
            } else {
                "load_file_missing_literal"
            },
            &raw_answers(&env),
        );
    }
}

#[test]
fn empty_filesystem_roots_load_the_current_directory() {
    run_probe("empty_root_probe");
}

#[test]
#[ignore = "runs in a child with a temporary cwd"]
fn empty_root_probe() {
    std::fs::write("plain", b"current directory").unwrap();
    let cwd = std::env::current_dir().unwrap();
    for root in ["", "file:", "file://", "local:", "local://"] {
        for constructor in [false, true] {
            let env = environment();
            if constructor {
                env.load_path(Path::new(root)).unwrap();
            } else {
                env.load_folder(Path::new(root)).unwrap();
            }
            let entries: Vec<_> = env.files().collect();
            assert_eq!(entries.len(), 1, "{root:?}, constructor={constructor}");
            assert_eq!(
                entries[0].name.as_bytes(),
                cwd.join("plain").as_os_str().as_encoded_bytes()
            );
            assert_eq!(raw(entries[0].file.clone()), b"current directory");
            oracle_case(
                &format!(
                    "{}:{root}",
                    if constructor {
                        "constructor_root"
                    } else {
                        "folder_root"
                    }
                ),
                &raw_answers(&env),
            );
        }
    }
}

#[test]
fn zip_none_sibling_blocks_an_older_cab() {
    for (old_key, name) in [
        ("CAB-X", "CAB-X"),
        ("dir/CAB-X", "CAB-X"),
        ("CAB-X", "cab-x"),
    ] {
        let env = environment();
        let File::Serialized(caller) = env
            .load(Source::Bytes {
                name: RawStr::from("caller"),
                data: serialized(&["CAB-X"], &[Obj::named(1, "caller", &[])]).into(),
            })
            .unwrap()
        else {
            panic!("not serialized")
        };
        env.load(Source::Bytes {
            name: RawStr::from(old_key),
            data: serialized(&[], &[Obj::named(7, "older", &[])]).into(),
        })
        .unwrap();
        let pointer = PPtr {
            file_id: 1,
            path_id: 7,
        };
        assert_eq!(env.deref(&caller, pointer).unwrap().path_id(), 7);
        // A detected ZIP assigns None to its own key after loading members.
        env.load_file(Source::Bytes {
            name: RawStr::from(name),
            data: archive(&[("member", b"abc", false)]).into(),
        })
        .unwrap();
        let result = env.deref(&caller, pointer);
        if name == "CAB-X" {
            assert!(matches!(result, Err(DerefError::NotFound { .. })));
        } else {
            // A later differently-cased None cannot shadow the first key.
            assert_eq!(result.as_ref().unwrap().path_id(), 7);
        }
        let after = result.map_or_else(
            |_| json!({"error":"FileNotFoundError"}),
            |obj| json!(obj.path_id()),
        );
        oracle_case(
            &format!("none:{old_key}:{name}"),
            &json!({"before":7,"after":after}),
        );
    }
}
