//! Round-three built-in unlit and packed sampler regressions.
mod collect_support;
mod material_support;
use material_support as h;
use unityrs_gltf::{AlphaMode, CorePbr, MaterialMapper, UnlitExtras};
use unityrs_testkit::{
    ngui::{self, V},
    scene3d as s,
};

#[test]
fn built_in_unlit_alpha_modes_use_shader_defaults_with_inherited_queue() {
    for mapper in [&CorePbr as &dyn MaterialMapper, &UnlitExtras] {
        for (shader, floats, mode, cutoff) in [
            (
                b"Unlit/Transparent".as_slice(),
                vec![],
                AlphaMode::Blend,
                0.5,
            ),
            (
                b"Unlit/Transparent Cutout",
                vec![("_Cutoff", 0.5)],
                AlphaMode::Mask,
                0.5,
            ),
            (b"Unlit/Transparent Cutout", vec![], AlphaMode::Mask, 0.5),
            (
                b"Unlit/Transparent Cutout",
                vec![("_Cutoff", 0.375)],
                AlphaMode::Mask,
                0.375,
            ),
        ] {
            let mut objects = h::objects(
                shader,
                &floats,
                &[],
                &[("_MainTex", s::tex_env((0, 0), [1.; 2], [0.; 2]))],
            );
            s::set(&mut objects[7].value, "m_CustomRenderQueue", V::Int(-1));
            let scene = h::collect(&objects, mapper, h::DEFAULT_PROFILE);
            let m = &scene.materials.as_slice()[0];
            assert_eq!(m.alpha_mode, mode, "{shader:?}");
            assert_eq!(m.alpha_cutoff, cutoff);
            assert!(m.base_color_texture.is_none());
            let json = h::json(&scene);
            assert_eq!(
                json["materials"][0]["alphaMode"],
                if mode == AlphaMode::Blend {
                    "BLEND"
                } else {
                    "MASK"
                }
            );
            if mode == AlphaMode::Mask {
                assert_eq!(json["materials"][0]["alphaCutoff"], cutoff);
            }
        }
    }
}

#[test]
fn unlit_color_ignores_populated_stale_base_textures_and_preserves_references() {
    for mapper in [&CorePbr as &dyn MaterialMapper, &UnlitExtras] {
        for keys in [
            vec!["_MainTex"],
            vec!["_BaseMap"],
            vec!["_MainTex", "_BaseMap"],
        ] {
            let envs: Vec<_> = keys
                .iter()
                .map(|&key| (key, s::tex_env((0, 12), [0.5, 0.25], [0.125, 0.375])))
                .collect();
            let scene = h::collect(
                &h::objects(
                    b"Unlit/Color",
                    &[],
                    &[("_Color", [0.25, 0.5, 0.75, 1.])],
                    &envs,
                ),
                mapper,
                h::DEFAULT_PROFILE,
            );
            let m = &scene.materials.as_slice()[0];
            assert!(m.base_color_texture.is_none(), "{keys:?}");
            assert_eq!(m.base_color, h::linear_color([0.25, 0.5, 0.75, 1.]));
            assert!(scene.textures.is_empty());
            assert!(scene.images.is_empty());
            let json = h::json(&scene);
            for key in keys {
                assert_eq!(
                    json["materials"][0]["extras"][key],
                    "0:12;scale=0.5,0.25;offset=0.125,0.375"
                );
            }
        }
    }
}

#[test]
fn equal_size_packed_maps_report_filter_conflicts_and_keep_metallic_sampler() {
    // The last pair differs only in minification; wrapping always agrees.
    for (metallic_filter, albedo_filter, mips, mag, min, conflict) in [
        (1, 1, 1, 9729, 9729, false),
        (0, 1, 1, 9728, 9728, true),
        (1, 2, 2, 9729, 9985, true),
    ] {
        let mut objects = h::objects(
            b"Standard",
            &[("_GlossMapScale", 0.5), ("_SmoothnessTextureChannel", 1.)],
            &[],
            &[
                ("_MetallicGlossMap", s::tex_env((0, 12), [1.; 2], [0.; 2])),
                ("_MainTex", s::tex_env((0, 13), [1.; 2], [0.; 2])),
            ],
        );
        objects[9] = ngui::texture(12, b"metallic", 4, (2, 2), [64, 0, 0, 192].repeat(4));
        s::set(&mut objects[9].value, "m_ColorSpace", V::Int(0));
        objects.push(ngui::texture(
            13,
            b"albedo",
            4,
            (2, 2),
            [0, 0, 0, 64].repeat(4),
        ));
        for (index, filter) in [(9, metallic_filter), (10, albedo_filter)] {
            s::set(&mut objects[index].value, "m_MipCount", V::Int(mips));
            s::set(
                &mut objects[index].value,
                "m_TextureSettings",
                ngui::st(
                    "GLTextureSettings",
                    vec![
                        ("m_WrapU", V::Int(0)),
                        ("m_WrapV", V::Int(0)),
                        ("m_FilterMode", V::Int(filter)),
                    ],
                ),
            );
        }
        let scene = h::collect(&objects, &CorePbr, h::DEFAULT_PROFILE);
        let texture = scene.materials.as_slice()[0]
            .metallic_roughness_texture
            .unwrap()
            .texture;
        let packed = &scene.textures.as_slice()[texture];
        let sampler = &scene.samplers.as_slice()[packed.sampler.unwrap()];
        assert_eq!((sampler.mag_filter, sampler.min_filter), (mag, min));
        h::assert_pixels(
            scene.images.as_slice()[packed.image].bytes.as_slice(),
            &[0, 223, 64, 255].repeat(4),
        );
        let count: u64 = scene
            .collection_issues
            .as_slice()
            .iter()
            .filter(|i| i.reason == "packed_sampler_conflict")
            .map(|i| i.count)
            .sum();
        assert_eq!(count, u64::from(conflict));
        if conflict {
            h::reason(&scene, "packed_sampler_conflict");
        }
    }
}

const URP_BASE: [&[u8]; 5] = [
    b"Universal Render Pipeline/Lit",
    b"Universal Render Pipeline/Unlit",
    b"Universal Render Pipeline/Simple Lit",
    b"Universal Render Pipeline/Baked Lit",
    b"Universal Render Pipeline/Particles/Unlit",
];

// These URP shaders draw `_BaseColor` and `_BaseMap`; `_Color` and `_MainTex`
// are aliases the URP material editor keeps in step, so a stale alias never
// wins over them.
#[test]
fn every_urp_shader_prefers_its_base_properties_over_the_legacy_aliases() {
    for shader in URP_BASE {
        for mapper in [&CorePbr as &dyn MaterialMapper, &UnlitExtras] {
            let json = aliased(shader, (0, 12), mapper);
            let pbr = &json["materials"][0]["pbrMetallicRoughness"];
            h::assert_factor(
                &pbr["baseColorFactor"],
                &h::linear_color([0.25, 0.5, 0.75, 1.]),
            );
            let tr = &pbr["baseColorTexture"]["extensions"]["KHR_texture_transform"];
            assert_eq!(tr["scale"], serde_json::json!([2, 3]), "{shader:?}");
            assert_eq!(tr["offset"], serde_json::json!([0.25, -2.5]), "{shader:?}");
        }
    }
}

// A cleared `_BaseMap` is the shader's white default: a `_MainTex` left
// behind is not drawn, so no base texture is exported.
#[test]
fn a_null_urp_base_map_is_no_texture_even_with_a_main_tex() {
    for shader in URP_BASE {
        for mapper in [&CorePbr as &dyn MaterialMapper, &UnlitExtras] {
            let json = aliased(shader, (0, 0), mapper);
            let pbr = &json["materials"][0]["pbrMetallicRoughness"];
            assert!(pbr.get("baseColorTexture").is_none(), "{shader:?}: {json}");
            assert!(json.get("textures").is_none(), "{shader:?}: {json}");
        }
    }
}

// Other URP shaders, SpeedTree8 among them, draw `_Color` and `_MainTex`; a
// `_BaseColor` or `_BaseMap` a shader switch left behind does not win.
#[test]
fn urp_shaders_outside_the_base_set_keep_color_and_main_tex() {
    for shader in [
        b"Universal Render Pipeline/Nature/SpeedTree8".as_slice(),
        b"Universal Render Pipeline/Terrain/Lit",
    ] {
        for mapper in [&CorePbr as &dyn MaterialMapper, &UnlitExtras] {
            let json = aliased(shader, (0, 12), mapper);
            let pbr = &json["materials"][0]["pbrMetallicRoughness"];
            h::assert_factor(
                &pbr["baseColorFactor"],
                &h::linear_color([0.75, 0.125, 0.5, 1.]),
            );
            let tr = &pbr["baseColorTexture"]["extensions"]["KHR_texture_transform"];
            assert_eq!(tr["scale"], serde_json::json!([0.5, 0.25]), "{shader:?}");
            assert_eq!(
                tr["offset"],
                serde_json::json!([0.125, 0.375]),
                "{shader:?}"
            );
        }
    }
}

/// A material of `shader` holding both the URP base properties and their
/// legacy aliases, each with its own values; `base_map` is `_BaseMap`'s
/// texture reference.
fn aliased(shader: &[u8], base_map: (i32, i64), mapper: &dyn MaterialMapper) -> serde_json::Value {
    let objects = h::objects(
        shader,
        &[],
        &[
            ("_Color", [0.75, 0.125, 0.5, 1.]),
            ("_BaseColor", [0.25, 0.5, 0.75, 1.]),
        ],
        &[
            ("_MainTex", s::tex_env((0, 12), [0.5, 0.25], [0.125, 0.375])),
            ("_BaseMap", s::tex_env(base_map, [2., 3.], [0.25, 0.5])),
        ],
    );
    h::json(&h::collect(&objects, mapper, h::DEFAULT_PROFILE))
}
