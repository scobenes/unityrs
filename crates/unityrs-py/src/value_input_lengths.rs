//! Writer protocol lengths are independent of retained storage sizes.
use crate::worker_error::Failure;
use unityrs_core::{Parsed, Value, limits::MeteredVec};

pub(crate) struct Lengths {
    root: Option<usize>,
    // Child Values live inside their parents' fixed native allocations. These
    // addresses identify immutable values; they are never dereferenced. The
    // root can move with Parsed and is therefore handled separately.
    entries: MeteredVec<(usize, usize)>,
}
impl Lengths {
    pub(super) fn resolve(
        parsed: &Parsed,
        logical: &MeteredVec<(u64, usize)>,
    ) -> crate::worker_error::Result<Self> {
        let meter = parsed.charge().meter();
        let mut result = Self {
            root: None,
            entries: MeteredVec::new(meter),
        };
        if logical.is_empty() {
            return Ok(result);
        }
        let mut pending = MeteredVec::new(meter);
        pending.push(parsed.value()).map_err(Failure::charge)?;
        let mut lengths = logical.as_slice().iter().peekable();
        let mut ordinal = 0;
        while let Some(value) = pending.pop() {
            meter.count_decode(1).map_err(Failure::charge)?;
            ordinal += 1;
            if let Some(&&(index, len)) = lengths.peek()
                && index == ordinal
            {
                if ordinal == 1 {
                    result.root = Some(len);
                } else {
                    result
                        .entries
                        .push((std::ptr::from_ref(value).addr(), len))
                        .map_err(Failure::charge)?;
                }
                lengths.next();
            }
            match value {
                Value::Dict(map) => {
                    for (_, child) in map.entries().iter().rev() {
                        pending.push(child).map_err(Failure::charge)?;
                    }
                }
                Value::List(items) => {
                    for child in items.iter().rev() {
                        pending.push(child).map_err(Failure::charge)?;
                    }
                }
                Value::Tuple(items) => {
                    for child in items.iter().rev() {
                        pending.push(child).map_err(Failure::charge)?;
                    }
                }
                _ => {}
            }
        }
        if lengths.next().is_some() {
            return Err(Failure::Static(
                "ValueError",
                "invalid writer length snapshot",
            ));
        }
        meter
            .count_decode((result.entries.len() as u64).saturating_mul(u64::from(usize::BITS)))
            .map_err(Failure::charge)?;
        result
            .entries
            .as_mut_slice()
            .sort_unstable_by_key(|&(address, _)| address);
        Ok(result)
    }
    pub(crate) fn get(&self, value: &Value, root: &Value) -> Option<usize> {
        if std::ptr::eq(value, root) {
            return self.root;
        }
        let address = std::ptr::from_ref(value).addr();
        self.entries
            .as_slice()
            .binary_search_by_key(&address, |&(key, _)| key)
            .ok()
            .and_then(|index| self.entries.as_slice().get(index).map(|entry| entry.1))
    }
}
