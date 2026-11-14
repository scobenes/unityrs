#![allow(clippy::float_cmp)]
use unityrs_core::limits::Meter;
use unityrs_gltf::*;
mod support;

#[test]
fn byte_exact_goldens_and_repeated_and_concurrent_writes() {
    let meter = Meter::unlimited();
    for (name, scene) in [
        ("empty", Scene::new(&meter)),
        ("triangle", support::triangle(&meter).unwrap()),
        ("rich", support::rich(&meter).unwrap()),
    ] {
        let (bytes, _) = support::write(&scene, Options::default());
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/goldens")
            .join(format!("{name}.glb"));
        if std::env::var_os("UNITYRS_GLTF_UPDATE_GOLDENS").is_some() {
            std::fs::write(&path, &bytes).unwrap();
        }
        assert_eq!(bytes, std::fs::read(path).unwrap(), "{name}");
        assert_eq!(bytes, support::write(&scene, Options::default()).0);
        std::thread::scope(|scope| {
            let workers: Vec<_> = (0..4)
                .map(|_| scope.spawn(|| support::write(&scene, Options::default()).0))
                .collect();
            for weights in workers {
                assert_eq!(bytes, weights.join().unwrap());
            }
        });
        assert_eq!(
            u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize,
            bytes.len()
        );
        assert_eq!(bytes.len() % 4, 0);
        let json = support::json(&bytes);
        if let Some(views) = json["bufferViews"].as_array() {
            for view in views {
                assert_eq!(view["byteOffset"].as_u64().unwrap() % 4, 0);
            }
        } else {
            assert_eq!(
                bytes.len(),
                20 + u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize
            );
        }
    }
}
#[test]
fn sparse_skin_shared_geometry_and_node_weights() {
    let scene = support::rich(&Meter::unlimited()).unwrap();
    let (bytes, _) = support::write(&scene, Options::default());
    let json = support::json(&bytes);
    assert_eq!(
        json["meshes"][0]["primitives"][0]["attributes"],
        json["meshes"][1]["primitives"][0]["attributes"]
    );
    assert_eq!(json["accessors"].as_array().unwrap().len(), 9);
    assert_eq!(json["nodes"][0]["weights"][0], 0.25);
    assert_eq!(json["nodes"][1]["weights"][0], 0.75);
    let a = &json["accessors"][7];
    assert!(a.get("bufferView").is_none());
    assert_eq!(a["count"], 3);
    assert_eq!(a["sparse"]["count"], 1);
    assert_eq!(a["min"], serde_json::json!([0, 0, 0]));
    assert_eq!(a["max"], serde_json::json!([0, 0, 0.25]));
    let view = &json["bufferViews"]
        [usize::try_from(a["sparse"]["indices"]["bufferView"].as_u64().unwrap()).unwrap()];
    let start = usize::try_from(view["byteOffset"].as_u64().unwrap()).unwrap();
    assert_eq!(&support::bin(&bytes)[start..start + 4], &1u32.to_le_bytes());
    assert_eq!(json["skins"][0]["joints"], serde_json::json!([2, 3]));
    assert_eq!(json["skins"][0]["inverseBindMatrices"], 8);
    assert_eq!(
        support::f32s(&bytes, &json, 8),
        vec![
            1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 1., 0., 0., 0., 0., 1.,
            0., 0., 0., 0., 1., 0., 0., 0., 0., 1.
        ]
    );
    assert_eq!(json["nodes"][0]["name"], "escaped\"\\\n\0é");
    assert_eq!(json["images"][0]["mimeType"], "image/png");
    let view = &json["bufferViews"][6];
    let start = usize::try_from(view["byteOffset"].as_u64().unwrap()).unwrap();
    assert_eq!(&support::bin(&bytes)[start..start + 4], &[0, 1, 2, 0]);
}
#[test]
fn normalisation_is_counted_once_and_raw_mode_keeps_bits() {
    let scene = support::rich(&Meter::unlimited()).unwrap();
    let (bytes, report) = support::write(&scene, Options::default());
    let json = support::json(&bytes);
    assert_eq!(report.normal_components, 3);
    assert_eq!(report.tangent_components, 6);
    assert_eq!(report.weight_components, 5);
    assert_eq!(report.zero_weight_rows, 1);
    assert!(report.issues.is_empty());
    assert_eq!(
        support::f32s(&bytes, &json, 1),
        vec![0., 0., 1., 0., 0., 1., 0., 0., 1.]
    );
    assert_eq!(
        support::f32s(&bytes, &json, 5),
        vec![0., 1., 0., 0., 1., 0., 0., 0., 0.5, 0.5, 0., 0.]
    );
    assert_eq!(json["asset"]["extras"]["normal_components"], 3);
    let (raw, report) = support::write(&scene, Options { normalise: false });
    let json = support::json(&raw);
    assert_eq!(
        report.normal_components + report.tangent_components + report.weight_components,
        0
    );
    assert_eq!(
        support::f32s(&raw, &json, 1),
        vec![0., 0., 2., 0., 0., 2., 0., 0., 2.]
    );
    assert_eq!(
        support::f32s(&raw, &json, 5),
        vec![-1., 2., 0., 0., 0., 0., 0., 0., 2., 2., 0., 0.]
    );
    assert_eq!(
        support::f32s(&raw, &json, 0)[0].to_bits(),
        (-0f32).to_bits()
    );
}
#[test]
fn invalid_positions_refuse_primitives_and_bad_normals_drop_directions() {
    let meter = Meter::unlimited();
    let mut scene = support::rich(&meter).unwrap();
    if let Data::Float(v) = &mut scene.accessors.as_mut_slice()[2].data {
        v.as_mut_slice()[0] = f64::NAN;
    }
    let (bytes, report) = support::write(&scene, Options::default());
    let json = support::json(&bytes);
    assert_eq!(report.issues.len(), 2);
    assert_eq!(report.issues.as_slice()[0].reason, Reason::InvalidNormal);
    assert!(
        json["meshes"][0]["primitives"][0]["attributes"]
            .get("NORMAL")
            .is_none()
    );
    assert!(
        json["meshes"][0]["primitives"][0]["attributes"]
            .get("TANGENT")
            .is_none()
    );
    if let Data::Float(v) = &mut scene.accessors.as_mut_slice()[0].data {
        v.as_mut_slice()[0] = f64::MAX;
    }
    let (bytes, report) = support::write(&scene, Options::default());
    let json = support::json(&bytes);
    assert_eq!(
        report
            .issues
            .as_slice()
            .iter()
            .filter(|i| i.reason == Reason::NonFinitePosition)
            .count(),
        2
    );
    assert!(json.get("meshes").is_none());
    assert!(json["nodes"][0].get("mesh").is_none());
    let (bytes, report) = support::write(&scene, Options { normalise: false });
    assert!(report.issues.is_empty());
    assert_eq!(support::json(&bytes)["meshes"].as_array().unwrap().len(), 2);
}
#[test]
fn rounding_bounds_are_f32_and_tiny_directions_are_repaired() {
    let meter = Meter::unlimited();
    let mut scene = support::rich(&meter).unwrap();
    if let Data::Float(v) = &mut scene.accessors.as_mut_slice()[0].data {
        v.as_mut_slice()[3] = 1. + f64::EPSILON;
    }
    if let Data::Float(v) = &mut scene.accessors.as_mut_slice()[2].data {
        v.as_mut_slice().fill(f64::from(f32::from_bits(1)));
    }
    let (bytes, report) = support::write(&scene, Options::default());
    let json = support::json(&bytes);
    assert_eq!(json["accessors"][0]["max"][0], 1);
    assert_eq!(report.normal_components, 9);
    assert!(
        support::f32s(&bytes, &json, 1)
            .iter()
            .all(|v| v.is_finite())
    );
}
#[test]
fn malformed_models_and_io_failures_are_errors() {
    let meter = Meter::unlimited();
    let mut scene = support::triangle(&meter).unwrap();
    scene.nodes.as_mut_slice()[0].children.push(0).unwrap();
    assert!(matches!(
        layout(&scene, &Options::default(), &meter),
        Err(GltfError::Invalid("node cycle"))
    ));
    let mut scene = support::triangle(&meter).unwrap();
    scene.accessors.as_mut_slice()[0].count = usize::MAX;
    assert!(layout(&scene, &Options::default(), &meter).is_err());
    let scene = support::triangle(&meter).unwrap();
    let mut out = [0u8; 24];
    assert!(matches!(
        write_glb(&scene, &Options::default(), &meter, &mut out.as_mut_slice()),
        Err(GltfError::Io(_))
    ));
}
