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
fn material_images_keep_the_unity_crunch_fork_on_desktop_and_xbox() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../unityrs-texture/tests/fixtures/unitycrunch_native.json"
    ))
    .unwrap();
    for (value, plain) in [(28, 10), (29, 12), (64, 34), (65, 47)] {
        let case = fixture["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["format"] == value && c["width"] == 1 && c["height"] == 1)
            .unwrap();
        let unhex = |s: &str| -> Vec<u8> {
            (0..s.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
                .collect()
        };
        let mut objects = h::simple();
        objects[9] = ngui::texture(
            12,
            b"asymmetric",
            plain,
            (1, 1),
            unhex(case["levels"][0]["blocks"].as_str().unwrap()),
        );
        let expected = collect_on(&objects, 13);
        for platform in [13, 11] {
            let mut data = unhex(case["data"]["hex"].as_str().unwrap());
            if platform == 11 && matches!(value, 28 | 29) {
                for pair in data.as_chunks_mut::<2>().0 {
                    pair.swap(0, 1);
                }
            }
            objects[9] = ngui::texture(12, b"asymmetric", value, (1, 1), data);
            let scene = collect_on(&objects, platform);
            assert_eq!(scene.images.len(), 1, "format {value}, platform {platform}");
            assert_eq!(h::embedded_image(&scene), h::embedded_image(&expected));
        }
    }
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
