//! Synthetic regressions for attribute rules found by a large corpus measurement.
use unityrs_core::limits::Meter;
use unityrs_gltf::*;
use unityrs_testkit::{
    ngui::{V, st, vector},
    scene3d::{self as s, mesh as m},
};
mod collect_support;
mod support;

fn short_skin(width: u8, weights: Option<[f32; 2]>) -> Vec<unityrs_testkit::ngui::Object> {
    let mut objects = collect_support::objects(true, false);
    let mut offset = 0;
    let channels: Vec<_> = (0..14)
        .map(|i| {
            let dimension = match i {
                0 => 3,
                12 if weights.is_some() => width,
                13 => width,
                _ => 0,
            };
            let c = st(
                "ChannelInfo",
                vec![
                    ("stream", V::UInt8(0)),
                    ("offset", V::UInt8(offset)),
                    ("format", V::UInt8(if i == 13 { 10 } else { 0 })),
                    ("dimension", V::UInt8(dimension)),
                ],
            );
            offset += dimension * 4;
            c
        })
        .collect();
    let mut data = Vec::new();
    for pos in [[1f32, 0., 0.], [0., 1., 0.], [0., 0., 0.]] {
        for v in pos {
            data.extend(v.to_le_bytes());
        }
        if let Some(weights) = weights {
            for v in weights.iter().take(usize::from(width)) {
                data.extend(v.to_le_bytes());
            }
        }
        for _ in 0..width {
            data.extend(0u32.to_le_bytes());
        }
    }
    data.resize(data.len().next_multiple_of(16), 0);
    s::set(
        &mut objects[6].value,
        "m_VertexData",
        st(
            "VertexData",
            vec![
                ("m_VertexCount", V::UInt(3)),
                ("m_Channels", vector(channels[0].clone(), channels)),
                ("m_DataSize", V::Bytes(data)),
            ],
        ),
    );
    s::set(
        &mut objects[6].value,
        "m_Shapes",
        m::blend_shapes(vec![], vec![], vec![], vec![]),
    );
    s::set(
        &mut objects[5].value,
        "m_BlendShapeWeights",
        vector(V::Float(0.), vec![]),
    );
    objects
}

#[test]
fn short_skin_rows_are_structurally_padded_even_in_raw_mode() {
    for (width, weights, normalized) in [
        (1, None, [1., 0., 0., 0.]),
        (1, Some([2., 0.]), [1., 0., 0., 0.]),
        (2, Some([0., 0.]), [1., 0., 0., 0.]),
        (2, Some([-1., 2.]), [0., 1., 0., 0.]),
        (2, Some([2., 2.]), [1., 0., 0., 0.]),
    ] {
        for normalise in [true, false] {
            let meter = Meter::unlimited();
            let (env, graph) = collect_support::graph(&short_skin(width, weights));
            let mut profile = Profile::new(&meter);
            profile.normalise = normalise;
            let scene = collect(&env, &graph, &profile, &meter).unwrap();
            assert!(
                !scene.meshes.is_empty(),
                "width {width}, raw {}",
                !normalise
            );
            let (bytes, report) = support::write(&scene, profile.options());
            let j = support::json(&bytes);
            let attrs = &j["meshes"][0]["primitives"][0]["attributes"];
            let id = usize::try_from(attrs["WEIGHTS_0"].as_u64().unwrap()).unwrap();
            let row = if normalise {
                normalized
            } else if let Some(w) = weights {
                [w[0], if width == 2 { w[1] } else { 0. }, 0., 0.]
            } else {
                [1., 0., 0., 0.]
            };
            assert_eq!(support::f32s(&bytes, &j, id), row.repeat(3));
            assert_eq!(
                j["accessors"][usize::try_from(attrs["JOINTS_0"].as_u64().unwrap()).unwrap()]["type"],
                "VEC4"
            );
            let issue = report
                .collection_issues
                .as_slice()
                .iter()
                .find(|i| i.reason == "padded_skin_rows")
                .unwrap();
            assert_eq!(issue.count, 3);
            assert!(
                j["asset"]["extras"]["collection_issues"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|i| i["reason"] == "padded_skin_rows" && i["count"] == 3)
            );
            if normalise {
                collect_support::validator_clean(&scene);
            }
        }
    }
}

fn colors(values: &[f64]) -> Scene {
    let m = Meter::unlimited();
    let mut scene = support::triangle(&m).unwrap();
    scene
        .accessors
        .push(support::floats(&m, Kind::Color, values).unwrap())
        .unwrap();
    scene.meshes.as_mut_slice()[0].primitives.as_mut_slice()[0]
        .attributes
        .push((Attribute::Color, 2))
        .unwrap();
    scene
}

#[test]
fn finite_colors_clamp_and_nonfinite_colors_drop_without_losing_geometry() {
    let scene = colors(&[-0.5, 2., 0.25, 1.].repeat(3));
    let (bytes, report) = support::write(&scene, Options::default());
    assert_eq!(report.color_components, 6);
    let j = support::json(&bytes);
    assert_eq!(support::f32s(&bytes, &j, 1), [0., 1., 0.25, 1.].repeat(3));
    assert_eq!(j["asset"]["extras"]["color_components"], 6);
    collect_support::validator_clean(&scene);
    // Clamp finite source values before f32 overflow could misclassify them.
    let huge = colors(&[f64::MAX, -f64::MAX, 0., 1.].repeat(3));
    let (bytes, report) = support::write(&huge, Options::default());
    let j = support::json(&bytes);
    assert_eq!(report.color_components, 6);
    assert_eq!(support::f32s(&bytes, &j, 1), [1., 0., 0., 1.].repeat(3));
    collect_support::validator_clean(&huge);
    let scene = colors(&[f64::NAN, 0., 0., 1.].repeat(3));
    let (bytes, _) = support::write(&scene, Options::default());
    let j = support::json(&bytes);
    assert!(
        j["meshes"][0]["primitives"][0]["attributes"]
            .get("COLOR_0")
            .is_none()
    );
    assert_eq!(j["asset"]["extras"]["dropped_color_attributes"], 1);
    collect_support::validator_clean(&scene);
    let (bytes, _) = support::write(&scene, Options { normalise: false });
    let j = support::json(&bytes);
    assert!(support::f32s(&bytes, &j, 1)[0].is_nan());
}

#[test]
fn bad_uvs_drop_per_primitive_and_texture_bindings_follow_surviving_sets() {
    let m = Meter::unlimited();
    let mut scene = support::triangle(&m).unwrap();
    scene
        .images
        .push(Image {
            name: None,
            mime: Mime::Png,
            bytes: array(&m, include_bytes!("goldens/pixel.png")).unwrap(),
        })
        .unwrap();
    scene
        .textures
        .push(Texture {
            image: 0,
            sampler: None,
        })
        .unwrap();
    let mut material = Material::new(&m);
    material.base_color_texture = Some(TextureInfo {
        texture: 0,
        scale: [1.; 2],
        offset: [0.; 2],
    });
    scene.materials.push(material).unwrap();
    scene
        .accessors
        .push(support::floats(&m, Kind::TexCoord, &[f64::INFINITY, 0.].repeat(3)).unwrap())
        .unwrap();
    scene
        .accessors
        .push(support::floats(&m, Kind::TexCoord, &[0.2, 0.3].repeat(3)).unwrap())
        .unwrap();
    for valid_alternative in [true, false] {
        let mut p = support::primitive(&m).unwrap();
        p.material = Some(0);
        p.attributes.push((Attribute::TexCoord(0), 2)).unwrap();
        if valid_alternative {
            p.attributes.push((Attribute::TexCoord(1), 3)).unwrap();
        }
        if valid_alternative {
            scene.meshes.as_mut_slice()[0].primitives.as_mut_slice()[0] = p;
        } else {
            scene.meshes.as_mut_slice()[0].primitives.push(p).unwrap();
        }
    }
    let (bytes, report) = support::write(&scene, Options::default());
    let j = support::json(&bytes);
    let p = &j["meshes"][0]["primitives"];
    assert!(p[0]["attributes"].get("TEXCOORD_0").is_some());
    assert!(p[0]["attributes"].get("TEXCOORD_1").is_none());
    assert!(p[1]["attributes"].get("TEXCOORD_0").is_none());
    let mats = &j["materials"];
    assert!(
        mats[usize::try_from(p[0]["material"].as_u64().unwrap()).unwrap()]["pbrMetallicRoughness"]
            .get("baseColorTexture")
            .is_some()
    );
    assert!(
        mats[usize::try_from(p[1]["material"].as_u64().unwrap()).unwrap()]["pbrMetallicRoughness"]
            .get("baseColorTexture")
            .is_none()
    );
    assert_eq!(report.issues.len(), 2);
    assert_eq!(report.dropped_texcoord_attributes, 2);
    assert_eq!(report.dropped_texture_references, 1);
    assert_eq!(j["asset"]["extras"]["dropped_texcoord_attributes"], 2);
    assert_eq!(j["asset"]["extras"]["dropped_texture_references"], 1);
    collect_support::validator_clean(&scene);
    let (bytes, _) = support::write(&scene, Options { normalise: false });
    let j = support::json(&bytes);
    assert!(
        j["meshes"][0]["primitives"][0]["attributes"]
            .get("TEXCOORD_1")
            .is_some()
    );
    assert!(support::f32s(&bytes, &j, 1)[0].is_infinite());
}

#[test]
#[allow(clippy::too_many_lines)] // The synthetic channel layout and its assertions stay together.
fn generated_uv_gaps_never_substitute_for_valid_source_uvs_after_a_drop() {
    for (bad, valid) in [(0, Some(2)), (1, None)] {
        let meter = Meter::unlimited();
        let mut objects = collect_support::objects(false, false);
        let mut offset = 0;
        let channels: Vec<_> = (0..14)
            .map(|i| {
                let dimension = match i {
                    0 => 3,
                    3 => 4,
                    i if i == 4 + bad || valid.is_some_and(|v| i == 4 + v) => 2,
                    _ => 0,
                };
                let channel = st(
                    "ChannelInfo",
                    vec![
                        ("stream", V::UInt8(0)),
                        ("offset", V::UInt8(offset)),
                        ("format", V::UInt8(0)),
                        ("dimension", V::UInt8(dimension)),
                    ],
                );
                offset += dimension * 4;
                channel
            })
            .collect();
        let mut data = Vec::new();
        for position in [[1f32, 0., 0.], [0., 1., 0.], [0., 0., 0.]] {
            for value in position
                .into_iter()
                .chain([2., -1., 0.5, 1.])
                .chain([f32::NAN, 0.])
            {
                data.extend(value.to_le_bytes());
            }
            if valid.is_some() {
                for value in [0.25f32, 0.75] {
                    data.extend(value.to_le_bytes());
                }
            }
        }
        data.resize(data.len().next_multiple_of(16), 0);
        s::set(
            &mut objects[6].value,
            "m_VertexData",
            st(
                "VertexData",
                vec![
                    ("m_VertexCount", V::UInt(3)),
                    ("m_Channels", vector(channels[0].clone(), channels)),
                    ("m_DataSize", V::Bytes(data)),
                ],
            ),
        );
        s::set(
            &mut objects[6].value,
            "m_Shapes",
            m::blend_shapes(vec![], vec![], vec![], vec![]),
        );
        let (env, graph) = collect_support::graph(&objects);
        let mut scene = collect(&env, &graph, &Profile::new(&meter), &meter).unwrap();
        assert!(!scene.meshes.is_empty());
        scene
            .images
            .push(Image {
                name: None,
                mime: Mime::Png,
                bytes: array(&meter, include_bytes!("goldens/pixel.png")).unwrap(),
            })
            .unwrap();
        scene
            .textures
            .push(Texture {
                image: 0,
                sampler: None,
            })
            .unwrap();
        let mut material = Material::new(&meter);
        material.base_color_texture = Some(TextureInfo {
            texture: 0,
            scale: [1.; 2],
            offset: [0.; 2],
        });
        scene.materials.push(material).unwrap();
        for primitive in scene.meshes.as_mut_slice()[0].primitives.as_mut_slice() {
            primitive.material = Some(0);
        }
        let (bytes, report) = support::write(&scene, Options::default());
        let j = support::json(&bytes);
        assert_eq!(report.color_components, 6); // Shared by both primitives.
        assert_eq!(report.dropped_texcoord_attributes, 2); // Actual bad sets only.
        assert_eq!(
            report.dropped_texture_references,
            if valid.is_some() { 0 } else { 2 }
        );
        let attrs = &j["meshes"][0]["primitives"][0]["attributes"];
        if valid.is_some() {
            let id = usize::try_from(attrs["TEXCOORD_0"].as_u64().unwrap()).unwrap();
            assert_eq!(support::f32s(&bytes, &j, id), [0.25, 0.25].repeat(3));
            assert!(attrs.get("TEXCOORD_1").is_none());
        } else {
            assert!(attrs.get("TEXCOORD_0").is_none());
        }
        collect_support::validator_clean(&scene);
        let (_, raw) = support::write(&scene, Options { normalise: false });
        assert_eq!(
            raw.color_components + raw.dropped_texcoord_attributes + raw.dropped_texture_references,
            0
        );
    }
}

#[test]
fn mismatched_short_skin_widths_remain_reported_renderer_errors() {
    for (joints, weights) in [(1, 2), (2, 1), (4, 1), (4, 2), (2, 0)] {
        let mut objects = short_skin(1, Some([1., 0.]));
        let V::Struct(_, fields) = &mut objects[6].value else {
            panic!("mesh");
        };
        let (_, V::Struct(_, vertex)) = fields
            .iter_mut()
            .find(|(key, _)| key == "m_VertexData")
            .unwrap()
        else {
            panic!("vertices");
        };
        let (_, V::Vector(_, _, values)) = vertex
            .iter_mut()
            .find(|(key, _)| key == "m_Channels")
            .unwrap()
        else {
            panic!("channels");
        };
        for (i, dimension) in [(12, weights), (13, joints)] {
            s::set(&mut values[i], "dimension", V::UInt8(dimension));
        }
        // Keep all channel reads in bounds so the geometry width check is tested.
        let (_, V::Bytes(data)) = vertex
            .iter_mut()
            .find(|(key, _)| key == "m_DataSize")
            .unwrap()
        else {
            panic!("data");
        };
        data.resize(256, 0);
        let (env, graph) = collect_support::graph(&objects);
        let meter = Meter::unlimited();
        let scene = collect(&env, &graph, &Profile::new(&meter), &meter).unwrap();
        assert!(scene.meshes.is_empty());
        assert!(
            scene
                .collection_issues
                .as_slice()
                .iter()
                .any(|i| i.reason == "skin attribute width mismatch")
        );
    }
}
