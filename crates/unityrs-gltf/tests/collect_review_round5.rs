mod collect_support;
use collect_support as h;
use unityrs_gltf::*;
use unityrs_testkit::{
    ngui::{V, st, vector},
    scene3d::{self as s, mesh as m},
};

fn skin_row(joints: [i32; 4], weights: [f64; 4]) -> V {
    st(
        "BoneWeights4",
        vec![
            ("weight_0_", V::Double(weights[0])),
            ("weight_1_", V::Double(weights[1])),
            ("weight_2_", V::Double(weights[2])),
            ("weight_3_", V::Double(weights[3])),
            ("boneIndex_0_", V::Int(joints[0])),
            ("boneIndex_1_", V::Int(joints[1])),
            ("boneIndex_2_", V::Int(joints[2])),
            ("boneIndex_3_", V::Int(joints[3])),
        ],
    )
}

fn emitted_components(doc: &serde_json::Value, bin: &[u8], name: &str, width: usize) -> Vec<u32> {
    let attributes = &doc["meshes"][0]["primitives"][0]["attributes"];
    let accessor = &doc["accessors"][usize::try_from(attributes[name].as_u64().unwrap()).unwrap()];
    let view =
        &doc["bufferViews"][usize::try_from(accessor["bufferView"].as_u64().unwrap()).unwrap()];
    let offset = usize::try_from(view["byteOffset"].as_u64().unwrap_or(0)).unwrap()
        + usize::try_from(accessor["byteOffset"].as_u64().unwrap_or(0)).unwrap();
    assert_eq!(
        accessor["componentType"],
        if width == 2 { 5123 } else { 5126 }
    );
    bin[offset..offset + 12 * width]
        .chunks_exact(width)
        .map(|v| {
            if width == 2 {
                u32::from(u16::from_le_bytes(v.try_into().unwrap()))
            } else {
                u32::from_le_bytes(v.try_into().unwrap())
            }
        })
        .collect()
}

fn assert_double_weight_fallback(weight: f64) {
    for duplicate_palette in [false, true] {
        for repeated_indices in [false, true] {
            let mut objects = if duplicate_palette {
                h::repair_objects()
            } else {
                h::hierarchy_objects(false)
            };
            let weights = if repeated_indices {
                [weight, weight, 0., 0.]
            } else {
                [weight, 0., 0., 0.]
            };
            // A healthy row forces influence merging even without a duplicate
            // palette. Zero rows must ignore their original joints.
            let healthy = if repeated_indices {
                skin_row([1, 1, 0, 0], [0.5, 0.5, 0., 0.])
            } else {
                skin_row([1, 0, 0, 0], [1., 0., 0., 0.])
            };
            objects[6] = m::mesh(
                7,
                b"double weights",
                &[[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]],
                &[0, 1, 2],
                m::MeshVersion::V2022,
            );
            s::set(
                &mut objects[6].value,
                "m_Skin",
                vector(
                    skin_row([0; 4], [0.; 4]),
                    vec![
                        skin_row([1, 1, 0, 0], weights),
                        healthy,
                        skin_row([0; 4], weights),
                    ],
                ),
            );
            let identity = m::matrix([
                1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
            ]);
            s::set(
                &mut objects[6].value,
                "m_BindPose",
                vector(
                    identity.clone(),
                    vec![identity; if duplicate_palette { 3 } else { 2 }],
                ),
            );
            s::set(
                &mut objects[5].value,
                "m_BlendShapeWeights",
                vector(V::Float(0.), vec![]),
            );
            let (env, graph) = h::graph(&objects);
            let meter = h::meter();
            let scene = collect(&env, &graph, &Profile::new(&meter), &meter).unwrap();
            assert_eq!(
                scene.skins.len(),
                1,
                "{weight}, {duplicate_palette}, {repeated_indices}"
            );
            assert_eq!(scene.skins.as_slice()[0].joints.len(), 2);

            // Check emitted attributes so the ordinary writer fallback and the
            // collector's merged attributes have the same observable behavior.
            let mut bytes = Vec::new();
            let report = write_glb(&scene, &Options::default(), &meter, &mut bytes).unwrap();
            assert_eq!(report.zero_weight_rows, 2);
            let json_len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
            let doc: serde_json::Value = serde_json::from_slice(&bytes[20..20 + json_len]).unwrap();
            let bin = &bytes[28 + json_len..];
            for (name, width, expected) in [
                ("JOINTS_0", 2, vec![0u32, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0]),
                (
                    "WEIGHTS_0",
                    4,
                    vec![
                        1f32.to_bits(),
                        0,
                        0,
                        0,
                        1f32.to_bits(),
                        0,
                        0,
                        0,
                        1f32.to_bits(),
                        0,
                        0,
                        0,
                    ],
                ),
            ] {
                assert_eq!(
                    emitted_components(&doc, bin, name, width),
                    expected,
                    "{name}: {weight}, {duplicate_palette}, {repeated_indices}"
                );
            }
            h::validator_clean(&scene);
        }
    }
}

#[test]
fn double_weight_underflow_uses_palette_zero_before_merging() {
    assert_double_weight_fallback(1e-50);
}

#[test]
fn double_weight_overflow_uses_palette_zero_before_merging() {
    assert_double_weight_fallback(1e50);
}
