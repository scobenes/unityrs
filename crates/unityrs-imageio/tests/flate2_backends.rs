//! The core's gzip stays on zlib-rs (`docs/plans/extract.md` E19): no crate
//! of the workspace enables a C zlib of flate2's, and no C zlib's `-sys`
//! crate is in the build. The vendored zlib 1.2.13 is this crate's alone,
//! compiled under `Z_PREFIX` (`build.rs`), and serves only the PNG encoder.
//!
//! cargo resolves the workspace's features once for all its members, as a
//! build of the workspace does, so `cargo metadata`'s resolve for the target
//! the tests run on gives flate2's features as every member gets them.

use std::process::Command;

use serde_json::{Value, json};
use unityrs_testkit::child;

/// flate2 1.1.10's features that build a C zlib (its `Cargo.toml`):
/// `zlib` (libz-sys), `zlib-default`, `zlib-ng` (libz-ng-sys),
/// `zlib-ng-compat`, `cloudflare_zlib` (`zlib` since 1.1) and `any_c_zlib`,
/// which each of them enables.
const C_FEATURES: [&str; 6] = [
    "any_c_zlib",
    "cloudflare_zlib",
    "zlib",
    "zlib-default",
    "zlib-ng",
    "zlib-ng-compat",
];

/// The C zlibs' `-sys` crates, any version.
const C_CRATES: [&str; 3] = ["cloudflare-zlib-sys", "libz-ng-sys", "libz-sys"];

/// What in `meta` (`cargo metadata`'s output) builds a C zlib for flate2:
/// each C feature of a flate2 in the resolve, and each C zlib crate there.
/// Also whether some flate2 there has `zlib-rs`.
fn c_zlib(meta: &Value) -> (Vec<String>, bool) {
    let names: Vec<(&str, &str)> = meta["packages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| (p["id"].as_str().unwrap(), p["name"].as_str().unwrap()))
        .collect();
    let name_of = |id: &str| names.iter().find(|(i, _)| *i == id).map(|(_, n)| *n);
    let mut found = Vec::new();
    let mut zlib_rs = false;
    for node in meta["resolve"]["nodes"].as_array().unwrap() {
        let id = node["id"].as_str().unwrap();
        let name = name_of(id).unwrap();
        if C_CRATES.contains(&name) {
            found.push(id.to_owned());
        }
        if name != "flate2" {
            continue;
        }
        for feature in node["features"].as_array().unwrap() {
            let feature = feature.as_str().unwrap();
            zlib_rs |= feature == "zlib-rs";
            if C_FEATURES.contains(&feature) {
                found.push(format!("{id}: {feature}"));
            }
        }
    }
    (found, zlib_rs)
}

#[test]
fn no_crate_builds_a_c_zlib_for_flate2() {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    // The target the tests were built for (build.rs): the resolve of the
    // packages this build has, which `--offline` needs.
    let target = env!("UNITYRS_IMAGEIO_TARGET");
    let out = child::output(
        Command::new(cargo)
            .args([
                "metadata",
                "--format-version",
                "1",
                "--locked",
                "--offline",
                "--filter-platform",
                target,
            ])
            .current_dir(env!("CARGO_MANIFEST_DIR")),
    )
    .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let meta: Value = serde_json::from_slice(&out.stdout).unwrap();
    let members = meta["workspace_members"].as_array().unwrap();
    assert!(members.len() >= 5, "the metadata is the workspace's");
    let (found, zlib_rs) = c_zlib(&meta);
    assert!(found.is_empty(), "a C zlib is built for flate2: {found:?}");
    assert!(zlib_rs, "flate2 is not on zlib-rs");
}

/// The check finds each C feature and crate in a resolve that has them.
#[test]
fn the_check_finds_a_c_zlib() {
    let meta = |features: Value, extra: &[&str]| {
        let mut packages = vec![json!({"id": "f", "name": "flate2"})];
        let mut nodes = vec![json!({"id": "f", "features": features})];
        for name in extra {
            packages.push(json!({"id": name, "name": name}));
            nodes.push(json!({"id": name, "features": []}));
        }
        json!({"packages": packages, "resolve": {"nodes": nodes}})
    };
    let clean = meta(json!(["any_impl", "any_zlib", "zlib-rs"]), &[]);
    assert_eq!(c_zlib(&clean), (vec![], true));
    for feature in C_FEATURES {
        let (found, _) = c_zlib(&meta(json!(["zlib-rs", feature]), &[]));
        assert_eq!(found, [format!("f: {feature}")]);
    }
    for name in C_CRATES {
        let (found, _) = c_zlib(&meta(json!(["zlib-rs"]), &[name]));
        assert_eq!(found, [name]);
    }
    assert!(!c_zlib(&meta(json!(["miniz_oxide"]), &[])).1);
}
