//! Serialized binding records, with widths independent of curve storage.
//!
//! The synthetic release-tree matrix covers the following layouts.
//!
//! | Engine version | Class field width | Byte flags | Alignment |
//! |---|---|---|---|
//! | 5.5 | unsigned 16-bit | `PPtr` | naturally aligned |
//! | 2019.4 | signed 32-bit | `PPtr` | end of record |
//! | 2022.1 | signed 32-bit | `PPtr`, integer | end of record |
//! | 2022.3 and 6000.0 | signed 32-bit | `PPtr`, integer, managed reference | end of record |
//!
//! Release-tree evidence for the managed-reference column is in the
//! [2022.3.0f1](https://raw.githubusercontent.com/AssetRipper/TypeTreeDumps/main/StructsDump/release/2022.3.0f1.dump)
//! and [6000.0.0f1](https://raw.githubusercontent.com/AssetRipper/TypeTreeDumps/main/StructsDump/release/6000.0.0f1.dump)
//! typetree dumps. Each marks the last flag with the align-after bit.
use crate::{
    AnimError,
    read::{Dict, version},
};
use std::sync::Arc;
use unityrs_core::ObjectRef;
use unityrs_core::env::PPtr;
use unityrs_core::limits::{Meter, MeteredVec};
use unityrs_core::typetree::{Kind, NodeId, Shape, Tree};
use unityrs_core::value::Value;

/// Checked typetree facts for the supported `GenericBinding` layouts.
/// All value flags are one byte. Alignment is applied after the record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BindingLayout {
    pub class_field: &'static str,
    pub class_bytes: u8,
    pub has_int_flag: bool,
    pub has_managed_reference_flag: bool,
    pub aligns_record: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ValueFlags {
    pub is_pptr: bool,
    pub is_int: bool,
    pub is_managed_reference: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GenericBinding {
    pub path: u32,
    pub attribute: u32,
    pub script: PPtr,
    pub class_id: i32,
    pub custom_type: u8,
    pub flags: ValueFlags,
}

#[derive(Debug)]
pub struct BindingTable {
    /// Owns the scope for the script and mapping pointers.
    pub clip: ObjectRef,
    pub layout: BindingLayout,
    bindings: MeteredVec<GenericBinding>,
    mapping: MeteredVec<PPtr>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransformProperty {
    Translation,
    Quaternion,
    Scale,
    Euler,
}

/// Value families and their use in the binding stage. Integer, object,
/// material and managed-reference bindings retain their slots as metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BindingKind {
    Transform(TransformProperty),
    Morph,
    Material,
    Integer,
    PPtr,
    ManagedReference,
    UnsupportedScalar,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlotBinding {
    pub binding: GenericBinding,
    pub kind: BindingKind,
    pub first: usize,
    pub width: u8,
}

/// Scalar counts in streamed, dense, then constant order. The clip reader
/// supplies counts from its storage layouts before interpreting sample values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SlotCounts {
    pub streamed: usize,
    pub dense: usize,
    pub constant: usize,
}

impl BindingTable {
    #[must_use]
    pub fn bindings(&self) -> &[GenericBinding] {
        self.bindings.as_slice()
    }

    #[must_use]
    pub fn pptr_mapping(&self) -> &[PPtr] {
        self.mapping.as_slice()
    }

    /// Assign contiguous scalar ranges only for a fully known table whose
    /// total agrees with the supplied storage counts.
    pub fn slots(
        &self,
        counts: SlotCounts,
        meter: &Arc<Meter>,
    ) -> Result<MeteredVec<SlotBinding>, AnimError> {
        let storage = counts
            .streamed
            .checked_add(counts.dense)
            .and_then(|sum| sum.checked_add(counts.constant))
            .ok_or(AnimError::SlotOverflow)?;
        let mut slots = MeteredVec::with_capacity(meter, self.bindings.len())?;
        let mut first = 0usize;
        meter.count_decode(self.bindings.len() as u64)?;
        for (index, &binding) in self.bindings().iter().enumerate() {
            let (kind, width) = binding.classify().ok_or(AnimError::UnknownBindingWidth {
                binding: index,
                class_id: binding.class_id,
                custom_type: binding.custom_type,
                attribute: binding.attribute,
            })?;
            slots.push(SlotBinding {
                binding,
                kind,
                first,
                width,
            })?;
            first = first
                .checked_add(usize::from(width))
                .ok_or(AnimError::SlotOverflow)?;
        }
        if first != storage {
            return Err(AnimError::SlotCount {
                bindings: first,
                storage,
            });
        }
        Ok(slots)
    }
}

impl GenericBinding {
    fn classify(self) -> Option<(BindingKind, u8)> {
        use BindingKind as K;
        use TransformProperty as T;
        match (self.class_id, self.custom_type) {
            (4, 0 | 4) => match self.attribute {
                1 => Some((K::Transform(T::Translation), 3)),
                2 => Some((K::Transform(T::Quaternion), 4)),
                3 => Some((K::Transform(T::Scale), 3)),
                4 => Some((K::Transform(T::Euler), 3)),
                _ => None,
            },
            _ if self.flags.is_managed_reference => Some((K::ManagedReference, 1)),
            _ if self.flags.is_pptr => Some((K::PPtr, 1)),
            _ if self.flags.is_int => Some((K::Integer, 1)),
            (137, 20) => Some((K::Morph, 1)),
            (23 | 137, 21 | 22) => Some((K::Material, 1)),
            (1, 0) | (95, 8) | (114, 0 | 24) => Some((K::UnsupportedScalar, 1)),
            _ => None,
        }
    }
}

/// Read the clip-owned binding table through the embedded or provider tree.
/// Other clip fields are left to the clip reader. A legacy value array has
/// no confirmed adapter here and is refused as an unsupported layout.
pub fn read_bindings(clip: &ObjectRef, meter: &Arc<Meter>) -> Result<BindingTable, AnimError> {
    if clip.class_id() != 74 {
        return Err(AnimError::WrongClass(clip.class_id()));
    }
    let tree = clip
        .tree(None)?
        .ok_or_else(|| unityrs_core::walk::ParseError::NoTypeTree {
            class_id: clip.class_id(),
            why: clip.why_no_tree(),
        })?;
    let layout = layout(clip, &tree.tree, meter)?;
    let parsed = clip.read_typetree(Some(&tree.tree), false)?;
    let dict = Dict {
        parsed: &parsed,
        meter,
    };
    let table = dict
        .root("m_ClipBindingConstant")?
        .ok_or(AnimError::UnsupportedBindingLayout)?;
    let records = dict.list(dict.required(table, "genericBindings")?, "genericBindings")?;
    let mut bindings = MeteredVec::with_capacity(meter, records.len())?;
    for (index, record) in records.iter().enumerate() {
        let binding = read_record(&dict, record, layout)?;
        let flags = binding.flags;
        if flags.is_pptr && flags.is_int
            || (matches!(
                (binding.class_id, binding.custom_type),
                (4, 0 | 4) | (137, 20)
            ) && (flags.is_pptr || flags.is_int || flags.is_managed_reference))
        {
            return Err(AnimError::ContradictoryFlags(index));
        }
        bindings.push(binding)?;
    }
    let pointers = dict.list(
        dict.required(table, "pptrCurveMapping")?,
        "pptrCurveMapping",
    )?;
    let mut mapping = MeteredVec::with_capacity(meter, pointers.len())?;
    for pointer in pointers {
        mapping.push(dict.pointer(pointer)?)?;
    }
    Ok(BindingTable {
        clip: clip.clone(),
        layout,
        bindings,
        mapping,
    })
}

/// Format evidence: AssetStudio v0.16.47 AnimationClip.cs (class width
/// and discrete flag), and the extended AssetStudio `GenericBinding` reader
/// (2022.2 managed-reference byte). Verify the tree even for an empty table.
fn layout(clip: &ObjectRef, tree: &Tree, meter: &Arc<Meter>) -> Result<BindingLayout, AnimError> {
    let unsupported = AnimError::UnsupportedBindingLayout;
    let version = version(clip).filter(|v| *v >= (4, 3)).ok_or(unsupported)?;
    let table = child(tree, tree.root(), b"m_ClipBindingConstant", meter)?;
    let elem = array_element(tree, child(tree, table, b"genericBindings", meter)?)?;
    let pointer_element = array_element(tree, child(tree, table, b"pptrCurveMapping", meter)?)?;
    let path_kind = if clip.file().header().version >= 14 {
        Kind::S64
    } else {
        Kind::S32
    };
    check_pointer(tree, pointer_element, path_kind)?;
    let list = tree.list();
    let fields = list.children(elem);
    meter.count_decode(fields.len() as u64)?;
    let wide = version >= (5, 6);
    let has_int_flag = version >= (2022, 1);
    let has_managed_reference_flag = version >= (2022, 2);
    let expected = 6 + usize::from(has_int_flag) + usize::from(has_managed_reference_flag);
    if fields.len() != expected || list.type_name(elem) != b"GenericBinding" {
        return Err(AnimError::UnsupportedBindingLayout);
    }
    let mut fields = fields.iter().copied();
    let class_field = match fields.nth(3).map(|id| list.name(id)) {
        Some(b"classID") => "classID",
        Some(b"typeID") => "typeID",
        _ => return Err(AnimError::UnsupportedBindingLayout),
    };
    let class_kind = if wide { Kind::S32 } else { Kind::U16 };
    let facts = [
        ("path", Kind::U32),
        ("attribute", Kind::U32),
        ("script", Kind::PPtr),
        (class_field, class_kind),
        ("customType", Kind::U8),
        ("isPPtrCurve", Kind::U8),
        ("isIntCurve", Kind::U8),
        ("isSerializeReferenceCurve", Kind::U8),
    ];
    let align = |id| tree.info(id).is_some_and(|info| info.align);
    let mut aligns_record = align(elem);
    for (index, (&id, (name, kind))) in list.children(elem).iter().zip(facts).enumerate() {
        let aligned = align(id);
        if list.name(id) != name.as_bytes()
            || !tree.info(id).is_some_and(|info| info.kind == kind)
            || (aligned && index + 1 != expected)
        {
            return Err(AnimError::UnsupportedBindingLayout);
        }
        if kind == Kind::PPtr {
            check_pointer(tree, id, path_kind)?;
        }
        aligns_record |= aligned;
    }
    if wide != aligns_record {
        return Err(AnimError::UnsupportedBindingLayout);
    }
    Ok(BindingLayout {
        class_field,
        class_bytes: if wide { 4 } else { 2 },
        has_int_flag,
        has_managed_reference_flag,
        aligns_record,
    })
}

fn array_element(tree: &Tree, array: NodeId) -> Result<NodeId, AnimError> {
    match tree.info(array).map(|info| info.shape) {
        Some(Shape::Vector { elem, .. }) => Ok(elem),
        _ => Err(AnimError::UnsupportedBindingLayout),
    }
}

fn check_pointer(tree: &Tree, pointer: NodeId, path_kind: Kind) -> Result<(), AnimError> {
    let list = tree.list();
    let [file_id, path_id] = list.children(pointer) else {
        return Err(AnimError::UnsupportedBindingLayout);
    };
    let kind = |id| tree.info(id).map(|info| info.kind);
    if kind(pointer) != Some(Kind::PPtr)
        || list.name(*file_id) != b"m_FileID"
        || kind(*file_id) != Some(Kind::S32)
        || list.name(*path_id) != b"m_PathID"
        || kind(*path_id) != Some(path_kind)
    {
        return Err(AnimError::UnsupportedBindingLayout);
    }
    Ok(())
}

fn child(
    tree: &Tree,
    parent: NodeId,
    name: &[u8],
    meter: &Arc<Meter>,
) -> Result<NodeId, AnimError> {
    let list = tree.list();
    let children = list.children(parent);
    meter.count_decode(children.len() as u64)?;
    children
        .iter()
        .copied()
        .find(|&id| list.name(id) == name)
        .ok_or(AnimError::UnsupportedBindingLayout)
}

fn read_record(
    dict: &Dict<'_, '_>,
    row: &Value,
    layout: BindingLayout,
) -> Result<GenericBinding, AnimError> {
    let uint = |name| {
        dict.required(row, name)?
            .as_u64()
            .ok_or(AnimError::Malformed(name))
    };
    let u32_field = |name| u32::try_from(uint(name)?).map_err(|_| AnimError::Malformed(name));
    let flag = |name| match uint(name)? {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(AnimError::Malformed(name)),
    };
    Ok(GenericBinding {
        path: u32_field("path")?,
        attribute: u32_field("attribute")?,
        script: dict.pointer(dict.required(row, "script")?)?,
        class_id: dict
            .required(row, layout.class_field)?
            .as_i64()
            .and_then(|id| i32::try_from(id).ok())
            .ok_or(AnimError::Malformed(layout.class_field))?,
        custom_type: u8::try_from(uint("customType")?)
            .map_err(|_| AnimError::Malformed("customType"))?,
        flags: ValueFlags {
            is_pptr: flag("isPPtrCurve")?,
            is_int: layout.has_int_flag && flag("isIntCurve")?,
            is_managed_reference: layout.has_managed_reference_flag
                && flag("isSerializeReferenceCurve")?,
        },
    })
}

const _: () = crate::send_sync::<BindingLayout>();
const _: () = crate::send_sync::<ValueFlags>();
const _: () = crate::send_sync::<GenericBinding>();
const _: () = crate::send_sync::<BindingTable>();
const _: () = crate::send_sync::<TransformProperty>();
const _: () = crate::send_sync::<BindingKind>();
const _: () = crate::send_sync::<SlotBinding>();
const _: () = crate::send_sync::<SlotCounts>();
