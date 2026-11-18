//! The humanoid slot table of a compiled clip. `m_IndexArray` maps each
//! humanoid DOF to a runtime slot of `m_Clip`, or to the absent sentinel.
use crate::{
    AnimError, SlotCounts, StorageKind,
    read::{Dict, version},
};
use unityrs_core::{
    ObjectRef,
    limits::MeteredVec,
    value::{PrimArray, Value},
};

/// The `m_IndexArray` value of a DOF without a runtime slot.
pub const ABSENT_SLOT: i32 = -1;

/// DOFs in the built 2022.3 and 6000 layout: 7 motion, 7 body (`RootT`,
/// `RootQ`), 4 goals of 7, 95 muscles in `HumanTrait` order and 21
/// translation DOFs of 3. A humanoid clip binding's `attribute` is its DOF.
pub const DOF_COUNT: usize = 200;
pub const MUSCLE_COUNT: usize = 95;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DofKind {
    Motion,
    Body,
    Goal,
    Muscle,
    Translation,
}

impl DofKind {
    /// The kind and the ordinal within it of a DOF in the built layout.
    #[must_use]
    pub const fn of(dof: usize) -> Option<(Self, usize)> {
        Some(match dof {
            0..7 => (Self::Motion, dof),
            7..14 => (Self::Body, dof - 7),
            14..42 => (Self::Goal, dof - 14),
            42..137 => (Self::Muscle, dof - 42),
            137..DOF_COUNT => (Self::Translation, dof - 137),
            _ => return None,
        })
    }
}

/// One DOF of the slot inventory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HumanoidSlot {
    pub dof: usize,
    pub kind: DofKind,
    pub ordinal: usize,
    /// The serialized `m_IndexArray` value, `ABSENT_SLOT` when absent.
    pub index_value: i32,
    /// The storage range and the slot's offset within it.
    pub storage: Option<(StorageKind, usize)>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SettingValue {
    Bool(bool),
    Float(f32),
}

/// One serialized `m_MuscleClip` setting. A missing field has no
/// confirmed layout default.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClipSetting {
    pub field: &'static str,
    pub value: Option<SettingValue>,
}

impl ClipSetting {
    /// False flags and zero offsets, the only values P2b could verify.
    #[must_use]
    pub fn neutral(&self) -> bool {
        match self.value {
            Some(SettingValue::Bool(value)) => !value,
            Some(SettingValue::Float(value)) => value == 0.,
            None => false,
        }
    }
}

/// Settings in refusal order: mirror and offsets, loop and bake flags,
/// then the root bases. Later fields default on code-made clips.
const SETTINGS: [(&str, bool); 14] = [
    ("m_Mirror", true),
    ("m_CycleOffset", false),
    ("m_Level", false),
    ("m_OrientationOffsetY", false),
    ("m_LoopTime", true),
    ("m_LoopBlend", true),
    ("m_LoopBlendOrientation", true),
    ("m_LoopBlendPositionY", true),
    ("m_LoopBlendPositionXZ", true),
    ("m_KeepOriginalOrientation", true),
    ("m_KeepOriginalPositionY", true),
    ("m_KeepOriginalPositionXZ", true),
    ("m_HeightFromFeet", true),
    ("m_StartAtOrigin", true),
];

/// The humanoid part of a compiled clip, read without the generic binding
/// layout so that a binding record change does not hide the humanoid causes.
#[derive(Debug)]
pub struct HumanoidClip {
    index: MeteredVec<i32>,
    counts: SlotCounts,
    settings: [ClipSetting; SETTINGS.len()],
    reference_pose: bool,
    generic_root: bool,
}

impl HumanoidClip {
    /// Serialized storage counts in streamed, dense, constant order.
    #[must_use]
    pub const fn counts(&self) -> SlotCounts {
        self.counts
    }

    #[must_use]
    pub fn settings(&self) -> &[ClipSetting] {
        &self.settings
    }

    /// `m_ValueArrayReferencePose` holds an additive reference pose.
    #[must_use]
    pub const fn has_reference_pose(&self) -> bool {
        self.reference_pose
    }

    /// `m_HasGenericRootTransform` or `m_HasMotionFloatCurves` is set.
    #[must_use]
    pub const fn has_generic_root(&self) -> bool {
        self.generic_root
    }

    /// The complete inventory in DOF order.
    pub fn slots(&self) -> impl Iterator<Item = HumanoidSlot> + '_ {
        self.index
            .as_slice()
            .iter()
            .enumerate()
            .filter_map(move |(dof, &index_value)| {
                let (kind, ordinal) = DofKind::of(dof)?;
                Some(HumanoidSlot {
                    dof,
                    kind,
                    ordinal,
                    index_value,
                    storage: self.storage(index_value),
                })
            })
    }

    /// The mapped DOFs of one kind.
    pub fn mapped(&self, kind: DofKind) -> impl Iterator<Item = HumanoidSlot> + '_ {
        self.slots()
            .filter(move |slot| slot.kind == kind && slot.storage.is_some())
    }

    fn storage(&self, index: i32) -> Option<(StorageKind, usize)> {
        let index = usize::try_from(index).ok()?;
        let SlotCounts {
            streamed,
            dense,
            constant,
        } = self.counts;
        if index < streamed {
            Some((StorageKind::Streamed, index))
        } else if index - streamed < dense {
            Some((StorageKind::Dense, index - streamed))
        } else if index - streamed - dense < constant {
            Some((StorageKind::Constant, index - streamed - dense))
        } else {
            None
        }
    }
}

/// Whether the serialized `m_IndexArray` maps any DOF. Absent or
/// non-integer arrays are left to the compiled reader's layout check.
pub(crate) fn is_humanoid(dict: &Dict<'_, '_>, muscle: &Value) -> Result<bool, AnimError> {
    Ok(match dict.field(muscle, "m_IndexArray")? {
        Some(Value::Prims(PrimArray::I32(values))) => {
            dict.meter.count_decode(values.len() as u64)?;
            values.iter().any(|&slot| slot != ABSENT_SLOT)
        }
        _ => false,
    })
}

pub(crate) fn read(
    object: &ObjectRef,
    dict: &Dict<'_, '_>,
    muscle: &Value,
) -> Result<HumanoidClip, AnimError> {
    if !version(object).is_some_and(|(major, minor)| (major == 2022 && minor >= 3) || major == 6000)
    {
        return Err(AnimError::UnsupportedHumanoidLayout);
    }
    let Some(Value::Prims(PrimArray::I32(values))) = dict.field(muscle, "m_IndexArray")? else {
        return Err(AnimError::UnsupportedHumanoidLayout);
    };
    if values.len() != DOF_COUNT {
        return Err(AnimError::UnsupportedHumanoidLayout);
    }
    let counts = counts(dict, muscle)?;
    let total = counts
        .streamed
        .checked_add(counts.dense)
        .and_then(|n| n.checked_add(counts.constant))
        .ok_or(AnimError::SlotOverflow)?;
    let mut owners = MeteredVec::with_capacity(dict.meter, total)?;
    dict.meter.count_decode(total as u64)?;
    for _ in 0..total {
        owners.push(None::<usize>)?;
    }
    let mut index = MeteredVec::with_capacity(dict.meter, DOF_COUNT)?;
    for (dof, &value) in values.iter().enumerate() {
        if value != ABSENT_SLOT {
            let owner = usize::try_from(value)
                .ok()
                .and_then(|slot| owners.as_mut_slice().get_mut(slot))
                .ok_or(AnimError::ConflictingHumanMapping(dof))?;
            if owner.replace(dof).is_some() {
                return Err(AnimError::ConflictingHumanMapping(dof));
            }
        }
        index.push(value)?;
    }
    check_bindings(dict, index.as_slice(), total)?;
    let mut settings = SETTINGS.map(|(field, _)| ClipSetting { field, value: None });
    for (setting, (field, boolean)) in settings.iter_mut().zip(SETTINGS) {
        setting.value = match (boolean, dict.field(muscle, field)?) {
            (_, None) => None,
            (true, Some(Value::Bool(value))) => Some(SettingValue::Bool(*value)),
            (false, Some(Value::F32(bits))) => Some(SettingValue::Float(f32::from_bits(*bits))),
            _ => return Err(AnimError::Malformed(field)),
        };
    }
    let reference_pose = match dict.field(muscle, "m_ValueArrayReferencePose")? {
        None => false,
        Some(Value::List(rows)) => !rows.is_empty(),
        Some(Value::Prims(PrimArray::F32(rows))) => !rows.is_empty(),
        Some(_) => return Err(AnimError::Malformed("m_ValueArrayReferencePose")),
    };
    let mut generic_root = false;
    for field in ["m_HasGenericRootTransform", "m_HasMotionFloatCurves"] {
        generic_root |= match dict.root(field)? {
            None => false,
            Some(Value::Bool(value)) => *value,
            Some(_) => return Err(AnimError::Malformed(field)),
        };
    }
    Ok(HumanoidClip {
        index,
        counts,
        settings,
        reference_pose,
        generic_root,
    })
}

fn counts(dict: &Dict<'_, '_>, muscle: &Value) -> Result<SlotCounts, AnimError> {
    let clip = dict.required(muscle, "m_Clip")?;
    let clip = dict.field(clip, "data")?.unwrap_or(clip);
    let stream = dict.required(clip, "m_StreamedClip")?;
    let discrete = match dict.field(stream, "discreteCurveCount")? {
        None => 0,
        Some(_) => dict.count(stream, "discreteCurveCount")?,
    };
    let constant = match dict.field(clip, "m_ConstantClip")? {
        None => 0,
        Some(constant) => dict.words(constant, "data", true)?.len(),
    };
    Ok(SlotCounts {
        streamed: dict
            .count(stream, "curveCount")?
            .checked_add(discrete)
            .ok_or(AnimError::SlotOverflow)?,
        dense: dict.count(dict.required(clip, "m_DenseClip")?, "m_CurveCount")?,
        constant,
    })
}

/// A humanoid DOF binding is an Animator (class 95) record with
/// `customType` 8, one slot wide, whose `attribute` is its DOF. Its slot is
/// its ordinal. Other bindings on a humanoid clip are outside P2b scope.
fn check_bindings(dict: &Dict<'_, '_>, index: &[i32], total: usize) -> Result<(), AnimError> {
    let Some(table) = dict.root("m_ClipBindingConstant")? else {
        return Err(AnimError::UnsupportedBindingLayout);
    };
    let rows = dict.list(dict.required(table, "genericBindings")?, "genericBindings")?;
    if rows.len() != total {
        return Err(AnimError::SlotCount {
            bindings: rows.len(),
            storage: total,
        });
    }
    for (slot, row) in rows.iter().enumerate() {
        let class = match dict.field(row, "typeID")? {
            Some(value) => value,
            None => dict.required(row, "classID")?,
        };
        if class.as_i64() != Some(95) || dict.int(row, "customType")? != 8 {
            return Err(AnimError::UnsupportedHumanoidLayout);
        }
        let dof = dict.count(row, "attribute")?;
        let mapped = index.get(dof).ok_or(AnimError::UnknownHumanDof(dof))?;
        if usize::try_from(*mapped).ok() != Some(slot) {
            return Err(AnimError::ConflictingHumanMapping(dof));
        }
    }
    Ok(())
}

const _: () = crate::send_sync::<HumanoidClip>();
const _: () = crate::send_sync::<HumanoidSlot>();
const _: () = crate::send_sync::<ClipSetting>();
const _: () = crate::send_sync::<SettingValue>();
const _: () = crate::send_sync::<DofKind>();
