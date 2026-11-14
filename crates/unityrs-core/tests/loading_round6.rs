//! Round six: physical trailing slashes and lexical folder keys.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use serde_json::{Value as J, json};
use std::{path::Path, process::Command};
use unityrs_core::{
    File,
    env::{EnvOptions, Environment, Source},
};
use unityrs_testkit::{child, scratch::Scratch, zip::archive};

fn environment() -> Environment {
    Environment::new(EnvOptions::unitypy())
}

fn raw_answers(env: &Environment) -> J {
    let prefix = format!("{}/", std::env::current_dir().unwrap().display());
    let entries: Vec<_> = env.files().collect();
    json!({
        "keys": entries.iter().map(|e| e.name.to_str().unwrap().strip_prefix(&prefix).unwrap_or(e.name.to_str().unwrap())).collect::<Vec<_>>(),
        "data": entries.iter().map(|e| {
            let File::Raw(raw) = &e.file else { panic!("not raw") };
            String::from_utf8(raw.data.to_vec()).unwrap()
        }).collect::<Vec<_>>()
    })
}

fn oracle_case(name: &str, actual: &J) {
    let fixture: J = serde_json::from_str(include_str!("fixtures/loading/oracle.json")).unwrap();
    let Some(expected) = fixture.get("round6").and_then(|cases| cases.get(name)) else {
        assert!(
            std::env::var_os("CI").is_none(),
            "regenerate loading.py for round six"
        );
        eprintln!("SKIP: round six oracle case {name} awaits coordinator generation");
        return;
    };
    assert_eq!(actual, expected, "round six oracle: {name}");
}

#[test]
fn local_paths_follow_fsspec_lexical_rules() {
    let scratch = Scratch::new("loading-round6");
    let output = child::output(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "lexical_paths_probe", "--ignored", "--nocapture"])
            .current_dir(scratch.path())
            .env("HOME", scratch.path().join("home")),
    )
    .unwrap();
    assert!(output.status.success(), "{output:?}");
}

#[test]
#[ignore = "runs in a child with a temporary cwd and HOME"]
fn lexical_paths_probe() {
    std::fs::create_dir_all("home").unwrap();
    let zip = archive(&[("member", b"zip data", false)]);
    std::fs::write("x.zip", &zip).unwrap();
    std::fs::write("home/x.zip", &zip).unwrap();
    for name in [
        "x.zip/",
        "x.zip///",
        "file:x.zip/",
        "local://x.zip///",
        "~/x.zip/",
        "file:~/x.zip///",
    ] {
        let env = environment();
        env.load_zip_file(Source::Path(name.into())).unwrap();
        let actual = raw_answers(&env);
        assert_eq!(
            actual,
            json!({"keys":["member"],"data":["zip data"]}),
            "{name}"
        );
        oracle_case(&format!("zip:{name}"), &actual);
    }
    // Physical trimming must leave the caller's registration key alone.
    std::fs::write("plain", b"raw data").unwrap();
    let env = environment();
    env.load(Source::Path("plain///".into())).unwrap();
    assert_eq!(
        raw_answers(&env),
        json!({"keys":["plain///"],"data":["raw data"]})
    );

    std::fs::create_dir_all("a/b/child").unwrap();
    std::fs::write("a/b/x", b"root file").unwrap();
    std::fs::write("a/b/child/y", b"nested file").unwrap();
    let cwd = std::env::current_dir().unwrap();
    for (root, expected) in [
        ("a//b", "a//b"),
        ("a/./b", "a/./b"),
        ("a/b/", "a/b"),
        ("./a//b", "a//b"),
        ("././a/b", "./a/b"),
        ("a//b///", "a//b"),
        ("a/b/../b", "a/b/../b"),
    ] {
        for absolute in [false, true] {
            let supplied = if absolute {
                format!("{}/{root}", cwd.display())
            } else {
                root.to_owned()
            };
            // Absolute paths keep even their leading './' component.
            let expected = if absolute {
                root.trim_end_matches('/')
            } else {
                expected
            };
            for constructor in [false, true] {
                let env = environment();
                if constructor {
                    env.load_path(Path::new(&supplied)).unwrap();
                } else {
                    env.load_folder(Path::new(&supplied)).unwrap();
                }
                let actual = raw_answers(&env);
                assert_eq!(
                    actual,
                    json!({
                        "keys":[format!("{expected}/x"), format!("{expected}/child/y")],
                        "data":["root file", "nested file"]
                    }),
                    "root={supplied:?}, constructor={constructor}"
                );
                oracle_case(
                    &format!(
                        "{}:{}{root}",
                        if constructor { "constructor" } else { "folder" },
                        if absolute { "absolute:" } else { "" }
                    ),
                    &actual,
                );
            }
        }
    }
    let env = environment();
    for root in ["a//b", "a/b"] {
        env.load_folder(Path::new(root)).unwrap();
    }
    assert_eq!(
        raw_answers(&env)["keys"],
        json!(["a//b/x", "a//b/child/y", "a/b/x", "a/b/child/y"])
    );
}
