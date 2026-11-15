//! Materials use the texture's platform layout as well as its file version.
mod collect_support;
mod material_support;

use material_support as h;
use unityrs_core::{
    Bytes, File, RawStr,
    env::{EnvOptions, Environment, Source},
    limits::Meter,
};
use unityrs_gltf::{Profile, Scene, UnlitExtras, collect_with_mapper};
use unityrs_scene::{GraphOptions, graph_from_scene_file};
use unityrs_testkit::{
    ngui::{self, Object, V},
    scene3d as s,
};

fn collect_on(objects: &[Object], platform: i32) -> Scene {
    let env = Environment::new(EnvOptions::unitypy());
    let File::Serialized(file) = env
        .load(Source::Bytes {
            name: RawStr::from_static(b"synthetic"),
            data: Bytes::from(ngui::file_on(Some(platform), &[], objects, true).unwrap()),
        })
        .unwrap()
    else {
        panic!("serialized file")
    };
    let meter = Meter::unlimited();
    let graph = graph_from_scene_file(&env, &file, &GraphOptions::default(), &meter).unwrap();
    collect_with_mapper(&env, &graph, &Profile::new(&meter), &UnlitExtras, &meter).unwrap()
}

fn switch_blob(exponent: u32) -> V {
    let mut blob = vec![0; 8];
    blob.extend_from_slice(&exponent.to_le_bytes());
    V::Bytes(blob)
}

#[test]
fn switch_material_images_deswizzle_promote_crop_and_flip() {
    for format in [3, 4, 8] {
        // At GOB height 2, two 16-byte texels hold the first two rows.
        // The original 2x2 image occupies eight bytes of each texel.
        let mut pixels = h::PIXELS;
        if format == 8 {
            for pixel in pixels.as_chunks_mut::<4>().0 {
                pixel.swap(0, 2);
            }
        }
        let mut data = vec![0; 1024];
        data[..8].copy_from_slice(&pixels[..8]);
        data[16..24].copy_from_slice(&pixels[8..]);
        let mut objects = h::texture_case(format, data);
        s::set(&mut objects[9].value, "m_PlatformBlob", switch_blob(1));
        let scene = collect_on(&objects, 38);
        assert_eq!(scene.images.len(), 1, "format {format}");
        h::assert_pixels(&h::embedded_image(&scene), &h::TOP_FIRST);
    }
}

#[test]
fn xbox_material_images_swap_pairs_before_decoding() {
    // BC1's RGB565 red endpoint is 0xf800 after the byte swap.
    let scene = collect_on(&h::texture_case(10, vec![0xf8, 0, 0, 0, 0, 0, 0, 0]), 11);
    assert_eq!(scene.images.len(), 1);
    h::assert_pixels(&h::embedded_image(&scene), &[255, 0, 0, 255].repeat(4));
}

#[test]
fn malformed_platform_and_pvrtc_layouts_omit_the_slot() {
    let mut objects = h::texture_case(4, vec![0; 1024]);
    s::set(&mut objects[9].value, "m_PlatformBlob", switch_blob(32));
    let scene = collect_on(&objects, 38);
    assert!(scene.images.is_empty());
    h::reason(&scene, "texture_malformed");

    let mut objects = h::simple();
    objects[9] = ngui::texture(12, b"invalid-grid", 32, (12, 12), vec![0; 72]);
    let scene = h::collect(&objects, &UnlitExtras, h::DEFAULT_PROFILE);
    assert!(scene.images.is_empty());
    h::reason(&scene, "texture_malformed");
}
