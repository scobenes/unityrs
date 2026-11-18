//! Preserve resource and object classifications through the 3D adapters.
use std::error::Error as StdError;
use unityrs_anim::AnimError;
use unityrs_core::{
    Error, LimitError,
    budget::BudgetExceeded,
    detect::DetectError,
    env::DerefError,
    limits::ChargeError,
    objmode::ViewError,
    typetree::TreeError,
    walk::{ParseError, ReadDetail, ReadError},
};
use unityrs_gltf::GltfError;
use unityrs_mesh::MeshError;
use unityrs_scene::SceneError;

use super::super::classify::{self, Class, Reason};
use crate::gltf::ExportError;

// Follow the typed causes for resource refusals and signature failures.
// Some core wrappers carry a cause without exposing it through Error::source.
fn required_failure(mut cause: &(dyn StdError + 'static)) -> Option<Class> {
    loop {
        if let Some(error) = cause.downcast_ref::<BudgetExceeded>() {
            return Some(classify::budget(error));
        }
        if let Some(error) = cause.downcast_ref::<LimitError>() {
            return Some(classify::limit(error));
        }
        if let Some(error) = cause.downcast_ref::<ChargeError>() {
            return Some(classify::charge(error));
        }
        if cause.is::<unityrs_core::budget::AllocError>() {
            return Some(Class::Run(Reason::plain("host_memory")));
        }
        if matches!(
            cause.downcast_ref::<Error>(),
            Some(Error::WebSignature(_) | Error::BundleSignature(_))
        ) {
            return Some(Class::Input(Reason {
                reason: "container_error".into(),
                detail: Some("malformed".into()),
                cut: false,
            }));
        }
        cause = source(cause)?;
    }
}

fn source<'a>(cause: &'a (dyn StdError + 'static)) -> Option<&'a (dyn StdError + 'static)> {
    match cause.downcast_ref::<Error>() {
        Some(Error::Budget(e)) => return Some(e),
        Some(Error::Limit(e)) => return Some(e),
        Some(Error::Allocator(e)) => return Some(e),
        Some(Error::Entry { source, .. }) => return Some(source.as_ref()),
        Some(Error::AssetBundle(e)) => return Some(e.as_ref()),
        Some(Error::Deref(e)) => return Some(e.as_ref()),
        Some(Error::Tree(e)) => return Some(e),
        Some(Error::Detect(e)) => return Some(e),
        Some(Error::Io(e)) => return Some(e.0.as_ref()),
        _ => {}
    }
    if let Some(DerefError::Load(e)) = cause.downcast_ref::<DerefError>() {
        return Some(e);
    }
    match cause.downcast_ref::<ParseError>() {
        Some(ParseError::Deref(e)) => return Some(e),
        Some(ParseError::Tree(e)) => return Some(e),
        Some(ParseError::Read(e)) => return Some(e),
        Some(ParseError::Provider(e)) => {
            if let Some(e) = &e.allocator {
                return Some(e);
            }
            if let Some(e) = &e.memory {
                return Some(e);
            }
            return e.limit.as_ref().map(|e| e as &dyn StdError);
        }
        _ => {}
    }
    match cause.downcast_ref::<ReadError>().map(|e| &e.detail) {
        Some(ReadDetail::Charge(e)) => return Some(e),
        Some(ReadDetail::Limit(e)) => return Some(e),
        Some(ReadDetail::Tree(e)) => return Some(e),
        _ => {}
    }
    match cause.downcast_ref::<TreeError>() {
        Some(TreeError::Charge(e)) => return Some(e),
        Some(TreeError::Limit(e)) => return Some(e),
        _ => {}
    }
    match cause.downcast_ref::<DetectError>() {
        Some(DetectError::Allocator(e)) => return Some(e),
        Some(DetectError::InflateLimit(e) | DetectError::Work(e)) => return Some(e),
        _ => {}
    }
    match cause
        .downcast_ref::<std::io::Error>()
        .and_then(std::io::Error::get_ref)
    {
        Some(e) => Some(e),
        None => cause.source(),
    }
}

fn loaded(error: &Error) -> Class {
    if let Some(class) = required_failure(error) {
        return match class {
            Class::Skip(r) if r.reason.starts_with("limit(") || r.reason == "budget(input)" => {
                Class::Input(r)
            }
            class => class,
        };
    }
    match classify::error(error) {
        Class::Skip(r) if r.reason == "decode_error" => Class::Input(Reason {
            reason: "container_error".into(),
            ..r
        }),
        class => class,
    }
}

pub(super) fn deref(error: &DerefError) -> Class {
    match error {
        DerefError::Load(error) => loaded(error),
        error => classify::deref(error),
    }
}

fn parse(error: &ParseError) -> Class {
    match error {
        ParseError::Deref(e) => deref(e),
        e => required_failure(e).unwrap_or_else(|| classify::parse(e)),
    }
}

pub(super) fn scene(error: &SceneError) -> Class {
    match error {
        SceneError::Charge(e) | SceneError::View(ViewError::Charge(e)) => classify::charge(e),
        SceneError::Parse(e) => parse(e),
        SceneError::Deref(e) => deref(e),
        SceneError::Malformed(field) => skip("malformed_scene", Some(field)),
        SceneError::NoTransform => skip("missing_transform", None),
        SceneError::View(_) | SceneError::WrongClass(_) => skip("malformed_scene", None),
    }
}
pub(super) fn gltf(error: GltfError) -> Class {
    let cause: Option<&dyn StdError> = match &error {
        GltfError::Mesh(e) => Some(e),
        GltfError::Io(e) => Some(e),
        _ => None,
    };
    if let Some(class) = cause.and_then(required_failure) {
        return class;
    }
    match error {
        GltfError::Charge(e)
        | GltfError::Mesh(
            MeshError::Charge(e)
            | MeshError::Decode(unityrs_meshdecode::MeshError::Charge(e))
            | MeshError::View(ViewError::Charge(e)),
        ) => classify::charge(&e),
        GltfError::Mesh(MeshError::Parse(e)) => parse(&e),
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
        ExportError::Input(e) => loaded(&e),
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
        AnimError::Parse(e) => parse(e),
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

pub(super) fn degraded(issue: &unityrs_gltf::CollectionIssue) -> bool {
    // Resource issues and dropped renderers lose source content. The other
    // collector notes include profile choices and representation adjustments
    // such as material-less submeshes, placeholders and padded skin rows.
    issue.dropped_renderer
        || issue.resource.is_some()
        || matches!(
            issue.reason,
            "unresolved_material"
                | "malformed_material"
                | "missing_material"
                | "missing_skin_attributes"
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use unityrs_core::{
        LimitKind, RawStr,
        budget::{BudgetExceeded, Reservation},
        walk::ParseError,
    };

    fn entry(error: Error) -> Error {
        Error::Entry {
            name: RawStr::from_static(b"stream.resS"),
            source: Arc::new(Error::Deref(Arc::new(DerefError::Load(error)))),
        }
    }

    #[test]
    fn prefab_classifiers_preserve_refusals_wrapped_by_reads_and_dereferences() {
        let cases = [
            (
                Error::Budget(BudgetExceeded {
                    requested: 1,
                    what: Reservation::Dependency,
                }),
                Class::Input(Reason::plain("budget(dependency)")),
            ),
            (
                Error::Budget(BudgetExceeded {
                    requested: 1,
                    what: Reservation::Input,
                }),
                skip("budget(input)", None),
            ),
            (
                Error::Limit(LimitError::refused(LimitKind::InputMemory, 2, 1)),
                skip("limit(input_memory, 1)", None),
            ),
            (
                Error::Limit(LimitError::refused(LimitKind::DecodeWork, 2, 1)),
                skip("limit(decode_work, 1)", None),
            ),
        ];
        for (error, expected) in cases {
            let wrapped = entry(error);
            assert_eq!(
                gltf(GltfError::Mesh(MeshError::Decode(
                    unityrs_meshdecode::MeshError::Resource(wrapped.clone()),
                ))),
                expected
            );
            let parse = ParseError::Deref(DerefError::Load(wrapped));
            let loaded = match expected {
                Class::Skip(r) if r.reason.starts_with("limit(") || r.reason == "budget(input)" => {
                    Class::Input(r)
                }
                result => result,
            };
            assert_eq!(scene(&SceneError::Parse(parse.clone())), loaded);
            assert_eq!(
                export(ExportError::Animation(AnimError::Parse(parse))),
                loaded
            );
        }
    }
}
