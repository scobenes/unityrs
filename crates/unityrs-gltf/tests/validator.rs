use std::process::Command;
use unityrs_core::limits::Meter;
use unityrs_gltf::*;
mod support;

#[test]
fn hand_built_scenes_are_validator_clean_and_raw_mode_is_not() {
    let Some(exe) = std::env::var_os("UNITYRS_GLTF_VALIDATOR") else {
        assert!(
            std::env::var_os("UNITYRS_GLTF_VALIDATOR_REQUIRED").is_none(),
            "UNITYRS_GLTF_VALIDATOR_REQUIRED is set but UNITYRS_GLTF_VALIDATOR is missing"
        );
        eprintln!(
            "glTF validator skipped: set UNITYRS_GLTF_VALIDATOR to the pinned Khronos executable"
        );
        return;
    };
    let m = Meter::unlimited();
    let scratch = unityrs_testkit::scratch::Scratch::new("gltf-validator");
    refuse_review_inputs(&m);
    validate_image_cases(&m, &exe, scratch.path());
    for (name, scene, opts, valid) in validator_scenes(&m) {
        let path = scratch.path().join(format!("{name}.glb"));
        let (bytes, _) = support::write(&scene, opts);
        std::fs::write(&path, bytes).unwrap();
        let output = unityrs_testkit::child::output(
            Command::new(&exe).arg("--stdout").arg("--all").arg(&path),
        )
        .unwrap();
        let r: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
            panic!(
                "validator did not return JSON: {}",
                String::from_utf8_lossy(&output.stderr)
            )
        });
        assert_eq!(r["validatorVersion"], "2.0.0-dev.3.10");
        if !valid {
            assert!(r["issues"]["numErrors"].as_u64().unwrap() > 0);
            assert!(!output.status.success());
            continue;
        }
        assert!(output.status.success(), "{name}: {r}");
        assert_eq!(r["issues"]["numErrors"], 0, "{name}: {r}");
        assert_eq!(r["issues"]["truncated"], false, "{name}: {r}");
        assert_eq!(r["issues"]["numWarnings"], 0, "{name}: {r}");
        for message in r["issues"]["messages"].as_array().unwrap() {
            assert!(
                message["severity"].as_u64().unwrap() > 1,
                "{name}: {message}"
            );
        }
    }
}

fn validator_scenes(m: &std::sync::Arc<Meter>) -> Vec<(&'static str, Scene, Options, bool)> {
    let mut bad = support::rich(m).unwrap();
    if let Data::Float(v) = &mut bad.accessors.as_mut_slice()[2].data {
        v.as_mut_slice()[0] = 0.;
        v.as_mut_slice()[2] = 0.;
    }
    // Boundary scenes accepted by the writer must also pass the real validator.
    let mut scenes = vec![
        ("empty", Scene::new(m), Options::default(), true),
        (
            "skinned-root-identity",
            support::skinned_triangle(m, [1, 0, 0, 0]),
            Options::default(),
            true,
        ),
        (
            "stale-zero-weight-joint",
            support::skinned_triangle(m, [1, 9, 0, 0]),
            Options::default(),
            true,
        ),
        (
            "minimal-png",
            support::image(m, Mime::Png, include_bytes!("goldens/pixel.png")),
            Options::default(),
            true,
        ),
        (
            "minimal-jpeg",
            support::image(m, Mime::Jpeg, support::MINIMAL_JPEG),
            Options::default(),
            true,
        ),
        (
            "texcoord-continuity",
            support::texcoords(m, &[0, 1, 2]),
            Options::default(),
            true,
        ),
        (
            "zero-weight-joints",
            support::zero_joints(m),
            Options::default(),
            true,
        ),
        (
            "empty-sparse-morph",
            support::empty_morph(m),
            Options::default(),
            true,
        ),
        (
            "triangle",
            support::triangle(m).unwrap(),
            Options::default(),
            true,
        ),
        ("rich", support::rich(m).unwrap(), Options::default(), true),
        ("dropped-normal", bad, Options::default(), true),
        (
            "morph-directions",
            support::morph_directions(m).unwrap(),
            Options::default(),
            true,
        ),
        (
            "raw",
            support::rich(m).unwrap(),
            Options { normalise: false },
            false,
        ),
    ];
    for (name, dropped_first) in [
        ("omitted-stale-joint-first", true),
        ("omitted-stale-joint-last", false),
    ] {
        scenes.push((
            name,
            support::omitted_joint_influence(m, dropped_first),
            Options::default(),
            true,
        ));
    }
    scenes.extend(rotation_scenes(m));
    scenes
}

fn refuse_review_inputs(m: &std::sync::Arc<Meter>) {
    // Invalid inputs exercise refusal before anything reaches the validator.
    let mut bad_png = support::rich(m).unwrap();
    bad_png.images.as_mut_slice()[0].bytes.as_mut_slice()[7] ^= 1;
    let mut bad_jpeg = support::rich(m).unwrap();
    bad_jpeg.images.as_mut_slice()[0].mime = Mime::Jpeg;
    let mut raw_rotation = support::triangle(m).unwrap();
    raw_rotation.nodes.as_mut_slice()[0].rotation = [1.000_000_4, 0., 0., 0.];
    for (mime, bytes, signature_len, required_len) in [
        (Mime::Png, &include_bytes!("goldens/pixel.png")[..], 8, 41),
        (Mime::Jpeg, support::MINIMAL_JPEG, 3, 15),
    ] {
        for end in [signature_len, required_len - 1] {
            let scene = support::image(m, mime, &bytes[..end]);
            let mut out = Vec::new();
            assert!(matches!(
                write_glb(&scene, &Options::default(), m, &mut out),
                Err(GltfError::Invalid(_))
            ));
            assert_eq!(out, [] as [u8; 0]);
        }
    }
    for (scene, opts) in [
        (support::texcoords(m, &[1]), Options::default()),
        (support::texcoords(m, &[0, 2]), Options::default()),
        (bad_png, Options::default()),
        (support::parented_skin(m), Options::default()),
        (support::transformed_skin(m), Options::default()),
        (bad_jpeg, Options::default()),
        (raw_rotation, Options { normalise: false }),
    ] {
        let mut out = Vec::new();
        assert!(matches!(
            write_glb(&scene, &opts, m, &mut out),
            Err(GltfError::Invalid(_))
        ));
        assert_eq!(out, [] as [u8; 0]);
    }
}

#[test]
fn warnings_have_no_allowlisted_exceptions() {
    let allow: serde_json::Value =
        serde_json::from_str(include_str!("validator-allowlist.json")).unwrap();
    assert_eq!(allow, serde_json::json!([]));
}
#[test]
fn required_validator_cannot_silently_skip() {
    let exe = std::env::current_exe().unwrap();
    for required in [false, true] {
        let mut cmd = Command::new(&exe);
        cmd.args([
            "--exact",
            "hand_built_scenes_are_validator_clean_and_raw_mode_is_not",
            "--nocapture",
        ])
        .env_remove("UNITYRS_GLTF_VALIDATOR")
        .env_remove("UNITYRS_GLTF_VALIDATOR_REQUIRED");
        if required {
            cmd.env("UNITYRS_GLTF_VALIDATOR_REQUIRED", "1");
        }
        let out = unityrs_testkit::child::output(&mut cmd).unwrap();
        assert_eq!(
            out.status.success(),
            !required,
            "{}",
            String::from_utf8_lossy(&out.stdout)
        );
    }
}
#[test]
fn required_validator_must_be_executable() {
    let scratch = unityrs_testkit::scratch::Scratch::new("validator-unavailable");
    for path in [scratch.path().join("missing"), scratch.path().to_path_buf()] {
        let out = unityrs_testkit::child::output(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "hand_built_scenes_are_validator_clean_and_raw_mode_is_not",
                ])
                .env("UNITYRS_GLTF_VALIDATOR_REQUIRED", "1")
                .env("UNITYRS_GLTF_VALIDATOR", path),
        )
        .unwrap();
        assert!(!out.status.success());
    }
}

#[test]
fn raw_skinned_mesh_placements_keep_the_expected_validator_warnings() {
    let Some(exe) = std::env::var_os("UNITYRS_GLTF_VALIDATOR") else {
        assert!(std::env::var_os("UNITYRS_GLTF_VALIDATOR_REQUIRED").is_none());
        return;
    };
    let m = Meter::unlimited();
    let scratch = unityrs_testkit::scratch::Scratch::new("gltf-skinned-placement");
    for (name, scene, warning) in [
        (
            "parented",
            support::parented_skin(&m),
            "NODE_SKINNED_MESH_NON_ROOT",
        ),
        (
            "transformed",
            support::transformed_skin(&m),
            "NODE_SKINNED_MESH_LOCAL_TRANSFORMS",
        ),
    ] {
        let path = scratch.path().join(format!("{name}.glb"));
        std::fs::write(
            &path,
            support::write(&scene, Options { normalise: false }).0,
        )
        .unwrap();
        let output = unityrs_testkit::child::output(
            Command::new(&exe).arg("--stdout").arg("--all").arg(&path),
        )
        .unwrap();
        let r: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(r["validatorVersion"], "2.0.0-dev.3.10");
        assert_eq!(r["issues"]["numErrors"], 0, "{name}: {r}");
        assert_eq!(r["issues"]["numWarnings"], 1, "{name}: {r}");
        assert!(
            r["issues"]["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v["code"] == warning && v["severity"] == 1),
            "{name}: {r}"
        );
    }
}

// Build an independent image-only GLB so refused bodies still reach the oracle.
fn unchecked_image_glb(mime: Mime, bytes: &[u8]) -> Vec<u8> {
    let mut json = serde_json::to_vec(&serde_json::json!({
        "asset": {"version": "2.0"},
        "buffers": [{"byteLength": bytes.len()}],
        "bufferViews": [{"buffer": 0, "byteLength": bytes.len()}],
        "images": [{"bufferView": 0, "mimeType": match mime {
            Mime::Png => "image/png", Mime::Jpeg => "image/jpeg",
        }}],
    }))
    .unwrap();
    while !json.len().is_multiple_of(4) {
        json.push(b' ');
    }
    let bin_length = (bytes.len() + 3) & !3;
    let mut out = Vec::new();
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(
        &u32::try_from(28 + json.len() + bin_length)
            .unwrap()
            .to_le_bytes(),
    );
    out.extend_from_slice(&u32::try_from(json.len()).unwrap().to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&json);
    out.extend_from_slice(&u32::try_from(bin_length).unwrap().to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(bytes);
    out.resize(28 + json.len() + bin_length, 0);
    out
}
fn validate_image_cases(
    m: &std::sync::Arc<Meter>,
    exe: &std::ffi::OsStr,
    scratch: &std::path::Path,
) {
    for (name, mime, bytes, error) in support::image_review_cases() {
        let scene = support::image(m, mime, &bytes);
        let mut out = Vec::new();
        let result = write_glb(&scene, &Options::default(), m, &mut out);
        let path = scratch.join(format!("image-{name}.glb"));
        // Test the real writer's output for accepted bodies; only refusals need
        // an independently framed GLB to observe the validator's diagnostic.
        std::fs::write(
            &path,
            if error.is_some() {
                unchecked_image_glb(mime, &bytes)
            } else {
                out.clone()
            },
        )
        .unwrap();
        let output = unityrs_testkit::child::output(
            Command::new(exe).arg("--stdout").arg("--all").arg(&path),
        )
        .unwrap();
        let r: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
            panic!(
                "{name}: validator JSON: {e}; status: {}; stderr: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            )
        });
        assert_eq!(r["validatorVersion"], "2.0.0-dev.3.10");
        if let Some(code) = error {
            assert!(
                r["issues"]["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|v| v["code"] == code),
                "{name}: {r}"
            );
            assert!(!output.status.success(), "{name}: {r}");
            assert!(
                matches!(result, Err(GltfError::Invalid(_))),
                "{name}: {result:?}"
            );
            assert!(out.is_empty(), "{name}");
        } else {
            assert!(result.is_ok(), "{name}: {result:?}");
            assert!(output.status.success(), "{name}: {r}");
            assert_eq!(r["issues"]["numErrors"], 0, "{name}: {r}");
            assert_eq!(r["issues"]["numWarnings"], 0, "{name}: {r}");
        }
    }
}

fn rotation_scenes(m: &std::sync::Arc<Meter>) -> Vec<(&'static str, Scene, Options, bool)> {
    let mut positive_rotation = support::triangle(m).unwrap();
    positive_rotation.nodes.as_mut_slice()[0].rotation = [1.000_000_4, 0., 0., 0.];
    let mut extreme_rotation = support::triangle(m).unwrap();
    extreme_rotation.nodes.as_mut_slice()[0].rotation = [f64::MAX, f64::MAX, 0., 0.];
    let mut tiny_rotation = Node::new(m);
    tiny_rotation.rotation = [1e-160, 1e-160, 0., 0.];
    extreme_rotation.nodes.push(tiny_rotation).unwrap();
    extreme_rotation.roots.push(1).unwrap();
    let mut negative_rotation = support::triangle(m).unwrap();
    negative_rotation.nodes.as_mut_slice()[0].rotation = [-1.000_000_4, 0., 0., 0.];
    vec![
        (
            "rotation-positive",
            positive_rotation,
            Options::default(),
            true,
        ),
        (
            "rotation-extreme",
            extreme_rotation,
            Options::default(),
            true,
        ),
        (
            "rotation-negative",
            negative_rotation,
            Options::default(),
            true,
        ),
    ]
}
