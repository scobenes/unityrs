//! Read an `AnimationClip` once and retain the serialized records alongside
//! the elected runtime representation.
mod compiled;
pub(crate) mod explicit;
mod packed;

pub use compiled::{CompiledClip, SlotValue, StorageKind, StreamKey, StreamSlot};
pub use packed::PackedRotation;

use crate::{
    AnimError, BindingTable, ClipTiming,
    bindings::read_table,
    read::{Dict, flag, int, version},
    timing::check_rate,
};
use explicit::Explicit;
use std::sync::Arc;
use unityrs_core::{
    ObjectRef,
    limits::{Meter, MeteredVec},
    value::{Parsed, Value},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipField {
    Position,
    Rotation,
    CompressedRotation,
    Euler,
    Scale,
    Float,
    PPtr,
    Editor,
    EulerEditor,
    Compiled,
    Events,
}

impl ClipField {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Position => "m_PositionCurves",
            Self::Rotation => "m_RotationCurves",
            Self::CompressedRotation => "m_CompressedRotationCurves",
            Self::Euler => "m_EulerCurves",
            Self::Scale => "m_ScaleCurves",
            Self::Float => "m_FloatCurves",
            Self::PPtr => "m_PPtrCurves",
            Self::Editor => "m_EditorCurves",
            Self::EulerEditor => "m_EulerEditorCurves",
            Self::Compiled => "m_MuscleClip",
            Self::Events => "m_Events",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClipSource {
    pub field: ClipField,
    /// Stored index in the curve array or generic binding table.
    pub ordinal: usize,
}

#[derive(Debug)]
#[allow(
    clippy::large_enum_variant,
    reason = "inline storage avoids a separate heap reservation"
)]
pub(crate) enum Playback {
    Explicit(MeteredVec<Explicit>),
    Compiled(CompiledClip),
}

/// Clip-owned fields, including raw authoring curves, inert events and
/// non-TRS properties. Keep this object across binds to different rigs.
#[derive(Debug)]
pub struct Clip {
    pub object: ObjectRef,
    parsed: Parsed,
    bindings: Option<BindingTable>,
    timing: Option<ClipTiming>,
    has_metadata_tracks: bool,
    pub(crate) playback: Result<Playback, AnimError>,
}

impl Clip {
    /// Checked typetree dictionary retaining serialized value bits.
    #[must_use]
    pub fn serialized(&self) -> &Parsed {
        &self.parsed
    }

    /// Access a runtime, authoring or event field without copying it.
    pub fn field(&self, field: ClipField, meter: &Arc<Meter>) -> Result<Option<&Value>, AnimError> {
        Dict {
            parsed: &self.parsed,
            meter,
        }
        .root(field.name())
    }

    #[must_use]
    pub fn bindings(&self) -> Option<&BindingTable> {
        self.bindings.as_ref()
    }

    #[must_use]
    pub fn issue(&self) -> Option<&AnimError> {
        self.playback.as_ref().err()
    }

    pub fn timing(&self) -> Result<Option<ClipTiming>, AnimError> {
        self.playback.as_ref().map_err(Clone::clone)?;
        Ok(self.timing)
    }

    pub fn compiled(&self) -> Result<Option<&CompiledClip>, AnimError> {
        match self.playback.as_ref().map_err(Clone::clone)? {
            Playback::Compiled(compiled) => Ok(Some(compiled)),
            Playback::Explicit(_) => Ok(None),
        }
    }

    #[must_use]
    pub fn has_metadata_tracks(&self) -> bool {
        self.has_metadata_tracks
    }

    /// Decode a packed legacy row for inspection even when another source
    /// owns the rotation. No playback slope semantics are assigned.
    pub fn packed_rotation(
        &self,
        ordinal: usize,
        meter: &Arc<Meter>,
    ) -> Result<PackedRotation, AnimError> {
        let dict = Dict {
            parsed: &self.parsed,
            meter,
        };
        let row = dict
            .rows(ClipField::CompressedRotation.name())?
            .get(ordinal)
            .ok_or(AnimError::Malformed(ClipField::CompressedRotation.name()))?;
        packed::read(&dict, row)
    }
}

/// Parse one clip in its owning file's scope. A serialized playback refusal
/// is retained on the result for inspection and returned by binding.
/// Resource and typetree failures are returned immediately.
pub fn read_clip(object: &ObjectRef, meter: &Arc<Meter>) -> Result<Clip, AnimError> {
    if object.class_id() != 74 {
        return Err(AnimError::WrongClass(object.class_id()));
    }
    meter.count_decode(u64::from(object.info().byte_size))?;
    let tree = object
        .tree(None)?
        .ok_or_else(|| unityrs_core::walk::ParseError::NoTypeTree {
            class_id: object.class_id(),
            why: object.why_no_tree(),
        })?;
    let parsed = object.read_typetree_lossless(Some(&tree.tree), true, meter)?;
    let dict = Dict {
        parsed: &parsed,
        meter,
    };
    let mut bindings = None;
    let decoded = (|| {
        let explicit_layout = is_legacy(&dict)?
            || (dict.root("m_MuscleClip")?.is_none()
                && dict.root("m_AnimationClipSettings")?.is_some());
        if dict.root("m_ClipBindingConstant")?.is_some() {
            match read_table(object, &tree.tree, &dict) {
                Ok(table) => bindings = Some(table),
                Err(error) if explicit_layout && !error.resource_refusal() => {}
                Err(error) => return Err(error),
            }
        }
        decode(object, &dict, explicit_layout, bindings.as_ref())
    })();
    let (playback, timing, has_metadata_tracks) = match decoded {
        Ok((playback, timing, metadata)) => (Ok(playback), timing, metadata),
        Err(error) if error.resource_refusal() => return Err(error),
        Err(error) => (Err(error), None, false),
    };
    Ok(Clip {
        object: object.clone(),
        parsed,
        bindings,
        playback,
        timing,
        has_metadata_tracks,
    })
}

fn is_legacy(dict: &Dict<'_, '_>) -> Result<bool, AnimError> {
    if let Some(value) = dict.root("m_AnimationType")? {
        return match int(value, "m_AnimationType")? {
            1 => Ok(true),
            2 => Ok(false),
            3 => Err(AnimError::UnsupportedHumanoidPlayback),
            _ => Err(AnimError::UnsupportedClipLayout),
        };
    }
    dict.root("m_Legacy")?
        .map(|v| flag(v, "m_Legacy"))
        .transpose()
        .map(Option::unwrap_or_default)
}

fn decode(
    object: &ObjectRef,
    dict: &Dict<'_, '_>,
    explicit_layout: bool,
    bindings: Option<&BindingTable>,
) -> Result<(Playback, Option<ClipTiming>, bool), AnimError> {
    let rate = dict.float(dict.parsed.value(), "m_SampleRate")?;
    check_rate(rate)?;
    let version = version(object).ok_or(AnimError::UnsupportedClipLayout)?;
    if !explicit_layout && let Some(muscle) = dict.root("m_MuscleClip")? {
        if let Some(clip) = compiled::decode(dict, muscle, bindings, rate)? {
            let timing = clip.timing;
            let metadata = clip.slot_bindings().iter().any(|slot| {
                matches!(
                    slot.kind,
                    crate::BindingKind::PPtr
                        | crate::BindingKind::Integer
                        | crate::BindingKind::ManagedReference
                        | crate::BindingKind::Material
                )
            });
            return Ok((Playback::Compiled(clip), Some(timing), metadata));
        }
    } else if !explicit_layout && bindings.is_some_and(|table| !table.bindings().is_empty()) {
        return Err(AnimError::UnsupportedClipLayout);
    }
    let (tracks, metadata) = explicit::decode(dict, version)?;
    let timing = if let Some(settings) = dict.root("m_AnimationClipSettings")? {
        Some(ClipTiming::new(
            dict.float(settings, "m_StartTime")?,
            dict.float(settings, "m_StopTime")?,
            rate,
        )?)
    } else {
        metadata
            .span
            .map(|(start, stop)| ClipTiming::new(start, stop, rate))
            .transpose()?
    };
    Ok((Playback::Explicit(tracks), timing, metadata.present))
}

const _: () = crate::send_sync::<ClipField>();
const _: () = crate::send_sync::<ClipSource>();
const _: () = crate::send_sync::<Clip>();
