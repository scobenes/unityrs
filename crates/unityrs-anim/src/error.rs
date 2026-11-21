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
    UnsupportedHumanoidLimbTwist(usize),
    UnsupportedHumanoidAvatarSetting(&'static str),
    UnsupportedHumanoidClipSettings(&'static str),
    UnsupportedHumanoidRootBasis(&'static str),
    UnsupportedAnimatorRoot,
    ConflictingHumanoidWriter(usize),
    UnsupportedBodyMotion(BodyMotion),
    UnsupportedLocomotion,
    UnsupportedTranslationDof,
    UnsupportedAdditivePose,
    UnsupportedHumanoidIk,
    InvalidBodyAvatar(BodyAvatarFault),
    UnsupportedHumanoidBodyModel(BodyModel),
}

/// Why a humanoid clip needs the unsupported body model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyMotion {
    /// A `RootT` or `RootQ` DOF maps to a runtime slot. The hips
    /// correction and root projection a recorded clip may need with absent
    /// body slots are measured on recordings, so only the qualification
    /// evidence reports them.
    BodySlots,
}

impl BodyMotion {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::BodySlots => "body_slots",
        }
    }
}

/// Why an Avatar's tables cannot feed the body model
/// (`docs/plans/humanoid-body.md` E0a to E0c). A slot is an index into the
/// 25-slot `m_HumanBoneIndex` order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyAvatarFault {
    /// The Avatar serializes no `m_HumanBoneMass`.
    MassMissing,
    /// `m_HumanBoneMass` has this many entries, not one per body slot.
    MassLength(usize),
    MassNonFinite(usize),
    MassNegative(usize),
    /// An unmapped slot holds a nonzero mass.
    MassAbsentSlot(usize),
    /// The masses do not sum to one within the derived bound.
    MassSum,
    /// An `m_TOS` ID appears again with another path: the reference takes
    /// the last, a first-match lookup the first, so neither is trusted.
    TosConflict(u32),
    /// A mapped slot's skeleton node has no `m_TOS` path.
    SlotPath(usize),
    /// A mapped slot's node resolves to an earlier slot's `m_TOS` path.
    SlotAlias(usize),
    /// `m_Scale` is not finite and positive.
    Scale,
    /// `m_RootX.t.y` is not `m_Scale` within two f32 roundings.
    RootHeight,
}

impl fmt::Display for BodyAvatarFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MassMissing => f.write_str("mass_missing"),
            Self::MassLength(n) => write!(f, "mass_length({n})"),
            Self::MassNonFinite(slot) => write!(f, "mass_non_finite({slot})"),
            Self::MassNegative(slot) => write!(f, "mass_negative({slot})"),
            Self::MassAbsentSlot(slot) => write!(f, "mass_absent_slot({slot})"),
            Self::MassSum => f.write_str("mass_sum"),
            Self::TosConflict(id) => write!(f, "tos_conflict({id})"),
            Self::SlotPath(slot) => write!(f, "slot_path({slot})"),
            Self::SlotAlias(slot) => write!(f, "slot_alias({slot})"),
            Self::Scale => f.write_str("scale"),
            Self::RootHeight => f.write_str("root_height"),
        }
    }
}

/// Why the frozen body model's scope leaves an Avatar out (plan 2.1 and
/// section 8).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyModel {
    /// A zero-mass leaf (an Eye or the Jaw) has a nonzero mass.
    LeafMass,
    ChestAbsent,
    NeckAbsent,
    /// Exactly one of the two Shoulders is mapped.
    OneShoulder,
    /// The absent body slots but the leaves are no recorded training rig's
    /// pattern.
    UnrecordedTopology,
    /// An edge between mapped slots that freeze v8 fitted no coefficient
    /// for.
    UnfittedEdge,
    /// The shoulder-hip twist exceeds the frozen limit.
    ShoulderHipTwist,
    /// The shoulder-hip twist is undefined: no `up`, or a lateral along it.
    UndefinedTwist,
    /// A spine Left-Right muscle exceeds the frozen lateral limit.
    SpineLateralRange,
    /// An edge of the Avatar is in a free coefficient combination's
    /// support.
    FreeCombination,
    /// The hips solve has no finite answer.
    DegenerateSolve,
    /// A node below the hips has no rest scale from the rig.
    MissingRestScale,
    /// A mapped bone is not below the hips.
    DetachedBone,
    /// Two `m_Skeleton` nodes resolve to one rig Transform.
    SharedTransform,
    /// A root rotation E4 cannot normalize: its norm is not above four
    /// times its sample's error.
    DegenerateRootRotation,
    /// A root yaw whose error E4 leaves unbounded: a half turn about a
    /// horizontal axis.
    UndefinedYaw,
    /// A rig Transform's parent is not its `m_Skeleton` parent's.
    MisparentedTransform,
    /// A rig Transform below the hips' is no `m_Skeleton` node's.
    UnmappedTransform,
    /// The rig's graph dropped a child of a Transform below the hips: past
    /// the depth limit, a cycle, a broken pointer or a child that failed
    /// to read.
    TruncatedRig,
    /// A body position, scale, weight, frame or result is not finite.
    NonFiniteBody,
    /// The frame's lateral is along `up` within its rounding.
    DegenerateFrame,
    /// The rest frame is not finite or not orthonormal.
    InvalidRestFrame,
}

impl BodyModel {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::LeafMass => "leaf_mass",
            Self::ChestAbsent => "chest_absent",
            Self::NeckAbsent => "neck_absent",
            Self::OneShoulder => "one_shoulder",
            Self::UnrecordedTopology => "unrecorded_topology",
            Self::UnfittedEdge => "unfitted_edge",
            Self::ShoulderHipTwist => "shoulder_hip_twist",
            Self::UndefinedTwist => "undefined_twist",
            Self::SpineLateralRange => "spine_lateral_range",
            Self::FreeCombination => "free_combination",
            Self::DegenerateSolve => "degenerate_solve",
            Self::MissingRestScale => "missing_rest_scale",
            Self::DetachedBone => "detached_bone",
            Self::SharedTransform => "shared_transform",
            Self::DegenerateRootRotation => "degenerate_root_rotation",
            Self::UndefinedYaw => "undefined_yaw",
            Self::MisparentedTransform => "misparented_transform",
            Self::UnmappedTransform => "unmapped_transform",
            Self::TruncatedRig => "truncated_rig",
            Self::NonFiniteBody => "non_finite_body",
            Self::DegenerateFrame => "degenerate_frame",
            Self::InvalidRestFrame => "invalid_rest_frame",
        }
    }
}

const _: () = crate::send_sync::<AnimError>();
const _: () = crate::send_sync::<BodyMotion>();
const _: () = crate::send_sync::<BodyAvatarFault>();
const _: () = crate::send_sync::<BodyModel>();

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
    #[allow(clippy::too_many_lines, reason = "one arm per typed error")]
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
            Self::UnsupportedHumanoidLimbTwist(muscle) => {
                write!(f, "unsupported_humanoid_limb_twist({muscle})")
            }
            Self::UnsupportedHumanoidAvatarSetting(field) => {
                write!(f, "unsupported_humanoid_avatar_setting({field})")
            }
            Self::UnsupportedHumanoidClipSettings(field) => {
                write!(f, "unsupported_humanoid_clip_settings({field})")
            }
            Self::UnsupportedHumanoidRootBasis(field) => {
                write!(f, "unsupported_humanoid_root_basis({field})")
            }
            Self::UnsupportedAnimatorRoot => f.write_str("unsupported_animator_root"),
            Self::ConflictingHumanoidWriter(binding) => {
                write!(f, "conflicting_humanoid_writer({binding})")
            }
            Self::UnsupportedBodyMotion(cause) => {
                write!(f, "unsupported_body_motion({})", cause.name())
            }
            Self::UnsupportedLocomotion => f.write_str("unsupported_locomotion"),
            Self::UnsupportedTranslationDof => f.write_str("unsupported_translation_dof"),
            Self::UnsupportedAdditivePose => f.write_str("unsupported_additive_pose"),
            Self::UnsupportedHumanoidIk => f.write_str("unsupported_humanoid_ik"),
            Self::InvalidBodyAvatar(fault) => write!(f, "invalid_body_avatar({fault})"),
            Self::UnsupportedHumanoidBodyModel(cause) => {
                write!(f, "unsupported_humanoid_body_model({})", cause.name())
            }
        }
    }
}

impl std::error::Error for AnimError {}
