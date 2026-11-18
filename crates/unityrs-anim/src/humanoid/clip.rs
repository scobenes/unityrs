//! The humanoid slot table of a compiled clip. `m_IndexArray` maps each
//! humanoid DOF to a runtime slot of `m_Clip`, or to the absent sentinel.
use crate::{
    AnimError, BindingKind, SlotCounts, StorageKind,
    clip::decode_stream,
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

    /// One of the root basis flags (`ROOT_BASIS`), whose neutral profile
    /// `HumanoidClip::root_basis_refusal` checks.
    #[must_use]
    pub fn is_root_basis(&self) -> bool {
        ROOT_BASIS.contains(&self.field)
    }
}

/// The root basis flags: the Original/Body Orientation, Y and XZ bases,
/// Based Upon Feet, and the runtime start-at-origin flag.
/// [Unity root motion](https://docs.unity3d.com/2022.3/Documentation/Manual/RootMotion.html)
/// · [Unity AnimationClipSettings](https://docs.unity3d.com/2022.3/Documentation/ScriptReference/AnimationClipSettings.html)
pub const ROOT_BASIS: [&str; 5] = [
    "m_KeepOriginalOrientation",
    "m_KeepOriginalPositionY",
    "m_KeepOriginalPositionXZ",
    "m_HeightFromFeet",
    "m_StartAtOrigin",
];

/// Settings in refusal order: mirror and offsets, loop and bake flags,
/// then the root bases, which `ROOT_BASIS` names.
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

/// A generic Transform binding on a humanoid clip: a second writer for
/// its node beside the muscle pose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransformWriter {
    /// The binding's row in `genericBindings`.
    pub binding: usize,
    /// The CRC32 of its path, as a human skeleton `m_ID`.
    pub path: u32,
}

/// The humanoid part of a compiled clip, read without the generic binding
/// layout so that a binding record change does not hide the humanoid causes.
#[derive(Debug)]
pub struct HumanoidClip {
    index: MeteredVec<i32>,
    counts: SlotCounts,
    settings: [ClipSetting; SETTINGS.len()],
    reference_pose: bool,
    generic_root: bool,
    writers: MeteredVec<TransformWriter>,
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

    /// The generic Transform bindings beside the humanoid ones.
    #[must_use]
    pub fn transform_writers(&self) -> &[TransformWriter] {
        self.writers.as_slice()
    }

    /// The first root basis flag off its neutral profile: the three
    /// Original bases and Based Upon Feet off, and `m_StartAtOrigin` the
    /// complement of `m_KeepOriginalPositionXZ`, as every built fixture
    /// clip stores it. A missing flag has no confirmed default.
    #[must_use]
    pub fn root_basis_refusal(&self) -> Option<&'static str> {
        let value = |field: &str| {
            self.settings
                .iter()
                .find(|setting| setting.field == field)
                .and_then(|setting| setting.value)
        };
        for &field in &ROOT_BASIS[..4] {
            if value(field) != Some(SettingValue::Bool(false)) {
                return Some(field);
            }
        }
        // The XZ flag is false here, so the start-at-origin flag is set.
        (value("m_StartAtOrigin") != Some(SettingValue::Bool(true))).then_some("m_StartAtOrigin")
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
    // Any 2022.3 or 6000 layout the type tree describes reads; the
    // qualification evidence records the exact Editor versions verified.
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
    // The binding table sizes the slot owners only once its widths cover
    // the serialized storage exactly.
    let rows = check_bindings(dict, values, total)?;
    let mut owners = MeteredVec::with_capacity(dict.meter, total)?;
    dict.meter.count_decode(total as u64)?;
    for _ in 0..total {
        owners.push(None::<usize>)?;
    }
    let mut writers = MeteredVec::new(dict.meter);
    let mut kinds = MeteredVec::with_capacity(dict.meter, counts.streamed)?;
    for row in rows.as_slice() {
        match row.role {
            Role::Human(dof) => {
                if let Some(owner) = owners.as_mut_slice().get_mut(row.first) {
                    *owner = Some(dof);
                }
            }
            Role::Transform(path) => writers.push(TransformWriter {
                binding: row.binding,
                path,
            })?,
            Role::Other => {}
        }
        for _ in 0..row.width {
            if kinds.len() < counts.streamed {
                kinds.push(row.kind)?;
            }
        }
    }
    // Every mapped DOF owns the slot of its own humanoid binding, so a
    // shared, generic or out-of-range slot conflicts.
    let mut index = MeteredVec::with_capacity(dict.meter, DOF_COUNT)?;
    for (dof, &value) in values.iter().enumerate() {
        if value != ABSENT_SLOT {
            let owner = usize::try_from(value)
                .ok()
                .and_then(|slot| owners.as_slice().get(slot).copied())
                .flatten();
            if owner != Some(dof) {
                return Err(AnimError::ConflictingHumanMapping(dof));
            }
        }
        index.push(value)?;
    }
    // The slots of the seven body DOFs (`RootT`, `RootQ`), when mapped.
    let body: [Option<usize>; 7] = std::array::from_fn(|i| {
        values
            .get(7 + i)
            .and_then(|&slot| usize::try_from(slot).ok())
    });
    check_payload(dict, muscle, counts, kinds.as_slice(), &body)?;
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
        writers,
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

/// The serialized payload must populate every counted slot: each
/// streamed curve keyed or seeded in valid frames (finite continuous
/// coefficients), and a dense grid of `m_FrameCount` frames of every dense
/// curve at a finite begin time and a finite, positive sample rate. A
/// mapped body slot's dense or constant values must be finite too.
fn check_payload(
    dict: &Dict<'_, '_>,
    muscle: &Value,
    counts: SlotCounts,
    kinds: &[BindingKind],
    body: &[Option<usize>],
) -> Result<(), AnimError> {
    let clip = dict.required(muscle, "m_Clip")?;
    let clip = dict.field(clip, "data")?.unwrap_or(clip);
    let stream = dict.required(clip, "m_StreamedClip")?;
    let words = dict.words(stream, "data", false)?;
    decode_stream(dict, counts.streamed, words, kinds, &[])?;
    let dense = dict.required(clip, "m_DenseClip")?;
    let frames = dict.count(dense, "m_FrameCount")?;
    let samples = dict.words(dense, "m_SampleArray", true)?;
    if counts.dense.checked_mul(frames) != Some(samples.len()) || (counts.dense > 0 && frames == 0)
    {
        return Err(AnimError::InvalidDenseGrid);
    }
    if counts.dense > 0 {
        let begin = dict.float(dense, "m_BeginTime")?;
        let rate = dict.float(dense, "m_SampleRate")?;
        if !begin.is_finite() || !rate.is_finite() || rate <= 0. {
            return Err(AnimError::InvalidDenseGrid);
        }
    }
    let constant = match dict.field(clip, "m_ConstantClip")? {
        None => &[][..],
        Some(constant) => dict.words(constant, "data", true)?,
    };
    let finite = |word: &u32| f32::from_bits(*word).is_finite();
    for &slot in body.iter().flatten() {
        // A streamed slot's coefficients were checked as it decoded.
        let Some(offset) = slot.checked_sub(counts.streamed) else {
            continue;
        };
        if offset < counts.dense {
            if !samples
                .iter()
                .skip(offset)
                .step_by(counts.dense)
                .all(finite)
            {
                return Err(AnimError::Malformed("m_SampleArray"));
            }
        } else if !constant.get(offset - counts.dense).is_some_and(finite) {
            return Err(AnimError::Malformed("m_ConstantClip"));
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum Role {
    /// A humanoid DOF binding and its DOF.
    Human(usize),
    /// A local Transform property and its path hash.
    Transform(u32),
    Other,
}

#[derive(Clone, Copy)]
struct Row {
    binding: usize,
    first: usize,
    width: usize,
    role: Role,
    /// How a streamed key of this row decodes; discrete rows keep raw bits.
    kind: BindingKind,
}

/// Read the binding rows by field name, so the 6000 `metaData` field does
/// not hide the humanoid causes. A humanoid DOF binding is an Animator
/// (class 95) record with `customType` 8, one slot wide, whose `attribute`
/// is its DOF and whose first slot `m_IndexArray` maps it to. Other rows
/// take their generic widths: 3, 4, 3 and 3 slots for local position,
/// rotation, scale and Euler, and one slot otherwise.
/// [AssetStudio AnimationClip v0.16.47](https://github.com/Perfare/AssetStudio/blob/v0.16.47/AssetStudio/Classes/AnimationClip.cs#L792-L824)
fn check_bindings(
    dict: &Dict<'_, '_>,
    index: &[i32],
    total: usize,
) -> Result<MeteredVec<Row>, AnimError> {
    let Some(table) = dict.root("m_ClipBindingConstant")? else {
        return Err(AnimError::UnsupportedBindingLayout);
    };
    let records = dict.list(dict.required(table, "genericBindings")?, "genericBindings")?;
    let mut rows = MeteredVec::with_capacity(dict.meter, records.len())?;
    let mut first = 0_usize;
    for (binding, record) in records.iter().enumerate() {
        let class = match dict.field(record, "typeID")? {
            Some(value) => value,
            None => dict.required(record, "classID")?,
        };
        let class = class.as_i64().ok_or(AnimError::Malformed("typeID"))?;
        let custom = dict.int(record, "customType")?;
        let attribute = dict.count(record, "attribute")?;
        let flag = |name| -> Result<bool, AnimError> {
            Ok(dict
                .field(record, name)?
                .and_then(Value::as_u64)
                .is_some_and(|value| value != 0))
        };
        let discrete =
            flag("isPPtrCurve")? || flag("isIntCurve")? || flag("isSerializeReferenceCurve")?;
        let transform = class == 4 || (class == 224 && (1..=4).contains(&attribute));
        let (role, width, kind) = if class == 95 && custom == 8 {
            if attribute >= DOF_COUNT {
                return Err(AnimError::UnknownHumanDof(attribute));
            }
            if index
                .get(attribute)
                .and_then(|&slot| usize::try_from(slot).ok())
                != Some(first)
            {
                return Err(AnimError::ConflictingHumanMapping(attribute));
            }
            (Role::Human(attribute), 1, BindingKind::UnsupportedScalar)
        } else if transform {
            let width = match (custom, attribute) {
                (0, 1 | 3) | (0..=5, 4) => 3,
                (0, 2) => 4,
                _ => {
                    return Err(AnimError::UnknownBindingWidth {
                        binding,
                        class_id: i32::try_from(class).unwrap_or(i32::MAX),
                        custom_type: u8::try_from(custom).unwrap_or(u8::MAX),
                        attribute: u32::try_from(attribute).unwrap_or(u32::MAX),
                    });
                }
            };
            let path = dict.count(record, "path")?;
            let path = u32::try_from(path).map_err(|_| AnimError::Malformed("path"))?;
            (Role::Transform(path), width, BindingKind::UnsupportedScalar)
        } else if discrete {
            (Role::Other, 1, BindingKind::ManagedReference)
        } else {
            (Role::Other, 1, BindingKind::UnsupportedScalar)
        };
        rows.push(Row {
            binding,
            first,
            width,
            role,
            kind,
        })?;
        first = first.checked_add(width).ok_or(AnimError::SlotOverflow)?;
    }
    if first != total {
        return Err(AnimError::SlotCount {
            bindings: records.len(),
            storage: total,
        });
    }
    Ok(rows)
}

const _: () = crate::send_sync::<HumanoidClip>();
const _: () = crate::send_sync::<HumanoidSlot>();
const _: () = crate::send_sync::<ClipSetting>();
const _: () = crate::send_sync::<TransformWriter>();
const _: () = crate::send_sync::<SettingValue>();
const _: () = crate::send_sync::<DofKind>();
