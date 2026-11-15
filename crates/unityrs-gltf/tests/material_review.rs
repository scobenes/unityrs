mod collect_support;
mod material_support;
use material_support as h;
use unityrs_gltf::*;
use unityrs_testkit::{
    ngui::{Object, V},
    scene3d as s,
};
#[test]
fn skip_shader_precedes_unused_malformed_saved_properties() {
    let mut objects = h::simple();
    s::set(
        &mut objects[7].value,
        "m_CustomRenderQueue",
        V::Str(b"invalid".to_vec()),
    );
    let scene = h::collect(
        &objects,
        &CorePbr,
        br#"{"profile_version":1,"shader_rules":{"skip":["Standard"]}}"#,
    );
    assert!(scene.meshes.is_empty());
    h::reason(&scene, "shader_skipped");
}
#[test]
fn procedural_renderer_uses_material_policy_including_skip() {
    use unityrs_core::{
        Bytes, RawStr,
        env::{EnvOptions, Environment, Source},
    };
    use unityrs_scene::{GraphOptions, graph_from_scene_file};
    let mut objects = h::simple();
    objects[4] = Object::new(5, 33, s::mesh_filter((0, 3), (1, 10210)));
    let env = Environment::new(EnvOptions::unitypy());
    let file = env
        .load(Source::Bytes {
            name: RawStr::from_static(b"synthetic"),
            data: Bytes::from(
                s::file(
                    s::Layout::Unity2022,
                    &["Library/unity default resources"],
                    &objects,
                )
                .unwrap(),
            ),
        })
        .unwrap();
    let unityrs_core::files::File::Serialized(file) = file else {
        panic!("file")
    };
    let meter = unityrs_core::limits::Meter::unlimited();
    let graph = graph_from_scene_file(&env, &file, &GraphOptions::default(), &meter).unwrap();
    for omitted in [false, true] {
        let mut p = Profile::new(&meter);
        p.builtin = Builtin::Procedural;
        if omitted {
            p.shader_rules
                .skip
                .push(text(&meter, "Standard").unwrap())
                .unwrap();
        }
        let scene = collect_with_mapper(&env, &graph, &p, &CorePbr, &meter).unwrap();
        if omitted {
            assert!(scene.meshes.is_empty());
            h::reason(&scene, "shader_skipped");
        } else {
            assert_eq!(scene.materials.len(), 1);
            assert_eq!(
                scene.meshes.as_slice()[0].primitives.as_slice()[0].material,
                Some(0)
            );
        }
        h::validate(&scene, "procedural-material");
    }
}

#[test]
fn cached_material_diagnostics_reach_each_affected_renderer_once() {
    let mut objects = h::simple();
    objects[8] = s::shader(11, b"Custom/Unknown");
    objects[0] = Object::new(1, 1, s::game_object(b"parent", &[(0, 2), (0, 20), (0, 21)]));
    objects.extend([
        Object::new(20, 33, s::mesh_filter((0, 1), (0, 7))),
        Object::new(
            21,
            23,
            s::mesh_renderer((0, 1), &[(0, 10), (0, 10)], (0, 0)),
        ),
    ]);
    let scene = h::collect(&objects, &CorePbr, h::DEFAULT_PROFILE);
    let issues: Vec<_> = scene
        .collection_issues
        .as_slice()
        .iter()
        .filter(|i| i.reason == "unknown_shader")
        .collect();
    assert_eq!(issues.len(), 2);
    assert_ne!(issues[0].node, issues[1].node);
}

#[test]
fn cross_file_identity_distinguishes_equal_ids_and_shares_resolved_textures() {
    use unityrs_core::{
        Bytes, RawStr,
        env::{EnvOptions, Environment, Source},
    };
    use unityrs_scene::{GraphOptions, graph_from_scene_file};
    for shared in [false, true] {
        let mut primary = h::simple();
        primary[5] = Object::new(6, 23, s::mesh_renderer((0, 3), &[(0, 10), (1, 10)], (0, 0)));
        let mut second = s::material(
            10,
            b"second",
            (0, 11),
            &[("_Surface", 1.)],
            &[],
            &[(
                "_MainTex",
                s::tex_env((i32::from(shared), 12), [1.; 2], [0.; 2]),
            )],
        );
        s::set(&mut second.value, "m_CustomRenderQueue", V::Int(3100));
        let secondary = vec![
            second,
            s::shader(11, b"Universal Render Pipeline/Lit"),
            primary[9].clone(),
        ];
        let env = Environment::new(EnvOptions::unitypy());
        env.load(Source::Bytes {
            name: RawStr::from_static(b"secondary"),
            data: Bytes::from(s::file(s::Layout::Unity2022, &["primary"], &secondary).unwrap()),
        })
        .unwrap();
        let file = env
            .load(Source::Bytes {
                name: RawStr::from_static(b"primary"),
                data: Bytes::from(s::file(s::Layout::Unity2022, &["secondary"], &primary).unwrap()),
            })
            .unwrap();
        let unityrs_core::files::File::Serialized(file) = file else {
            panic!("file")
        };
        let m = unityrs_core::limits::Meter::unlimited();
        let g = graph_from_scene_file(&env, &file, &GraphOptions::default(), &m).unwrap();
        let scene = collect_with_mapper(&env, &g, &Profile::new(&m), &CorePbr, &m).unwrap();
        assert_eq!(scene.materials.len(), 2);
        assert_eq!(scene.textures.len(), if shared { 1 } else { 2 });
        assert_eq!(scene.images.len(), scene.textures.len());
        assert_eq!(scene.materials.as_slice()[1].alpha_mode, AlphaMode::Blend);
        h::validate(&scene, "cross-file-materials");
    }
}

#[test]
fn texture_malformed_name_has_a_texture_reason_and_keeps_factors() {
    let mut objects = h::simple();
    s::set(&mut objects[9].value, "m_Name", V::Int(0));
    let scene = h::collect(&objects, &CorePbr, h::DEFAULT_PROFILE);
    assert!(scene.materials.as_slice()[0].base_color_texture.is_none());
    h::reason(&scene, "texture_malformed");
}
#[test]
fn nonfinite_st_and_factors_degrade_with_counted_reasons() {
    let objects = h::objects(
        b"Standard",
        &[("_Metallic", 2.), ("_Glossiness", f32::NAN)],
        &[("_Color", [f32::NAN, -1., 2., 1.])],
        &[("_MainTex", s::tex_env((0, 12), [f32::NAN, 1.], [0.; 2]))],
    );
    let scene = h::collect(&objects, &CorePbr, h::DEFAULT_PROFILE);
    assert_eq!(
        scene.materials.as_slice()[0].base_color,
        h::linear_color([1., 0., 1., 1.])
    );
    assert_eq!(scene.materials.as_slice()[0].metallic, 1.);
    assert!(scene.images.is_empty());
    for reason in [
        "invalid_texture_transform",
        "invalid_material_value",
        "material_value_clamped",
    ] {
        h::reason(&scene, reason);
    }
}

#[test]
fn malformed_custom_mapper_extras_degrade_before_writing() {
    struct BadExtras;
    impl MaterialMapper for BadExtras {
        fn map(&self, _: &MaterialInput<'_>, cx: &mut MapCx<'_>) -> Result<Material, GltfError> {
            let mut m = Material::new(cx.meter);
            for _ in 0..2 {
                m.extras
                    .push((text(cx.meter, "duplicate")?, text(cx.meter, "value")?))?;
            }
            Ok(m)
        }
    }
    let (env, g) = collect_support::graph(&h::simple());
    let m = collect_support::meter();
    let scene = collect_with_mapper(&env, &g, &Profile::new(&m), &BadExtras, &m).unwrap();
    assert!(scene.materials.is_empty());
    h::reason(&scene, "malformed_material");
    h::validate(&scene, "custom-mapper");
}
#[test]
fn overflowing_transformed_st_is_omitted_before_embedding() {
    struct ExtremeSt;
    impl MaterialMapper for ExtremeSt {
        fn map(&self, _: &MaterialInput<'_>, cx: &mut MapCx<'_>) -> Result<Material, GltfError> {
            let mut m = Material::new(cx.meter);
            m.base_color_texture = cx.texture(
                TextureInput {
                    pointer: unityrs_core::env::PPtr {
                        file_id: 0,
                        path_id: 12,
                    },
                    scale: [1., f64::MAX],
                    offset: [0., f64::MAX],
                },
                b"_MainTex",
            )?;
            Ok(m)
        }
    }
    let (env, g) = collect_support::graph(&h::simple());
    let m = collect_support::meter();
    let scene = collect_with_mapper(&env, &g, &Profile::new(&m), &ExtremeSt, &m).unwrap();
    assert!(scene.images.is_empty());
    assert!(scene.textures.is_empty());
    h::reason(&scene, "invalid_texture_transform");
    h::validate(&scene, "overflow-st");
}

#[test]
fn streamed_resource_embeds_decoded_pixels_and_uses_property_name_fallback() {
    let mut objects = h::simple();
    objects[9] =
        unityrs_testkit::ngui::texture_streamed(12, b"", 4, (2, 2), "archive:/PIXELS.resS", 0, 16);
    let (env, g) = collect_support::graph(&objects);
    env.load(unityrs_core::env::Source::Bytes {
        name: unityrs_core::RawStr::from_static(b"pixels.resS"),
        data: unityrs_core::Bytes::from_static(&h::PIXELS),
    })
    .unwrap();
    let m = collect_support::meter();
    let scene = collect_with_mapper(&env, &g, &Profile::new(&m), &CorePbr, &m).unwrap();
    assert_eq!(scene.images.len(), 1);
    assert_eq!(
        scene.images.as_slice()[0].name.as_ref().unwrap().as_bytes(),
        b"_MainTex.png"
    );
    h::assert_pixels(&h::embedded_image(&scene), &h::TOP_FIRST);
    h::validate(&scene, "streamed-material");
}
