use std::{path::Path, process::Command};
use unityrs_testkit::{child, ngui::Object, scene3d as s, scratch::Scratch};

fn run(args: &[&std::ffi::OsStr]) -> std::process::Output {
    child::output(
        Command::new(env!("CARGO_BIN_EXE_unityrs"))
            .arg("gltf")
            .args(args),
    )
    .unwrap()
}
fn strings(args: &[&str]) -> std::process::Output {
    run(&args.iter().map(std::ffi::OsStr::new).collect::<Vec<_>>())
}
fn objects() -> Vec<Object> {
    vec![
        Object::new(100, 1, s::game_object(b"first", &[(0, 90)])),
        Object::new(90, 4, s::transform((0, 100), (0, 0), &[], false)),
        Object::new(1, 1, s::game_object(b"second", &[(0, 3)])),
        Object::new(3, 4, s::transform((0, 1), (0, 0), &[], false)),
    ]
}
fn input(tmp: &Scratch) -> std::path::PathBuf {
    let path = tmp.join("level0");
    std::fs::write(
        &path,
        s::file(s::Layout::Unity2022, &[], &objects()).unwrap(),
    )
    .unwrap();
    path
}
fn json(path: &Path) -> serde_json::Value {
    let bytes = std::fs::read(path).unwrap();
    assert_eq!(&bytes[..4], b"glTF");
    let n = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    serde_json::from_slice(&bytes[20..20 + n]).unwrap()
}
#[test]
fn gltf_help_names_units_profiles_builtins_limits_and_reports() {
    let out = strings(&["--help"]);
    assert!(out.status.success());
    let help = String::from_utf8(out.stdout).unwrap();
    for flag in [
        "--scene",
        "--path-id",
        "--prefab",
        "--profile",
        "--builtin-file",
        "--report",
        "--max-input-memory",
    ] {
        assert!(help.contains(flag), "{flag}: {help}");
    }
}
#[test]
fn scene_roots_follow_object_table_and_prefab_selects_one_root() {
    let tmp = Scratch::new("gltf-cli-scene");
    let input = input(&tmp);
    let out = tmp.join("scene.glb");
    let report = tmp.join("report.json");
    let result = run(&[
        input.as_os_str(),
        "--scene".as_ref(),
        "level0".as_ref(),
        "-o".as_ref(),
        out.as_os_str(),
        "--report".as_ref(),
        report.as_os_str(),
    ]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let scene = json(&out);
    assert_eq!(scene["scenes"][0]["nodes"], serde_json::json!([0, 1]));
    assert_eq!(scene["nodes"][0]["name"], "first");
    assert_eq!(scene["nodes"][1]["name"], "second");
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&report).unwrap()).unwrap();
    assert!(report["peak_charged_bytes"].as_u64().unwrap() > 0);
    for phase in ["input_peak_charged_bytes", "export_peak_charged_bytes"] {
        let phase_peak = report[phase].as_u64().expect("phase peak is reported");
        assert!(phase_peak > 0, "{phase}");
        assert!(phase_peak <= report["peak_charged_bytes"].as_u64().unwrap());
    }
    let result = run(&[
        input.as_os_str(),
        "--path-id".as_ref(),
        "1".as_ref(),
        "-o".as_ref(),
        out.as_os_str(),
    ]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(json(&out)["nodes"][0]["name"], "second");
    assert_eq!(json(&out)["nodes"].as_array().unwrap().len(), 1);
}
#[test]
fn argument_and_selection_errors_exit_one_without_output() {
    for args in [
        vec![],
        vec!["--nope"],
        vec!["x", "-o", "y"],
        vec!["x", "--path-id", "bad", "-o", "y"],
        vec!["x", "--scene", "x", "--path-id", "1", "-o", "y"],
        vec!["x", "--scene", "x", "-o", "y", "--max-input-memory", "bad"],
    ] {
        let out = strings(&args);
        assert_eq!(out.status.code(), Some(1), "{args:?}");
    }
    let tmp = Scratch::new("gltf-cli-errors");
    let input = input(&tmp);
    let out = tmp.join("out.glb");
    for flags in [
        vec!["--path-id", "999"],
        vec!["--path-id", "90"],
        vec!["--scene", "missing"],
        vec!["--prefab", "missing"],
        vec!["--scene", "level0", "--max-input-memory", "1"],
        vec!["--scene", "level0", "--max-decode-work", "1"],
        vec!["--scene", "level0", "--max-input-bytes", "1"],
    ] {
        let mut args = vec![input.as_os_str(), "-o".as_ref(), out.as_os_str()];
        args.extend(flags.iter().map(std::ffi::OsStr::new));
        let result = run(&args);
        assert_eq!(result.status.code(), Some(1), "{flags:?}");
        assert!(!out.exists());
    }
}
#[test]
fn strict_profile_and_failed_output_preserve_destination_and_cleanup() {
    let tmp = Scratch::new("gltf-cli-profile");
    let input = input(&tmp);
    let profile = tmp.join("profile.json");
    let out = tmp.join("out.glb");
    std::fs::write(&out, b"old").unwrap();
    std::fs::write(&profile, br#"{"profile_version":1,"typo":true}"#).unwrap();
    let result = run(&[
        input.as_os_str(),
        "--scene".as_ref(),
        "level0".as_ref(),
        "--profile".as_ref(),
        profile.as_os_str(),
        "-o".as_ref(),
        out.as_os_str(),
    ]);
    assert_eq!(result.status.code(), Some(1));
    assert_eq!(std::fs::read(&out).unwrap(), b"old");
    std::fs::write(
        &profile,
        br#"{"profile_version":1,"scene_roots":["second"]}"#,
    )
    .unwrap();
    let result = run(&[
        input.as_os_str(),
        "--scene".as_ref(),
        "level0".as_ref(),
        "--profile".as_ref(),
        profile.as_os_str(),
        "-o".as_ref(),
        out.as_os_str(),
    ]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(json(&out)["nodes"][0]["name"], "second");
    let result = run(&[
        input.as_os_str(),
        "--scene".as_ref(),
        "level0".as_ref(),
        "-o".as_ref(),
        tmp.path().as_os_str(),
    ]);
    assert_eq!(result.status.code(), Some(1));
    assert!(!std::fs::read_dir(tmp.path()).unwrap().any(|p| {
        p.unwrap()
            .file_name()
            .as_encoded_bytes()
            .starts_with(b".unityrs-gltf-")
    }));
}
