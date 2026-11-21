//! B5's release gate (plan "B5's release gate", B5b): a body-bearing clip
//! plays only in a configuration class E5's same-bundle replay recorded
//! and passed in both Editor versions. A class is what the runtime can
//! check, never a rig's identity: the Avatar's absent body slots, the
//! chain's F5 feature and placement, a human-scale interval its recordings
//! span, and the clip's admitted root variant.
//!
//! `VERIFIED` is empty until B5b fills it from E5, so `release_gate`
//! refuses every configuration with today's refusal,
//! `unsupported_body_motion` (`body_slots`). B4's admission by argument
//! (the plan's (b)) admits nothing here: only a recorded, passing class
//! does, so a combination B4's scope allows but E5 never recorded (a
//! scaled Animator on a rig without toes, say) stays refused.
use super::{
    BODY_BONES, BodyAvatar, LEAVES,
    chain::{AnimatorChain, ChainFeature, admit_chain},
    root::RootSettings,
    topology_scope,
};
use crate::{AnimError, BodyMotion};
use unityrs_core::limits::Meter;

/// The Editor version families a verified class passed in, each needed
/// (`anim_body_runs.EDITORS`).
pub const EDITORS: [&str; 2] = ["2022", "6000"];

/// One configuration class E5 recorded and passed in every `EDITORS`
/// family.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VerifiedClass {
    /// The absent body slots, a bit per `BODY_SLOTS` index, the leaves
    /// (Eyes and Jaw) aside.
    pub absent: u32,
    pub feature: ChainFeature,
    /// Whether the Animator node is at `OFFSET_PLACEMENT`.
    pub offset: bool,
    /// The human scale `m_Scale`'s interval, inclusive, its ends the
    /// recorded rigs' values.
    pub human_scale: [f32; 2],
    /// The clip's admitted root variant (`AdmittedRoot::variant`).
    pub variant: &'static str,
}

/// The classes E5 verified: none until B5b fills the table from E5.
pub const VERIFIED: [VerifiedClass; 0] = [];

/// One runtime configuration, as `release_gate` compares it with a class.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Configuration {
    pub absent: u32,
    pub feature: ChainFeature,
    pub offset: bool,
    pub human_scale: f32,
    pub variant: &'static str,
    /// The Editor version family of the bundle (an `EDITORS` entry).
    pub editor: &'static str,
}

/// The configuration of a body-bearing clip on `body`, after B3's and
/// B4's checks: the topology scope, the chain's validated scope, its
/// admission of the clip's settings and the bundle's Editor family. Each
/// refuses as those checks do; an Editor outside `EDITORS` refuses with
/// today's refusal.
pub fn configuration(
    body: &BodyAvatar<'_>,
    chain: &AnimatorChain,
    settings: &RootSettings,
    unity_version: &str,
    meter: &Meter,
) -> Result<Configuration, AnimError> {
    let human = body.human();
    topology_scope(human, &body.masses, meter)?;
    admit_chain(chain, settings)?;
    let (feature, offset) = chain
        .validated()
        .ok_or(AnimError::UnsupportedAnimatorRoot)?;
    let family = unity_version.split('.').next().unwrap_or_default();
    let editor = EDITORS
        .into_iter()
        .find(|&known| known == family)
        .ok_or(AnimError::UnsupportedBodyMotion(BodyMotion::BodySlots))?;
    let mut absent = 0_u32;
    for slot in 0..BODY_BONES {
        if human
            .bones
            .as_slice()
            .get(slot)
            .is_none_or(|&node| node < 0)
        {
            absent |= 1 << slot;
        }
    }
    Ok(Configuration {
        absent: absent & !LEAVES,
        feature,
        offset,
        human_scale: body.scale(),
        variant: settings.admitted().variant,
        editor,
    })
}

/// Whether `class` holds `configuration`.
#[must_use]
pub fn holds(class: &VerifiedClass, configuration: &Configuration) -> bool {
    let [low, high] = class.human_scale;
    class.absent == configuration.absent
        && class.feature == configuration.feature
        && class.offset == configuration.offset
        && (low..=high).contains(&configuration.human_scale)
        && class.variant == configuration.variant
        && EDITORS.contains(&configuration.editor)
}

/// The release gate: `Ok` only for a configuration some class of `table`
/// holds, otherwise today's refusal. The runtime passes `VERIFIED`.
pub fn release_gate(
    table: &[VerifiedClass],
    configuration: &Configuration,
) -> Result<(), AnimError> {
    if table.iter().any(|class| holds(class, configuration)) {
        Ok(())
    } else {
        Err(AnimError::UnsupportedBodyMotion(BodyMotion::BodySlots))
    }
}

const _: () = crate::send_sync::<VerifiedClass>();
const _: () = crate::send_sync::<Configuration>();
