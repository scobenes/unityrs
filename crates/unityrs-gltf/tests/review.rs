#![allow(clippy::float_cmp)]
use unityrs_core::limits::Meter;
use unityrs_gltf::*;
mod support;

fn refuses(s: &Scene, opts: Options) {
    let mut out = Vec::new();
    assert!(matches!(
        write_glb(s, &opts, &Meter::unlimited(), &mut out),
        Err(GltfError::Invalid(_))
    ));
    assert_eq!(out, [] as [u8; 0]);
}
#[test]
fn texcoord_sets_start_at_zero_and_have_no_gaps() {
    let m = Meter::unlimited();
    for indices in [&[1][..], &[0, 2], &[0, 1, 3]] {
        for normalise in [true, false] {
            refuses(&support::texcoords(&m, indices), Options { normalise });
        }
    }
    let (bytes, _) = support::write(&support::texcoords(&m, &[0, 1, 2]), Options::default());
    assert_eq!(
        support::json(&bytes)["meshes"][0]["primitives"][0]["attributes"]["TEXCOORD_2"],
        1
    );
}
#[test]
fn every_zero_weight_joint_is_cleared_and_counted_once() {
    let s = support::zero_joints(&Meter::unlimited());
    let (bytes, r) = support::write(&s, Options::default());
    let j = support::json(&bytes);
    let view = &j["bufferViews"][4];
    let start = usize::try_from(view["byteOffset"].as_u64().unwrap()).unwrap();
    let actual: Vec<_> = support::bin(&bytes)[start..start + 24]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|v| u16::from_le_bytes(*v))
        .collect();
    assert_eq!(actual, [0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0]);
    assert_eq!(r.joint_components, 9);
    assert_eq!(j["asset"]["extras"]["joint_components"], 9);
    assert_eq!(
        support::write(&s, Options { normalise: false })
            .1
            .joint_components,
        0
    );
}
fn different_weights(m: &std::sync::Arc<Meter>) -> Scene {
    let mut s = support::rich(m).unwrap();
    s.accessors
        .push(
            support::floats(
                m,
                Kind::Weights,
                &[1., 0., 0., 0., 0., 0., 0., 0., 2., 2., 0., 0.],
            )
            .unwrap(),
        )
        .unwrap();
    s.meshes.as_mut_slice()[1].primitives.as_mut_slice()[0]
        .attributes
        .as_mut_slice()[5]
        .1 = 9;
    s
}
#[test]
fn shared_joints_conflict_on_any_column_of_the_written_row() {
    let m = Meter::unlimited();
    // The first joint is already zero. Only column 1 differs after repair.
    refuses(&different_weights(&m), Options::default());
}
#[test]
fn dropped_primitives_do_not_conflict_with_surviving_joint_repairs() {
    let m = Meter::unlimited();
    for dropped in [0, 1] {
        let mut s = different_weights(&m);
        if let Data::U16(v) = &mut s.accessors.as_mut_slice()[5].data {
            v.as_mut_slice()[4] = 1;
        }
        if let Data::Float(v) = &mut s.accessors.as_mut_slice()[9].data {
            v.as_mut_slice()[4] = 1.;
        }
        s.accessors
            .push(
                support::floats(
                    &m,
                    Kind::Position,
                    &[f64::NAN, 0., 0., 1., 0., 0., 0., 1., 0.],
                )
                .unwrap(),
            )
            .unwrap();
        s.meshes.as_mut_slice()[dropped].primitives.as_mut_slice()[0]
            .attributes
            .as_mut_slice()[0]
            .1 = 10;
        let (bytes, _) = support::write(&s, Options::default());
        assert_eq!(support::json(&bytes)["meshes"].as_array().unwrap().len(), 1);
    }
}
#[test]
fn near_unit_rotations_are_renormalised_in_f64_and_counted() {
    let m = Meter::unlimited();
    for rotation in [
        [1.000_000_4, 0., 0., 0.],
        [-1.000_000_4, 0., 0., 0.],
        [0., 0., 0.6, 0.800_000_1],
    ] {
        let mut s = support::triangle(&m).unwrap();
        s.nodes.as_mut_slice()[0].rotation = rotation;
        let (bytes, report) = support::write(&s, Options::default());
        let j = support::json(&bytes);
        assert_eq!(
            report.rotation_components,
            if rotation[2] == 0. { 1 } else { 2 }
        );
        let out = j["nodes"][0]["rotation"].as_array().unwrap();
        assert!(out.iter().all(|v| v.as_f64().unwrap().abs() <= 1.));
        let norm: f64 = out.iter().map(|v| v.as_f64().unwrap().powi(2)).sum();
        assert!((norm - 1.).abs() < 1e-15);
        assert!(
            j["asset"]["extras"]["rotation_components"]
                .as_u64()
                .unwrap()
                > 0
        );
    }
}
#[test]
fn raw_rotations_refuse_components_outside_the_schema_range() {
    let m = Meter::unlimited();
    for x in [1.000_000_4, -1.000_000_4] {
        let mut s = support::triangle(&m).unwrap();
        s.nodes.as_mut_slice()[0].rotation = [x, 0., 0., 0.];
        refuses(&s, Options { normalise: false });
    }
}
#[test]
fn omitted_accessors_skip_value_checks() {
    let m = Meter::unlimited();
    let baseline = support::write(&support::triangle(&m).unwrap(), Options::default()).0;
    let mut s = support::triangle(&m).unwrap();
    for (kind, values) in [
        (Kind::Color, vec![2.; 4]),
        (Kind::InverseBind, vec![2.; 16]),
        (Kind::MorphPosition, vec![f64::NAN; 3]),
        (Kind::TexCoord, vec![f64::INFINITY; 2]),
    ] {
        s.accessors
            .push(support::floats(&m, kind, &values).unwrap())
            .unwrap();
    }
    assert_eq!(support::write(&s, Options::default()).0, baseline);
}
#[test]
fn dropped_primitive_accessors_skip_value_checks() {
    let m = Meter::unlimited();
    let mut s = support::rich(&m).unwrap();
    if let Data::Float(v) = &mut s.accessors.as_mut_slice()[0].data {
        v.as_mut_slice()[0] = f64::NAN;
    }
    if let Data::Float(v) = &mut s.accessors.as_mut_slice()[7].data {
        v.as_mut_slice()[0] = f64::NAN;
    }
    let (bytes, _) = support::write(&s, Options::default());
    assert!(support::json(&bytes).get("meshes").is_none());
}
#[test]
fn empty_sparse_morph_is_an_implicit_zero_accessor() {
    let s = support::empty_morph(&Meter::unlimited());
    let (bytes, _) = support::write(&s, Options::default());
    let j = support::json(&bytes);
    let a = &j["accessors"][7];
    assert_eq!(a["count"], 3);
    assert_eq!(a["min"], serde_json::json!([0, 0, 0]));
    assert_eq!(a["max"], serde_json::json!([0, 0, 0]));
    assert!(a.get("bufferView").is_none());
    assert!(a.get("sparse").is_none());
    let target = &j["meshes"][0]["primitives"][0]["targets"][0];
    for key in ["NORMAL", "TANGENT"] {
        let id = usize::try_from(target[key].as_u64().unwrap()).unwrap();
        let a = &j["accessors"][id];
        assert_eq!(a["count"], 3);
        assert_eq!(a["min"], serde_json::json!([0, 0, 0]));
        assert_eq!(a["max"], serde_json::json!([0, 0, 0]));
        assert!(a.get("bufferView").is_none());
        assert!(a.get("sparse").is_none());
    }
    assert!(
        j["bufferViews"]
            .as_array()
            .unwrap()
            .iter()
            .all(|v| v["byteLength"].as_u64().unwrap() > 0)
    );
}
#[test]
fn image_signatures_must_match_the_declared_mime() {
    let m = Meter::unlimited();
    for (mime, bytes) in [
        (Mime::Png, &b"garbage!"[..]),
        (Mime::Png, &[0x89, b'P', b'N', b'G']),
        (Mime::Jpeg, &[0xff, 0xd8]),
        (Mime::Jpeg, include_bytes!("goldens/pixel.png")),
        (Mime::Png, &[0xff, 0xd8, 0xff]),
    ] {
        let mut s = Scene::new(&m);
        s.images
            .push(Image {
                name: None,
                mime,
                bytes: array(&m, bytes).unwrap(),
            })
            .unwrap();
        refuses(&s, Options::default());
    }
    for (mime, bytes) in [
        (Mime::Png, &include_bytes!("goldens/pixel.png")[..]),
        (Mime::Jpeg, support::MINIMAL_JPEG),
    ] {
        let mut s = Scene::new(&m);
        s.images
            .push(Image {
                name: None,
                mime,
                bytes: array(&m, bytes).unwrap(),
            })
            .unwrap();
        assert!(layout(&s, &Options::default(), &m).is_ok());
    }
}
#[test]
fn issue_indices_distinguish_source_and_output_after_omissions() {
    let m = Meter::unlimited();
    let mut s = support::rich(&m).unwrap();
    s.accessors
        .push(
            support::floats(
                &m,
                Kind::Position,
                &[f64::NAN, 0., 0., 1., 0., 0., 0., 1., 0.],
            )
            .unwrap(),
        )
        .unwrap();
    s.meshes.as_mut_slice()[0].primitives.as_mut_slice()[0]
        .attributes
        .as_mut_slice()[0]
        .1 = 9;
    let mut dropped = support::primitive(&m).unwrap();
    dropped.attributes = array(
        &m,
        s.meshes.as_slice()[1].primitives.as_slice()[0]
            .attributes
            .as_slice(),
    )
    .unwrap();
    dropped.attributes.as_mut_slice()[0].1 = 9;
    dropped.targets.push([Some(7), None, None]).unwrap();
    let primitives = &mut s.meshes.as_mut_slice()[1].primitives;
    let surviving = std::mem::replace(&mut primitives.as_mut_slice()[0], dropped);
    primitives.push(surviving).unwrap();
    if let Data::Float(v) = &mut s.accessors.as_mut_slice()[2].data {
        v.as_mut_slice()[0] = f64::NAN;
    }
    let (bytes, _) = support::write(&s, Options::default());
    let j = support::json(&bytes);
    let issues = j["asset"]["extras"]["issues"].as_array().unwrap();
    for issue in issues {
        if issue["source_mesh"] == 1 && issue["source_primitive"] == 1 {
            assert_eq!(issue["mesh"], 0);
            assert_eq!(issue["primitive"], 0);
        } else {
            assert!(issue["mesh"].is_null());
            assert!(issue["primitive"].is_null());
        }
        assert!(issue["source_mesh"].as_u64().is_some());
        assert!(issue["source_primitive"].as_u64().is_some());
    }
    assert!(
        issues
            .iter()
            .any(|i| i["source_mesh"] == 1 && i["source_primitive"] == 1)
    );
}

#[test]
fn normalise_mode_rescales_nonzero_rotations_without_overflow_or_underflow() {
    let m = Meter::unlimited();
    for magnitude in [2., f64::MAX, f64::MIN_POSITIVE, f64::from_bits(1)] {
        let mut s = support::triangle(&m).unwrap();
        s.nodes.as_mut_slice()[0].rotation = [0., -magnitude, 0., 0.];
        let (bytes, report) = support::write(&s, Options::default());
        assert_eq!(
            support::json(&bytes)["nodes"][0]["rotation"],
            serde_json::json!([0, -1, 0, 0])
        );
        assert_eq!(report.rotation_components, 1);
    }
}

#[test]
fn rotations_with_subnormal_squared_length_remain_unit() {
    let m = Meter::unlimited();
    let mut s = support::triangle(&m).unwrap();
    s.nodes.as_mut_slice()[0].rotation = [1e-160, 1e-160, 0., 0.];
    let (bytes, _) = support::write(&s, Options::default());
    let j = support::json(&bytes);
    let out = j["nodes"][0]["rotation"].as_array().unwrap();
    let norm: f64 = out.iter().map(|v| v.as_f64().unwrap().powi(2)).sum();
    assert!((norm - 1.).abs() < 1e-15);
}

#[test]
fn images_require_only_the_bytes_consumed_by_the_validator_in_both_modes() {
    let m = Meter::unlimited();
    for (mime, bytes, required_len) in [
        (Mime::Png, &include_bytes!("goldens/pixel.png")[..], 41),
        (Mime::Jpeg, support::MINIMAL_JPEG, 15),
    ] {
        for normalise in [true, false] {
            for end in 0..=bytes.len() {
                let scene = support::image(&m, mime, &bytes[..end]);
                if end < required_len {
                    refuses(&scene, Options { normalise });
                } else {
                    support::write(&scene, Options { normalise });
                }
            }
            let mut trailing = bytes.to_vec();
            trailing.push(0);
            support::write(&support::image(&m, mime, &trailing), Options { normalise });
        }
    }
}

#[test]
fn png_requires_a_complete_ihdr_before_idat() {
    let m = Meter::unlimited();
    let png = include_bytes!("goldens/pixel.png");
    for (start, end) in [(8, 12), (12, 16)] {
        let mut bytes = png.to_vec();
        bytes[start..end].fill(0);
        for normalise in [true, false] {
            refuses(
                &support::image(&m, Mime::Png, &bytes),
                Options { normalise },
            );
        }
    }
    // The pinned decoder aborts then double-completes its future for this body;
    // keep the writer's refusal covered locally instead of crashing the oracle.
    let mut no_header = png[..8].to_vec();
    no_header.extend_from_slice(&png[33..]);
    refuses(
        &support::image(&m, Mime::Png, &no_header),
        Options::default(),
    );
    // A trailer cannot stand in for the missing IHDR body/CRC.
    let mut bytes = png[..16].to_vec();
    bytes.extend_from_slice(&png[png.len() - 12..]);
    refuses(&support::image(&m, Mime::Png, &bytes), Options::default());
}
#[test]
fn jpeg_requires_a_start_of_frame_marker_excluding_table_markers() {
    let m = Meter::unlimited();
    for marker in 0xc0..=0xcf {
        let mut bytes = support::MINIMAL_JPEG.to_vec();
        bytes[3] = marker;
        for normalise in [true, false] {
            let s = support::image(&m, Mime::Jpeg, &bytes);
            if matches!(marker, 0xc4 | 0xc8 | 0xcc) {
                refuses(&s, Options { normalise });
            } else {
                support::write(&s, Options { normalise });
            }
        }
    }
    refuses(
        &support::image(&m, Mime::Jpeg, &[0xff, 0xd8, 0xff, 0xd9]),
        Options::default(),
    );
}

#[test]
fn palette_bounds_use_effective_joints_in_normalise_mode_and_raw_joints_in_raw_mode() {
    let m = Meter::unlimited();
    let s = support::skinned_triangle(&m, [1, 9, 0, 0]);
    let (bytes, _) = support::write(&s, Options::default());
    let j = support::json(&bytes);
    let id = usize::try_from(
        j["meshes"][0]["primitives"][0]["attributes"]["JOINTS_0"]
            .as_u64()
            .unwrap(),
    )
    .unwrap();
    let view = usize::try_from(j["accessors"][id]["bufferView"].as_u64().unwrap()).unwrap();
    let offset = usize::try_from(j["bufferViews"][view]["byteOffset"].as_u64().unwrap()).unwrap();
    assert_eq!(
        &support::bin(&bytes)[offset..offset + 24],
        &[1, 0, 0, 0, 0, 0, 0, 0].repeat(3)
    );
    refuses(&s, Options { normalise: false });
    let mut s = support::skinned_triangle(&m, [1, 9, 0, 0]);
    if let Data::Float(w) = &mut s.accessors.as_mut_slice()[3].data {
        w.as_mut_slice()[1] = 1.;
    }
    refuses(&s, Options::default());
}

#[test]
fn normalise_refuses_parented_skinned_meshes_and_raw_preserves_the_parent() {
    let m = Meter::unlimited();
    let s = support::parented_skin(&m);
    refuses(&s, Options::default());
    let (bytes, _) = support::write(&s, Options { normalise: false });
    let j = support::json(&bytes);
    assert_eq!(j["nodes"][5]["children"], serde_json::json!([0]));
    assert_eq!(j["scenes"][0]["nodes"], serde_json::json!([5, 1]));
    assert_eq!(j["nodes"][0]["skin"], 0);
}
#[test]
fn normalise_refuses_each_nonidentity_skinned_trs_and_raw_preserves_it() {
    let m = Meter::unlimited();
    for component in 0..9 {
        let mut s = support::skinned_triangle(&m, [1, 0, 0, 0]);
        let n = &mut s.nodes.as_mut_slice()[0];
        match component {
            0..=2 => n.translation[component] = 1.,
            3..=5 => {
                n.rotation = [0.; 4];
                n.rotation[component - 3] = 1.;
            }
            _ => n.scale[component - 6] = 2.,
        }
        let (translation, rotation, scale) = (n.translation, n.rotation, n.scale);
        refuses(&s, Options::default());
        let (bytes, _) = support::write(&s, Options { normalise: false });
        let j = support::json(&bytes);
        assert_eq!(
            j["nodes"][0]["translation"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_f64().unwrap())
                .collect::<Vec<_>>(),
            translation
        );
        assert_eq!(
            j["nodes"][0]["rotation"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_f64().unwrap())
                .collect::<Vec<_>>(),
            rotation
        );
        assert_eq!(
            j["nodes"][0]["scale"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_f64().unwrap())
                .collect::<Vec<_>>(),
            scale
        );
    }
    // Ordinary mesh transforms and joint-node transforms remain valid.
    let mut s = support::triangle(&m).unwrap();
    s.nodes.as_mut_slice()[0].translation = [1., 2., 3.];
    support::write(&s, Options::default());
    let mut s = support::skinned_triangle(&m, [1, 0, 0, 0]);
    s.nodes.as_mut_slice()[1].translation = [1., 2., 3.];
    support::write(&s, Options::default());
}

#[test]
fn palette_bounds_exclude_primitives_omitted_by_g8() {
    let m = Meter::unlimited();
    for dropped_first in [false, true] {
        let s = support::omitted_joint_influence(&m, dropped_first);
        let (bytes, report) = support::write(&s, Options::default());
        let j = support::json(&bytes);
        assert_eq!(j["meshes"][0]["primitives"].as_array().unwrap().len(), 1);
        assert_eq!(report.joint_components, 3);
        assert_eq!(
            report.issues.as_slice()[0].reason,
            Reason::NonFinitePosition
        );
    }
}
#[test]
fn image_walker_matches_the_validator_review_cases() {
    let m = Meter::unlimited();
    for (name, mime, bytes, error) in support::image_review_cases() {
        for normalise in [true, false] {
            let scene = support::image(&m, mime, &bytes);
            let mut out = Vec::new();
            let result = write_glb(&scene, &Options { normalise }, &m, &mut out);
            assert_eq!(result.is_ok(), error.is_none(), "{name}: {result:?}");
            if error.is_some() {
                assert!(matches!(result, Err(GltfError::Invalid(_))), "{name}");
                assert!(out.is_empty(), "{name}");
            }
        }
    }
}
