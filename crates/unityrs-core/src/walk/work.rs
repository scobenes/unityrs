//! The work of one read or one write, counted on its open's meter as it is
//! done ([`OpenWork`]), and one read's bounds of its own ([`Work`]): the
//! module docs of [`super`], "Work"; plan 3.10, "Bounds" and "Work per
//! open".

use super::{ReadDetail, ReadError, ReadErrorKind, limit_error};
use crate::error::{LimitError, LimitKind};
use crate::limits::{Limits, Meter};
use crate::value::Value;

/// The open's work one value costs a read or a write, in bytes
/// ([`crate::limits::Limits::max_decode_work`]): the size of the [`Value`]
/// that holds it, whether the value is kept or not, as a decode counts every
/// byte it writes whether its output is kept or not. A child a read or a
/// write skips costs as much, as it costs a value against
/// `max_values_per_object`. An element of a primitive run that a read holds
/// as a run costs its width instead (`Work::run`), and so does a primitive a
/// write writes.
pub const VALUE_WORK: u64 = size_of::<Value>() as u64;

/// The open's work one step costs a read or a write that builds no value,
/// in bytes: a step of a pass over a node's children or a dict's entries
/// that sizes a dict, clears its slots or finds a name, and an entry a name
/// table indexes. 8, as a step of a bundle read (plan 3.6, "What a read
/// costs"). A name a step compares or hashes counts its bytes besides.
pub const STEP_WORK: u64 = 8;

/// The most work a read or a write counts on its own before it adds it to
/// its open's meter, in bytes (`OpenWork`): so the most it can be past
/// its open's allowance before it learns of it, and the most each read or
/// write in progress on other threads can add past it.
pub const WORK_CHUNK: u64 = 64 << 10;

/// The open's work refusal once its work has passed its allowance, `Ok`
/// while it is within it: what a read or a write meets before it starts and
/// when it ends (module docs of [`super`], "Work").
pub(crate) fn work_left(meter: &Meter) -> Result<(), LimitError> {
    meter.work_used_up().map_or(Ok(()), Err)
}

/// The open's work of one read or one write, counted on the open's meter
/// as it is done (module docs, "Work"). A step is counted before it is
/// done; the steps add up here, and every [`WORK_CHUNK`] of them goes to the
/// meter in one atomic check-and-add, so reads and writes on several
/// threads that share one meter learn of each other's work as it is done,
/// and stop once the open's work has passed its allowance (review round 6:
/// each read added its work only when it ended, so two that started
/// together within the allowance both ran to their end past it). A chunk
/// the allowance cannot hold is refused: the steps before the one that
/// filled it are done, and counted; that one is not done, nor counted. The
/// read or write then fails with [`LimitKind::DecodeWork`]. When it ends
/// ([`Self::finish`]), what is left is counted and checked: a read or a
/// write whose own work took its open past its allowance fails too, so the
/// work of those that succeed is never past it, however they interleave.
/// Each one in progress can count at most a chunk and one step past it.
pub(crate) struct OpenWork<'m> {
    meter: &'m Meter,
    /// Counted here and not yet on the meter: less than [`WORK_CHUNK`].
    pending: u64,
}

impl<'m> OpenWork<'m> {
    /// The work of a read or a write on `meter`: refused before it starts
    /// once the open's work has passed its allowance.
    pub(crate) fn start(meter: &'m Meter) -> Result<Self, LimitError> {
        work_left(meter)?;
        Ok(Self { meter, pending: 0 })
    }

    /// Counts `n` bytes of work, about to be done.
    #[inline]
    pub(crate) fn add(&mut self, n: u64) -> Result<(), LimitError> {
        let done = self.pending;
        let pending = done.saturating_add(n);
        if pending < WORK_CHUNK {
            self.pending = pending;
            return Ok(());
        }
        self.pending = 0;
        self.meter.count_work(pending).inspect_err(|_| {
            // The steps before this one are done, whatever the bound says.
            self.meter.add_work(done);
        })
    }

    /// Counts `n` steps of [`STEP_WORK`], about to be taken.
    #[inline]
    pub(crate) fn steps(&mut self, n: usize) -> Result<(), LimitError> {
        self.add(
            u64::try_from(n)
                .unwrap_or(u64::MAX)
                .saturating_mul(STEP_WORK),
        )
    }

    /// Counts what is left, all of it done, and refuses once the open's work
    /// has passed its allowance.
    pub(crate) fn finish(&mut self) -> Result<(), LimitError> {
        self.meter.add_work(std::mem::take(&mut self.pending));
        work_left(self.meter)
    }
}

impl Drop for OpenWork<'_> {
    /// Work left uncounted by a read or a write that never reached
    /// [`OpenWork::finish`] is done all the same.
    fn drop(&mut self) {
        self.meter.add_work(std::mem::take(&mut self.pending));
    }
}

/// A [`Limits`] bound's refusal, or the open's work's, while reading: kind
/// `other` (R16).
pub(crate) fn limit_refused(at: u64, limit: LimitError) -> ReadError {
    ReadError {
        kind: ReadErrorKind::Other,
        at,
        detail: ReadDetail::Limit(limit),
    }
}

/// The reader's limits on work, shared by both readers (plan 3.10, "Bounds").
pub(crate) struct Work<'m> {
    pub(crate) values: u64,
    pub(crate) zero_size: u64,
    pub(crate) depth: u32,
    /// The open's work, counted as the read goes.
    pub(crate) open: OpenWork<'m>,
    pub(crate) limits: Limits,
}

impl<'m> Work<'m> {
    /// The work of a read on `meter`: refused before it starts once the
    /// open's work has passed its allowance (module docs, "Work").
    pub(crate) fn start(limits: Limits, meter: &'m Meter) -> Result<Self, ReadError> {
        let open = OpenWork::start(meter).map_err(|l| limit_refused(0, l))?;
        Ok(Self {
            values: 0,
            zero_size: 0,
            depth: 0,
            open,
            limits,
        })
    }

    /// Counts what is left of the read's work on its open's meter, whatever
    /// the read's outcome, and refuses once the open's work has passed its
    /// allowance ([`OpenWork::finish`]).
    pub(crate) fn finish(&mut self, at: u64) -> Result<(), ReadError> {
        self.open.finish().map_err(|l| limit_refused(at, l))
    }

    /// Counts `n` steps that build no value ([`STEP_WORK`]), about to be
    /// taken: the open's work, and not against `max_values_per_object`,
    /// which counts what UnityPy's reader counts.
    pub(crate) fn steps(&mut self, n: usize, at: u64) -> Result<(), ReadError> {
        self.open.steps(n).map_err(|l| limit_refused(at, l))
    }

    /// Counts `n` values against `max_values_per_object`, each
    /// [`VALUE_WORK`] of the open's work.
    pub(crate) fn values(&mut self, n: u64, at: u64) -> Result<(), ReadError> {
        self.count(n, n.saturating_mul(VALUE_WORK), at)
    }

    /// Counts a primitive run of `n` elements of `width` bytes each, held as
    /// a run (a [`crate::value::PrimRun`] or a slice of the file): `n`
    /// values against `max_values_per_object`, and their bytes as the
    /// open's work, as a decode counts the bytes it writes.
    pub(crate) fn run(&mut self, n: u64, width: u64, at: u64) -> Result<(), ReadError> {
        self.count(n, n.saturating_mul(width), at)
    }

    fn count(&mut self, n: u64, work: u64, at: u64) -> Result<(), ReadError> {
        self.values = self.values.saturating_add(n);
        if self.values > self.limits.max_values_per_object {
            return Err(limit_error(
                at,
                LimitKind::ValuesPerObject,
                self.values,
                self.limits.max_values_per_object,
            ));
        }
        self.open.add(work).map_err(|l| limit_refused(at, l))
    }

    /// Counts one array element whose read ended no further on than it
    /// began: it read nothing (an empty class, a class whose children were
    /// all skipped), or, in the pure-Python reader, a negative
    /// `TypelessData` length took the position back. Such elements cost
    /// values without bytes, so a count in a few bytes could build any
    /// number of them; every other element reads bytes, so the input
    /// bounds them. What an element really read is the test, not its
    /// tree's [`crate::typetree::NodeInfo::min_size`]: which children a read
    /// skips depends on the registry in scope, and the pure-Python reader
    /// reads a malformed `pair` or `Array` UnityPyBoost refuses (review
    /// round 4: both were refused past the bound where UnityPy read them).
    pub(crate) fn zero_size(&mut self, at: u64) -> Result<(), ReadError> {
        self.zero_size = self.zero_size.saturating_add(1);
        if self.zero_size > self.limits.max_zero_size_elements {
            return Err(limit_error(
                at,
                LimitKind::ZeroSizeElements,
                self.zero_size,
                self.limits.max_zero_size_elements,
            ));
        }
        Ok(())
    }

    /// Enters one level of value nesting.
    pub(crate) fn enter(&mut self, at: u64) -> Result<(), ReadError> {
        self.depth = self.depth.saturating_add(1);
        if self.depth > self.limits.max_value_depth {
            return Err(limit_error(
                at,
                LimitKind::ValueDepth,
                u64::from(self.depth),
                u64::from(self.limits.max_value_depth),
            ));
        }
        Ok(())
    }

    pub(crate) fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }
}
