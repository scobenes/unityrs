#![allow(clippy::float_cmp)]
use std::{io::Write, sync::Arc};
use unityrs_core::{
    budget::{Reservation, Unbounded},
    limits::{Meter, MeteredVec},
};
use unityrs_gltf::*;
mod support;

struct Observe {
    max: usize,
    bytes: usize,
}
impl Write for Observe {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.max = self.max.max(bytes.len());
        self.bytes += bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
#[test]
fn the_bin_streams_under_a_constant_heap_bound() {
    let model_meter = Meter::unlimited();
    let mut scene = support::triangle(&model_meter).unwrap();
    let count = 96_000;
    let mut data = MeteredVec::with_capacity(&model_meter, count * 3).unwrap();
    for _ in 0..count {
        data.extend_from_slice(&[1., 2., 3.]).unwrap();
    }
    scene.accessors.as_mut_slice()[0] = Accessor {
        kind: Kind::Position,
        count,
        data: Data::Float(data),
        sparse_indices: None,
    };
    scene.meshes.as_mut_slice()[0].primitives.as_mut_slice()[0].indices = None;
    let writer_meter = Meter::new(16 << 10, Arc::new(Unbounded), Reservation::Input);
    let mut output = Observe { max: 0, bytes: 0 };
    let report = write_glb(&scene, &Options::default(), &writer_meter, &mut output).unwrap();
    assert_eq!(output.max, 65536);
    assert!(output.bytes > 1_000_000);
    drop(report);
    assert_eq!(writer_meter.used(), 0);
}
#[test]
fn layout_uses_first_reach_instead_of_accessor_table_order() {
    let meter = Meter::unlimited();
    let a = support::rich(&meter).unwrap();
    let mut bytes = support::rich(&meter).unwrap();
    bytes.accessors.as_mut_slice().swap(0, 8);
    let remap = |i: usize| match i {
        0 => 8,
        8 => 0,
        _ => i,
    };
    for mesh in bytes.meshes.as_mut_slice() {
        for p in mesh.primitives.as_mut_slice() {
            for (_, id) in p.attributes.as_mut_slice() {
                *id = remap(*id);
            }
            p.indices = p.indices.map(remap);
            for target in p.targets.as_mut_slice() {
                for id in target.iter_mut().flatten() {
                    *id = remap(*id);
                }
            }
        }
    }
    bytes.skins.as_mut_slice()[0].inverse_bind_matrices = Some(0);
    // An unreferenced array must not affect layout or output.
    bytes
        .accessors
        .push(support::floats(&meter, Kind::Normal, &[0., 2., 0.]).unwrap())
        .unwrap();
    assert_eq!(
        support::write(&a, Options::default()).0,
        support::write(&bytes, Options::default()).0
    );
}
#[test]
fn zero_weight_rows_select_palette_joint_zero_and_report_the_change() {
    let meter = Meter::unlimited();
    let mut scene = support::rich(&meter).unwrap();
    if let Data::U16(v) = &mut scene.accessors.as_mut_slice()[5].data {
        v.as_mut_slice()[4] = 1;
    }
    let (bytes, report) = support::write(&scene, Options::default());
    let json = support::json(&bytes);
    assert_eq!(report.joint_components, 1);
    assert_eq!(json["asset"]["extras"]["joint_components"], 1);
    let id = usize::try_from(
        json["meshes"][0]["primitives"][0]["attributes"]["JOINTS_0"]
            .as_u64()
            .unwrap(),
    )
    .unwrap();
    let view = &json["bufferViews"]
        [usize::try_from(json["accessors"][id]["bufferView"].as_u64().unwrap()).unwrap()];
    let start = usize::try_from(view["byteOffset"].as_u64().unwrap()).unwrap();
    assert_eq!(
        &support::bin(&bytes)[start + 8..start + 10],
        &0u16.to_le_bytes()
    );
    let (raw, report) = support::write(&scene, Options { normalise: false });
    assert_eq!(report.joint_components, 0);
    assert_eq!(
        &support::bin(&raw)[start + 8..start + 10],
        &1u16.to_le_bytes()
    );
}
#[test]
fn dense_and_fully_sparse_morph_bounds_and_index_widths() {
    let meter = Meter::unlimited();
    for full_sparse in [false, true] {
        let mut scene = support::rich(&meter).unwrap();
        scene.accessors.as_mut_slice()[7] = Accessor {
            kind: Kind::MorphPosition,
            count: 3,
            data: Data::Float(array(&meter, &[1., 2., 3., 2., 3., 4., 3., 4., 5.]).unwrap()),
            sparse_indices: full_sparse.then(|| array(&meter, &[0, 1, 2]).unwrap()),
        };
        let (bytes, _) = support::write(&scene, Options::default());
        let json = support::json(&bytes);
        assert_eq!(json["accessors"][7]["min"], serde_json::json!([1, 2, 3]));
        assert_eq!(json["accessors"][7]["max"], serde_json::json!([3, 4, 5]));
    }
    for wide in [false, true] {
        let mut scene = support::triangle(&meter).unwrap();
        scene.accessors.as_mut_slice()[1].data = if wide {
            Data::U32(array(&meter, &[0, 1, 2]).unwrap())
        } else {
            Data::U16(array(&meter, &[0, 1, 2]).unwrap())
        };
        let (bytes, _) = support::write(&scene, Options::default());
        let json = support::json(&bytes);
        assert_eq!(
            json["accessors"][1]["componentType"],
            if wide { 5125 } else { 5123 }
        );
        assert_eq!(
            &support::bin(&bytes)[36..],
            if wide {
                &[0, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0][..]
            } else {
                &[0, 0, 1, 0, 2, 0, 0, 0][..]
            }
        );
    }
}
#[test]
fn zero_normal_and_invalid_tangent_rules_preserve_other_attributes() {
    let meter = Meter::unlimited();
    for normal in [false, true] {
        let mut scene = support::rich(&meter).unwrap();
        if let Data::Float(v) = &mut scene.accessors.as_mut_slice()[if normal { 2 } else { 3 }].data
        {
            v.as_mut_slice()[..3].fill(0.);
        }
        let (bytes, report) = support::write(&scene, Options::default());
        let json = support::json(&bytes);
        let a = &json["meshes"][0]["primitives"][0]["attributes"];
        assert!(a.get("TANGENT").is_none());
        assert_eq!(a.get("NORMAL").is_none(), normal);
        assert!(a.get("TEXCOORD_0").is_some());
        assert_eq!(report.issues.len(), 2);
        assert_eq!(
            report.issues.as_slice()[0].reason,
            if normal {
                Reason::InvalidNormal
            } else {
                Reason::InvalidTangent
            }
        );
    }
}
#[test]
fn weights_clamp_non_finites_and_large_values_before_rescaling() {
    let meter = Meter::unlimited();
    let mut scene = support::rich(&meter).unwrap();
    if let Data::Float(v) = &mut scene.accessors.as_mut_slice()[6].data {
        v.as_mut_slice()[..4].copy_from_slice(&[f64::NAN, f64::NEG_INFINITY, -3., f64::MAX]);
        v.as_mut_slice()[8..12].copy_from_slice(&[
            f64::from(f32::MAX),
            f64::from(f32::MAX),
            0.,
            0.,
        ]);
    }
    let (bytes, report) = support::write(&scene, Options::default());
    let json = support::json(&bytes);
    let weights = support::f32s(&bytes, &json, 5);
    assert_eq!(&weights[..4], &[1., 0., 0., 0.]);
    assert_eq!(&weights[8..], &[0.5, 0.5, 0., 0.]);
    assert_eq!(report.zero_weight_rows, 2);
}
#[test]
fn malformed_models_refuse_before_any_output() {
    let meter = Meter::unlimited();
    let mutations: [fn(&mut Scene); 19] = [
        |scene| scene.roots.truncate(2),
        |scene| {
            if let Data::Float(v) = &mut scene.accessors.as_mut_slice()[8].data {
                v.as_mut_slice()[15] = 2.;
            }
        },
        |scene| scene.roots.push(0).unwrap(),
        |scene| scene.nodes.as_mut_slice()[0].children.push(500).unwrap(),
        |scene| scene.nodes.as_mut_slice()[1].children.push(3).unwrap(),
        |scene| scene.nodes.as_mut_slice()[0].rotation = [0.; 4],
        |scene| scene.nodes.as_mut_slice()[0].translation[0] = f64::NAN,
        |scene| scene.nodes.as_mut_slice()[0].mesh = Some(999),
        |scene| scene.nodes.as_mut_slice()[0].weights.push(1.).unwrap(),
        |scene| scene.skins.as_mut_slice()[0].joints.push(2).unwrap(),
        |scene| scene.skins.as_mut_slice()[0].skeleton = Some(0),
        |scene| scene.accessors.as_mut_slice()[8].count = 1,
        |scene| {
            scene.accessors.as_mut_slice()[7]
                .sparse_indices
                .as_mut()
                .unwrap()
                .as_mut_slice()[0] = 3;
        },
        |scene| scene.meshes.as_mut_slice()[0].primitives.as_mut_slice()[0].material = Some(900),
        |scene| {
            scene.meshes.as_mut_slice()[0].primitives.as_mut_slice()[0]
                .attributes
                .as_mut_slice()
                .swap(0, 1);
        },
        |scene| scene.textures.as_mut_slice()[0].image = 100,
        |scene| scene.images.as_mut_slice()[0].bytes.truncate(0),
        |scene| {
            scene.meshes.as_mut_slice()[0].primitives.as_mut_slice()[0]
                .targets
                .as_mut_slice()[0] = [None; 3];
        },
        |scene| {
            if let Data::U16(v) = &mut scene.accessors.as_mut_slice()[5].data {
                v.as_mut_slice()[9] = 0;
            }
        },
    ];
    for (i, mutate) in mutations.iter().enumerate() {
        let mut scene = support::rich(&meter).unwrap();
        mutate(&mut scene);
        let writer_meter = Meter::unlimited();
        let mut out = Vec::new();
        assert!(
            matches!(
                write_glb(&scene, &Options::default(), &writer_meter, &mut out),
                Err(GltfError::Invalid(_))
            ),
            "mutation {i}"
        );
        assert!(out.is_empty(), "mutation {i}");
        assert_eq!(writer_meter.used(), 0);
    }
}
#[test]
fn moved_model_buffers_and_returned_reports_keep_their_own_charges() {
    let meter = Meter::unlimited();
    let data = {
        let scene = support::rich(&meter).unwrap();
        scene.accessors
    };
    assert!(meter.used() > 0);
    drop(data);
    assert_eq!(meter.used(), 0);
    let mut scene = support::rich(&meter).unwrap();
    if let Data::Float(v) = &mut scene.accessors.as_mut_slice()[2].data {
        v.as_mut_slice()[0] = f64::NAN;
    }
    let report = write_glb(&scene, &Options::default(), &meter, &mut std::io::sink()).unwrap();
    drop(scene);
    assert_eq!(meter.used(), report.issues.charge().bytes());
    drop(report);
    assert_eq!(meter.used(), 0);
}

#[test]
fn sparse_morph_directions_are_deltas_and_are_not_normalised() {
    let scene = support::morph_directions(&Meter::unlimited()).unwrap();
    let (bytes, _) = support::write(&scene, Options::default());
    let json = support::json(&bytes);
    let target = &json["meshes"][0]["primitives"][0]["targets"][0];
    for (key, expected) in [("NORMAL", [0f32, 0., 0.1]), ("TANGENT", [0.2, 0., 0.])] {
        let a = &json["accessors"][usize::try_from(target[key].as_u64().unwrap()).unwrap()];
        let view = &json["bufferViews"]
            [usize::try_from(a["sparse"]["values"]["bufferView"].as_u64().unwrap()).unwrap()];
        let start = usize::try_from(view["byteOffset"].as_u64().unwrap()).unwrap();
        let actual: Vec<_> = support::bin(&bytes)[start..start + 12]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|v| f32::from_le_bytes(*v))
            .collect();
        assert_eq!(actual, expected);
    }
}
#[test]
fn a_single_bad_primitive_keeps_the_rest_of_its_mesh() {
    let meter = Meter::unlimited();
    let mut scene = support::triangle(&meter).unwrap();
    let mut p = support::primitive(&meter).unwrap();
    p.attributes.as_mut_slice()[0].1 = 2;
    scene
        .accessors
        .push(
            support::floats(
                &meter,
                Kind::Position,
                &[f64::NAN, 0., 0., 1., 0., 0., 0., 1., 0.],
            )
            .unwrap(),
        )
        .unwrap();
    scene.meshes.as_mut_slice()[0].primitives.push(p).unwrap();
    let (bytes, report) = support::write(&scene, Options::default());
    let json = support::json(&bytes);
    assert_eq!(json["meshes"][0]["primitives"].as_array().unwrap().len(), 1);
    assert_eq!(
        report.issues.as_slice(),
        &[Issue {
            mesh: None,
            primitive: None,
            source_mesh: 0,
            source_primitive: 1,
            reason: Reason::NonFinitePosition
        }]
    );
}
#[test]
#[allow(clippy::cast_possible_truncation)]
fn raw_nan_conversion_has_a_defined_sign_and_payload_rule() {
    let meter = Meter::unlimited();
    let mut scene = support::triangle(&meter).unwrap();
    let values = [
        0x7ff0_0000_0000_0001u64,
        0xfff8_1234_5678_9000,
        0x7fff_ffff_ffff_ffff,
    ];
    if let Data::Float(v) = &mut scene.accessors.as_mut_slice()[0].data {
        for (slot, bits) in v.as_mut_slice().iter_mut().zip(values) {
            *slot = f64::from_bits(bits);
        }
    }
    let (bytes, _) = support::write(&scene, Options { normalise: false });
    let json = support::json(&bytes);
    let got = support::f32s(&bytes, &json, 0);
    for (v, bits) in got.iter().zip(values) {
        let expected = (((bits >> 32) as u32) & 0x8000_0000)
            | 0x7fc0_0000
            | (((bits >> 29) as u32) & 0x007f_ffff);
        assert_eq!(v.to_bits(), expected);
    }
}
