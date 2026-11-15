//! HDR content errors stay recoverable during material collection.

mod collect_support;
mod material_support;

use material_support as h;
use unityrs_gltf::CorePbr;
use unityrs_testkit::{ngui, scene3d as s};

#[test]
fn malformed_hdr_base_texture_preserves_geometry_and_other_material_textures() {
    for format in [24, 66, 67, 68, 69, 70, 71] {
        let mut bad = vec![0; 16];
        if format == 24 {
            bad[0] = 19;
        }
        let mut objects = h::objects(
            b"Standard",
            &[],
            &[],
            &[
                ("_MainTex", s::tex_env((0, 12), [1.; 2], [0.; 2])),
                ("_BumpMap", s::tex_env((0, 13), [1.; 2], [0.; 2])),
            ],
        );
        objects[9] = ngui::texture(12, b"bad", format, (1, 1), bad);
        objects.push(ngui::texture(13, b"good", 4, (2, 2), h::PIXELS.to_vec()));
        let scene = h::collect(&objects, &CorePbr, h::DEFAULT_PROFILE);
        h::reason(&scene, "texture_malformed");
        assert!(!scene.meshes.is_empty());
        assert_eq!(scene.materials.len(), 1);
        let material = &scene.materials.as_slice()[0];
        assert!(material.base_color_texture.is_none());
        assert!(material.normal_texture.is_some());
        assert_eq!(scene.images.len(), 1);
        assert_eq!(scene.textures.len(), 1);
    }
}
