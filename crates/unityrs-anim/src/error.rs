use std::fmt;
use unityrs_core::env::DerefError;
use unityrs_core::limits::ChargeError;
use unityrs_core::walk::ParseError;
use unityrs_scene::SceneError;

/// A serialized data, target resolution or resource failure.
#[derive(Clone, Debug)]
pub enum AnimError {
    Charge(ChargeError),
    Parse(ParseError),
    Deref(DerefError),
    OutsideView(DerefError),
    Malformed(&'static str),
    WrongClass(i32),
    NegativeFileId(i32),
    ControllerCycle,
    ControllerDepth(u32),
    NestedOverrideController,
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
    UnsupportedClipLayout,
    UnsupportedHumanoidPlayback,
    UnsupportedPackedRotationSlope,
    UnsupportedSupplementalCurve,
    UnsupportedMixedTransformStorage(usize),
    UnsupportedCurveWrap(i32),
    UnsupportedRotationOrder(Option<i32>),
    UnsupportedTangentMode,
    InvalidCurveKeys,
    InvalidQuaternion,
    InvalidSampleValue,
    InvalidRestPose(usize),
    InvalidClipInterval,
    InvalidSampleRate,
    InvalidSchedule,
    AmbiguousPlaybackSource,
    NoPlayableChannels,
    InvalidStreamFrame,
    MissingStreamKey(usize),
    UnverifiedSeed(usize),
    InvalidDenseGrid,
    InvalidPPtrIndex(usize),
    MissingAvatar,
    InvalidAvatarIndex(&'static str),
    UnsupportedHumanoidLayout,
    UnknownHumanDof(usize),
    ConflictingHumanMapping(usize),
    MissingHumanBone(usize),
    UnsupportedHumanoidHierarchy(usize),
    UnsupportedHumanoidAxes(usize),
    UnsupportedHumanoidTwist(&'static str),
    UnsupportedHumanoidAvatarSetting(&'static str),
    UnsupportedHumanoidClipSettings(&'static str),
    UnsupportedBodyMotion(BodyMotion),
    UnsupportedLocomotion,
    UnsupportedTranslationDof,
    UnsupportedAdditivePose,
    UnsupportedHumanoidIk,
}

/// Why a humanoid clip needs the unsupported body model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyMotion {
    /// A `RootT` or `RootQ` DOF maps to a runtime slot.
    BodySlots,
    /// The body slots are absent, but recorded playback still places the
    /// hips from the muscle pose, so hips-local TRS needs the body model.
    HipsCorrection,
}

impl BodyMotion {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::BodySlots => "body_slots",
            Self::HipsCorrection => "hips_correction",
        }
    }
}

const _: () = crate::send_sync::<AnimError>();
const _: () = crate::send_sync::<BodyMotion>();

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
            Self::NegativeFileId(id) => write!(f, "negative_animation_file_id({id})"),
            Self::ControllerCycle => f.write_str("controller_cycle"),
            Self::ControllerDepth(limit) => write!(f, "controller_depth({limit})"),
            Self::NestedOverrideController => f.write_str("nested_override_controller"),
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
            Self::UnsupportedClipLayout => f.write_str("unsupported_clip_layout"),
            Self::UnsupportedHumanoidPlayback => f.write_str("unsupported_humanoid_playback"),
            Self::UnsupportedPackedRotationSlope => {
                f.write_str("unsupported_packed_rotation_slope")
            }
            Self::UnsupportedSupplementalCurve => f.write_str("unsupported_supplemental_curve"),
            Self::UnsupportedMixedTransformStorage(slot) => {
                write!(f, "unsupported_mixed_transform_storage({slot})")
            }
            Self::UnsupportedCurveWrap(mode) => write!(f, "unsupported_curve_wrap({mode})"),
            Self::UnsupportedRotationOrder(order) => {
                write!(f, "unsupported_rotation_order({order:?})")
            }
            Self::UnsupportedTangentMode => f.write_str("unsupported_tangent_mode"),
            Self::InvalidCurveKeys => f.write_str("invalid_curve_keys"),
            Self::InvalidQuaternion => f.write_str("invalid_animation_quaternion"),
            Self::InvalidSampleValue => f.write_str("invalid_animation_sample"),
            Self::InvalidRestPose(node) => write!(f, "invalid_rest_pose({node})"),
            Self::InvalidClipInterval => f.write_str("invalid_clip_interval"),
            Self::InvalidSampleRate => f.write_str("invalid_clip_sample_rate"),
            Self::InvalidSchedule => f.write_str("invalid_sample_schedule"),
            Self::AmbiguousPlaybackSource => f.write_str("ambiguous_playback_source"),
            Self::NoPlayableChannels => f.write_str("no_playable_channels"),
            Self::InvalidStreamFrame => f.write_str("invalid_streamed_frame"),
            Self::MissingStreamKey(slot) => write!(f, "missing_streamed_key({slot})"),
            Self::UnverifiedSeed(slot) => write!(f, "unverified_pre_first_key_seed({slot})"),
            Self::InvalidDenseGrid => f.write_str("invalid_dense_grid"),
            Self::InvalidPPtrIndex(slot) => write!(f, "invalid_pptr_index({slot})"),
            Self::MissingAvatar => f.write_str("missing_avatar"),
            Self::InvalidAvatarIndex(field) => write!(f, "invalid_avatar_index({field})"),
            Self::UnsupportedHumanoidLayout => f.write_str("unsupported_humanoid_layout"),
            Self::UnknownHumanDof(dof) => write!(f, "unknown_human_dof({dof})"),
            Self::ConflictingHumanMapping(dof) => write!(f, "conflicting_human_mapping({dof})"),
            Self::MissingHumanBone(bone) => write!(f, "missing_human_bone({bone})"),
            Self::UnsupportedHumanoidHierarchy(node) => {
                write!(f, "unsupported_humanoid_hierarchy({node})")
            }
            Self::UnsupportedHumanoidAxes(bone) => write!(f, "unsupported_humanoid_axes({bone})"),
            Self::UnsupportedHumanoidTwist(field) => {
                write!(f, "unsupported_humanoid_twist({field})")
            }
            Self::UnsupportedHumanoidAvatarSetting(field) => {
                write!(f, "unsupported_humanoid_avatar_setting({field})")
            }
            Self::UnsupportedHumanoidClipSettings(field) => {
                write!(f, "unsupported_humanoid_clip_settings({field})")
            }
            Self::UnsupportedBodyMotion(cause) => {
                write!(f, "unsupported_body_motion({})", cause.name())
            }
            Self::UnsupportedLocomotion => f.write_str("unsupported_locomotion"),
            Self::UnsupportedTranslationDof => f.write_str("unsupported_translation_dof"),
            Self::UnsupportedAdditivePose => f.write_str("unsupported_additive_pose"),
            Self::UnsupportedHumanoidIk => f.write_str("unsupported_humanoid_ik"),
        }
    }
}

impl std::error::Error for AnimError {}
