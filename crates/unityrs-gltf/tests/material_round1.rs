//! Round-one regressions; asymmetric texels and malformed sizes from `mat_edge.py`.
mod collect_support;
mod material_support;
use material_support as h;
use unityrs_gltf::{AlphaMode, CorePbr};
use unityrs_testkit::{
    ngui::{self, Object, V},
    scene3d as s,
};

fn collect(
    shader: &[u8],
    floats: &[(&str, f32)],
    colors: &[(&str, [f32; 4])],
    envs: &[(&str, V)],
) -> unityrs_gltf::Scene {
    h::collect(
        &h::objects(shader, floats, colors, envs),
        &CorePbr,
        h::DEFAULT_PROFILE,
    )
}
fn linear(x: f64) -> f64 {
    if x <= 0.04045 {
        x / 12.92
    } else {
        ((x + 0.055) / 1.055).powf(2.4)
    }
}
fn texture_pixels(scene: &unityrs_gltf::Scene, texture: usize, expected: &[u8]) {
    let image = scene.textures.as_slice()[texture].image;
    h::assert_pixels(scene.images.as_slice()[image].bytes.as_slice(), expected);
}
fn settings(object: &mut Object, u: i32, v: i32, filter: i32) {
    s::set(
        &mut object.value,
        "m_TextureSettings",
        ngui::st(
            "GLTextureSettings",
            vec![
                ("m_WrapU", V::Int(u)),
                ("m_WrapV", V::Int(v)),
                ("m_FilterMode", V::Int(filter)),
            ],
        ),
    );
}
#[test]
fn alpha_uses_shader_controls_not_saved_cutoff_or_queue() {
    for (shader, controls, expected) in [
        (
            b"Standard".as_slice(),
            vec![("_Mode", 0.)],
            AlphaMode::Opaque,
        ),
        (b"Standard", vec![("_Mode", 1.)], AlphaMode::Mask),
        (b"Standard", vec![("_Mode", 2.)], AlphaMode::Blend),
        (b"Standard", vec![("_Mode", 3.)], AlphaMode::Blend),
        (
            b"Universal Render Pipeline/Lit",
            vec![("_Surface", 0.), ("_AlphaClip", 0.)],
            AlphaMode::Opaque,
        ),
        (
            b"Universal Render Pipeline/Lit",
            vec![("_Surface", 0.), ("_AlphaClip", 1.)],
            AlphaMode::Mask,
        ),
        (
            b"Universal Render Pipeline/Lit",
            vec![("_Surface", 1.), ("_AlphaClip", 1.)],
            AlphaMode::Blend,
        ),
    ] {
        let mut floats = controls;
        floats.push(("_Cutoff", 0.375));
        let mut objects = h::objects(shader, &floats, &[], &[]);
        s::set(&mut objects[7].value, "m_CustomRenderQueue", V::Int(3100));
        let scene = h::collect(&objects, &CorePbr, h::DEFAULT_PROFILE);
        let m = &scene.materials.as_slice()[0];
        assert_eq!(m.alpha_mode, expected, "{shader:?}");
        assert_eq!(m.alpha_cutoff, 0.375);
    }
}
#[test]
fn saved_srgb_factors_use_exact_eotf_and_preserve_alpha() {
    let scene = collect(
        b"Standard",
        &[],
        &[("_Color", [0.03125, 0.5, 0.75, 0.375])],
        &[],
    );
    assert_eq!(
        scene.materials.as_slice()[0].base_color,
        [linear(0.03125), linear(0.5), linear(0.75), 0.375]
    );
}
#[test]
fn normal_rgorag_unpack_preserves_positive_green_and_bakes_standard_scale() {
    for (format, pixels, texel) in [
        (4, [255, 192, 0, 128].repeat(4), [128, 176, 245, 255]),
        (4, [128, 192, 255, 255].repeat(4), [128, 176, 245, 255]),
        (3, [128, 192, 255].repeat(4), [128, 176, 245, 255]),
    ] {
        let mut objects = h::objects(
            b"Standard",
            &[("_BumpScale", 0.75)],
            &[],
            &[("_BumpMap", s::tex_env((0, 12), [1.; 2], [0.; 2]))],
        );
        objects[9] = ngui::texture(12, b"normal", format, (2, 2), pixels);
        s::set(&mut objects[9].value, "m_ColorSpace", V::Int(0));
        let scene = h::collect(&objects, &CorePbr, h::DEFAULT_PROFILE);
        let m = &scene.materials.as_slice()[0];
        texture_pixels(&scene, m.normal_texture.unwrap().texture, &texel.repeat(4));
        assert_eq!(m.normal_scale, 1.);
    }
}
#[test]
fn metallic_gloss_repacked_to_linear_blue_green_with_unit_factors() {
    for (shader, key) in [
        (b"Standard".as_slice(), "_GlossMapScale"),
        (b"Universal Render Pipeline/Lit", "_Smoothness"),
    ] {
        let mut objects = h::objects(
            shader,
            &[(key, 0.5), ("_Metallic", 0.2)],
            &[],
            &[("_MetallicGlossMap", s::tex_env((0, 12), [1.; 2], [0.; 2]))],
        );
        objects[9] = ngui::texture(12, b"metal", 4, (2, 2), [64, 20, 30, 128].repeat(4));
        s::set(&mut objects[9].value, "m_ColorSpace", V::Int(0));
        let scene = h::collect(&objects, &CorePbr, h::DEFAULT_PROFILE);
        let j = h::json(&scene);
        let pbr = &j["materials"][0]["pbrMetallicRoughness"];
        assert_eq!(pbr["metallicFactor"], 1.);
        assert_eq!(pbr["roughnessFactor"], 1.);
        let t =
            usize::try_from(pbr["metallicRoughnessTexture"]["index"].as_u64().unwrap()).unwrap();
        texture_pixels(&scene, t, &[0, 191, 64, 255].repeat(4));
    }
}
#[test]
fn metallic_uses_albedo_alpha_when_selected_and_caches_by_recipe() {
    let mut objects = h::objects(
        b"Standard",
        &[("_GlossMapScale", 0.5), ("_SmoothnessTextureChannel", 1.)],
        &[],
        &[
            ("_MetallicGlossMap", s::tex_env((0, 12), [1.; 2], [0.; 2])),
            ("_MainTex", s::tex_env((0, 13), [1.; 2], [0.; 2])),
        ],
    );
    objects[9] = ngui::texture(12, b"metal", 4, (2, 2), [64, 20, 30, 128].repeat(4));
    s::set(&mut objects[9].value, "m_ColorSpace", V::Int(0));
    objects.push(ngui::texture(
        13,
        b"albedo",
        4,
        (2, 2),
        [0, 0, 0, 64].repeat(4),
    ));
    let scene = h::collect(&objects, &CorePbr, h::DEFAULT_PROFILE);
    let j = h::json(&scene);
    let t = usize::try_from(
        j["materials"][0]["pbrMetallicRoughness"]["metallicRoughnessTexture"]["index"]
            .as_u64()
            .unwrap(),
    )
    .unwrap();
    texture_pixels(&scene, t, &[0, 223, 64, 255].repeat(4));
}
#[test]
fn specular_workflow_is_counted_and_retains_unapproximated_saved_inputs() {
    for (shader, floats) in [
        (
            b"Standard (Specular setup)".as_slice(),
            vec![("_Glossiness", 0.75)],
        ),
        (
            b"Universal Render Pipeline/Lit",
            vec![("_WorkflowMode", 0.), ("_Smoothness", 0.75)],
        ),
    ] {
        let scene = collect(
            shader,
            &floats,
            &[("_SpecColor", [0.25, 0.5, 0.75, 1.])],
            &[("_SpecGlossMap", s::tex_env((0, 12), [1.; 2], [0.; 2]))],
        );
        assert_eq!(scene.materials.as_slice()[0].metallic, 0.);
        h::reason(&scene, "specular_workflow");
        let j = h::json(&scene);
        let extras = j["materials"][0]["extras"].to_string();
        assert!(
            extras.contains("_SpecColor") && extras.contains("_SpecGlossMap"),
            "{extras}"
        );
    }
}
#[test]
fn occlusion_uses_green_in_a_separate_cached_image() {
    let mut objects = h::objects(
        b"Standard",
        &[("_OcclusionStrength", 0.5)],
        &[],
        &[
            ("_MainTex", s::tex_env((0, 12), [1.; 2], [0.; 2])),
            ("_OcclusionMap", s::tex_env((0, 12), [1.; 2], [0.; 2])),
        ],
    );
    s::set(&mut objects[9].value, "m_ColorSpace", V::Int(0));
    let scene = h::collect(&objects, &CorePbr, h::DEFAULT_PROFILE);
    let m = &scene.materials.as_slice()[0];
    assert_ne!(
        m.base_color_texture.unwrap().texture,
        m.occlusion_texture.unwrap().texture
    );
    let mut expected = h::TOP_FIRST;
    for p in expected.as_chunks_mut::<4>().0 {
        p[0] = p[1];
    }
    texture_pixels(&scene, m.occlusion_texture.unwrap().texture, &expected);
    assert_eq!(
        h::json(&scene)["materials"][0]["occlusionTexture"]["strength"],
        0.5
    );
}
#[test]
fn emission_keyword_gates_and_hdr_uses_strength_after_linearization() {
    for keywords in [None, Some("m_ShaderKeywords"), Some("m_ValidKeywords")] {
        let mut objects = h::objects(
            b"Standard",
            &[],
            &[("_EmissionColor", [2., 1., 0.5, 1.])],
            &[("_EmissionMap", s::tex_env((0, 12), [1.; 2], [0.; 2]))],
        );
        if let Some(key) = keywords {
            let value = if key == "m_ValidKeywords" {
                ngui::vector(V::Str(vec![]), vec![V::Str(b"_EMISSION".to_vec())])
            } else {
                V::Str(b"_FOO _EMISSION _BAR".to_vec())
            };
            s::set(&mut objects[7].value, key, value);
        }
        let scene = h::collect(&objects, &CorePbr, h::DEFAULT_PROFILE);
        let m = &scene.materials.as_slice()[0];
        if keywords.is_none() {
            assert_eq!(m.emissive, [0.; 3]);
            assert!(m.emissive_texture.is_none());
        } else {
            assert_eq!(m.emissive, [1., 1. / linear(2.), linear(0.5) / linear(2.)]);
            let j = h::json(&scene);
            assert_eq!(
                j["materials"][0]["extensions"]["KHR_materials_emissive_strength"]["emissiveStrength"],
                linear(2.)
            );
            assert!(
                j["extensionsUsed"]
                    .as_array()
                    .unwrap()
                    .contains(&serde_json::json!("KHR_materials_emissive_strength"))
            );
        }
    }
}
#[test]
fn texture_color_space_is_converted_for_each_slot_encoding() {
    for (flag, color, occlusion) in [(0, 188, 128), (1, 128, 55)] {
        let mut objects = h::objects(
            b"Standard",
            &[],
            &[],
            &[
                ("_MainTex", s::tex_env((0, 12), [1.; 2], [0.; 2])),
                ("_OcclusionMap", s::tex_env((0, 12), [1.; 2], [0.; 2])),
            ],
        );
        objects[9] = ngui::texture(12, b"gray", 4, (2, 2), [128, 128, 128, 64].repeat(4));
        s::set(&mut objects[9].value, "m_ColorSpace", V::Int(flag));
        let scene = h::collect(&objects, &CorePbr, h::DEFAULT_PROFILE);
        let m = &scene.materials.as_slice()[0];
        texture_pixels(
            &scene,
            m.base_color_texture.unwrap().texture,
            &[color, color, color, 64].repeat(4),
        );
        texture_pixels(
            &scene,
            m.occlusion_texture.unwrap().texture,
            &[occlusion, occlusion, occlusion, 64].repeat(4),
        );
    }
}
#[test]
fn known_lit_slots_share_base_st_including_negative_scale() {
    for shader in [b"Standard".as_slice(), b"Universal Render Pipeline/Lit"] {
        let mut objects = h::rich(shader);
        // Give the base slot a negative scale, and every other slot a different ST.
        objects[7] = s::material(
            10,
            b"paint",
            (0, 11),
            &[],
            &[],
            &[
                ("_MainTex", s::tex_env((0, 12), [-2., 0.5], [0.25, 0.125])),
                ("_BumpMap", s::tex_env((0, 12), [3.; 2], [0.5; 2])),
                ("_OcclusionMap", s::tex_env((0, 12), [3.; 2], [0.5; 2])),
                ("_EmissionMap", s::tex_env((0, 12), [3.; 2], [0.5; 2])),
            ],
        );
        s::set(
            &mut objects[7].value,
            "m_ShaderKeywords",
            V::Str(b"_EMISSION".to_vec()),
        );
        let scene = h::collect(&objects, &CorePbr, h::DEFAULT_PROFILE);
        let m = &scene.materials.as_slice()[0];
        for t in [
            m.base_color_texture,
            m.normal_texture,
            m.occlusion_texture,
            m.emissive_texture,
        ]
        .into_iter()
        .flatten()
        {
            assert_eq!(t.scale, [-2., 0.5]);
            assert_eq!(t.offset, [0.25, 0.375]);
        }
    }
}
#[test]
fn samplers_preserve_wrap_filter_and_deduplicate() {
    for (u, v, filter, wrap_s, wrap_t, mag, min) in [
        (0, 1, 0, 10497, 33071, 9728, 9728),
        (1, 2, 1, 33071, 33648, 9729, 9729),
        (2, 0, 2, 33648, 10497, 9729, 9729),
        (3, 3, 0, 33648, 33648, 9728, 9728),
    ] {
        let mut objects = h::simple();
        settings(&mut objects[9], u, v, filter);
        let scene = h::collect(&objects, &CorePbr, h::DEFAULT_PROFILE);
        let j = h::json(&scene);
        assert_eq!(j["samplers"].as_array().unwrap().len(), 1);
        let sampler = &j["samplers"][0];
        assert_eq!(sampler["wrapS"], wrap_s);
        assert_eq!(sampler["wrapT"], wrap_t);
        assert_eq!(sampler["magFilter"], mag);
        assert_eq!(sampler["minFilter"], min);
        assert_eq!(j["textures"][0]["sampler"], 0);
        if u == 3 {
            h::reason(&scene, "mirror_once");
        }
    }
}
#[test]
fn submesh_without_material_keeps_geometry_with_counted_default() {
    let mut objects = h::simple();
    objects[5] = Object::new(6, 23, s::mesh_renderer((0, 3), &[(0, 10)], (0, 0)));
    let scene = h::collect(&objects, &CorePbr, h::DEFAULT_PROFILE);
    let p = scene.meshes.as_slice()[0].primitives.as_slice();
    assert_eq!(p.len(), 2);
    assert!(p[1].material.is_none());
    h::reason(&scene, "submesh_without_material");
}
#[test]
fn malformed_huge_claimed_texture_is_local_data_skip() {
    // The empirical review's huge_claimed_texture: > 2^28 pixels, five bytes.
    let mut objects = h::simple();
    objects[9] = ngui::texture(12, b"huge", 4, (1 << 15, 1 << 15), vec![0; 5]);
    let scene = h::collect(&objects, &CorePbr, h::DEFAULT_PROFILE);
    assert_eq!(scene.materials.len(), 1);
    assert!(scene.images.is_empty());
    h::reason(&scene, "texture_too_large");
}
fn field<'a>(v: &'a mut V, name: &str) -> &'a mut V {
    let V::Struct(_, fields) = v else {
        panic!("struct")
    };
    &mut fields.iter_mut().find(|(n, _)| n == name).unwrap().1
}
fn add_colors(mesh: &mut Object) {
    let v = field(&mut mesh.value, "m_VertexData");
    let V::Vector(_, _, channels) = field(v, "m_Channels") else {
        panic!("channels")
    };
    channels[3] = channels[2].clone();
    s::set(&mut channels[3], "offset", V::UInt8(40));
    for c in &mut channels[4..] {
        if let V::UInt8(offset) = field(c, "offset") {
            *offset += 16;
        }
    }
    let V::Bytes(data) = field(v, "m_DataSize") else {
        panic!("data")
    };
    let mut colored = Vec::new();
    for row in data.as_chunks::<80>().0 {
        colored.extend_from_slice(&row[..40]);
        colored.extend(
            [0.25f32, 0.5, 0.75, 1.]
                .into_iter()
                .flat_map(f32::to_le_bytes),
        );
        colored.extend_from_slice(&row[40..]);
    }
    *data = colored;
}
#[test]
fn ignored_vertex_colors_are_custom_attributes_per_material_binding() {
    for (shader, ignored) in [
        (b"Standard".as_slice(), true),
        (b"Standard (Specular setup)", true),
        (b"Universal Render Pipeline/Lit", true),
        (b"Universal Render Pipeline/Simple Lit", true),
        (b"Unlit/Texture", true),
        (b"Unlit/Color", true),
        (b"Sprites/Default", false),
        (b"Custom/Unknown", false),
    ] {
        let mut objects = h::objects(shader, &[], &[], &[]);
        // Tangent's [1,0,0,1] is also a valid colour.
        add_colors(&mut objects[6]);
        let scene = h::collect(&objects, &CorePbr, h::DEFAULT_PROFILE);
        let j = h::json(&scene);
        assert!(!scene.meshes.is_empty(), "{scene:?}");
        let a = &j["meshes"][0]["primitives"][0]["attributes"];
        assert_eq!(a.get("_COLOR_0").is_some(), ignored, "{shader:?}: {a}");
        assert_eq!(a.get("COLOR_0").is_some(), !ignored, "{shader:?}: {a}");
        if ignored {
            h::reason(&scene, "vertex_colors_ignored");
        }
    }
}
#[test]
fn legacy_subset_permutation_pairs_materials_by_subset_position() {
    let mut objects = h::simple();
    s::set(
        &mut objects[6].value,
        "m_SubMeshes",
        ngui::vector(
            s::mesh::submesh(0, 0, 0, 0, 0, 0),
            vec![
                s::mesh::submesh(0, 3, 0, 0, 0, 3),
                s::mesh::submesh(12, 3, 0, 0, 0, 3),
                s::mesh::submesh(24, 3, 0, 0, 0, 3),
            ],
        ),
    );
    s::set(
        &mut objects[6].value,
        "m_IndexBuffer",
        V::Bytes(
            [0u32, 1, 2, 0, 1, 2, 0, 1, 2]
                .into_iter()
                .flat_map(u32::to_le_bytes)
                .collect(),
        ),
    );
    objects[5] = Object::new(
        6,
        23,
        s::mesh_renderer((0, 3), &[(0, 10), (0, 13), (0, 14)], (0, 0)),
    );
    s::set(
        &mut objects[5].value,
        "m_SubsetIndices",
        ngui::vector(V::UInt(0), vec![V::UInt(2), V::UInt(0), V::UInt(1)]),
    );
    objects.extend([
        s::material(13, b"B", (0, 11), &[], &[], &[]),
        s::material(14, b"C", (0, 11), &[], &[], &[]),
    ]);
    let scene = h::collect(&objects, &CorePbr, h::DEFAULT_PROFILE);
    let names: Vec<_> = scene.meshes.as_slice()[0]
        .primitives
        .as_slice()
        .iter()
        .map(|p| {
            scene.materials.as_slice()[p.material.unwrap()]
                .name
                .as_ref()
                .unwrap()
                .as_bytes()
        })
        .collect();
    assert_eq!(names, vec![b"paint".as_slice(), b"B", b"C"]);
}

#[test]
fn normal_dxt5nm_and_bc5_blocks_match_the_unity_channel_reference() {
    // Constant blocks from mat_gen.normal_texture's construction.
    let bc5 = vec![128, 128, 0, 0, 0, 0, 0, 0, 192, 192, 0, 0, 0, 0, 0, 0];
    let dxt5 = vec![128, 128, 0, 0, 0, 0, 0, 0, 0, 252, 0, 0, 0, 0, 0, 0];
    for (format, data, expected) in [
        (27, bc5, [128, 192, 237, 255]),
        (12, dxt5, [128, 130, 255, 255]),
    ] {
        let mut objects = h::objects(
            b"Standard",
            &[],
            &[],
            &[("_BumpMap", s::tex_env((0, 12), [1.; 2], [0.; 2]))],
        );
        objects[9] = ngui::texture(12, b"packed normal", format, (2, 2), data);
        s::set(&mut objects[9].value, "m_ColorSpace", V::Int(0));
        let scene = h::collect(&objects, &CorePbr, h::DEFAULT_PROFILE);
        texture_pixels(
            &scene,
            scene.materials.as_slice()[0]
                .normal_texture
                .unwrap()
                .texture,
            &expected.repeat(4),
        );
    }
}
#[test]
fn metallic_cache_distinguishes_smoothness() {
    let mut objects = h::objects(
        b"Standard",
        &[("_GlossMapScale", 0.5)],
        &[],
        &[("_MetallicGlossMap", s::tex_env((0, 12), [1.; 2], [0.; 2]))],
    );
    objects[9] = ngui::texture(12, b"metal", 4, (2, 2), [64, 0, 0, 128].repeat(4));
    s::set(&mut objects[9].value, "m_ColorSpace", V::Int(0));
    objects[5] = Object::new(6, 23, s::mesh_renderer((0, 3), &[(0, 10), (0, 13)], (0, 0)));
    objects.push(s::material(
        13,
        b"other smoothness",
        (0, 11),
        &[("_GlossMapScale", 1.)],
        &[],
        &[("_MetallicGlossMap", s::tex_env((0, 12), [1.; 2], [0.; 2]))],
    ));
    let scene = h::collect(&objects, &CorePbr, h::DEFAULT_PROFILE);
    let j = h::json(&scene);
    let indices: Vec<_> = j["materials"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            usize::try_from(
                m["pbrMetallicRoughness"]["metallicRoughnessTexture"]["index"]
                    .as_u64()
                    .unwrap(),
            )
            .unwrap()
        })
        .collect();
    assert_ne!(indices[0], indices[1]);
    texture_pixels(&scene, indices[0], &[0, 191, 64, 255].repeat(4));
    texture_pixels(&scene, indices[1], &[0, 127, 64, 255].repeat(4));
}
#[test]
fn mip_filters_and_legacy_wrap_are_explicit_and_shared_across_usages() {
    for (filter, mips, mag, min) in [
        (0, 1, 9728, 9728),
        (1, 1, 9729, 9729),
        (2, 1, 9729, 9729),
        (0, 2, 9728, 9984),
        (1, 2, 9729, 9985),
        (2, 2, 9729, 9987),
    ] {
        let mut objects = h::rich(b"Standard");
        s::set(
            &mut objects[9].value,
            "m_TextureSettings",
            ngui::st(
                "GLTextureSettings",
                vec![("m_WrapMode", V::Int(2)), ("m_FilterMode", V::Int(filter))],
            ),
        );
        s::set(&mut objects[9].value, "m_MipCount", V::Int(mips));
        let scene = h::collect(&objects, &CorePbr, h::DEFAULT_PROFILE);
        let j = h::json(&scene);
        assert_eq!(j["samplers"].as_array().unwrap().len(), 1);
        assert_eq!(j["samplers"][0]["wrapS"], 33648);
        assert_eq!(j["samplers"][0]["wrapT"], 33648);
        assert_eq!(j["samplers"][0]["magFilter"], mag);
        assert_eq!(j["samplers"][0]["minFilter"], min);
        for texture in j["textures"].as_array().unwrap() {
            assert_eq!(texture["sampler"], 0);
        }
    }
}
#[test]
fn shared_st_uses_null_base_env_and_explicit_vector_for_metallic_too() {
    let scene = collect(
        b"Standard",
        &[],
        &[("_MainTex_ST", [-1., 0.5, 0.25, 0.125])],
        &[
            ("_MainTex", s::tex_env((0, 0), [3.; 2], [2.; 2])),
            ("_MetallicGlossMap", s::tex_env((0, 12), [4.; 2], [1.; 2])),
            ("_BumpMap", s::tex_env((0, 12), [4.; 2], [1.; 2])),
        ],
    );
    let m = &scene.materials.as_slice()[0];
    let j = h::json(&scene);
    assert_eq!(
        j["materials"][0]["pbrMetallicRoughness"]["metallicRoughnessTexture"]["extensions"]["KHR_texture_transform"]
            ["scale"],
        serde_json::json!([-1, 0.5])
    );
    assert_eq!(m.normal_texture.unwrap().scale, [-1., 0.5]);
    assert_eq!(m.normal_texture.unwrap().offset, [0.25, 0.375]);
}

#[test]
fn metallic_cache_distinguishes_albedo_alpha_source_identity() {
    let mut objects = h::objects(
        b"Standard",
        &[("_GlossMapScale", 1.), ("_SmoothnessTextureChannel", 1.)],
        &[],
        &[
            ("_MetallicGlossMap", s::tex_env((0, 12), [1.; 2], [0.; 2])),
            ("_MainTex", s::tex_env((0, 14), [1.; 2], [0.; 2])),
        ],
    );
    objects[9] = ngui::texture(12, b"metal", 4, (2, 2), [64, 0, 0, 128].repeat(4));
    s::set(&mut objects[9].value, "m_ColorSpace", V::Int(0));
    objects[5] = Object::new(6, 23, s::mesh_renderer((0, 3), &[(0, 10), (0, 13)], (0, 0)));
    objects.extend([
        s::material(
            13,
            b"other alpha",
            (0, 11),
            &[("_GlossMapScale", 1.), ("_SmoothnessTextureChannel", 1.)],
            &[],
            &[
                ("_MetallicGlossMap", s::tex_env((0, 12), [1.; 2], [0.; 2])),
                ("_MainTex", s::tex_env((0, 15), [1.; 2], [0.; 2])),
            ],
        ),
        ngui::texture(14, b"first", 4, (2, 2), [0, 0, 0, 32].repeat(4)),
        ngui::texture(15, b"second", 4, (2, 2), [0, 0, 0, 192].repeat(4)),
    ]);
    let scene = h::collect(&objects, &CorePbr, h::DEFAULT_PROFILE);
    let j = h::json(&scene);
    let indices: Vec<_> = j["materials"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            usize::try_from(
                m["pbrMetallicRoughness"]["metallicRoughnessTexture"]["index"]
                    .as_u64()
                    .unwrap(),
            )
            .unwrap()
        })
        .collect();
    assert_ne!(indices[0], indices[1]);
    texture_pixels(&scene, indices[0], &[0, 223, 64, 255].repeat(4));
    texture_pixels(&scene, indices[1], &[0, 63, 64, 255].repeat(4));
}
