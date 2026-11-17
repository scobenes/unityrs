//! Ordered clip discovery through the caller's file view.
use crate::AnimError;
use crate::controller::{ControllerTable, read_leaves};
use crate::read::{Dict, identity_key, same_object, version};
use std::sync::Arc;
use unityrs_core::ObjectRef;
use unityrs_core::env::{Environment, PPtr};
use unityrs_core::files::File;
use unityrs_core::limits::{Meter, MeteredVec};
use unityrs_core::value::{Parsed, Value};
use unityrs_scene::{Component, Graph, NodeId};

/// Serialized occurrence. Only the file/object identity, field and ordinal
/// participate in selection. The pointer remains available for diagnostics.
#[derive(Debug)]
pub struct SourceSlot {
    pub owner: ObjectRef,
    pub field: &'static str,
    pub ordinal: usize,
    pub pointer: Option<PPtr>,
}

impl SourceSlot {
    fn matches(&self, other: &Self) -> bool {
        self.field == other.field
            && self.ordinal == other.ordinal
            && same_object(&self.owner, &other.owner)
    }
}

/// A failed reference and the exact occurrence that failed.
#[derive(Debug)]
pub struct SlotError {
    pub source: SourceSlot,
    pub cause: AnimError,
}

/// A base slot after all applicable overrides. Duplicates remain addressable.
#[derive(Debug)]
pub struct ClipSlot {
    pub source: SourceSlot,
    /// The last replacement that supplied the effective clip, if any.
    pub replacement: Option<SourceSlot>,
    result: Result<ObjectRef, SlotError>,
    first: bool,
}

impl ClipSlot {
    pub fn clip(&self) -> Result<&ObjectRef, &SlotError> {
        self.result.as_ref()
    }

    /// Whether this is the first successful occurrence of its effective clip.
    #[must_use]
    pub fn is_first(&self) -> bool {
        self.first
    }
}

#[derive(Debug)]
struct ResolvedObject {
    object: ObjectRef,
    // Keep a controller's sibling scope alive while following its pointers.
    _container: Option<File>,
}

/// One Animation/Animator component and its original ordering.
#[derive(Debug)]
pub struct RigClips {
    pub node: NodeId,
    pub root: ObjectRef,
    pub component: ObjectRef,
    slots: MeteredVec<ClipSlot>,
    issues: MeteredVec<SlotError>,
    controller: Option<ControllerTable>,
}

impl RigClips {
    #[must_use]
    pub fn slots(&self) -> &[ClipSlot] {
        self.slots.as_slice()
    }

    /// The effective, deduplicated clips in discovery order.
    pub fn clips(&self) -> impl Iterator<Item = &ClipSlot> {
        self.slots().iter().filter(|slot| slot.is_first())
    }

    #[must_use]
    pub fn issues(&self) -> &[SlotError] {
        self.issues.as_slice()
    }

    #[must_use]
    pub fn controller(&self) -> Option<&ControllerTable> {
        self.controller.as_ref()
    }
}

#[derive(Debug)]
pub struct Discovery(MeteredVec<RigClips>);

/// Direct clip selection in an already identified rig.
#[derive(Clone, Copy, Debug)]
pub enum ClipSelector<'a> {
    Clip(&'a ObjectRef),
    Source(&'a SourceSlot),
}

#[derive(Debug)]
pub enum SelectionError<'a> {
    Missing,
    Ambiguous,
    Unresolved(&'a SlotError),
    Resource(AnimError),
}

impl Discovery {
    #[must_use]
    pub fn rigs(&self) -> &[RigClips] {
        self.0.as_slice()
    }

    /// An unresolved source selector returns its original typed failure.
    pub fn select(
        &self,
        root: &ObjectRef,
        component: &ObjectRef,
        selector: ClipSelector<'_>,
        meter: &Arc<Meter>,
    ) -> Result<&ClipSlot, SelectionError<'_>> {
        let mut found = None;
        for rig in self.rigs() {
            meter
                .count_decode(1)
                .map_err(|error| SelectionError::Resource(error.into()))?;
            if !same_object(root, &rig.root) || !same_object(component, &rig.component) {
                continue;
            }
            for slot in rig.slots() {
                meter
                    .count_decode(1)
                    .map_err(|error| SelectionError::Resource(error.into()))?;
                let matches = match selector {
                    ClipSelector::Clip(clip) => {
                        if slot.first
                            && let Ok(candidate) = slot.clip()
                        {
                            let key = identity_key(clip);
                            let other = identity_key(candidate);
                            meter
                                .count_decode(key.2.len().min(other.2.len()) as u64)
                                .map_err(|error| SelectionError::Resource(error.into()))?;
                            key == other
                        } else {
                            false
                        }
                    }
                    ClipSelector::Source(source) => slot.source.matches(source),
                };
                if matches && found.replace(slot).is_some() {
                    return Err(SelectionError::Ambiguous);
                }
            }
        }
        let slot = found.ok_or(SelectionError::Missing)?;
        slot.clip().map_err(SelectionError::Unresolved)?;
        Ok(slot)
    }
}

/// Visit the graph and each component in their stored order.
pub fn discover(
    env: &Environment,
    graph: &Graph,
    meter: &Arc<Meter>,
) -> Result<Discovery, AnimError> {
    let mut rigs = MeteredVec::<RigClips>::new(meter);
    for (id, node) in graph.nodes().iter().enumerate() {
        meter.count_decode(1)?;
        let start = rigs.len();
        for component in node.components.as_slice() {
            meter.count_decode(1)?;
            let Component::Other(95 | 111, object) = component else {
                continue;
            };
            let previous = rigs
                .as_slice()
                .get(start..)
                .ok_or(AnimError::Malformed("rig index"))?;
            meter.count_decode(previous.len() as u64)?;
            if previous
                .iter()
                .any(|rig| same_object(&rig.component, object))
            {
                continue;
            }
            let mut rig = RigClips {
                node: id,
                root: node.transform.clone(),
                component: object.clone(),
                slots: MeteredVec::new(meter),
                issues: MeteredVec::new(meter),
                controller: None,
            };
            let result = object
                .parse_as_dict(false)
                .map_err(AnimError::from)
                .and_then(|parsed| {
                    let dict = Dict {
                        parsed: &parsed,
                        meter,
                    };
                    if object.class_id() == 111 {
                        push_slots(&mut rig.slots, env, object, &dict, "m_Animation", false)?;
                        push_slots(&mut rig.slots, env, object, &dict, "m_Animations", true)
                    } else {
                        controller(env, &mut rig, &dict)
                    }
                });
            if let Err(cause) = result {
                note(&mut rig.issues, source(object, "Base", 0, None), cause)?;
            }
            mark_first(&mut rig.slots)?;
            rigs.push(rig)?;
        }
    }
    Ok(Discovery(rigs))
}

fn source(
    owner: &ObjectRef,
    field: &'static str,
    ordinal: usize,
    pointer: Option<PPtr>,
) -> SourceSlot {
    SourceSlot {
        owner: owner.clone(),
        field,
        ordinal,
        pointer,
    }
}

fn note(
    issues: &mut MeteredVec<SlotError>,
    source: SourceSlot,
    cause: AnimError,
) -> Result<(), AnimError> {
    if cause.resource_refusal() {
        return Err(cause);
    }
    Ok(issues.push(SlotError { source, cause })?)
}

fn resolve(
    env: &Environment,
    from: &SourceSlot,
    accepts: impl FnOnce(i32) -> bool,
    meter: &Arc<Meter>,
) -> Result<ResolvedObject, AnimError> {
    let ptr = from.pointer.ok_or(AnimError::Malformed(from.field))?;
    meter.count_decode(1)?;
    // Unity uses zero for the owning file and positive one-based indices for
    // externals. Core also supports UnityPy's negative indexing.
    // https://github.com/Unity-Technologies/UnityDataTools/blob/main/Documentation/assetbundle-format.md#bundle-dependencies-and-object-references
    if ptr.file_id < 0 {
        return Err(AnimError::NegativeFileId(ptr.file_id));
    }
    let (object, container) = env.deref_held(from.owner.file(), ptr, meter)?;
    if !accepts(object.class_id()) {
        return Err(AnimError::WrongClass(object.class_id()));
    }
    Ok(ResolvedObject {
        object,
        _container: container,
    })
}

fn clip_at(
    env: &Environment,
    from: &SourceSlot,
    meter: &Arc<Meter>,
) -> Result<ObjectRef, AnimError> {
    Ok(resolve(env, from, |class| class == 74, meter)?.object)
}

fn push_slots(
    slots: &mut MeteredVec<ClipSlot>,
    env: &Environment,
    owner: &ObjectRef,
    dict: &Dict<'_, '_>,
    field: &'static str,
    array: bool,
) -> Result<(), AnimError> {
    let Some(value) = dict.root(field)? else {
        return if field == "m_AnimationClips" {
            Err(AnimError::Malformed(field))
        } else {
            Ok(())
        };
    };
    let values = if array {
        dict.list(value, field)?
    } else {
        std::slice::from_ref(value)
    };
    slots.reserve_exact(values.len())?;
    for (ordinal, value) in values.iter().enumerate() {
        let ptr = dict.pointer(value);
        let from = source(owner, field, ordinal, ptr.as_ref().ok().copied());
        let result = stored_result(&from, ptr.and_then(|_| clip_at(env, &from, dict.meter)))?;
        slots.push(ClipSlot {
            source: from,
            replacement: None,
            result,
            first: false,
        })?;
    }
    Ok(())
}

fn stored_result(
    from: &SourceSlot,
    result: Result<ObjectRef, AnimError>,
) -> Result<Result<ObjectRef, SlotError>, AnimError> {
    match result {
        Err(cause) if cause.resource_refusal() => Err(cause),
        Err(cause) => Ok(Err(SlotError {
            source: source(&from.owner, from.field, from.ordinal, from.pointer),
            cause,
        })),
        Ok(clip) => Ok(Ok(clip)),
    }
}

fn controller(
    env: &Environment,
    rig: &mut RigClips,
    animator: &Dict<'_, '_>,
) -> Result<(), AnimError> {
    let meter = animator.meter;
    let field = "m_Controller";
    let mut edge = match pointer_source(animator, &rig.component, animator.parsed.value(), 0, field)
    {
        Ok(edge) => edge,
        Err(cause) => {
            return note(
                &mut rig.issues,
                source(&rig.component, field, 0, None),
                cause,
            );
        }
    };
    if !edge.pointer.is_some_and(|ptr| ptr.is_set()) {
        return Ok(());
    }
    let limit = rig.component.file().context().limits.max_tree_depth;
    let mut chain = MeteredVec::<(ResolvedObject, Parsed)>::new(meter);
    loop {
        if chain.len() as u64 >= u64::from(limit) {
            return note(&mut rig.issues, edge, AnimError::ControllerDepth(limit));
        }
        let resolved = match resolve(env, &edge, |id| matches!(id, 91 | 93 | 221), meter) {
            Ok(resolved) => resolved,
            Err(cause) => return note(&mut rig.issues, edge, cause),
        };
        let object = &resolved.object;
        meter.count_decode(chain.len() as u64)?;
        if chain
            .as_slice()
            .iter()
            .any(|(ancestor, _)| same_object(&ancestor.object, object))
        {
            return note(&mut rig.issues, edge, AnimError::ControllerCycle);
        }
        // Unity documents the restriction as early as 2019.4.
        // https://docs.unity3d.com/2019.4/Documentation/ScriptReference/AnimatorOverrideController-ctor.html
        if object.class_id() == 221
            && edge.owner.class_id() == 221
            && version(&edge.owner).is_some_and(|v| v >= (2019, 4))
        {
            note(
                &mut rig.issues,
                source(&edge.owner, edge.field, edge.ordinal, edge.pointer),
                AnimError::NestedOverrideController,
            )?;
        }
        let parsed = match object.parse_as_dict(false) {
            Ok(parsed) => parsed,
            Err(cause) => return note(&mut rig.issues, edge, cause.into()),
        };
        let dict = Dict {
            parsed: &parsed,
            meter,
        };
        if object.class_id() != 221 {
            let clips_field = "m_AnimationClips";
            if let Err(cause) = push_slots(&mut rig.slots, env, object, &dict, clips_field, true) {
                return note(&mut rig.issues, source(object, clips_field, 0, None), cause);
            }
            let leaves = match read_leaves(object, &dict, rig.slots.len()) {
                Ok(leaves) => leaves,
                Err(cause) => {
                    note(&mut rig.issues, edge, cause)?;
                    MeteredVec::new(meter)
                }
            };
            rig.controller = Some(ControllerTable {
                object: resolved.object,
                serialized: parsed,
                leaves,
            });
            break;
        }
        edge = match pointer_source(&dict, object, parsed.value(), 0, field) {
            Ok(edge) => edge,
            Err(cause) => return note(&mut rig.issues, source(object, field, 0, None), cause),
        };
        chain.push((resolved, parsed))?;
    }
    while let Some((resolved, parsed)) = chain.pop() {
        let object = &resolved.object;
        let dict = Dict {
            parsed: &parsed,
            meter,
        };
        if let Err(cause) = apply_overrides(env, rig, object, &dict) {
            note(&mut rig.issues, source(object, "m_Clips", 0, None), cause)?;
        }
    }
    Ok(())
}

fn pointer_source(
    dict: &Dict<'_, '_>,
    owner: &ObjectRef,
    row: &Value,
    ordinal: usize,
    field: &'static str,
) -> Result<SourceSlot, AnimError> {
    let pointer = dict
        .field(row, field)?
        .map(|v| dict.pointer(v))
        .transpose()?;
    Ok(source(owner, field, ordinal, pointer))
}

#[derive(Clone, Copy)]
struct OverrideMatch {
    ordinal: usize,
    ambiguous: bool,
}

fn apply_overrides(
    env: &Environment,
    rig: &mut RigClips,
    owner: &ObjectRef,
    dict: &Dict<'_, '_>,
) -> Result<(), AnimError> {
    let Some(rows) = dict.root("m_Clips")? else {
        return Ok(());
    };
    let rows = dict.list(rows, "m_Clips")?;
    if rows.is_empty() {
        return Ok(());
    }
    let meter = dict.meter;
    let mut indexed = MeteredVec::new(meter);
    let mut key_work = 1;
    meter.count_decode(rig.slots.len() as u64)?;
    for (index, slot) in rig.slots.as_slice().iter().enumerate() {
        if let Ok(clip) = slot.clip() {
            let key = identity_key(clip);
            key_work = key_work.max(1 + key.2.len() as u64);
            indexed.push((key, index, None::<OverrideMatch>))?;
        }
    }
    sort_work(meter, indexed.len(), key_work)?;
    let slots = indexed.as_mut_slice();
    slots.sort_unstable_by_key(|&(key, index, _)| (key, index));
    for (ordinal, row) in rows.iter().enumerate() {
        let field = "m_OriginalClip";
        let original = match pointer_source(dict, owner, row, ordinal, field) {
            Ok(original) => original,
            Err(cause) => {
                note(&mut rig.issues, source(owner, field, ordinal, None), cause)?;
                continue;
            }
        };
        let clip = match clip_at(env, &original, meter) {
            Ok(clip) => clip,
            Err(cause) => {
                note(&mut rig.issues, original, cause)?;
                continue;
            }
        };
        let key = identity_key(&clip);
        meter.count_decode(log_search(slots.len()).saturating_mul(key_work))?;
        let start = slots.partition_point(|(candidate, _, _)| *candidate < key);
        if let Some((_, _, matched)) = slots.get_mut(start).filter(|(k, _, _)| *k == key) {
            *matched = Some(OverrideMatch {
                ordinal: matched.map_or(ordinal, |row| row.ordinal),
                ambiguous: matched.is_some(),
            });
        }
    }
    // Copy each match to every duplicate slot before mutating the clips.
    meter.count_decode((slots.len() as u64).saturating_mul(key_work))?;
    let mut previous = None;
    let mut replacements = MeteredVec::new(meter);
    for &(key, index, matched) in slots.iter() {
        if previous.is_none_or(|(last, _)| last != key) {
            previous = Some((key, matched));
        }
        if let Some((_, Some(matched))) = previous {
            replacements.push((index, matched))?;
        }
    }
    drop(indexed);
    sort_work(meter, replacements.len(), 1)?;
    replacements
        .as_mut_slice()
        .sort_unstable_by_key(|&(index, _)| index);
    for &(index, OverrideMatch { ordinal, ambiguous }) in replacements.as_slice() {
        let slot = rig
            .slots
            .as_mut_slice()
            .get_mut(index)
            .ok_or(AnimError::Malformed("clip slot"))?;
        let row = rows.get(ordinal).ok_or(AnimError::Malformed("m_Clips"))?;
        let field = "m_OverrideClip";
        let replacement = match pointer_source(dict, owner, row, ordinal, field) {
            Ok(replacement) => replacement,
            Err(cause) => {
                slot.result = stored_result(&source(owner, field, ordinal, None), Err(cause))?;
                slot.replacement = None;
                continue;
            }
        };
        if ambiguous {
            slot.result = stored_result(&replacement, Err(AnimError::AmbiguousOverride))?;
            slot.replacement = None;
        } else if replacement.pointer.is_some_and(|ptr| ptr.is_set()) {
            slot.result = stored_result(&replacement, clip_at(env, &replacement, dict.meter))?;
            slot.replacement = slot.result.is_ok().then_some(replacement);
        }
    }
    Ok(())
}

fn log_search(len: usize) -> u64 {
    u64::from(len.bit_width()) + 1
}

fn sort_work(meter: &Meter, len: usize, key_work: u64) -> Result<(), AnimError> {
    Ok(meter.count_decode(
        (len as u64)
            .saturating_mul(log_search(len))
            .saturating_mul(key_work),
    )?)
}

fn mark_first(slots: &mut MeteredVec<ClipSlot>) -> Result<(), AnimError> {
    let meter = slots.charge().meter().clone();
    let mut keys = MeteredVec::new(&meter);
    let mut key_work = 1;
    meter.count_decode(slots.len() as u64)?;
    for (ordinal, slot) in slots.as_mut_slice().iter_mut().enumerate() {
        if let Ok(clip) = &slot.result {
            let key = identity_key(clip);
            key_work = key_work.max(1 + key.2.len() as u64);
            keys.push((key, ordinal, &mut slot.first))?;
        }
    }
    sort_work(&meter, keys.len(), key_work)?;
    keys.as_mut_slice()
        .sort_unstable_by_key(|&(key, ordinal, _)| (key, ordinal));
    meter.count_decode((keys.len() as u64).saturating_mul(key_work))?;
    let mut previous = None;
    for (key, _, first) in keys.as_mut_slice() {
        if previous != Some(*key) {
            **first = true;
            previous = Some(*key);
        }
    }
    Ok(())
}

const _: () = crate::send_sync::<SourceSlot>();
const _: () = crate::send_sync::<SlotError>();
const _: () = crate::send_sync::<ClipSlot>();
const _: () = crate::send_sync::<RigClips>();
const _: () = crate::send_sync::<Discovery>();
const _: () = crate::send_sync::<ClipSelector<'static>>();
const _: () = crate::send_sync::<SelectionError<'static>>();
