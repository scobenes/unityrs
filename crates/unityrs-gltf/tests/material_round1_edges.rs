//! Shader edge cases: smoothness alpha is independent of surface opacity.
mod collect_support;
mod material_support;
use material_support as h;
use unityrs_gltf::CorePbr;
use unityrs_testkit::{
    ngui::{self, V},
    scene3d as s,
};

#[test]
fn albedo_alpha_smoothness_does_not_reduce_surface_opacity() {
    for shader in [b"Standard".as_slice(), b"Universal Render Pipeline/Lit"] {
        let mut objects = h::objects(
            shader,
            &[
                ("_Mode", 1.),
                ("_AlphaClip", 1.),
                ("_SmoothnessTextureChannel", 1.),
            ],
            &[("_Color", [1.; 4])],
            &[("_MainTex", s::tex_env((0, 12), [1.; 2], [0.; 2]))],
        );
        objects[9] = ngui::texture(12, b"albedo alpha", 4, (2, 2), [64, 128, 192, 64].repeat(4));
        let scene = h::collect(&objects, &CorePbr, h::DEFAULT_PROFILE);
        let t = scene.materials.as_slice()[0]
            .base_color_texture
            .unwrap()
            .texture;
        let image = scene.textures.as_slice()[t].image;
        h::assert_pixels(
            scene.images.as_slice()[image].bytes.as_slice(),
            &[64, 128, 192, 255].repeat(4),
        );
    }
}
#[test]
fn smoothness_channel_with_null_albedo_uses_white_alpha_not_gloss_alpha() {
    for with_map in [false, true] {
        let mut objects = h::objects(
            b"Standard",
            &[
                ("_GlossMapScale", 0.5),
                ("_Glossiness", 0.125),
                ("_SmoothnessTextureChannel", 1.),
            ],
            &[],
            &[
                ("_MainTex", s::tex_env((0, 0), [1.; 2], [0.; 2])),
                (
                    "_MetallicGlossMap",
                    s::tex_env((0, if with_map { 12 } else { 0 }), [1.; 2], [0.; 2]),
                ),
            ],
        );
        objects[9] = ngui::texture(12, b"gloss alpha", 4, (2, 2), [64, 0, 0, 64].repeat(4));
        s::set(&mut objects[9].value, "m_ColorSpace", V::Int(0));
        let scene = h::collect(&objects, &CorePbr, h::DEFAULT_PROFILE);
        let m = &scene.materials.as_slice()[0];
        if with_map {
            let j = h::json(&scene);
            let t = usize::try_from(
                j["materials"][0]["pbrMetallicRoughness"]["metallicRoughnessTexture"]["index"]
                    .as_u64()
                    .unwrap(),
            )
            .unwrap();
            let image = scene.textures.as_slice()[t].image;
            h::assert_pixels(
                scene.images.as_slice()[image].bytes.as_slice(),
                &[0, 128, 64, 255].repeat(4),
            );
        } else {
            assert_eq!(m.roughness, 0.5);
        }
    }
}
