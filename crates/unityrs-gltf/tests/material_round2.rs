//! Round-two shader state, packing and diagnostics regressions.
mod collect_support;
mod material_support;
use material_support as h;
use unityrs_gltf::{AlphaMode, CorePbr, Scene};
use unityrs_testkit::{
    ngui::{self, Object, V},
    scene3d as s,
};

fn pixels(scene: &Scene, texture: usize, size: (u32, u32), expected: &[u8]) {
    let image = scene.textures.as_slice()[texture].image;
    h::assert_pixels_sized(
        scene.images.as_slice()[image].bytes.as_slice(),
        size,
        expected,
    );
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
fn urp_opaque_to_transparent_disables_albedo_smoothness_in_both_slots() {
    for transparent in [false, true] {
        let surface = f32::from(transparent);
        let mut objects = h::objects(
            b"Universal Render Pipeline/Lit",
            &[
                ("_Surface", surface),
                ("_SmoothnessTextureChannel", 1.),
                ("_Smoothness", 0.5),
            ],
            &[],
            &[
                ("_BaseMap", s::tex_env((0, 12), [1.; 2], [0.; 2])),
                ("_MetallicGlossMap", s::tex_env((0, 13), [1.; 2], [0.; 2])),
            ],
        );
        objects[9] = ngui::texture(12, b"albedo", 4, (2, 2), [64, 128, 192, 64].repeat(4));
        objects.push(ngui::texture(
            13,
            b"gloss",
            4,
            (2, 2),
            [64, 0, 0, 192].repeat(4),
        ));
        s::set(&mut objects[10].value, "m_ColorSpace", V::Int(0));
        let scene = h::collect(&objects, &CorePbr, h::DEFAULT_PROFILE);
        let m = &scene.materials.as_slice()[0];
        pixels(
            &scene,
            m.base_color_texture.unwrap().texture,
            (2, 2),
            &[64, 128, 192, if transparent { 64 } else { 255 }].repeat(4),
        );
        pixels(
            &scene,
            m.metallic_roughness_texture.unwrap().texture,
            (2, 2),
            &[0, if transparent { 159 } else { 223 }, 64, 255].repeat(4),
        );
    }
}
#[test]
fn transparent_alpha_clip_is_counted_and_preserves_cutoff() {
    let scene = h::collect(
        &h::objects(
            b"Universal Render Pipeline/Lit",
            &[("_Surface", 1.), ("_AlphaClip", 1.), ("_Cutoff", 0.375)],
            &[],
            &[],
        ),
        &CorePbr,
        h::DEFAULT_PROFILE,
    );
    assert_eq!(scene.materials.as_slice()[0].alpha_mode, AlphaMode::Blend);
    h::reason(&scene, "alpha_clip_dropped");
    assert_eq!(
        h::json(&scene)["materials"][0]["extras"]["_Cutoff"],
        "0.375"
    );
}
#[test]
fn texture_only_unlit_shaders_ignore_leftover_color_but_preserve_it() {
    for shader in [
        b"Unlit/Texture".as_slice(),
        b"Unlit/Transparent",
        b"Unlit/Transparent Cutout",
        b"Unlit/Color",
    ] {
        let scene = h::collect(
            &h::objects(shader, &[], &[("_Color", [0.25, 0.5, 0.75, 0.375])], &[]),
            &CorePbr,
            h::DEFAULT_PROFILE,
        );
        assert_eq!(
            scene.materials.as_slice()[0].base_color,
            if shader == b"Unlit/Color" {
                h::linear_color([0.25, 0.5, 0.75, 0.375])
            } else {
                [1.; 4]
            }
        );
        if shader != b"Unlit/Color" {
            assert_eq!(
                h::json(&scene)["materials"][0]["extras"]["_Color"],
                "0.25,0.5,0.75,0.375"
            );
        }
    }
}
#[test]
fn standard_bakes_scale_before_z_while_urp_scales_after_z() {
    for shader in [
        b"Standard".as_slice(),
        b"Standard (Specular setup)",
        b"Universal Render Pipeline/Lit",
    ] {
        for scale in [0., 0.5, 1., 2., -0.5] {
            let mut objects = h::objects(
                shader,
                &[("_BumpScale", scale)],
                &[],
                &[("_BumpMap", s::tex_env((0, 12), [1.; 2], [0.; 2]))],
            );
            objects[9] = ngui::texture(12, b"normal", 4, (2, 2), [179, 192, 0, 255].repeat(4));
            s::set(&mut objects[9].value, "m_ColorSpace", V::Int(0));
            let scene = h::collect(&objects, &CorePbr, h::DEFAULT_PROFILE);
            let m = &scene.materials.as_slice()[0];
            let standard = shader != b"Universal Render Pipeline/Lit";
            let original_scale = scale;
            let scale = if standard { f64::from(scale) } else { 1. };
            let (x, y) = (
                (179. / 255. * 2. - 1.) * scale,
                (192. / 255. * 2. - 1.) * scale,
            );
            let z = (1. - x * x - y * y).max(0.).sqrt();
            let length = if standard {
                (x * x + y * y + z * z).sqrt()
            } else {
                1.
            };
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let expected = [x / length, y / length, z / length]
                .map(|v| ((v * 0.5 + 0.5) * 255.).round() as u8);
            pixels(
                &scene,
                m.normal_texture.unwrap().texture,
                (2, 2),
                &[expected[0], expected[1], expected[2], 255].repeat(4),
            );
            assert_eq!(
                m.normal_scale,
                if standard {
                    1.
                } else {
                    f64::from(original_scale)
                }
            );
        }
    }
}
#[test]
fn broken_albedo_alpha_keeps_valid_metallic_and_white_smoothness() {
    for broken in [0, 1, 2] {
        let mut objects = h::objects(
            b"Standard",
            &[("_GlossMapScale", 0.5), ("_SmoothnessTextureChannel", 1.)],
            &[],
            &[
                ("_MetallicGlossMap", s::tex_env((0, 12), [1.; 2], [0.; 2])),
                (
                    "_MainTex",
                    s::tex_env((0, if broken == 0 { 999 } else { 13 }), [1.; 2], [0.; 2]),
                ),
            ],
        );
        objects[9] = ngui::texture(12, b"gloss", 4, (2, 2), [64, 0, 0, 64].repeat(4));
        s::set(&mut objects[9].value, "m_ColorSpace", V::Int(0));
        objects.push(ngui::texture(
            13,
            b"bad albedo",
            if broken == 1 { 999 } else { 4 },
            (2, 2),
            vec![0; 3],
        ));
        let scene = h::collect(&objects, &CorePbr, h::DEFAULT_PROFILE);
        let m = &scene.materials.as_slice()[0];
        assert!(m.base_color_texture.is_none());
        pixels(
            &scene,
            m.metallic_roughness_texture.unwrap().texture,
            (2, 2),
            &[0, 128, 64, 255].repeat(4),
        );
        h::reason(
            &scene,
            [
                "missing_texture",
                "undecodable_texture",
                "texture_malformed",
            ][broken],
        );
    }
}
#[test]
fn packed_map_retains_larger_albedo_grid_and_reports_wrap_conflicts() {
    for conflict in [false, true] {
        let mut objects = h::objects(
            b"Standard",
            &[("_GlossMapScale", 0.638), ("_SmoothnessTextureChannel", 1.)],
            &[],
            &[
                ("_MetallicGlossMap", s::tex_env((0, 12), [1.; 2], [0.; 2])),
                ("_MainTex", s::tex_env((0, 13), [1.; 2], [0.; 2])),
            ],
        );
        objects[9] = ngui::texture(12, b"gloss", 4, (2, 2), [64, 0, 0, 64].repeat(4));
        s::set(&mut objects[9].value, "m_ColorSpace", V::Int(0));
        settings(&mut objects[9], 0, 0, 1);
        let albedo: Vec<u8> = (0..64)
            .flat_map(|i| [0, 0, 0, u8::try_from((i % 8) * 255 / 7).unwrap()])
            .collect();
        objects.push(ngui::texture(13, b"alpha ramp", 4, (8, 8), albedo.clone()));
        settings(&mut objects[10], i32::from(conflict), 0, 1);
        let scene = h::collect(&objects, &CorePbr, h::DEFAULT_PROFILE);
        let t = scene.materials.as_slice()[0]
            .metallic_roughness_texture
            .unwrap()
            .texture;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let expected: Vec<_> = albedo
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| {
                [
                    0,
                    ((1. - f64::from(p[3]) / 255. * f64::from(0.638f32)) * 255.).round() as u8,
                    64,
                    255,
                ]
            })
            .collect();
        pixels(&scene, t, (8, 8), &expected);
        let sampler = scene.textures.as_slice()[t].sampler.unwrap();
        assert_eq!(scene.samplers.as_slice()[sampler].wrap_s, 10497);
        let count = scene
            .collection_issues
            .as_slice()
            .iter()
            .filter(|i| i.reason == "packed_sampler_conflict")
            .count();
        assert_eq!(count, usize::from(conflict));
        if conflict {
            h::reason(&scene, "packed_sampler_conflict");
        }
    }
}
#[test]
fn combined_union_material_diagnostics_are_emitted_once_per_primitive() {
    for missing in [false, true] {
        let mut objects = h::simple();
        objects[2] = Object::new(
            3,
            1,
            s::game_object(b"child", &[(0, 4), (0, 5), (0, 6), (0, 14)]),
        );
        let ptr = if missing { (0, 0) } else { (0, 10) };
        objects[5] = Object::new(6, 23, s::mesh_renderer((0, 3), &[ptr], (0, 1)));
        objects.push(Object::new(
            14,
            23,
            s::mesh_renderer((0, 3), &[ptr], (1, 1)),
        ));
        // Existing mesh fixture has tangents but no vertex colors. Add a color stream.
        super_color_stream(&mut objects[6]);
        let scene = h::collect(
            &objects,
            &CorePbr,
            br#"{"profile_version":1,"static_batch":"keep_combined"}"#,
        );
        let reason = if missing {
            "missing_material"
        } else {
            "vertex_colors_ignored"
        };
        let count: u64 = scene
            .collection_issues
            .as_slice()
            .iter()
            .filter(|i| i.reason == reason)
            .map(|i| i.count)
            .sum();
        assert_eq!(count, 2, "{reason}: {:?}", scene.collection_issues);
    }
}
fn field<'a>(v: &'a mut V, name: &str) -> &'a mut V {
    let V::Struct(_, fields) = v else {
        panic!("struct")
    };
    &mut fields.iter_mut().find(|(n, _)| n == name).unwrap().1
}
fn super_color_stream(mesh: &mut Object) {
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
fn normal_cache_separates_baked_standard_scale_and_shares_urp_pixels() {
    for shader in [b"Standard".as_slice(), b"Universal Render Pipeline/Lit"] {
        let mut objects = h::objects(
            shader,
            &[("_BumpScale", 0.5)],
            &[],
            &[("_BumpMap", s::tex_env((0, 12), [1.; 2], [0.; 2]))],
        );
        objects[5] = Object::new(6, 23, s::mesh_renderer((0, 3), &[(0, 10), (0, 14)], (0, 0)));
        objects.push(s::material(
            14,
            b"second scale",
            (0, 11),
            &[("_BumpScale", 2.)],
            &[],
            &[("_BumpMap", s::tex_env((0, 12), [1.; 2], [0.; 2]))],
        ));
        let scene = h::collect(&objects, &CorePbr, h::DEFAULT_PROFILE);
        let a = scene.materials.as_slice()[0]
            .normal_texture
            .unwrap()
            .texture;
        let b = scene.materials.as_slice()[1]
            .normal_texture
            .unwrap()
            .texture;
        assert_eq!(a == b, shader == b"Universal Render Pipeline/Lit");
    }
}

#[test]
fn packed_grid_takes_each_axis_maximum_and_each_sources_filter_and_wrap() {
    for (metal_filter, albedo_filter, wrap, reds, greens) in [
        (1, 0, 0, [64, 64, 191, 191], [255, 255, 128, 128]),
        (0, 1, 0, [0, 0, 255, 255], [223, 223, 159, 159]),
        (1, 1, 1, [0, 64, 191, 255], [255, 223, 159, 128]),
        (1, 1, 2, [0, 64, 191, 255], [255, 223, 159, 128]),
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
        objects[9] = ngui::texture(
            12,
            b"metal",
            4,
            (2, 4),
            [0, 0, 0, 255, 255, 0, 0, 255].repeat(4),
        );
        s::set(&mut objects[9].value, "m_ColorSpace", V::Int(0));
        settings(&mut objects[9], wrap, wrap, metal_filter);
        let alpha: Vec<_> = [255, 0]
            .into_iter()
            .flat_map(|a| [0, 0, 0, a].repeat(4))
            .collect();
        objects.push(ngui::texture(13, b"alpha", 4, (4, 2), alpha));
        settings(&mut objects[10], wrap, wrap, albedo_filter);
        let scene = h::collect(&objects, &CorePbr, h::DEFAULT_PROFILE);
        let t = scene.materials.as_slice()[0]
            .metallic_roughness_texture
            .unwrap()
            .texture;
        let expected: Vec<_> = greens
            .into_iter()
            .flat_map(|g| reds.into_iter().flat_map(move |r| [0, g, r, 255]))
            .collect();
        pixels(&scene, t, (4, 4), &expected);
    }
}
