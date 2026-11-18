//! Spec 3's inline profile and its ordered per-input selectors.
use std::sync::Arc;

use super::{
    BadSpec, Rule,
    fields::{Array, Field, Ty, field, int, one_of, required, string},
    node::Node,
    pointer,
};

pub const PROFILE_BYTES: usize = 64 * 1024;
const UINT: Ty = int(0, u32::MAX as u64);
const TEXT: Ty = string(0, Some(1024), None);
const STRINGS: Ty = Ty::Array(&list(TEXT, 256));
const ADDRESS: Ty = Ty::Object(&[
    required("file_index", UINT),
    required("path_id", Ty::PathId),
]);
const SOURCE: Ty = Ty::Object(&[
    required("owner", ADDRESS),
    required("field", TEXT),
    required("ordinal", UINT),
]);
pub const CLIP: &[Field] = &[
    required("root", ADDRESS),
    required("component", ADDRESS),
    field("clip", ADDRESS),
    field("source", SOURCE),
];
const SELECTORS: Ty = Ty::Array(&list(Ty::ClipSelector, 256));
const INPUTS: Ty = Ty::Map {
    value: &SELECTORS,
    entries: 256,
};
const fn list(item: Ty, max: u64) -> Array {
    Array {
        item,
        min: 0,
        max: Some(max),
        unique: false,
    }
}
pub const PROFILE: Ty = Ty::Object(&[
    required("profile_version", int(1, 1)),
    field("root_naming", one_of(&["name", "path_id"])),
    field("layer_skips", Ty::Array(&list(int(0, 31), 256))),
    field(
        "shader_rules",
        Ty::Object(&[
            field("drawable", STRINGS),
            field("additive", STRINGS),
            field("skip", STRINGS),
        ]),
    ),
    field("inactive", one_of(&["skip", "include", "include_hidden"])),
    field("material_property_allowlist", STRINGS),
    field("scene_roots", STRINGS),
    field("exclusions", STRINGS),
    field("builtin", one_of(&["placeholder", "resolve", "procedural"])),
    field("extras_naming", one_of(&["unity", "none"])),
    field(
        "export_metadata",
        Ty::Object(&[
            field("generator", TEXT),
            field("source", TEXT),
            field("input_sha256", Ty::ProfileDigest),
        ]),
    ),
    field("lod", one_of(&["lod0", "all"])),
    field("static_batch", one_of(&["unbatch", "keep_combined"])),
    field("normalise", Ty::Bool),
    field("clip_states", Ty::Array(&EMPTY)),
    field("clips", Ty::DefaultClips),
    field("sample_rate", Ty::SampleRate),
    field("skin_influences", int(4, 4)),
    field(
        "clips_by_prefab",
        Ty::Map {
            value: &INPUTS,
            entries: 256,
        },
    ),
]);
pub const EMPTY: Array = Array {
    item: TEXT,
    min: 0,
    max: Some(0),
    unique: false,
};

/// Immutable profile bytes, in the shared standalone profile encoding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Glb {
    pub prefab_key_template: String,
    pub profile: Vec<u8>,
    pub overrides: Vec<PrefabOverride>,
    pub keys: Vec<Arc<str>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrefabOverride {
    pub key: Arc<str>,
    pub input: String,
    pub clips: Vec<u8>,
}

impl PrefabOverride {
    pub(crate) fn pointer(&self) -> String {
        let mut pointer = key_pointer(&self.key);
        super::push_token(&mut pointer, &self.input);
        pointer
    }
}

pub(super) fn key_pointer(key: &str) -> String {
    pointer(&["outputs", "glb", "profile", "clips_by_prefab", key])
}

pub(super) fn build(node: Node<'_>, at: &str) -> Result<Glb, BadSpec> {
    let mut glb = Glb {
        prefab_key_template: String::new(),
        profile: br#"{"profile_version":1,"clips":"all_discovered"}"#.to_vec(),
        overrides: Vec::new(),
        keys: Vec::new(),
    };
    let invalid = || BadSpec::new(Rule::Type, at);
    let Node::Obj(fields) = node else {
        return Err(invalid());
    };
    for (name, value) in fields {
        match value {
            Node::Str(template) if name == "prefab_key_template" => {
                glb.prefab_key_template = template.into_owned();
            }
            Node::Obj(fields) if name == "profile" => {
                let mut defaults = Vec::from(fields);
                if let Some(i) = defaults
                    .iter()
                    .position(|(key, _)| key == "clips_by_prefab")
                {
                    let (_, Node::Obj(prefabs)) = defaults.remove(i) else {
                        return Err(invalid());
                    };
                    for (key, inputs) in prefabs {
                        let key: Arc<str> = Arc::from(key.as_ref());
                        glb.keys.push(key.clone());
                        let Node::Obj(inputs) = inputs else {
                            return Err(invalid());
                        };
                        for (input, clips) in inputs {
                            glb.overrides.push(PrefabOverride {
                                key: key.clone(),
                                input: input.into_owned(),
                                clips: serde_json::to_vec(&clips).map_err(|_| invalid())?,
                            });
                        }
                    }
                }
                if !defaults.iter().any(|(name, _)| name == "clips") {
                    defaults.push(("clips".into(), Node::Str("all_discovered".into())));
                }
                glb.profile = serde_json::to_vec(&Node::Obj(defaults.into_boxed_slice()))
                    .map_err(|_| invalid())?;
            }
            _ => return Err(invalid()),
        }
    }
    Ok(glb)
}
