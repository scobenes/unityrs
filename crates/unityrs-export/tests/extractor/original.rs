//! Every permitted Tier C departure has a specific original exception and
//! partial tree. Adding a case cannot silently opt out of original replay.
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub(super) fn check(case: &Value) {
    let label = case["case"].as_str().unwrap();
    let exception = match label {
        "missing_dst" => Some("FileNotFoundError"),
        "image_extension_pcx"
        | "image_extension_unknown"
        | "image_extension_eps"
        | "sprite_name_plain"
        | "sprite_name_.hidden"
        | "unknown_no_tree" => Some("ValueError"),
        "image_extension_psd" => Some("KeyError"),
        "non_utf8_basename_im" => Some("UnicodeEncodeError"),
        "zero_height" | "zero_sprite" | "zero_height_im" | "zero_height_png"
        | "zero_height_bmp" | "zero_height_tga" => Some("SystemError"),
        "vertexless_continues" => Some("TypeError"),
        "failed_export_continues"
        | "monobehaviours"
        | "unknown_tree"
        | "gameobject_cycles"
        | "crawl_distinct_files_same_id_overwrite"
        | "crawl_skips_sprite_returned_texture"
        | "crawl_failed_export_continues" => Some("AttributeError"),
        "mono_no_tree" => Some("EOFError"),
        _ => None,
    };
    let mut tree = match label {
        "zero_height" | "zero_height_im" | "zero_height_png" | "zero_height_bmp"
        | "zero_height_tga" | "zero_height_ico" => case["expected"]["tree"].clone(),
        "image_extension_ico" => json!({}),
        _ if exception.is_some() => json!({}),
        _ => case["expected"]["tree"].clone(),
    };
    let extra = match label {
        "vertexless_continues" => Some(("empty.mesh", b"".as_slice())),
        "failed_export_continues" => Some(("broken.obj", b"".as_slice())),
        "image_extension_ico" => Some(("Assets/image.ico", b"\0\0\x01\0\0\0".as_slice())),
        "zero_height_ico" => Some(("empty.ico", b"\0\0\x01\0\0\0".as_slice())),
        _ => None,
    };
    if let Some((name, bytes)) = extra {
        tree[super::hex(name.as_bytes())] = json!({"size":bytes.len(),
            "sha256":super::hex(&Sha256::digest(bytes))});
    }
    let original = if let Some(exception) = exception {
        json!({"exception":exception,"tree":tree})
    } else {
        json!({"exported":case["expected"]["exported"],"tree":tree})
    };
    assert_eq!(
        case["unitypy"], original,
        "{label}: original exception/partial tree or Tier A output"
    );
}
