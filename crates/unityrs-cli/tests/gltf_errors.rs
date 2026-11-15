//! Export failures keep complete old destinations and remove staged siblings.
use std::{ffi::OsStr, path::Path, process::Command};
use unityrs_testkit::{child, ngui::Object, scene3d as s, scratch::Scratch};
fn run(input: &Path, output: &Path, flags: &[&OsStr]) -> std::process::Output {
    child::output(
        Command::new(env!("CARGO_BIN_EXE_unityrs"))
            .arg("gltf")
            .arg(input)
            .arg("-o")
            .arg(output)
            .args(flags),
    )
    .unwrap()
}
fn source(path: &Path) {
    std::fs::write(
        path,
        s::file(
            s::Layout::Unity2022,
            &[],
            &[
                Object::new(1, 1, s::game_object(b"root", &[(0, 2)])),
                Object::new(2, 4, s::transform((0, 1), (0, 0), &[], false)),
            ],
        )
        .unwrap(),
    )
    .unwrap();
}
fn clean(tmp: &Scratch, out: &Path) {
    assert_eq!(std::fs::read(out).unwrap(), b"old complete output");
    assert!(!std::fs::read_dir(tmp.path()).unwrap().any(|p| {
        p.unwrap()
            .file_name()
            .as_encoded_bytes()
            .starts_with(b".unityrs-gltf-")
    }));
}
#[test]
fn each_missing_value_duplicate_and_bad_numeric_is_rejected() {
    let tmp = Scratch::new("gltf-cli-arg-errors");
    let input = tmp.join("level0");
    source(&input);
    let output = tmp.join("out.glb");
    std::fs::write(&output, b"old complete output").unwrap();
    for flag in [
        "--scene",
        "--path-id",
        "--prefab",
        "--profile",
        "--tpk",
        "--builtin-file",
        "--report",
        "--mapper",
        "--max-input-memory",
        "--max-input-bytes",
        "--max-decode-work",
        "--max-tree-depth",
    ] {
        let failed = run(&input, &output, &[flag.as_ref()]);
        assert_eq!(failed.status.code(), Some(1), "{flag}");
        clean(&tmp, &output);
    }
    for flag in [
        "--max-input-memory",
        "--max-input-bytes",
        "--max-decode-work",
        "--max-tree-depth",
    ] {
        for invalid in ["NaN", "-1", "18446744073709551616"] {
            let failed = run(
                &input,
                &output,
                &[
                    "--path-id".as_ref(),
                    "1".as_ref(),
                    flag.as_ref(),
                    invalid.as_ref(),
                ],
            );
            assert_eq!(failed.status.code(), Some(1), "{flag}: {invalid}");
            clean(&tmp, &output);
        }
    }
    for flags in [
        vec!["--path-id", "1", "--mapper", "other"],
        vec!["--path-id", "1", "--path-id", "1"],
        vec!["--scene", "level0", "--prefab", "root"],
        vec!["--path-id", "1", "--profile", "x", "--profile", "x"],
        vec!["--path-id", "1", "--tpk", "x", "--tpk", "x"],
        vec!["--path-id", "1", "--report", "x", "--report", "x"],
        vec![
            "--path-id",
            "1",
            "--builtin-file",
            "x",
            "--builtin-file",
            "x",
        ],
        vec!["--path-id", "1", "--mapper", "pbr", "--mapper", "unlit"],
        vec!["--path-id", "1", "--max-tree-depth", "4294967296"],
        vec!["--path-id", "1", "--max-tree-depth", "0"],
    ] {
        let failed = run(
            &input,
            &output,
            &flags.iter().map(OsStr::new).collect::<Vec<_>>(),
        );
        assert_eq!(failed.status.code(), Some(1), "{flags:?}");
        clean(&tmp, &output);
    }
}
#[test]
fn input_provider_profile_and_staging_failures_do_not_publish() {
    let tmp = Scratch::new("gltf-cli-io-errors");
    let input = tmp.join("level0");
    source(&input);
    let output = tmp.join("out.glb");
    std::fs::write(&output, b"old complete output").unwrap();
    let missing = tmp.join("absent");
    let invalid = tmp.join("invalid");
    std::fs::write(&invalid, b"invalid source data").unwrap();
    for file in [&missing, &invalid] {
        let failed = run(file, &output, &["--scene".as_ref(), "level0".as_ref()]);
        assert_eq!(failed.status.code(), Some(1));
        clean(&tmp, &output);
        for flag in ["--tpk", "--profile"] {
            let failed = run(
                &input,
                &output,
                &[
                    "--path-id".as_ref(),
                    "1".as_ref(),
                    flag.as_ref(),
                    file.as_os_str(),
                ],
            );
            assert_eq!(failed.status.code(), Some(1), "{flag}");
            clean(&tmp, &output);
        }
    }
    let directory = tmp.join("report-directory");
    std::fs::create_dir(&directory).unwrap();
    for report in [
        &missing.join("report.json"),
        &directory,
        &output,
        &tmp.join("./out.glb"),
    ] {
        let failed = run(
            &input,
            &output,
            &[
                "--path-id".as_ref(),
                "1".as_ref(),
                "--report".as_ref(),
                report.as_os_str(),
            ],
        );
        assert_eq!(failed.status.code(), Some(1), "{report:?}");
        clean(&tmp, &output);
    }
    let failed = run(
        &input,
        &missing.join("out.glb"),
        &["--path-id".as_ref(), "1".as_ref()],
    );
    assert_eq!(failed.status.code(), Some(1));
    clean(&tmp, &output);
}
#[test]
fn ambiguous_ids_and_scene_basenames_are_errors() {
    let tmp = Scratch::new("gltf-cli-ambiguous");
    let input = tmp.join("level0");
    source(&input);
    let dir = tmp.join("another");
    std::fs::create_dir(&dir).unwrap();
    let other = dir.join("level0");
    source(&other);
    let output = tmp.join("out.glb");
    std::fs::write(&output, b"old complete output").unwrap();
    for flags in [["--scene", "level0"], ["--path-id", "1"]] {
        let failed = run(
            &input,
            &output,
            &[other.as_os_str(), flags[0].as_ref(), flags[1].as_ref()],
        );
        assert_eq!(failed.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&failed.stderr).contains("ambiguous"));
        clean(&tmp, &output);
    }
}
#[test]
fn casefolded_destinations_are_usage_errors_before_either_output_is_published() {
    let tmp = Scratch::new("gltf-cli-casefold");
    let input = tmp.join("level0");
    source(&input);
    let alias = tmp.join("alias");
    std::os::unix::fs::symlink(tmp.path(), &alias).unwrap();
    for (first, second) in [
        ("scene.glb", "SCENE.GLB"),
        ("sCeNe.GlB", "ScEnE.gLb"),
        ("Straße.glb", "STRASSE.GLB"),
        ("ος.glb", "ΟΣ.GLB"),
        ("café.glb", "cafe\u{301}.glb"),
        ("café.glb", "CAFE\u{301}.GLB"),
    ] {
        for (output_name, report_name) in [(first, second), (second, first)] {
            let output = tmp.join(output_name);
            let report = alias.join(report_name);
            for preexisting in [false, true] {
                if preexisting {
                    for path in [&output, &report] {
                        std::fs::write(path, b"old complete output").unwrap();
                    }
                }
                let failed = if output_name == first {
                    run(
                        &input,
                        &output,
                        &[
                            "--path-id".as_ref(),
                            "1".as_ref(),
                            "--report".as_ref(),
                            report.as_os_str(),
                        ],
                    )
                } else {
                    child::output(
                        Command::new(env!("CARGO_BIN_EXE_unityrs"))
                            .arg("gltf")
                            .arg(&input)
                            .args(["--path-id", "1", "--report"])
                            .arg(&report)
                            .arg("-o")
                            .arg(&output),
                    )
                    .unwrap()
                };
                assert_eq!(
                    failed.status.code(),
                    Some(1),
                    "{output_name}, {report_name}"
                );
                let stderr = String::from_utf8_lossy(&failed.stderr);
                assert!(
                    stderr.contains("report and GLB destinations must differ"),
                    "{stderr}"
                );
                assert!(stderr.contains("usage: unityrs gltf"), "{stderr}");
                if preexisting {
                    clean(&tmp, &output);
                    assert_eq!(std::fs::read(&report).unwrap(), b"old complete output");
                    std::fs::remove_file(&output).unwrap();
                    if report.exists() {
                        std::fs::remove_file(&report).unwrap();
                    }
                } else {
                    assert!(!output.exists());
                    assert!(!report.exists());
                }
            }
        }
    }
}

#[test]
fn distinct_destinations_and_equal_folded_names_in_different_directories_are_accepted() {
    let tmp = Scratch::new("gltf-cli-distinct");
    let input = tmp.join("level0");
    source(&input);
    let other = tmp.join("other");
    std::fs::create_dir(&other).unwrap();
    for (output, report) in [
        (tmp.join("scene.glb"), tmp.join("scene-report.json")),
        (tmp.join("scene.glb"), other.join("SCENE.GLB")),
    ] {
        let result = run(
            &input,
            &output,
            &[
                "--path-id".as_ref(),
                "1".as_ref(),
                "--report".as_ref(),
                report.as_os_str(),
            ],
        );
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(std::fs::read(&output).unwrap().starts_with(b"glTF"));
        serde_json::from_slice::<serde_json::Value>(&std::fs::read(report).unwrap()).unwrap();
    }
}
#[test]
fn glb_and_report_replace_symlinks_without_writing_targets() {
    let tmp = Scratch::new("gltf-cli-links");
    let input = tmp.join("level0");
    source(&input);
    let target = tmp.join("target");
    std::fs::write(&target, b"kept").unwrap();
    let output = tmp.join("out.glb");
    let report = tmp.join("report.json");
    for path in [&output, &report] {
        std::os::unix::fs::symlink(&target, path).unwrap();
    }
    let result = run(
        &input,
        &output,
        &[
            "--path-id".as_ref(),
            "1".as_ref(),
            "--report".as_ref(),
            report.as_os_str(),
        ],
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(std::fs::read(&target).unwrap(), b"kept");
    assert!(std::fs::symlink_metadata(output).unwrap().is_file());
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(report).unwrap()).unwrap();
    assert!(String::from_utf8_lossy(&result.stderr).contains(&format!(
        "peak charged bytes {}",
        report["peak_charged_bytes"]
    )));
}
