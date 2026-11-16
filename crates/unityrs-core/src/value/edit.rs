//! Scalar edits retain the parsed allocation and any charged byte owners.
use super::{Parsed, Value, builder::owns_heap};
use crate::limits::ChargeError;

impl Parsed {
    /// Replace an existing field without allocating. Strings and bytes must
    /// own charged storage, as for `ValueBuilder::scalar`. Heap containers
    /// are refused; their charges cannot be transferred through this API.
    /// Missing paths and containers return false, leaving the value intact.
    pub fn replace_scalar(&mut self, path: &[&[u8]], value: Value) -> Result<bool, ChargeError> {
        if owns_heap(&value) || path.is_empty() {
            return Ok(false);
        }
        let mut current = &mut self.value;
        for name in path {
            let Value::Dict(map) = current else {
                return Ok(false);
            };
            let Some(list) = self.lists.get(map.list as usize) else {
                return Ok(false);
            };
            let mut found = None;
            for (key, child) in &mut map.entries {
                let key_name = list.name(*key);
                self.charge
                    .meter()
                    .count_decode(1 + key_name.len().min(name.len()) as u64)?;
                if key_name == *name {
                    found = Some(child);
                    break;
                }
            }
            let Some(child) = found else { return Ok(false) };
            current = child;
        }
        *current = value;
        Ok(true)
    }
}
