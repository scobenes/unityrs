mod collect_support;
mod material_support;
use material_support as h;
use unityrs_gltf::{AlphaMode, CorePbr, UnlitExtras};
use unityrs_testkit::{
    ngui::{self, Object, V},
    scene3d as s,
};

#[test]
fn default_mapper_exports_unlit_color_texture_and_deduplicated_materials() {
    let (env, graph) = collect_support::graph(&h::simple());
    let meter = collect_support::meter();
    let scene =
        unityrs_gltf::collect(&env, &graph, &unityrs_gltf::Profile::new(&meter), &meter).unwrap();
    h::validate(&scene, "default-unlit");
    assert_eq!(scene.materials.len(), 1);
    let mat = &scene.materials.as_slice()[0];
    assert!(mat.unlit);
    assert_eq!(mat.base_color, h::linear_color([0.25, 0.5, 0.75, 1.]));
    assert_eq!(mat.base_color_texture.as_ref().unwrap().texture, 0);
    assert_eq!(mat.name.as_ref().unwrap().as_bytes(), b"paint");
    assert_eq!(scene.images.len(), 1);
    assert_eq!(scene.textures.len(), 1);
    for primitive in scene.meshes.as_slice()[0].primitives.as_slice() {
        assert_eq!(primitive.material, Some(0));
    }
    let json = h::json(&scene);
    h::assert_factor(
        &json["materials"][0]["pbrMetallicRoughness"]["baseColorFactor"],
        &h::linear_color([0.25, 0.5, 0.75, 1.]),
    );
    assert_eq!(
        json["materials"][0]["extensions"]["KHR_materials_unlit"],
        serde_json::json!({})
    );
    assert!(
        json["extensionsUsed"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("KHR_materials_unlit"))
    );
}

#[test]
fn unlit_aliases_and_allowlisted_properties_are_preserved() {
    let objects = h::objects(
        b"Standard",
        &[("_Keep", 0.25), ("_Secret", 0.5)],
        &[("_BaseColor", [0.5, 0.25, 0.125, 1.])],
        &[("_BaseMap", s::tex_env((0, 12), [1.; 2], [0.; 2]))],
    );
    let scene = h::collect(
        &objects,
        &UnlitExtras,
        br#"{"profile_version":1,"material_property_allowlist":["_Keep"]}"#,
    );
    assert_eq!(
        scene.materials.as_slice()[0].base_color,
        h::linear_color([0.5, 0.25, 0.125, 1.])
    );
    assert_eq!(
        scene.materials.as_slice()[0]
            .base_color_texture
            .as_ref()
            .unwrap()
            .texture,
        0
    );
    let extras = &h::json(&scene)["materials"][0]["extras"];
    assert!(extras.to_string().contains("_Keep"), "{extras}");
    assert!(extras.to_string().contains("0.25"), "{extras}");
    assert!(!extras.to_string().contains("_Secret"), "{extras}");
}

#[test]
fn core_pbr_maps_supported_shader_factors_and_all_texture_slots() {
    for shader in [
        b"Standard".as_slice(),
        b"Standard (Specular setup)",
        b"Universal Render Pipeline/Lit",
    ] {
        let scene = h::collect(&h::rich(shader), &CorePbr, h::DEFAULT_PROFILE);
        let mat = &scene.materials.as_slice()[0];
        assert!(!mat.unlit);
        assert_eq!(mat.base_color, h::linear_color([0.25, 0.5, 0.75, 1.]));
        assert_eq!(
            mat.metallic,
            if shader == b"Standard (Specular setup)" {
                0.
            } else {
                0.25
            }
        );
        assert_eq!(mat.roughness, 0.25);
        assert_eq!(
            mat.normal_scale,
            if shader == b"Universal Render Pipeline/Lit" {
                0.75
            } else {
                1.
            }
        );
        assert_eq!(mat.occlusion_strength, 0.5);
        assert_eq!(mat.emissive, [0.125, 0.25, 0.5].map(h::linear));
        assert_eq!(mat.alpha_mode, AlphaMode::Mask);
        assert_eq!(mat.alpha_cutoff, 0.375);
        assert!(mat.double_sided);
        assert_eq!(scene.images.len(), 3);
        assert_eq!(scene.textures.len(), 3);
        for (id, slot) in [
            &mat.base_color_texture,
            &mat.normal_texture,
            &mat.occlusion_texture,
            &mat.emissive_texture,
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(slot.as_ref().unwrap().texture, [0, 1, 2, 0][id]);
        }
        let json = h::json(&scene);
        let mapped = &json["materials"][0];
        assert_eq!(
            mapped["pbrMetallicRoughness"]["metallicFactor"],
            mat.metallic
        );
        assert_eq!(mapped["pbrMetallicRoughness"]["roughnessFactor"], 0.25);
        assert_eq!(mapped["normalTexture"]["scale"], mat.normal_scale);
        assert_eq!(mapped["occlusionTexture"]["strength"], 0.5);
        h::assert_factor(
            &mapped["emissiveFactor"],
            &[0.125, 0.25, 0.5].map(h::linear),
        );
        assert_eq!(mapped["alphaMode"], "MASK");
        assert_eq!(mapped["alphaCutoff"], 0.375);
        assert_eq!(mapped["doubleSided"], true);
    }
}

#[test]
fn texture_transform_and_asymmetric_image_keep_the_same_uv_lookup() {
    let scene = h::collect(&h::rich(b"Standard"), &CorePbr, h::DEFAULT_PROFILE);
    let texture = scene.materials.as_slice()[0]
        .base_color_texture
        .as_ref()
        .unwrap();
    assert_eq!(texture.scale, [0.5, 0.25]);
    assert_eq!(texture.offset, [0.125, 0.625]); // 1 - sy - oy.
    let json = h::json(&scene);
    let tr = &json["materials"][0]["pbrMetallicRoughness"]["baseColorTexture"]["extensions"]["KHR_texture_transform"];
    assert_eq!(tr["scale"], serde_json::json!([0.5, 0.25]));
    assert_eq!(tr["offset"], serde_json::json!([0.125, 0.625]));
    assert!(
        json["extensionsUsed"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("KHR_texture_transform"))
    );
    let embedded = h::embedded_image(&scene);
    assert_eq!(embedded, scene.images.as_slice()[0].bytes.as_slice());
    h::assert_pixels(&embedded, &h::TOP_FIRST);
    let decoded = unityrs_texture::decode(
        unityrs_texture::Format::Rgba32,
        &h::PIXELS,
        2,
        2,
        &collect_support::meter(),
    )
    .unwrap();
    h::assert_pixels(&embedded, decoded.pixels());
    let attributes = scene.meshes.as_slice()[0].primitives.as_slice()[0]
        .attributes
        .as_slice();
    let uv = attributes
        .iter()
        .find(|(kind, _)| *kind == unityrs_gltf::Attribute::TexCoord(0))
        .unwrap()
        .1;
    let tangent = attributes
        .iter()
        .find(|(kind, _)| *kind == unityrs_gltf::Attribute::Tangent)
        .unwrap()
        .1;
    assert_eq!(collect_support::floats(&scene, uv)[1], 0.75);
    assert_eq!(collect_support::floats(&scene, tangent)[3], -1.);
}

#[test]
fn smoothness_alpha_surface_and_queue_rules_have_expected_defaults() {
    for (floats, queue, mode, roughness) in [
        (vec![], -1, AlphaMode::Opaque, 0.5),
        (
            vec![("_Smoothness", 0.625), ("_Surface", 1.)],
            -1,
            AlphaMode::Blend,
            0.375,
        ),
        (vec![("_AlphaClip", 1.)], -1, AlphaMode::Mask, 0.5),
        (vec![], 3000, AlphaMode::Opaque, 0.5),
    ] {
        let mut objects = h::objects(b"Universal Render Pipeline/Lit", &floats, &[], &[]);
        s::set(&mut objects[7].value, "m_CustomRenderQueue", V::Int(queue));
        let scene = h::collect(&objects, &CorePbr, h::DEFAULT_PROFILE);
        let mat = &scene.materials.as_slice()[0];
        assert_eq!(mat.alpha_mode, mode);
        assert_eq!(mat.roughness, roughness);
        assert_eq!(mat.alpha_cutoff, 0.5);
        assert!(!mat.double_sided);
    }
}

#[test]
fn missing_materials_and_textures_degrade_with_report_and_asset_reasons() {
    let mut objects = h::simple();
    objects[5] = Object::new(
        6,
        23,
        s::mesh_renderer((0, 3), &[(0, 999), (0, 999)], (0, 0)),
    );
    let scene = h::collect(&objects, &CorePbr, h::DEFAULT_PROFILE);
    assert!(scene.materials.is_empty());
    assert!(
        scene.meshes.as_slice()[0]
            .primitives
            .as_slice()
            .iter()
            .all(|p| p.material.is_none())
    );
    h::reason(&scene, "missing_material");
    let scene = h::collect(&h::missing_texture_objects(), &CorePbr, h::DEFAULT_PROFILE);
    assert_eq!(scene.materials.len(), 1);
    assert!(scene.images.is_empty());
    assert!(scene.materials.as_slice()[0].base_color_texture.is_none());
    h::reason(&scene, "missing_texture");
}

#[test]
fn unsupported_and_missing_or_short_texture_data_leave_factors() {
    for (objects, reason) in [
        (h::texture_case(999, vec![0; 16]), "undecodable_texture"),
        // ARGBFloat remains unsupported; BC6H now decodes.
        (h::texture_case(6, vec![0; 16]), "undecodable_texture"),
        (
            h::texture_case(ngui::format::RGBA32, vec![]),
            "missing_texture",
        ),
        (
            h::texture_case(ngui::format::RGBA32, vec![0; 3]),
            "texture_malformed",
        ),
    ] {
        let scene = h::collect(&objects, &CorePbr, h::DEFAULT_PROFILE);
        assert!(scene.images.is_empty());
        assert!(scene.textures.is_empty());
        let mat = &scene.materials.as_slice()[0];
        assert_eq!(mat.base_color, h::linear_color([0.25, 0.5, 0.75, 1.]));
        assert!(mat.base_color_texture.is_none());
        h::reason(&scene, reason);
    }
}

#[test]
fn distinct_materials_share_one_decoded_image_and_texture() {
    let mut objects = h::simple();
    objects[5] = Object::new(6, 23, s::mesh_renderer((0, 3), &[(0, 10), (0, 13)], (0, 0)));
    objects.push(s::material(
        13,
        b"second",
        (0, 11),
        &[],
        &[("_Color", [1., 0., 0., 1.])],
        &[("_MainTex", s::tex_env((0, 12), [2.; 2], [0.; 2]))],
    ));
    let scene = h::collect(&objects, &CorePbr, h::DEFAULT_PROFILE);
    assert_eq!(scene.materials.len(), 2);
    assert_eq!(scene.images.len(), 1);
    assert_eq!(scene.textures.len(), 1);
    assert_eq!(
        scene.meshes.as_slice()[0].primitives.as_slice()[0].material,
        Some(0)
    );
    assert_eq!(
        scene.meshes.as_slice()[0].primitives.as_slice()[1].material,
        Some(1)
    );
    assert_eq!(
        scene.materials.as_slice()[0]
            .base_color_texture
            .as_ref()
            .unwrap()
            .texture,
        scene.materials.as_slice()[1]
            .base_color_texture
            .as_ref()
            .unwrap()
            .texture
    );
    assert_eq!(
        scene.materials.as_slice()[1]
            .base_color_texture
            .as_ref()
            .unwrap()
            .scale,
        [2.; 2]
    );
}

#[test]
fn static_batch_first_submesh_uses_material_slot_zero() {
    let mut objects = h::simple();
    objects[5] = Object::new(6, 23, s::mesh_renderer((0, 3), &[(0, 10)], (1, 1)));
    let scene = h::collect(&objects, &CorePbr, h::DEFAULT_PROFILE);
    assert_eq!(scene.materials.len(), 1);
    assert_eq!(scene.meshes.as_slice()[0].primitives.len(), 1);
    assert_eq!(
        scene.meshes.as_slice()[0].primitives.as_slice()[0].material,
        Some(0)
    );
    assert!(
        !scene
            .collection_issues
            .as_slice()
            .iter()
            .any(|i| i.reason == "missing_material")
    );
}

#[test]
fn unknown_shader_falls_back_unlit_and_shader_patterns_apply_precedence() {
    let objects = h::objects(b"Synthetic/Glow", &[], &[], &[]);
    let scene = h::collect(&objects, &CorePbr, h::DEFAULT_PROFILE);
    assert!(scene.materials.as_slice()[0].unlit);
    h::reason(&scene, "unknown_shader");
    let scene = h::collect(
        &objects,
        &CorePbr,
        br#"{"profile_version":1,"shader_rules":{"drawable":["Synthetic/*"]}}"#,
    );
    assert!(!scene.materials.as_slice()[0].unlit);
    assert!(
        !scene
            .collection_issues
            .as_slice()
            .iter()
            .any(|i| i.reason == "unknown_shader")
    );
    let scene = h::collect(&objects, &CorePbr, br#"{"profile_version":1,"shader_rules":{"drawable":["Synthetic/*"],"additive":["*/Glow"]}}"#);
    assert_eq!(scene.materials.as_slice()[0].alpha_mode, AlphaMode::Blend);
    let scene = h::collect(&objects, &CorePbr, br#"{"profile_version":1,"shader_rules":{"drawable":["Synthetic/*"],"additive":["*/Glow"],"skip":["Synthetic/Glo?"]}}"#);
    assert!(scene.materials.is_empty());
    assert!(
        scene
            .meshes
            .as_slice()
            .iter()
            .all(|mesh| mesh.primitives.is_empty())
    );
    h::reason(&scene, "shader_skipped");
}

#[test]
fn malformed_source_objects_degrade_without_losing_geometry() {
    for (index, reason) in [
        (7, "malformed_material"),
        (8, "unknown_shader"),
        (9, "texture_malformed"),
    ] {
        let mut objects = h::simple();
        objects[index] = objects[index].clone().cut_to(0);
        let scene = h::collect(&objects, &CorePbr, h::DEFAULT_PROFILE);
        assert_eq!(scene.meshes.as_slice()[0].primitives.len(), 2);
        if index == 7 {
            assert!(scene.materials.is_empty());
        } else {
            assert_eq!(scene.materials.len(), 1);
            let mat = &scene.materials.as_slice()[0];
            assert_eq!(mat.base_color, h::linear_color([0.25, 0.5, 0.75, 1.]));
            if index == 8 {
                assert!(mat.unlit);
            } else {
                assert!(mat.base_color_texture.is_none());
            }
        }
        h::reason(&scene, reason);
    }
    for (index, field, value, reason) in [
        (7, "m_SavedProperties", V::Int(7), "malformed_material"),
        (8, "m_ParsedForm", V::Int(7), "unknown_shader"),
        (
            9,
            "m_Width",
            V::Str(b"invalid".to_vec()),
            "texture_malformed",
        ),
    ] {
        let mut objects = h::simple();
        s::set(&mut objects[index].value, field, value);
        let scene = h::collect(&objects, &CorePbr, h::DEFAULT_PROFILE);
        assert_eq!(scene.meshes.as_slice()[0].primitives.len(), 2);
        h::reason(&scene, reason);
    }
}

#[test]
fn malformed_factor_after_texture_decode_does_not_leave_orphan_resources() {
    let mut objects = h::simple();
    let V::Struct(_, material) = &mut objects[7].value else {
        panic!("material")
    };
    let (_, saved) = material
        .iter_mut()
        .find(|(key, _)| key == "m_SavedProperties")
        .unwrap();
    let bad = ngui::pair(V::Str(b"_Metallic".to_vec()), V::Str(b"invalid".to_vec()));
    s::set(
        saved,
        "m_Floats",
        V::Vector("map".into(), Box::new(bad.clone()), vec![bad]),
    );
    let scene = h::collect(&objects, &CorePbr, h::DEFAULT_PROFILE);
    assert!(scene.materials.is_empty());
    assert!(scene.images.is_empty());
    assert!(scene.textures.is_empty());
    h::reason(&scene, "malformed_material");
}

#[test]
fn missing_streamed_texture_resource_leaves_material_factors() {
    let mut objects = h::simple();
    objects[9] = ngui::texture_streamed(
        12,
        b"asymmetric",
        ngui::format::RGBA32,
        (2, 2),
        "synthetic.resS",
        0,
        16,
    );
    let scene = h::collect(&objects, &CorePbr, h::DEFAULT_PROFILE);
    assert_eq!(
        scene.materials.as_slice()[0].base_color,
        h::linear_color([0.25, 0.5, 0.75, 1.])
    );
    assert!(scene.materials.as_slice()[0].base_color_texture.is_none());
    assert!(scene.images.is_empty());
    assert!(!scene.collection_issues.is_empty());
}

#[test]
fn primitives_without_uvs_keep_material_factors_without_texture_slots() {
    let mut objects = h::rich(b"Standard");
    objects[6] = s::mesh::mesh(
        7,
        b"mesh",
        &[[1., 0., 0.], [0., 1., 0.], [0., 0., 0.]],
        &[0, 1, 2],
        s::mesh::MeshVersion::V2022,
    );
    let scene = h::collect(&objects, &CorePbr, h::DEFAULT_PROFILE);
    assert_eq!(scene.materials.len(), 1);
    let mat = &scene.materials.as_slice()[0];
    assert_eq!(mat.base_color, h::linear_color([0.25, 0.5, 0.75, 1.]));
    assert_eq!(mat.metallic, 0.25);
    for texture in [
        &mat.base_color_texture,
        &mat.normal_texture,
        &mat.occlusion_texture,
        &mat.emissive_texture,
    ] {
        assert!(texture.is_none());
    }
    h::reason(&scene, "missing_texture_uv");
}

#[test]
fn shared_material_gets_distinct_textured_and_factors_only_bindings() {
    let mut objects = h::simple();
    s::set(
        &mut objects[1].value,
        "m_Children",
        s::pointers("Transform", &[(0, 4), (0, 21)]),
    );
    objects.extend([
        Object::new(
            20,
            1,
            s::game_object(b"without-uv", &[(0, 21), (0, 22), (0, 23)]),
        ),
        Object::new(21, 4, s::transform((0, 20), (0, 2), &[], false)),
        Object::new(22, 33, s::mesh_filter((0, 20), (0, 24))),
        Object::new(23, 23, s::mesh_renderer((0, 20), &[(0, 10)], (0, 0))),
        s::mesh::mesh(
            24,
            b"without-uv",
            &[[1., 0., 0.], [0., 1., 0.], [0., 0., 0.]],
            &[0, 1, 2],
            s::mesh::MeshVersion::V2022,
        ),
    ]);
    let scene = h::collect(&objects, &CorePbr, h::DEFAULT_PROFILE);
    assert_eq!(scene.materials.len(), 2);
    assert_eq!(scene.images.len(), 1);
    assert_eq!(scene.textures.len(), 1);
    let textured = scene.meshes.as_slice()[0].primitives.as_slice()[0]
        .material
        .unwrap();
    let plain = scene.meshes.as_slice()[1].primitives.as_slice()[0]
        .material
        .unwrap();
    assert_ne!(textured, plain);
    assert!(
        scene.materials.as_slice()[textured]
            .base_color_texture
            .is_some()
    );
    assert!(
        scene.materials.as_slice()[plain]
            .base_color_texture
            .is_none()
    );
    assert_eq!(
        scene.materials.as_slice()[textured].base_color,
        scene.materials.as_slice()[plain].base_color
    );
    h::reason(&scene, "missing_texture_uv");
}

#[test]
fn urp_property_aliases_map_base_normal_and_smoothness() {
    let objects = h::objects(
        b"Universal Render Pipeline/Lit",
        &[("_Smoothness", 0.625), ("_BumpScale", 0.5)],
        &[("_BaseColor", [0.125, 0.25, 0.5, 1.])],
        &[
            ("_BaseMap", s::tex_env((0, 12), [1.; 2], [0.; 2])),
            ("_NormalMap", s::tex_env((0, 12), [1.; 2], [0.; 2])),
        ],
    );
    let scene = h::collect(&objects, &CorePbr, h::DEFAULT_PROFILE);
    let mat = &scene.materials.as_slice()[0];
    assert_eq!(mat.base_color, h::linear_color([0.125, 0.25, 0.5, 1.]));
    assert_eq!(mat.roughness, 0.375);
    assert_eq!(mat.normal_scale, 0.5);
    assert!(mat.base_color_texture.is_some());
    assert!(mat.normal_texture.is_some());
}

// Standard's null `_MainTex` falls through to a populated `_BaseMap`. A URP
// base shader's null `_BaseMap` is no texture: `material_round3.rs`
// a_null_urp_base_map_is_no_texture_even_with_a_main_tex.
#[test]
fn null_preferred_texture_falls_through_to_populated_alias() {
    let objects = h::objects(
        b"Standard",
        &[],
        &[],
        &[
            ("_MainTex", s::tex_env((0, 0), [1.; 2], [0.; 2])),
            ("_BaseMap", s::tex_env((0, 12), [1.; 2], [0.; 2])),
        ],
    );
    let scene = h::collect(&objects, &CorePbr, h::DEFAULT_PROFILE);
    assert_eq!(
        scene.materials.as_slice()[0]
            .base_color_texture
            .as_ref()
            .unwrap()
            .texture,
        0
    );
    assert_eq!(scene.images.len(), 1);
}

#[test]
fn keep_combined_submeshes_retain_their_own_material_range() {
    let mut objects = h::simple();
    objects[2] = Object::new(
        3,
        1,
        s::game_object(b"child", &[(0, 4), (0, 5), (0, 6), (0, 14)]),
    );
    objects[5] = Object::new(6, 23, s::mesh_renderer((0, 3), &[(0, 10)], (0, 1)));
    objects.extend([
        s::material(
            13,
            b"second",
            (0, 11),
            &[("_Surface", 1.)],
            &[("_Color", [1., 0., 0., 1.])],
            &[],
        ),
        Object::new(14, 23, s::mesh_renderer((0, 3), &[(0, 13)], (1, 1))),
    ]);
    let scene = h::collect(
        &objects,
        &CorePbr,
        br#"{"profile_version":1,"static_batch":"keep_combined"}"#,
    );
    assert_eq!(scene.meshes.len(), 1);
    assert_eq!(scene.materials.len(), 2);
    assert_eq!(scene.meshes.as_slice()[0].primitives.len(), 2);
    let first = &scene.meshes.as_slice()[0].primitives.as_slice()[0];
    let second = &scene.meshes.as_slice()[0].primitives.as_slice()[1];
    assert_ne!(first.material, second.material);
    assert_eq!(
        scene.materials.as_slice()[first.material.unwrap()]
            .name
            .as_ref()
            .unwrap()
            .as_bytes(),
        b"paint"
    );
    assert_eq!(
        scene.materials.as_slice()[second.material.unwrap()]
            .name
            .as_ref()
            .unwrap()
            .as_bytes(),
        b"second"
    );
    assert!(
        !scene
            .collection_issues
            .as_slice()
            .iter()
            .any(|issue| issue.reason == "missing_material")
    );
}

#[test]
fn material_output_is_deterministic_and_all_cases_pass_pinned_validator() {
    for (name, scene) in h::validator_cases() {
        assert_eq!(
            collect_support::write(&scene),
            collect_support::write(&scene)
        );
        h::validate(&scene, name);
    }
}
