//! Clip retention and pointer resolution across dependency evictions.

use std::collections::HashMap;
use std::sync::Arc;

use unityrs_core::ObjectRef;
use unityrs_core::budget::{BudgetExceeded, Reservation};
use unityrs_core::env::{DerefError, Environment, FileOccurrence, PPtr};
use unityrs_core::error::Error;
use unityrs_core::files::File;
use unityrs_core::limits::{Charge, ChargeError, Meter, heap_cost};
use unityrs_core::walk::ParseError;

use super::SourceSlot;
use crate::AnimError;

#[derive(Debug)]
pub(super) struct RetainedClip {
    pub(super) object: ObjectRef,
    _charge: Charge,
}

type ClipKey = (FileOccurrence, i64);

struct ClipEntry {
    clip: Arc<RetainedClip>,
    // Keep the dependency's open alive across cache evictions while discovering.
    container: Option<File>,
}

pub(super) struct ClipPool {
    entries: HashMap<ClipKey, ClipEntry>,
    table_charge: Charge,
    table_capacity: usize,
    key_work: u64,
}

impl ClipPool {
    pub(super) fn new(meter: &Arc<Meter>) -> Self {
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

    pub(super) fn retain(
        &mut self,
        resolved: ResolvedObject,
    ) -> Result<Arc<RetainedClip>, AnimError> {
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

    pub(super) fn release_slot(&mut self, clip: Arc<RetainedClip>) {
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
pub(super) struct ResolvedObject {
    pub(super) object: ObjectRef,
    container: Option<File>,
}

pub(super) fn checked_pointer(from: &SourceSlot, meter: &Meter) -> Result<PPtr, AnimError> {
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

fn provider_dependency_refusal(error: &Error) -> bool {
    let deref = match error {
        Error::Entry { source, .. } => return provider_dependency_refusal(source),
        Error::AssetBundle(error) => match error.as_ref() {
            ParseError::Provider(provider) => {
                return provider
                    .memory
                    .as_ref()
                    .is_some_and(|budget| budget.what == Reservation::Dependency);
            }
            ParseError::Deref(deref) => deref,
            _ => return false,
        },
        Error::Deref(deref) => deref,
        _ => return false,
    };
    matches!(deref, DerefError::Load(error) if provider_dependency_refusal(error))
}

fn with_clip_room<T>(
    clips: &mut ClipPool,
    mut lookup: impl FnMut(&ClipPool) -> Result<T, DerefError>,
) -> Result<T, AnimError> {
    match lookup(clips) {
        Err(DerefError::Load(ref error))
            if (error.is_dependency_budget_refusal() || provider_dependency_refusal(error))
                && clips.release_containers()? =>
        {
            // Retry without discovery's extra container holds.
            Ok(lookup(clips)?)
        }
        result => Ok(result?),
    }
}

pub(super) fn resolve(
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

pub(super) enum ResolvedClip {
    Retained(Arc<RetainedClip>),
    Loaded(ResolvedObject),
}

impl ResolvedClip {
    pub(super) fn object(&self) -> &ObjectRef {
        match self {
            Self::Retained(clip) => &clip.object,
            Self::Loaded(resolved) => &resolved.object,
        }
    }
}

pub(super) fn clip_at(
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

#[cfg(test)]
mod tests {
    use super::super::{Dict, MeteredVec, RigClips, apply_overrides, push_slots};
    use super::{ClipPool, Environment, File, Meter};
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
