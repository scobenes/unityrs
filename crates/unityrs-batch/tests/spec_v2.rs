//! Version 2 of the spec (`docs/BATCH_SPEC.md` 1.1 and 1.5): its three
//! members, each refusal they add, and version 1's parse beside it, which
//! refuses them.

mod common;

use common::{accepted, refused, refused_text, with};
use serde_json::{Value, json};
use unityrs_batch::spec::model::Image;
use unityrs_export::ngui::{Format, Formats};

/// [`common::base`] at version 2, with `f` applied.
fn v2(f: impl FnOnce(&mut Value)) -> Value {
    with(|v| {
        v["spec_version"] = json!(2);
        f(v);
    })
}

#[test]
fn a_version_2_spec_takes_version_1s_defaults() {
    let s = accepted(&v2(|_| {})).spec;
    assert_eq!(s.spec_version, 2);
    assert_eq!(s.outputs.formats, Formats::V1);
    assert_eq!(s.outputs.image, Image::Png);
    assert_eq!(s.unitycn_key, None);
    let s = accepted(&v2(|v| {
        v["outputs"]["formats"] = json!(["RGB24", "RGBA32", "DXT1", "DXT5", "BC7"]);
        v["outputs"]["image"] = json!("png");
        v["unitycn_key"] = Value::Null;
    }))
    .spec;
    assert_eq!(s.outputs.formats, Formats::V1);
    assert_eq!(s.unitycn_key, None);
}

#[test]
fn formats_name_any_decoded_format_in_any_order() {
    let s = accepted(&v2(|v| {
        v["outputs"]["formats"] = json!(["BC4", "RGB565", "DXT1", "ETC2_RGBA8", "ASTC_RGB_6x6"]);
    }))
    .spec;
    let want = Formats::NONE
        .with(Format::Bc4)
        .with(Format::Rgb565)
        .with(Format::Dxt1)
        .with(Format::Etc2Rgba8)
        .with(Format::AstcRgb6x6);
    assert_eq!(s.outputs.formats, want);
    // Every format this binary decodes is accepted, all at once.
    let all: Vec<_> = Format::ALL.iter().map(|f| f.name()).collect();
    let s = accepted(&v2(|v| v["outputs"]["formats"] = json!(all))).spec;
    assert_eq!(s.outputs.formats, Formats::DECODED);
}

#[test]
fn each_refusal_formats_adds() {
    let formats = |list: Value| refused(&v2(|v| v["outputs"]["formats"] = list));
    // Not one of UnityPy's names, by case too.
    assert_eq!(formats(json!(["Foo"])), "range /outputs/formats/0");
    assert_eq!(formats(json!(["dxt1"])), "range /outputs/formats/0");
    assert_eq!(formats(json!([""])), "range /outputs/formats/0");
    // One of UnityPy's names that this binary does not decode: `range`
    // too (the stricter rule), never a skip at run time.
    assert_eq!(
        formats(json!(["DXT1", "PVRTC_RGB2"])),
        "range /outputs/formats/1"
    );
    assert_eq!(formats(json!(["DXT1Crunched"])), "range /outputs/formats/0");
    assert_eq!(formats(json!(["ASTC_HDR_4x4"])), "range /outputs/formats/0");
    // A repeat, at the later item; none at all.
    assert_eq!(
        formats(json!(["BC4", "DXT1", "BC4"])),
        "range /outputs/formats/2"
    );
    assert_eq!(formats(json!([])), "range /outputs/formats");
    // Types.
    assert_eq!(formats(json!("DXT1")), "type /outputs/formats");
    assert_eq!(formats(json!([10])), "type /outputs/formats/0");
    assert_eq!(formats(Value::Null), "type /outputs/formats");
}

#[test]
fn image_is_png_and_the_reserved_values_are_refused() {
    let image = |value: Value| refused(&v2(|v| v["outputs"]["image"] = value));
    for reserved in ["png_fast", "webp", "PNG", ""] {
        assert_eq!(image(json!(reserved)), "range /outputs/image", "{reserved}");
    }
    assert_eq!(
        image(json!({"webp": {"quality": 80}})),
        "type /outputs/image"
    );
    assert_eq!(image(Value::Null), "type /outputs/image");
}

#[test]
fn the_unitycn_key_is_a_variable_name_never_a_value() {
    let s = accepted(&v2(|v| v["unitycn_key"] = json!({"env": "UNITYCN_KEY"}))).spec;
    assert_eq!(s.unitycn_key.as_deref(), Some("UNITYCN_KEY"));
    let key = |value: Value| refused(&v2(|v| v["unitycn_key"] = value));
    assert_eq!(key(json!({})), "missing /unitycn_key/env");
    assert_eq!(key(json!({"env": ""})), "range /unitycn_key/env");
    assert_eq!(
        key(json!({"env": "A".repeat(257)})),
        "range /unitycn_key/env"
    );
    assert_eq!(key(json!({"env": "1KEY"})), "format /unitycn_key/env");
    assert_eq!(key(json!({"env": "KEY-1"})), "format /unitycn_key/env");
    assert_eq!(
        key(json!({"env": "K", "value": "x"})),
        "unknown_field /unitycn_key/value"
    );
    // The key itself has no place in a spec.
    assert_eq!(
        key(json!("73796e7468657469632d6b65792d3031")),
        "type /unitycn_key"
    );
    assert_eq!(key(json!({"env": 1})), "type /unitycn_key/env");
}

#[test]
fn version_1_refuses_version_2s_members_as_before() {
    let s = accepted(&with(|_| {})).spec;
    assert_eq!(s.spec_version, 1);
    assert_eq!(s.outputs.formats, Formats::V1);
    assert_eq!(s.outputs.image, Image::Png);
    assert_eq!(
        refused(&with(|v| v["unitycn_key"] = json!({"env": "K"}))),
        "unknown_field /unitycn_key"
    );
    assert_eq!(
        refused(&with(|v| v["outputs"]["formats"] = json!(["DXT1"]))),
        "unknown_field /outputs/formats"
    );
    assert_eq!(
        refused(&with(|v| v["outputs"]["image"] = json!("png"))),
        "unknown_field /outputs/image"
    );
}

/// The version decides the table wherever `spec_version` stands: a
/// version 2 member before it is read by version 2's table, and a version
/// 1 document's is `unknown_field` at its place in document order.
#[test]
fn the_version_is_known_before_the_members_it_governs() {
    let text = |version: u32, tail: &str| {
        format!(
            r#"{{"run_id":"r","attempt":1,"threads":1,"download_concurrency":1,
            "max_inflight_uncompressed_bytes":268435456,"max_decoded_bytes":16777216,
            "dep_cache_bytes":33554432,
            "spool":{{"dir":"/spool","size_bytes":1073741824,"reserve_bytes":0}},
            "inputs":[{{"id":"a","path":"/in/a"}}],
            "outputs":{{"textures":{{"key":"{{name}}.png"}},"formats":["BC4"]}},
            "unitycn_key":{{"env":"K"}},{tail}
            "sink":{{"kind":"directory","root":"/out"}},"spec_version":{version}}}"#
        )
    };
    let s = accepted(&serde_json::from_str(&text(2, "")).unwrap()).spec;
    assert_eq!(s.outputs.formats, Formats::NONE.with(Format::Bc4));
    let parsed = common::parse_text(&text(2, "")).unwrap();
    assert_eq!(parsed.spec.unitycn_key.as_deref(), Some("K"));
    assert_eq!(refused_text(&text(1, "")), "unknown_field /outputs/formats");
    // A later error never hides the earlier one, in either version.
    assert_eq!(
        refused_text(&text(1, r#""threads":0,"#)),
        "unknown_field /outputs/formats"
    );
    assert_eq!(refused_text(&text(2, r#""zzz":0,"#)), "unknown_field /zzz");
    // `spec_version`'s own rules still come first.
    assert_eq!(refused_text(&text(3, "")), "version /spec_version");
}

#[test]
fn outputs_still_needs_one_kind_in_version_2() {
    let v = v2(|v| v["outputs"] = json!({"formats": ["DXT1"], "image": "png"}));
    assert_eq!(refused(&v), "outputs_empty /outputs");
}
