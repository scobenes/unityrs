//! Charged hash set of transform identities. `RandomState` prevents an input
//! from choosing collisions. Both tables are charged while rehashing.
use crate::SceneError;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hash};
use std::sync::Arc;
use unityrs_core::ObjectRef;
use unityrs_core::limits::{Meter, MeteredVec};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Key(usize, i64);
pub(crate) struct Visited {
    slots: MeteredVec<Option<Key>>,
    len: usize,
    hash: RandomState,
}
impl Visited {
    pub fn new(meter: &Arc<Meter>) -> Self {
        Self {
            slots: MeteredVec::new(meter),
            len: 0,
            hash: RandomState::new(),
        }
    }
    fn slot(&self, key: Key) -> usize {
        usize::try_from(self.hash.hash_one(key) % self.slots.len() as u64).unwrap_or(0)
    }
    fn put(&mut self, key: Key) -> Result<bool, SceneError> {
        let meter = self.slots.charge().meter().clone();
        let mut at = self.slot(key);
        loop {
            meter.count_decode(1)?;
            let slot = self
                .slots
                .as_mut_slice()
                .get_mut(at)
                .ok_or(SceneError::Malformed("visited table"))?;
            if *slot == Some(key) {
                return Ok(false);
            }
            if slot.is_none() {
                *slot = Some(key);
                self.len += 1;
                return Ok(true);
            }
            at = (at + 1) % self.slots.len();
        }
    }
    pub fn insert(&mut self, obj: &ObjectRef) -> Result<bool, SceneError> {
        let key = Key(Arc::as_ptr(obj.file()) as usize, obj.path_id());
        if self.len >= self.slots.len() / 2 {
            let n = self.slots.len().saturating_mul(2).max(16);
            let meter = self.slots.charge().meter();
            let mut slots = MeteredVec::with_capacity(meter, n)?;
            meter.count_decode(n as u64)?;
            for _ in 0..n {
                slots.push(None)?;
            }
            let old = std::mem::replace(&mut self.slots, slots);
            self.len = 0;
            old.charge().meter().count_decode(old.len() as u64)?;
            for key in old.as_slice().iter().flatten() {
                self.put(*key)?;
            }
        }
        self.put(key)
    }
}
