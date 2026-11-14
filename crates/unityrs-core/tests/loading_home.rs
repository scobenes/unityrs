//! Local filesystem expansion, isolated in a child so HOME cannot race tests.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use std::{path::Path, process::Command, sync::Arc};
use unityrs_core::{
    Error, File, FileContext, LimitKind,
    env::{EnvOptions, Environment, Source},
};
use unityrs_testkit::{child, scratch::Scratch, zip::archive};

#[test]
fn folder_walk_normalises_children_with_a_relative_home() {
    let scratch = Scratch::new("loading-relative-home");
    std::fs::create_dir_all(scratch.path().join("relative/assets/child/deep")).unwrap();
    for name in ["top", "child/nested", "child/deep/leaf"] {
        std::fs::write(scratch.path().join("relative/assets").join(name), name).unwrap();
    }
    for home in ["relative", "relative///", "./relative"] {
        let output = child::output(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "relative_home_walk_probe",
                    "--ignored",
                    "--nocapture",
                ])
                .env("HOME", home)
                .current_dir(scratch.path()),
        )
        .unwrap();
        assert!(output.status.success(), "HOME={home:?}: {output:?}");
    }
}

#[test]
#[ignore = "runs in a child with a relative HOME and controlled cwd"]
fn relative_home_walk_probe() {
    let cwd = std::env::current_dir().unwrap();
    let home = std::env::var("HOME").unwrap();
    for prefix in ["", "file:", "file://", "local:", "local://"] {
        let env = Environment::new(EnvOptions::unitypy());
        env.load_folder(Path::new(&format!("{prefix}~/assets")))
            .unwrap();
        let mut keys: Vec<_> = env
            .files()
            .map(|e| e.name.to_str().unwrap().to_owned())
            .collect();
        keys.sort();
        let mut expected = vec![
            format!("{}/assets/top", home.trim_end_matches('/')),
            format!("{}/relative/assets/child/nested", cwd.display()),
            format!("{}/relative/assets/child/deep/leaf", cwd.display()),
        ];
        expected.sort();
        assert_eq!(keys, expected, "prefix={prefix:?}");
    }
}

#[test]
fn home_unset_leaves_tilde_literal() {
    let scratch = Scratch::new("loading-home-unset");
    std::fs::create_dir(scratch.path().join("~")).unwrap();
    std::fs::write(
        scratch.path().join("~/x.zip"),
        archive(&[("literal-home", b"abc", false)]),
    )
    .unwrap();
    let output = child::output(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "home_unset_probe", "--ignored", "--nocapture"])
            .env_remove("HOME")
            .current_dir(scratch.path()),
    )
    .unwrap();
    assert!(output.status.success(), "{output:?}");
}

#[test]
#[ignore = "runs in a child without HOME"]
fn home_unset_probe() {
    let env = Environment::new(EnvOptions::unitypy());
    env.load_zip_file(Source::Path("~/x.zip".into())).unwrap();
    assert_eq!(env.files().next().unwrap().name.as_bytes(), b"literal-home");
    let env = Environment::new(EnvOptions::unitypy());
    env.load_folder(Path::new("~")).unwrap();
    assert_eq!(env.files().next().unwrap().name.as_bytes(), b"literal-home");
}

#[test]
fn local_filesystem_expands_a_temporary_home() {
    let scratch = Scratch::new("loading-home");
    for name in ["plain", "spaces "] {
        let home = scratch.path().join(name);
        std::fs::create_dir(&home).unwrap();
        let mut home = home.into_os_string();
        if name == "spaces " {
            home.push("///");
        }
        let output = child::output(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "home_expansion_probe",
                    "--ignored",
                    "--nocapture",
                ])
                .env("HOME", home),
        )
        .unwrap();
        assert!(
            output.status.success(),
            "stdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
#[ignore = "runs in a child with a temporary HOME"]
fn home_expansion_probe() {
    let home = std::env::var_os("HOME").unwrap();
    let home = Path::new(&home);
    std::fs::write(home.join("x.zip"), archive(&[("member", b"abc", false)])).unwrap();
    std::fs::create_dir(home.join("assets")).unwrap();
    std::fs::write(home.join("assets/plain"), b"abcdef").unwrap();
    std::fs::write(home.join("a.split0"), b"abc").unwrap();
    std::fs::write(home.join("a.split1"), b"def").unwrap();
    for prefix in ["", "file:", "file://", "local:", "local://"] {
        let env = Environment::new(EnvOptions::unitypy());
        env.load_zip_file(Source::Path(format!("{prefix}~/x.zip").into()))
            .unwrap();
        assert_eq!(env.files().next().unwrap().name.as_bytes(), b"member");
        let env = Environment::new(EnvOptions::unitypy());
        env.load_folder(Path::new(&format!("{prefix}~/assets")))
            .unwrap();
        assert_eq!(env.files().count(), 1);
        let env = Environment::new(EnvOptions::unitypy());
        // load_files uses plain open: no literal tilde/protocol path exists.
        assert!(env.load_files([format!("{prefix}~/assets/plain")]).is_err());
        assert_eq!(env.files().count(), 0);
        let env = Environment::new(EnvOptions::unitypy());
        let file = env
            .load(Source::Path(format!("{prefix}~/assets/plain").into()))
            .unwrap();
        let File::Raw(raw) = file else {
            panic!("not raw")
        };
        assert_eq!(raw.data.as_ref(), b"abcdef");
        assert_eq!(
            env.files().next().unwrap().name.as_bytes(),
            format!("{prefix}~/assets/plain").as_bytes()
        );
        let env = Environment::new(EnvOptions::unitypy());
        env.load_path(Path::new(&format!("{prefix}~/x.zip")))
            .unwrap();
        assert_eq!(env.files().next().unwrap().name.as_bytes(), b"member");
        let env = Environment::new(EnvOptions::unitypy());
        let file = env
            .load_file(Source::Path(format!("{prefix}~/a.split0").into()))
            .unwrap()
            .unwrap();
        let File::Raw(raw) = file else {
            panic!("not raw")
        };
        assert_eq!(raw.data.as_ref(), b"abcdef");
        assert_eq!(
            env.files().next().unwrap().name.as_bytes(),
            format!("{prefix}~/a").as_bytes()
        );
    }
    // An expanded path must be paid for before any filesystem access.
    let mut ctx = FileContext::unitypy();
    ctx.limits.max_input_memory = 1;
    let mut options = EnvOptions::unitypy();
    options.context = Arc::new(ctx);
    let env = Environment::new(options);
    assert!(
        matches!(env.load_folder(Path::new("~/absent")), Err(Error::Limit(e)) if e.kind() == LimitKind::InputMemory)
    );
}
