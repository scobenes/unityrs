//! Strict, metered version-1 export policy. See the crate README for its schema.
use crate::{GltfError, Options, Text};
use std::sync::Arc;
use unityrs_core::limits::{Meter, MeteredVec};
mod read;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Inactive {
    Skip,
    Include,
    IncludeHidden,
}
const _: () = crate::send_sync::<Inactive>();
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lod {
    Lod0,
    All,
}
const _: () = crate::send_sync::<Lod>();
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StaticBatch {
    Unbatch,
    KeepCombined,
}
const _: () = crate::send_sync::<StaticBatch>();
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Builtin {
    Placeholder,
    Resolve,
    Procedural,
}
const _: () = crate::send_sync::<Builtin>();
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootNaming {
    Name,
    PathId,
}
const _: () = crate::send_sync::<RootNaming>();
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtrasNaming {
    Unity,
    None,
}
const _: () = crate::send_sync::<ExtrasNaming>();
/// Shader name glob lists, consumed by the material pass in PR 7.
#[derive(Debug)]
pub struct ShaderRules {
    pub drawable: MeteredVec<Text>,
    pub additive: MeteredVec<Text>,
    pub skip: MeteredVec<Text>,
}
const _: () = crate::send_sync::<ShaderRules>();
/// Optional caller-supplied provenance; no host paths are inferred.
#[derive(Debug, Default)]
pub struct ExportMetadata {
    pub generator: Option<Text>,
    pub source: Option<Text>,
    pub input_sha256: Option<Text>,
}
const _: () = crate::send_sync::<ExportMetadata>();
/// Typed profile, independently charged while alive. Missing fields use defaults;
/// JSON must explicitly supply `profile_version: 1`. Clip states are P2b-only.
#[derive(Debug)]
pub struct Profile {
    pub root_naming: RootNaming,
    pub layer_skips: MeteredVec<u32>,
    pub shader_rules: ShaderRules,
    pub inactive: Inactive,
    pub material_property_allowlist: MeteredVec<Text>,
    pub scene_roots: MeteredVec<Text>,
    pub exclusions: MeteredVec<Text>,
    pub builtin: Builtin,
    pub extras_naming: ExtrasNaming,
    pub export_metadata: ExportMetadata,
    pub lod: Lod,
    pub static_batch: StaticBatch,
    pub normalise: bool,
}
const _: () = crate::send_sync::<Profile>();
impl Profile {
    #[must_use]
    pub fn new(meter: &Arc<Meter>) -> Self {
        Self {
            root_naming: RootNaming::Name,
            layer_skips: MeteredVec::new(meter),
            shader_rules: ShaderRules {
                drawable: MeteredVec::new(meter),
                additive: MeteredVec::new(meter),
                skip: MeteredVec::new(meter),
            },
            inactive: Inactive::Skip,
            material_property_allowlist: MeteredVec::new(meter),
            scene_roots: MeteredVec::new(meter),
            exclusions: MeteredVec::new(meter),
            builtin: Builtin::Placeholder,
            extras_naming: ExtrasNaming::Unity,
            export_metadata: ExportMetadata::default(),
            lod: Lod::Lod0,
            static_batch: StaticBatch::Unbatch,
            normalise: true,
        }
    }
    pub fn parse(json: &[u8], meter: &Arc<Meter>) -> Result<Self, GltfError> {
        read::parse(json, meter)
    }
    #[must_use]
    pub fn options(&self) -> Options {
        Options {
            normalise: self.normalise,
        }
    }
}
