//! One-call split deduplication, with charged hash table room and hash work.
use crate::limits::{Charge, Meter, hash_table_cost};
use crate::{Error, RawStr};
use std::{collections::HashSet, mem::size_of, sync::Arc};
pub(super) struct Seen {
    names: HashSet<RawStr>,
    charge: Charge,
    hash_work: u64,
}
impl Seen {
    pub(super) fn new(meter: &Arc<Meter>) -> Self {
        Self {
            names: HashSet::new(),
            charge: Charge::zero(meter),
            hash_work: 0,
        }
    }
    pub(super) fn contains_or_insert(&mut self, name: &[u8]) -> Result<bool, Error> {
        let meter = self.charge.meter().clone();
        meter.count_decode((name.len() as u64 + 1).saturating_mul(2))?;
        if self.names.contains(name) {
            return Ok(true);
        }
        let cost = hash_table_cost(self.names.len() + 1, size_of::<RawStr>());
        self.charge.grow(cost.saturating_sub(self.charge.bytes()))?;
        // Growing std's table hashes all the old keys again.
        if self.names.len() == self.names.capacity() {
            meter.count_decode(self.hash_work)?;
        }
        self.names.insert(RawStr::charged(name, &meter)?);
        self.hash_work = self.hash_work.saturating_add(name.len() as u64 + 1);
        Ok(false)
    }
}
