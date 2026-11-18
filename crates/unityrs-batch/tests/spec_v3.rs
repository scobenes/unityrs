mod common;

use serde_json::json;
use unityrs_batch::spec::{self, Rule};

#[test]
fn glb_only_requires_spec_three() {
    let mut doc = common::base();
    doc["outputs"] = json!({"glb":{"prefab_key_template":"prefabs/{safe_container_path}.glb"}});
    for version in [1, 2] {
        doc["spec_version"] = json!(version);
        let error = spec::parse(serde_json::to_vec(&doc).unwrap()).unwrap_err();
        assert_eq!(error.rule, Rule::UnknownField);
        assert_eq!(error.pointer, "/outputs/glb");
    }
    doc["spec_version"] = json!(3);
    spec::parse(serde_json::to_vec(&doc).unwrap()).unwrap();
}

#[test]
fn checks_prefab_override_shape_before_planning() {
    let mut doc = common::base();
    doc["spec_version"] = json!(3);
    doc["outputs"] = json!({"glb":{
        "prefab_key_template":"prefabs/{safe_container_path}.glb",
        "profile":{"profile_version":1,"clips_by_prefab":{"prefabs/absent.glb":{"a":[]}}}
    }});
    let input = doc["inputs"][0]["id"].as_str().unwrap().to_owned();
    doc["outputs"]["glb"]["profile"]["clips_by_prefab"]["prefabs/absent.glb"] = json!({input:[]});
    spec::parse(serde_json::to_vec(&doc).unwrap()).unwrap();
    doc["outputs"]["glb"]["profile"]["clips_by_prefab"]["prefabs/absent.glb"]["unknown"] =
        json!([]);
    let error = spec::parse(serde_json::to_vec(&doc).unwrap()).unwrap_err();
    assert_eq!(error.rule, Rule::UnknownInput);
    assert_eq!(
        error.pointer,
        "/outputs/glb/profile/clips_by_prefab/prefabs~1absent.glb/unknown"
    );
}

#[test]
fn empty_override_map_still_validates_its_literal_key() {
    let mut doc = common::base();
    doc["spec_version"] = json!(3);
    doc["outputs"] = json!({"glb":{
        "prefab_key_template":"{safe_container_path}.glb",
        "profile":{"profile_version":1,"clips_by_prefab":{"/invalid":{}}}
    }});
    let error = spec::parse(serde_json::to_vec(&doc).unwrap()).unwrap_err();
    assert_eq!(error.rule.name(), "key(leading_slash)");
    assert_eq!(
        error.pointer,
        "/outputs/glb/profile/clips_by_prefab/~1invalid"
    );
}

#[test]
fn rejects_duplicate_profile_members_and_nonempty_global_clip_lists() {
    let mut doc = common::base();
    doc["spec_version"] = json!(3);
    doc["outputs"] = json!({"glb":{
        "prefab_key_template":"{safe_container_path}.glb",
        "profile":{"profile_version":1,"clips":[]}
    }});
    let text = doc
        .to_string()
        .replace("\"clips\":[]", "\"clips\":[],\"clips\":[]");
    let duplicate = spec::parse(text.into_bytes()).unwrap_err();
    assert_eq!(duplicate.rule, Rule::DuplicateMember);
    assert_eq!(duplicate.pointer, "/outputs/glb/profile/clips");
    doc["outputs"]["glb"]["profile"]["clips"] = json!([{}]);
    let nonempty = spec::parse(serde_json::to_vec(&doc).unwrap()).unwrap_err();
    assert_eq!(nonempty.rule, Rule::Range);
    assert_eq!(nonempty.pointer, "/outputs/glb/profile/clips");
}
