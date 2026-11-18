//! Preserve resource and object classifications through the 3D adapters.
use unityrs_anim::AnimError;
use unityrs_core::{env::DerefError, objmode::ViewError};
use unityrs_gltf::GltfError;
use unityrs_mesh::MeshError;
use unityrs_scene::SceneError;

use super::super::classify::{self, Class, Reason};
use crate::gltf::ExportError;

pub(super) fn deref(error: &DerefError) -> Class {
    match error {
        DerefError::Load(error) => {
            let class = classify::error(error);
            match class {
                Class::Skip(r) if r.reason == "decode_error" && r.detail.is_some() => {
                    Class::Input(Reason {
                        reason: "container_error".into(),
                        ..r
                    })
                }
                Class::Skip(r) if r.reason.starts_with("limit(") => Class::Input(r),
                class => class,
            }
        }
        error => classify::deref(error),
    }
}
pub(super) fn scene(error: &SceneError) -> Class {
    match error {
        SceneError::Charge(e) | SceneError::View(ViewError::Charge(e)) => classify::charge(e),
        SceneError::Parse(e) => classify::parse(e),
        SceneError::Deref(e) => deref(e),
        SceneError::Malformed(field) => skip("malformed_scene", Some(field)),
        SceneError::NoTransform => skip("missing_transform", None),
        SceneError::View(_) | SceneError::WrongClass(_) => skip("malformed_scene", None),
    }
}
pub(super) fn gltf(error: GltfError) -> Class {
    match error {
        GltfError::Charge(e)
        | GltfError::Mesh(
            MeshError::Charge(e)
            | MeshError::Decode(unityrs_meshdecode::MeshError::Charge(e))
            | MeshError::View(ViewError::Charge(e)),
        ) => classify::charge(&e),
        GltfError::Mesh(MeshError::Parse(e)) => classify::parse(&e),
        GltfError::Mesh(MeshError::Core(e)) => classify::error(&e),
        GltfError::Mesh(MeshError::Scene(e)) => scene(&e),
        GltfError::Mesh(MeshError::Deref(e)) => deref(&e),
        GltfError::Mesh(MeshError::Texture(e)) => classify::decode(&e),
        GltfError::Mesh(MeshError::Io(_)) | GltfError::Io(_) => {
            Class::Run(Reason::plain("host_io"))
        }
        GltfError::Mesh(_) => skip("decode_error", Some("mesh")),
        GltfError::Invalid(reason) => skip("invalid_glb", Some(reason)),
        GltfError::UnsupportedSkinInfluences => skip("unsupported_skin_influences", None),
        GltfError::UnsupportedControllerEvaluation => {
            skip("unsupported_controller_evaluation", None)
        }
        GltfError::AnimationCallbackRequired => super::internal(),
    }
}
pub fn export(error: ExportError) -> Class {
    match error {
        ExportError::Gltf(e) => gltf(e),
        ExportError::Animation(e) => animation(&e),
        ExportError::Charge(e) => classify::charge(&e),
        ExportError::Input(e) => classify::error(&e),
        ExportError::OutsideSelectionView(address) => Class::Skip(Reason {
            reason: format!("outside_view(file_index={})", address.file_index),
            detail: Some(format!("path_id={}", address.path_id)),
            cut: false,
        }),
        ExportError::MissingClipSelection => skip("missing_clip_selection", None),
        ExportError::AmbiguousClipSelection => skip("ambiguous_clip_selection", None),
        ExportError::AnimatedStaticBatch => skip("animated_static_batch", None),
        ExportError::Bake(detail) => skip("animation_bake_error", Some(detail)),
        ExportError::Unresolved(slot) => {
            let mut class = animation(&slot.cause);
            if let Class::Skip(reason) = &mut class {
                let source = &slot.source;
                let (detail, cut) = classify::text(
                    format!(
                        "{}[{}] owner={} pointer={:?}",
                        source.field,
                        source.ordinal,
                        source.owner.path_id(),
                        source.pointer
                    )
                    .as_bytes(),
                );
                reason.detail = Some(detail);
                reason.cut |= cut;
            }
            class
        }
    }
}

fn animation(error: &AnimError) -> Class {
    match error {
        AnimError::Charge(e) => classify::charge(e),
        AnimError::Parse(e) => classify::parse(e),
        AnimError::Deref(e) | AnimError::OutsideView(e) => deref(e),
        AnimError::Malformed(field) => skip("malformed_animation", Some(field)),
        AnimError::WrongClass(_) => skip("malformed_animation", None),
        error => {
            let text = error.to_string();
            let (code, detail) = text
                .split_once('(')
                .map_or((text.as_str(), None), |(code, detail)| {
                    (code, detail.strip_suffix(')'))
                });
            skip(code, detail)
        }
    }
}

pub(super) fn skip(reason: &str, detail: Option<&str>) -> Class {
    Class::Skip(Reason {
        reason: reason.to_owned(),
        detail: detail.map(str::to_owned),
        cut: false,
    })
}

pub(super) fn degraded(reason: &str) -> bool {
    matches!(
        reason,
        "missing_texture"
            | "texture_malformed"
            | "undecodable_texture"
            | "unsupported_texture_platform"
            | "texture_encode_failed"
            | "texture_too_large"
            | "unresolved_material"
            | "malformed_material"
            | "missing_material"
    ) || required_renderer(reason)
}

pub(super) fn required_renderer(reason: &str) -> bool {
    matches!(
        reason,
        "mesh_unresolved"
            | "missing_mesh"
            | "mesh_parse_failed"
            | "mesh_resource_failed"
            | "mesh_decode_failed"
            | "malformed_mesh_field"
            | "source_not_mesh"
            | "empty_mesh"
    )
}
