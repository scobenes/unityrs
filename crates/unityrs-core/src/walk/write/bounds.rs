//! What a write counts against its bounds as it goes: the open's work, the
//! values `max_values_per_object` bounds, and the frames `max_value_depth`
//! bounds (plan 3.10, "Bounds"; 3.11).

use super::{VALUE_WORK, WriteError, Writer};
use crate::error::{LimitError, LimitKind};

impl Writer<'_, '_> {
    /// Counts one value written (its open's work `work`, about to be done)
    /// toward the open's work, and against `max_values_per_object` (the
    /// bound a read counts against, plan 3.10, "Bounds"; 3.11) when nothing
    /// else bounds it: under a child whose name a sibling repeats, where one
    /// value is written once for each child of the name, which no output
    /// bound stops when its node writes nothing (claude B, review round 3:
    /// `K` × `N` values into `4K + 4N` bytes). Elsewhere each value written
    /// is one of the value's own, once (the value is held, its memory
    /// charged) or an attribute a class takes from a value that is not a
    /// dict, which [`Self::class`] counts; so a primitive run of any length
    /// writes as UnityPy writes it (review round 9). A write of a value
    /// through the tree it was read with counts no more than UnityPyBoost's
    /// read of it did, unless the tree repeats a name.
    pub(super) fn count(&mut self, work: u64) -> Result<(), WriteError> {
        if self.repeated > 0 {
            self.bound()?;
        }
        self.open.add(work).map_err(WriteError::Limit)
    }

    /// Counts one child skipped: [`VALUE_WORK`] of the open's work, and
    /// always against `max_values_per_object` (a class's loop visits it
    /// whatever it writes).
    pub(super) fn skip(&mut self) -> Result<(), WriteError> {
        self.bound()?;
        self.open.add(VALUE_WORK).map_err(WriteError::Limit)
    }

    /// One more against `max_values_per_object`.
    pub(super) fn bound(&mut self) -> Result<(), WriteError> {
        self.values = self.values.saturating_add(1);
        let max = self.limits.max_values_per_object;
        if self.values > max {
            return Err(WriteError::Limit(LimitError {
                kind: LimitKind::ValuesPerObject,
                value: self.values,
                max,
            }));
        }
        Ok(())
    }

    /// `child`'s value written by `write`: under a name a sibling repeats,
    /// every value counts against `max_values_per_object` ([`Self::count`]).
    #[inline]
    pub(super) fn child(
        &mut self,
        repeated: bool,
        write: impl FnOnce(&mut Self) -> Result<(), WriteError>,
    ) -> Result<(), WriteError> {
        if !repeated {
            return write(self);
        }
        self.repeated = self.repeated.saturating_add(1);
        let r = write(self);
        self.repeated = self.repeated.saturating_sub(1);
        r
    }

    /// One frame UnityPy's `write_value` pushes: a `write_value` call, or the
    /// list comprehension that writes an array's elements (`:435`). Each
    /// counts one level against `max_value_depth`, as the pure-Python reader
    /// counts its own (plan 3.10, "Bounds"), so every value whose write
    /// raises `RecursionError` in UnityPy (past Python's 1,000 frames) is
    /// refused here first.
    pub(super) fn frame(
        &mut self,
        f: impl FnOnce(&mut Self) -> Result<(), WriteError>,
    ) -> Result<(), WriteError> {
        self.depth = self.depth.saturating_add(1);
        if self.depth > self.max_depth {
            let e = WriteError::Limit(LimitError {
                kind: LimitKind::ValueDepth,
                value: u64::from(self.depth),
                max: u64::from(self.max_depth),
            });
            self.depth = self.depth.saturating_sub(1);
            return Err(e);
        }
        let r = f(self);
        self.depth = self.depth.saturating_sub(1);
        r
    }
}
