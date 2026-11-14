use unityrs_core::limits::Meter;
use unityrs_gltf::{Inactive, Lod, Profile, StaticBatch};
#[test]
fn strict_typed_profile() {
    let m = Meter::unlimited();
    let p = Profile::parse(br#"{"profile_version":1,"inactive":"include_hidden","lod":"all","static_batch":"keep_combined","normalise":false,"layer_skips":[2,31],"scene_roots":["root/child"],"clip_states":[]}"#, &m).unwrap();
    assert_eq!(p.inactive, Inactive::IncludeHidden);
    assert_eq!(p.lod, Lod::All);
    assert_eq!(p.static_batch, StaticBatch::KeepCombined);
    assert!(!p.normalise);
    for bad in [
        "{}",
        r#"{"profile_version":2}"#,
        r#"{"profile_version":1,"typo":true}"#,
        r#"{"profile_version":1,"normalise":1}"#,
        r#"{"profile_version":1,"normalise":true,"normalise":false}"#,
        r#"{"profile_version":1,"layer_skips":[32]}"#,
        r#"{"profile_version":1,"clip_states":[{}]}"#,
        r#"{"profile_version":1,"export_metadata":{"typo":true}}"#,
    ] {
        assert!(Profile::parse(bad.as_bytes(), &m).is_err(), "{bad}");
    }
}

#[test]
fn every_field_is_typed_and_nested_keys_are_strict() {
    let m = Meter::unlimited();
    for (key, value) in [
        ("root_naming", "true"),
        ("layer_skips", "[1.0]"),
        ("shader_rules", "[]"),
        ("inactive", "null"),
        ("material_property_allowlist", "[1]"),
        ("scene_roots", "[false]"),
        ("exclusions", "{}"),
        ("builtin", "1"),
        ("extras_naming", "\"typo\""),
        ("export_metadata", "{\"source\":1}"),
        ("lod", "false"),
        ("static_batch", "\"typo\""),
        ("normalise", "\"true\""),
        ("clip_states", "{}"),
        ("shader_rules", "{\"skip\":[\"*\"],\"skip\":[]}"),
        ("export_metadata", "{\"input_sha256\":\"short\"}"),
    ] {
        let json = format!("{{\"profile_version\":1,\"{key}\":{value}}}");
        assert!(Profile::parse(json.as_bytes(), &m).is_err(), "{json}");
    }
    let p=Profile::parse(r#"{"profile_version":1,"scene_roots":["\u00e9\ud83d\ude00","日本語"],"shader_rules":{"drawable":["*"],"additive":[],"skip":[]},"export_metadata":{"generator":"test","source":"synthetic","input_sha256":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"}}"#.as_bytes(), &m);
    // SHA-256 must have exactly 64 hex digits.
    assert!(p.is_err());
    let p = Profile::parse(
        "{\"profile_version\":1,\"scene_roots\":[\"\\u00e9\\ud83d\\ude00\",\"日本語\"]}".as_bytes(),
        &m,
    )
    .unwrap();
    assert_eq!(p.scene_roots.as_slice()[0].as_bytes(), "é😀".as_bytes());
    for bad in [
        r#"{"profile_version":1,"scene_roots":["\ud800"]}"#,
        r#"{"profile_version":1,}"#,
        r#"{"profile_version":1,"layer_skips":[01]}"#,
        r#"{"profile_version":1} null"#,
    ] {
        assert!(Profile::parse(bad.as_bytes(), &m).is_err());
    }
}
