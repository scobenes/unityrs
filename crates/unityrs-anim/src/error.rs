use std::fmt;
use unityrs_core::env::DerefError;
use unityrs_core::limits::ChargeError;
use unityrs_core::walk::ParseError;
use unityrs_scene::SceneError;

/// A serialized data, target resolution or resource failure.
#[derive(Debug)]
pub enum AnimError {
    Charge(ChargeError),
    Parse(ParseError),
    Deref(DerefError),
    OutsideView(DerefError),
    Malformed(&'static str),
    WrongClass(i32),
    ControllerCycle,
    ControllerDepth(u32),
    AmbiguousOverride,
    UnsupportedControllerLayout,
    InvalidClipSlot(u32),
    UnmappedControllerLeaf(u32),
    UnsupportedBindingLayout,
    UnsupportedBindingTarget,
    ContradictoryFlags(usize),
    UnknownBindingWidth {
        binding: usize,
        class_id: i32,
        custom_type: u8,
        attribute: u32,
    },
    SlotOverflow,
    SlotCount {
        bindings: usize,
        storage: usize,
    },
    MissingPath,
    AmbiguousPath,
    CrcCollision(u32),
    PathEvidenceMismatch,
    FilteredTarget(usize),
    MissingAttachment(usize),
    MissingRenderer(usize),
    AmbiguousRenderer(usize),
    MissingChannel(usize),
    AmbiguousChannel(usize),
    UnsupportedMorphProperty,
}

const _: () = crate::send_sync::<AnimError>();

impl AnimError {
    pub(crate) fn resource_refusal(&self) -> bool {
        match self {
            Self::Charge(_) => true,
            Self::Parse(error) => SceneError::Parse(error.clone()).resource_refusal(),
            Self::Deref(error) => SceneError::Deref(error.clone()).resource_refusal(),
            _ => false,
        }
    }
}

impl From<ChargeError> for AnimError {
    fn from(error: ChargeError) -> Self {
        Self::Charge(error)
    }
}

impl From<ParseError> for AnimError {
    fn from(error: ParseError) -> Self {
        Self::Parse(error)
    }
}

impl From<DerefError> for AnimError {
    fn from(error: DerefError) -> Self {
        if matches!(error, DerefError::NotFound { .. }) {
            Self::OutsideView(error)
        } else {
            Self::Deref(error)
        }
    }
}

impl fmt::Display for AnimError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Charge(e) => e.fmt(f),
            Self::Parse(e) => e.fmt(f),
            Self::Deref(e) => e.fmt(f),
            Self::OutsideView(e) => write!(f, "outside_view: {e}"),
            Self::Malformed(field) => write!(f, "malformed animation field: {field}"),
            Self::WrongClass(id) => write!(f, "unexpected animation class: {id}"),
            Self::ControllerCycle => f.write_str("controller_cycle"),
            Self::ControllerDepth(limit) => write!(f, "controller_depth({limit})"),
            Self::AmbiguousOverride => f.write_str("ambiguous_override"),
            Self::UnsupportedControllerLayout => f.write_str("unsupported_controller_layout"),
            Self::InvalidClipSlot(slot) => write!(f, "invalid_controller_clip_slot({slot})"),
            Self::UnmappedControllerLeaf(id) => write!(f, "unmapped_controller_leaf({id})"),
            Self::UnsupportedBindingLayout => f.write_str("unsupported_binding_layout"),
            Self::UnsupportedBindingTarget => f.write_str("unsupported_binding_target"),
            Self::ContradictoryFlags(row) => write!(f, "contradictory_binding_flags({row})"),
            Self::UnknownBindingWidth {
                binding,
                class_id,
                custom_type,
                attribute,
            } => write!(
                f,
                "unknown_binding_width({binding}, {class_id}, {custom_type}, {attribute})"
            ),
            Self::SlotOverflow => f.write_str("binding_slot_overflow"),
            Self::SlotCount { bindings, storage } => {
                write!(f, "binding_slot_count({bindings}, {storage})")
            }
            Self::MissingPath => f.write_str("missing_path"),
            Self::AmbiguousPath => f.write_str("ambiguous_path"),
            Self::CrcCollision(hash) => write!(f, "path_crc_collision({hash:#010x})"),
            Self::PathEvidenceMismatch => f.write_str("path_evidence_mismatch"),
            Self::FilteredTarget(node) => write!(f, "filtered_animation_target({node})"),
            Self::MissingAttachment(node) => write!(f, "missing_renderer_attachment({node})"),
            Self::MissingRenderer(node) => write!(f, "missing_renderer({node})"),
            Self::AmbiguousRenderer(node) => write!(f, "ambiguous_renderer({node})"),
            Self::MissingChannel(node) => write!(f, "missing_renderer_channel({node})"),
            Self::AmbiguousChannel(node) => write!(f, "ambiguous_renderer_channel({node})"),
            Self::UnsupportedMorphProperty => f.write_str("unsupported_morph_property"),
        }
    }
}

impl std::error::Error for AnimError {}
