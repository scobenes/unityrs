//! Synthetic L9 files, made with embedded trees and no external assets.
#![allow(
    clippy::format_collect,
    clippy::needless_pass_by_value,
    clippy::too_many_lines,
    reason = "synthetic recorder table and JSON protocol"
)]
use serde_json::{Value, json};
use unityrs_testkit::{
    ngui::{Object, SpriteSpec, V, pair, pptr, s, st, texture, vector},
    scene3d::{
        self, Layout,
        mesh::{self, MeshVersion},
    },
};
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn text(id: i64, name: &str, payload: &str) -> Object {
    Object::new(
        id,
        49,
        st(
            "TextAsset",
            vec![("m_Name", s(name)), ("m_Script", s(payload))],
        ),
    )
}
fn font(id: i64, name: &str, data: V) -> Object {
    Object::new(
        id,
        128,
        st("Font", vec![("m_Name", s(name)), ("m_FontData", data)]),
    )
}
fn audio(id: i64, name: &str, bytes: Vec<u8>) -> Object {
    Object::new(
        id,
        83,
        st(
            "AudioClip",
            vec![("m_Name", s(name)), ("m_AudioData", V::Bytes(bytes))],
        ),
    )
}
fn bundle(entries: &[(&str, i64)]) -> Object {
    bundle_bytes(
        &entries
            .iter()
            .map(|(n, id)| (n.as_bytes(), *id))
            .collect::<Vec<_>>(),
    )
}
fn bundle_bytes(entries: &[(&[u8], i64)]) -> Object {
    let info = |id| {
        st(
            "AssetInfo",
            vec![
                ("preloadIndex", V::Int(0)),
                ("preloadSize", V::Int(0)),
                ("asset", pptr("Object", (0, id))),
            ],
        )
    };
    Object::new(
        100,
        142,
        st(
            "AssetBundle",
            vec![
                ("m_Name", s("synthetic")),
                ("m_PreloadTable", vector(pptr("Object", (0, 0)), vec![])),
                (
                    "m_Container",
                    V::Vector(
                        "map".into(),
                        Box::new(pair(s(""), info(0))),
                        entries
                            .iter()
                            .map(|(n, id)| pair(V::Str(n.to_vec()), info(*id)))
                            .collect(),
                    ),
                ),
            ],
        ),
    )
}
fn case(name: &str, objects: &[Object], options: Value) -> Value {
    json!({"case": name, "input": hex(&scene3d::file(Layout::Unity2022, &[], objects).unwrap()), "options": options})
}
fn main() {
    let mut cases = Vec::new();
    let tex = texture(
        1,
        b"texture",
        4,
        (4, 4),
        (0..16).flat_map(|i| [i, 2 * i, 3 * i, 255]).collect(),
    );
    let sprite = SpriteSpec::rect("sprite.png", [1., 0., 2., 3.], (0, 1)).object(2);
    let mut data = unityrs_testkit::writer::ByteWriter::le();
    unityrs_testkit::ngui::write(&mut data, &tex.value).unwrap();
    let input = unityrs_testkit::serialized::build(&unityrs_testkit::serialized::SerializedSpec {
        unity_version: b"5.0.0a1536".to_vec(),
        types: vec![unityrs_testkit::serialized::TypeSpec {
            class_id: 28,
            nodes: unityrs_testkit::ngui::nodes(0, "Base", &tex.value).unwrap(),
            ..Default::default()
        }],
        objects: vec![unityrs_testkit::serialized::ObjectSpec {
            path_id: 1,
            data: data.into_inner(),
            ..Default::default()
        }],
        ..Default::default()
    });
    cases.push(
        json!({"case": "non_crunched_wide_version", "input": hex(&input),
        "options": {"use_container": false}}),
    );
    let mut objects = vec![
        tex.clone(),
        sprite,
        text(3, "text", "héllo\n"),
        font(
            4,
            "font",
            vector(
                V::UInt8(0),
                b"OTTO\0".iter().map(|b| V::UInt8(*b)).collect(),
            ),
        ),
        font(5, "empty", V::Bytes(vec![])),
        mesh::mesh(
            6,
            b"mesh",
            &[[0., 0., 0.], [1., 0., 0.], [0., 1., 0.]],
            &[0, 1, 2],
            MeshVersion::V2022,
        ),
        audio(7, "clip", b"OggSsynthetic".to_vec()),
    ];
    let paths = [
        ("Assets/A/shared.png", 1),
        ("Assets/A/shared.png", 2),
        ("Assets/A/keep.bytes", 3),
        ("Assets/A/font.otf", 4),
        ("Assets/A/empty", 5),
        ("Assets/A/mesh.odd", 6),
        ("Assets/A/clip.ogg", 7),
        ("Assets/A/later.bytes", 3),
    ];
    objects.insert(0, bundle(&paths));
    cases.push(case(
        "container_priority_overwrites",
        &objects,
        json!({"ignore_first_container_dirs":1}),
    ));
    cases.push(case(
        "container_append_none",
        &objects,
        json!({"append_path_id":true}),
    ));
    cases.push(case(
        "container_negative_slice",
        &objects,
        json!({"ignore_first_container_dirs":-1}),
    ));
    cases.push(case(
        "class_filter",
        &objects,
        json!({"asset_filter":[49,128]}),
    ));
    cases.push(case(
        "no_container_skips_returned_texture",
        &objects[1..],
        json!({"use_container":false}),
    ));
    cases.push(case(
        "missing_dst",
        &objects[1..],
        json!({"use_container":false}),
    ));
    cases.push(case(
        "no_container_append_none",
        &objects[1..],
        json!({"use_container":false,"append_path_id":true}),
    ));
    for extension in [
        "bmp", "dib", "TGA", "ppm", "pcx", "psd", "unknown", "ico", "im", "sgi", "tiff", "dds",
        "eps", "png", "", "...png",
    ] {
        let path = if extension.is_empty() {
            "Assets/plain".to_owned()
        } else {
            format!("Assets/image.{extension}")
        };
        cases.push(case(
            &format!("image_extension_{extension}"),
            &[bundle(&[(&path, 1)]), tex.clone()],
            json!({}),
        ));
    }
    for name in ["plain", "sprite.bmp", ".hidden", "many.dots.png"] {
        cases.push(case(
            &format!("sprite_name_{name}"),
            &[
                tex.clone(),
                SpriteSpec::rect(name, [0., 0., 2., 2.], (0, 1)).object(2),
            ],
            json!({"use_container":false}),
        ));
    }
    for (label, name) in [
        ("non_utf8_parent_sgi", b"Asset\xff/image.sgi".as_slice()),
        ("non_utf8_parent_im", b"Asset\xff/image.im"),
        ("non_utf8_basename_sgi", b"Asset/image\xff\xfe.sgi"),
        ("non_utf8_basename_im", b"Asset/image\xff\xfe.im"),
    ] {
        cases.push(case(
            label,
            &[
                bundle_bytes(&[(name, 1)]),
                texture(1, b"image", 4, (2, 2), vec![100; 16]),
            ],
            json!({}),
        ));
    }
    for extension in ["sgi", "im", "ico", "icns", "png", "bmp", "tga"] {
        cases.push(case(
            &format!("zero_height_{extension}"),
            &[
                bundle(&[(&format!("empty.{extension}"), 1), ("after.txt", 3)]),
                texture(1, b"empty", 4, (4, 0), vec![0]),
                text(3, "after", "continues"),
            ],
            json!({}),
        ));
    }
    for (label, width, height) in [("zero_width", 0, 4), ("zero_height", 4, 0)] {
        cases.push(case(
            label,
            &[
                bundle(&[("empty.png", 1), ("after.txt", 3)]),
                texture(1, b"empty", 4, (width, height), vec![0]),
                text(3, "after", "continues"),
            ],
            json!({}),
        ));
    }
    cases.push(case(
        "zero_sprite",
        &[
            tex.clone(),
            SpriteSpec::rect("zero.png", [0., 0., 0., 2.], (0, 1)).object(2),
        ],
        json!({"use_container":false}),
    ));
    cases.push(case(
        "zero_width_never_fetches_resource",
        &[
            bundle(&[("zero.png", 1)]),
            unityrs_testkit::ngui::texture_streamed(1, b"zero", 4, (0, 4), "missing.resS", 0, 64),
        ],
        json!({}),
    ));
    cases.push(case(
        "vertexless_continues",
        &[
            bundle(&[("empty.mesh", 6), ("after.txt", 3)]),
            mesh::mesh(6, b"empty", &[], &[], MeshVersion::V2022),
            text(3, "after", "continues"),
        ],
        json!({}),
    ));
    let broken = Object::new(6, 43, st("Mesh", vec![("m_Name", s("broken"))]));
    cases.push(case(
        "failed_export_continues",
        &[
            bundle(&[("broken.obj", 6), ("after.txt", 3)]),
            broken,
            text(3, "after", "continues"),
        ],
        json!({}),
    ));
    let mono = Object::new(
        8,
        114,
        st("MonoBehaviour", vec![("m_Name", s("é")), ("n", V::Int(7))]),
    );
    let raw = Object::new(
        9,
        114,
        st(
            "MonoBehaviour",
            vec![("m_Name", s("raw")), ("data", V::Bytes(vec![0, 255]))],
        ),
    );
    cases.push(case(
        "monobehaviours",
        &[
            bundle(&[("mono.asset", 8), ("raw.asset", 9), ("after.txt", 3)]),
            mono.clone(),
            raw,
            text(3, "after", "continues"),
        ],
        json!({}),
    ));
    let unknown = Object::new(
        10,
        21,
        st(
            "Material",
            vec![("m_Name", s("unknown")), ("number", V::Int(9))],
        ),
    );
    cases.push(case(
        "unknown_tree",
        &[bundle(&[("unknown.asset", 10)]), unknown],
        json!({"export_unknown_as_typetree":true}),
    ));
    let go = Object::new(
        20,
        1,
        scene3d::game_object(b"root", &[(0, 8), (0, 30), (0, 3), (0, 8)]),
    );
    let link = Object::new(30, 1, scene3d::game_object(b"child", &[(0, 20), (0, 31)]));
    let mono_link = Object::new(
        8,
        114,
        st(
            "MonoBehaviour",
            vec![
                ("m_Name", s("")),
                (
                    "refs",
                    vector(
                        pptr("Object", (0, 0)),
                        vec![pptr("Object", (0, 31)), pptr("Object", (0, 20))],
                    ),
                ),
            ],
        ),
    );
    cases.push(case(
        "gameobject_cycles",
        &[
            bundle(&[("Assets/root.prefab", 20)]),
            go,
            link,
            mono_link,
            text(3, "text.bytes", "first"),
            text(31, "other.txt", "second"),
        ],
        json!({}),
    ));
    for (label, names) in [
        ("audio_many", Some(&[&b"x"[..], &b"y.wav"[..]][..])),
        ("audio_unnamed", None),
    ] {
        let bank = unityrs_testkit::fsb5::bank(
            2,
            &[(&[1, 0, 2, 0], 2, 1, 8, &[]), (&[3, 0, 4, 0], 2, 1, 8, &[])],
            1,
            names,
        )
        .unwrap();
        cases.push(case(
            label,
            &[audio(7, "bank.fsb", bank)],
            json!({"use_container":false}),
        ));
    }
    cases.push(case(
        "font_bytes",
        &[font(4, "font.odd", V::Bytes(b"OTTO\0".to_vec()))],
        json!({"use_container":false}),
    ));
    // A missing schema is a separate file, preserving actual no-tree state.
    let spec = unityrs_testkit::serialized::SerializedSpec {
        type_trees: false,
        types: vec![unityrs_testkit::serialized::TypeSpec {
            class_id: 114,
            ..Default::default()
        }],
        objects: vec![unityrs_testkit::serialized::ObjectSpec {
            path_id: 1,
            data: vec![1, 2, 3, 255],
            ..Default::default()
        }],
        ..Default::default()
    };
    cases.push(json!({"case":"mono_no_tree","input":hex(&unityrs_testkit::serialized::build(&spec)),"options":{"use_container":false}}));
    let mut unknown_spec = spec;
    unknown_spec.types[0].class_id = 21;
    let input = hex(&unityrs_testkit::serialized::build(&unknown_spec));
    cases.push(json!({"case":"unknown_no_tree","input":input,"options":{"use_container":false,"export_unknown_as_typetree":true}}));
    cases.push(json!({"case":"unknown_ignored","input":input,"options":{"use_container":false}}));
    let main = scene3d::file(
        Layout::Unity2022,
        &["other.assets"],
        &[
            bundle(&[("root.prefab", 20)]),
            Object::new(
                20,
                1,
                scene3d::game_object(b"root", &[(0, 3), (1, 3), (0, 20)]),
            ),
            text(3, "same.txt", "first"),
        ],
    )
    .unwrap();
    let other = scene3d::file(Layout::Unity2022, &[], &[text(3, "same.txt", "later")]).unwrap();
    cases.push(json!({"case":"crawl_distinct_files_same_id_overwrite","input":hex(&unityrs_testkit::env::bundle(&[("main.assets",main),("other.assets",other)])),"options":{}}));
    let mut alpha = SpriteSpec::rect("alpha.png", [0., 0., 2., 2.], (0, 1));
    alpha.alpha = (0, 3);
    cases.push(case(
        "alpha_texture_is_not_returned",
        &[
            tex.clone(),
            alpha.object(2),
            texture(3, b"alpha", 4, (4, 4), vec![100; 64]),
        ],
        json!({"use_container":false}),
    ));
    cases.push(case(
        "crawl_skips_sprite_returned_texture",
        &[
            bundle(&[("root.prefab", 20)]),
            Object::new(
                20,
                1,
                scene3d::game_object(b"root", &[(0, 2), (0, 1), (0, 3)]),
            ),
            tex,
            SpriteSpec::rect("sprite.png", [0., 0., 2., 2.], (0, 1)).object(2),
            text(3, "after", "continues"),
        ],
        json!({}),
    ));
    cases.push(case(
        "crawl_failed_export_continues",
        &[
            bundle(&[("root.prefab", 20)]),
            Object::new(
                20,
                1,
                scene3d::game_object(b"root", &[(0, 6), (0, 3), (0, 20)]),
            ),
            Object::new(6, 43, st("Mesh", vec![("m_Name", s("broken"))])),
            text(3, "after", "continues"),
        ],
        json!({}),
    ));
    println!("{}", serde_json::to_string(&cases).unwrap());
}
