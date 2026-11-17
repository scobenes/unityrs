//! Ordered clip discovery through the caller's file view.
use crate::AnimError;
use crate::controller::{ControllerTable, read_leaves};
use crate::read::{Dict, identity_key, same_object, version};
use std::collections::HashMap;
use std::sync::Arc;
use unityrs_core::ObjectRef;
use unityrs_core::budget::BudgetExceeded;
use unityrs_core::env::{DerefError, Environment, FileOccurrence, PPtr};
use unityrs_core::error::Error;
use unityrs_core::files::File;
use unityrs_core::limits::{Charge, ChargeError, Meter, MeteredVec, heap_cost};
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
    result: Result<Arc<RetainedClip>, SlotError>,
    first: bool,
}

impl ClipSlot {
    pub fn clip(&self) -> Result<&ObjectRef, &SlotError> {
        self.result.as_ref().map(|clip| &clip.object)
    }

    /// Whether this is the first successful occurrence of its effective clip.
    #[must_use]
    pub fn is_first(&self) -> bool {
        self.first
    }
}

#[derive(Debug)]
struct RetainedClip {
    object: ObjectRef,
    _charge: Charge,
}

type ClipKey = (FileOccurrence, i64);

struct ClipEntry {
    clip: Arc<RetainedClip>,
    // Keep the dependency's open alive across cache evictions while discovering.
    container: Option<File>,
}

struct ClipPool {
    entries: HashMap<ClipKey, ClipEntry>,
    table_charge: Charge,
    table_capacity: usize,
    key_work: u64,
}

impl ClipPool {
    fn new(meter: &Arc<Meter>) -> Self {
        Self {
            entries: HashMap::new(),
            table_charge: Charge::zero(meter),
            table_capacity: 0,
            key_work: 0,
        }
    }

    fn resize_table(&mut self, capacity: usize) -> Result<(), AnimError> {
        const BUCKET: u64 = (size_of::<(ClipKey, ClipEntry)>() + 1) as u64;
        // The standard table uses power-of-two bucket counts, at most 7/8
        // full above the small-table sizes. Charge both tables during a rebuild.
        let buckets = ((capacity as u64).saturating_mul(8) / 7)
            .max(4)
            .checked_next_power_of_two()
            .unwrap_or(u64::MAX);
        let bytes = heap_cost(BUCKET.saturating_mul(buckets).saturating_add(16));
        let meter = self.table_charge.meter();
        let charge = meter.charge(bytes)?;
        meter.count_decode((self.table_capacity as u64).saturating_add(self.key_work))?;
        // A parsed file's occurrence is published before the key is created.
        #[allow(clippy::mutable_key_type)]
        let mut entries = HashMap::new();
        entries.try_reserve(capacity).map_err(|_| {
            ChargeError::Budget(BudgetExceeded {
                requested: bytes,
                what: meter.reservation(),
            })
        })?;
        self.table_capacity = entries.capacity();
        entries.extend(self.entries.drain());
        self.entries = entries;
        self.table_charge = charge;
        Ok(())
    }

    fn get(&self, key: &ClipKey) -> Result<Option<Arc<RetainedClip>>, ChargeError> {
        if self.entries.is_empty() {
            return Ok(None);
        }
        self.table_charge
            .meter()
            .count_decode(1 + key.0.path_len() as u64)?;
        Ok(self.entries.get(key).map(|entry| entry.clip.clone()))
    }

    fn retain(&mut self, resolved: ResolvedObject) -> Result<Arc<RetainedClip>, AnimError> {
        const ARC: u64 = heap_cost((size_of::<RetainedClip>() + 2 * size_of::<usize>()) as u64);

        let ResolvedObject { object, container } = resolved;
        let meter = self.table_charge.meter().clone();
        let key = (FileOccurrence::of(object.file()), object.path_id());
        let work = 1 + key.0.path_len() as u64;
        meter.count_decode(work)?;
        if let Some(entry) = self.entries.get_mut(&key) {
            if entry.container.is_none() && Arc::ptr_eq(entry.clip.object.file(), object.file()) {
                entry.container = container;
            }
            return Ok(entry.clip.clone());
        }
        let clip_charge = meter.charge(ARC)?;
        let len = self.entries.len();
        if len == self.entries.capacity() {
            self.resize_table(len.saturating_mul(2).max(2))?;
        }

        let clip = Arc::new(RetainedClip {
            object,
            _charge: clip_charge,
        });
        self.entries.insert(
            key,
            ClipEntry {
                clip: clip.clone(),
                container,
            },
        );
        self.key_work += work;
        Ok(clip)
    }

    fn release_slot(&mut self, clip: Arc<RetainedClip>) {
        // One handle belongs to the pool and one came from the replaced slot.
        if Arc::strong_count(&clip) == 2 {
            let key = (
                FileOccurrence::of(clip.object.file()),
                clip.object.path_id(),
            );
            self.entries.remove(&key);
            self.key_work -= 1 + key.0.path_len() as u64;
            drop((key, clip));
            let len = self.entries.len();
            if len == 0 {
                self.entries = HashMap::new();
                self.table_capacity = 0;
                self.table_charge.release(self.table_charge.bytes());
            } else if len <= self.table_capacity / 4 {
                // If compaction cannot reserve a second table, keep the
                // existing allocation charged and try again on a later removal.
                let _ = self.resize_table(len.saturating_mul(2));
            }
        }
    }

    fn release_containers(&mut self) -> Result<bool, AnimError> {
        self.table_charge
            .meter()
            .count_decode(self.table_capacity as u64)?;
        let mut released = false;
        for entry in self.entries.values_mut() {
            released |= entry.container.take().is_some();
        }
        Ok(released)
    }
}

#[derive(Debug)]
struct ResolvedObject {
    object: ObjectRef,
    container: Option<File>,
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
    let mut clips = ClipPool::new(meter);
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
                        push_slots(
                            &mut rig.slots,
                            &mut clips,
                            env,
                            object,
                            &dict,
                            "m_Animation",
                            false,
                        )?;
                        push_slots(
                            &mut rig.slots,
                            &mut clips,
                            env,
                            object,
                            &dict,
                            "m_Animations",
                            true,
                        )
                    } else {
                        controller(env, &mut rig, &dict, &mut clips)
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

fn checked_pointer(from: &SourceSlot, meter: &Meter) -> Result<PPtr, AnimError> {
    let ptr = from.pointer.ok_or(AnimError::Malformed(from.field))?;
    meter.count_decode(1)?;
    // Unity uses zero for the owning file and positive one-based indices for
    // externals. Core also supports UnityPy's negative indexing.
    // https://github.com/Unity-Technologies/UnityDataTools/blob/main/Documentation/assetbundle-format.md#bundle-dependencies-and-object-references
    if ptr.file_id < 0 {
        return Err(AnimError::NegativeFileId(ptr.file_id));
    }
    Ok(ptr)
}

fn with_clip_room<T>(
    clips: &mut ClipPool,
    mut lookup: impl FnMut(&ClipPool) -> Result<T, DerefError>,
) -> Result<T, AnimError> {
    match lookup(clips) {
        Err(DerefError::Load(ref error))
            if error.is_dependency_budget_refusal() && clips.release_containers()? =>
        {
            // The environment has already tried evicting its cache. Drop
            // discovery's extra container holds to make room for this load.
            Ok(lookup(clips)?)
        }
        result => Ok(result?),
    }
}

fn resolve(
    env: &Environment,
    owner: &ObjectRef,
    ptr: PPtr,
    accepts: impl FnOnce(i32) -> bool,
    meter: &Meter,
    clips: &mut ClipPool,
) -> Result<ResolvedObject, AnimError> {
    let (object, container) = with_clip_room(clips, |_| env.deref_held(owner.file(), ptr, meter))?;
    if !accepts(object.class_id()) {
        return Err(AnimError::WrongClass(object.class_id()));
    }
    Ok(ResolvedObject { object, container })
}

enum ResolvedClip {
    Retained(Arc<RetainedClip>),
    Loaded(ResolvedObject),
}

impl ResolvedClip {
    fn object(&self) -> &ObjectRef {
        match self {
            Self::Retained(clip) => &clip.object,
            Self::Loaded(resolved) => &resolved.object,
        }
    }
}

fn clip_at(
    env: &Environment,
    from: &SourceSlot,
    meter: &Meter,
    clips: &mut ClipPool,
) -> Result<ResolvedClip, AnimError> {
    let ptr = checked_pointer(from, meter)?;
    let mut retained = None;
    let (object, container) = with_clip_room(clips, |pool| {
        env.deref_held_reusing(from.owner.file(), ptr, meter, |file| {
            retained = pool.get(&(file, ptr.path_id)).map_err(Error::from)?;
            Ok(retained.as_ref().map(|clip| clip.object.clone()))
        })
    })?;
    if let Some(clip) = retained {
        return Ok(ResolvedClip::Retained(clip));
    }
    if object.class_id() != 74 {
        return Err(AnimError::WrongClass(object.class_id()));
    }
    Ok(ResolvedClip::Loaded(ResolvedObject { object, container }))
}

fn push_slots(
    slots: &mut MeteredVec<ClipSlot>,
    clips: &mut ClipPool,
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
        let resolved = ptr.and_then(|_| clip_at(env, &from, dict.meter, clips));
        let result = stored_result(&from, clips, resolved)?;
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
    clips: &mut ClipPool,
    result: Result<ResolvedClip, AnimError>,
) -> Result<Result<Arc<RetainedClip>, SlotError>, AnimError> {
    match result {
        Err(cause) if cause.resource_refusal() => Err(cause),
        Err(cause) => Ok(Err(SlotError {
            source: source(&from.owner, from.field, from.ordinal, from.pointer),
            cause,
        })),
        Ok(ResolvedClip::Retained(clip)) => Ok(Ok(clip)),
        Ok(ResolvedClip::Loaded(resolved)) => Ok(Ok(clips.retain(resolved)?)),
    }
}

fn controller(
    env: &Environment,
    rig: &mut RigClips,
    animator: &Dict<'_, '_>,
    clips: &mut ClipPool,
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
        let resolved = match checked_pointer(&edge, meter).and_then(|ptr| {
            resolve(
                env,
                &edge.owner,
                ptr,
                |id| matches!(id, 91 | 93 | 221),
                meter,
                clips,
            )
        }) {
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
            if let Err(cause) =
                push_slots(&mut rig.slots, clips, env, object, &dict, clips_field, true)
            {
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
        if let Err(cause) = apply_overrides(env, rig, object, &dict, clips) {
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
    clips: &mut ClipPool,
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
        let clip = match clip_at(env, &original, meter, clips) {
            Ok(clip) => clip,
            Err(cause) => {
                note(&mut rig.issues, original, cause)?;
                continue;
            }
        };
        let key = identity_key(clip.object());
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
        let (replacement, result) = match pointer_source(dict, owner, row, ordinal, field) {
            Err(cause) => (source(owner, field, ordinal, None), Err(cause)),
            Ok(replacement) if ambiguous => (replacement, Err(AnimError::AmbiguousOverride)),
            Ok(replacement) if replacement.pointer.is_some_and(|ptr| ptr.is_set()) => {
                let result = clip_at(env, &replacement, meter, clips);
                (replacement, result)
            }
            Ok(_) => continue,
        };
        let result = stored_result(&replacement, clips, result)?;
        slot.replacement = result.is_ok().then_some(replacement);
        if let Ok(previous) = std::mem::replace(&mut slot.result, result) {
            clips.release_slot(previous);
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
            let key = identity_key(&clip.object);
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

#[cfg(test)]
mod tests {
    use super::*;
    use unityrs_core::env::{EnvOptions, Source};
    use unityrs_core::{Bytes, RawStr};
    use unityrs_testkit::scene3d::{Layout, anim, file};

    #[test]
    fn distinct_overrides_charge_only_the_live_pool_entry() {
        let mut objects: Vec<_> = (1..=32).map(anim::clip).collect();
        objects.push(anim::controller(100, &[(0, 1), (0, 1)], &[0, 1]));
        objects.extend(
            (1..32).map(|id| anim::overrides(100 + id, (0, 99 + id), &[((0, id), (0, id + 1))])),
        );
        let env = Environment::new(EnvOptions::unitypy());
        let File::Serialized(file) = env
            .load(Source::Bytes {
                name: RawStr::from_static(b"clips"),
                data: Bytes::from(file(Layout::Unity2022, &[], &objects).unwrap()),
            })
            .unwrap()
        else {
            panic!("serialized clips");
        };
        let meter = Meter::unlimited();
        let mut pool = ClipPool::new(&meter);
        let controller = file.object(100).unwrap();
        let parsed = controller.parse_as_dict(false).unwrap();
        let mut rig = RigClips {
            node: 0,
            root: controller.clone(),
            component: controller.clone(),
            slots: MeteredVec::new(&meter),
            issues: MeteredVec::new(&meter),
            controller: None,
        };
        push_slots(
            &mut rig.slots,
            &mut pool,
            &env,
            &controller,
            &Dict {
                parsed: &parsed,
                meter: &meter,
            },
            "m_AnimationClips",
            true,
        )
        .unwrap();
        let one_entry = meter.used();
        for id in 1..32 {
            let owner = file.object(100 + id).unwrap();
            let parsed = owner.parse_as_dict(false).unwrap();
            let dict = Dict {
                parsed: &parsed,
                meter: &meter,
            };
            apply_overrides(&env, &mut rig, &owner, &dict, &mut pool).unwrap();
            for slot in rig.slots() {
                assert_eq!(slot.clip().unwrap().path_id(), id + 1);
            }
            assert_eq!(pool.entries.len(), 1);
            assert_eq!(meter.used(), one_entry);
        }
        drop((pool, rig));
        assert_eq!(meter.used(), 0);
    }
}
