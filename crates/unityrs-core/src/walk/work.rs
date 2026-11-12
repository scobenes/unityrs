//! The work of one read or one write, counted on its open's meter as it is
//! done ([`OpenWork`]), and one read's bounds of its own ([`Work`]): the
//! module docs of [`super`], "Work"; plan 3.10, "Bounds" and "Work per
//! open".

use super::{ReadDetail, ReadError, ReadErrorKind, first_reserve, limit_error};
use crate::error::{LimitError, LimitKind};
use crate::limits::{Limits, Meter};
use crate::value::Value;

/// The open's work one value costs a read or a write, in bytes
/// ([`crate::limits::Limits::max_decode_work`]): the size of the [`Value`]
/// that holds it, whether the value is kept or not, as a decode counts every
/// byte it writes whether its output is kept or not. A child a read or a
/// write skips costs as much. An element of a primitive run that a read
/// holds as a run costs its width instead (`Work::run`, `Work::elements`),
/// and so does a primitive a write writes.
pub const VALUE_WORK: u64 = size_of::<Value>() as u64;

/// The open's work one step costs a read or a write that builds no value,
/// in bytes: a slot a dict's table of positions grows by, an entry a lookup
/// by name passes, and an entry a name table indexes. 8, as a step of a
/// bundle read (plan 3.6, "What a read costs"). A name a step compares or
/// hashes counts its bytes besides.
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
    meter.work_refused().map_or(Ok(()), Err)
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
///
/// **What counts against `max_values_per_object`.** The bound is for the
/// work of one read that neither its bytes nor its memory bounds. Every
/// value a read keeps sits in a slot charged to the input's meter (a list's
/// item, a dict's entry, a tuple's item: at least a [`Value`] each), and a
/// primitive run is one value whose elements are bytes of the input
/// (UnityPyBoost's `vector<UInt8>`, a slice) or of a charged run. So
/// UnityPyBoost's reader, whose position never goes back, counts against it
/// only what it does and keeps nothing of: each child it skips
/// ([`Self::skipped`]), and each value it builds under a child whose name a
/// sibling repeats ([`Self::enter_repeated`]), one of which replaces the
/// other's value ([`Self::kept`]). Everything else it builds is bounded by
/// `max_input_memory` and by the object's bytes, as UnityPy's own values are
/// by the memory they take (review round 9: a `Mesh` of a 2^26 + 1 byte
/// `vector<UInt8>`, and any struct array of more than 2^26 values, was
/// refused where UnityPy reads it). The pure-Python reader counts every
/// value it builds and every element of a primitive run
/// ([`Self::elements`]): a negative `TypelessData` length takes it back, so
/// it can read the same bytes again any number of times, and a
/// `vector<UInt8>`'s elements are a slice that holds none of them.
pub(crate) struct Work<'m> {
    pub(crate) values: u64,
    pub(crate) zero_size: u64,
    /// How many times the read has done work it keeps nothing of
    /// ([`Self::discard`]); an element's [`Self::zero_size`] compares it
    /// with its value before the element.
    discards: u64,
    pub(crate) depth: u32,
    /// How many children whose name a sibling repeats are being read
    /// ([`crate::typetree::NodeInfo::repeated`]): while any is, every value
    /// built counts against `max_values_per_object`.
    repeated: u32,
    /// Elements the read's open lists reserved before their bytes were read
    /// and have not filled yet ([`Self::first_room`]).
    unfilled: usize,
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
            discards: 0,
            depth: 0,
            repeated: 0,
            unfilled: 0,
            open,
            limits,
        })
    }

    /// The first reservation of a list of `len` elements of at least
    /// `min_size` bytes each, `remaining` bytes left ([`first_reserve`]): at
    /// most what the read's open lists leave of `max_prealloc`, so every
    /// list a read holds open at once (vectors nested in vectors, a ref type
    /// re-entered level after level) reserves no more together than one may
    /// (review round 7's sweep: each reserved `max_prealloc` of its own, so
    /// a read nested `d` deep held `d` times it before the bytes were read).
    /// It is unfilled until [`Self::filled`] counts its elements.
    pub(crate) fn first_room(&mut self, len: usize, remaining: usize, min_size: u32) -> usize {
        let room = self.limits.max_prealloc.saturating_sub(self.unfilled);
        let first = first_reserve(len, remaining, min_size, room);
        self.unfilled = self.unfilled.saturating_add(first);
        first
    }

    /// Element `i` of a list whose first reservation was `first` is read.
    pub(crate) fn filled(&mut self, i: usize, first: usize) {
        if i < first {
            self.unfilled = self.unfilled.saturating_sub(1);
        }
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
    /// [`VALUE_WORK`] of the open's work: every value the pure-Python reader
    /// builds.
    pub(crate) fn values(&mut self, n: u64, at: u64) -> Result<(), ReadError> {
        self.count(n, n.saturating_mul(VALUE_WORK), at)
    }

    /// Counts one child the read skips (a `ReferencedObject`'s data that
    /// resolves to no tree, a `ManagedReferencesRegistry` once one is in
    /// scope) as a value ([`Self::values`]): the class's loop visits it
    /// whether or not a value comes out, and keeps nothing of it
    /// ([`Self::discard`]).
    pub(crate) fn skipped(&mut self, at: u64) -> Result<(), ReadError> {
        self.discard();
        self.values(1, at)
    }

    /// Marks work the read keeps nothing of: a child it skips
    /// ([`Self::skipped`]), a value a later sibling of the same name
    /// replaces (`walk::dict::Dict::set`), and, in the pure-Python reader, a
    /// read that takes the position back, so later reads read the same
    /// bytes again. An element that does any of these and reads no byte is
    /// what [`Self::zero_size`] counts.
    pub(crate) fn discard(&mut self) {
        self.discards = self.discards.wrapping_add(1);
    }

    /// The mark an element's [`Self::zero_size`] is given: how many times
    /// the read had done work it keeps nothing of before the element.
    pub(crate) fn discards(&self) -> u64 {
        self.discards
    }

    /// Counts one value UnityPyBoost's reader builds: [`VALUE_WORK`] of the
    /// open's work, and against `max_values_per_object` only under a child
    /// whose name a sibling repeats (the type's docs).
    pub(crate) fn kept(&mut self, at: u64) -> Result<(), ReadError> {
        let n = u64::from(self.repeated > 0);
        self.count(n, VALUE_WORK, at)
    }

    /// Counts a primitive run of `bytes` bytes that UnityPyBoost's reader
    /// holds as one value (a [`crate::value::PrimRun`] or a slice of the
    /// object), its vector's: its bytes as the open's work, as a decode
    /// counts the bytes it writes, and no value of its own. Its elements
    /// are the object's bytes, which its position passes once.
    pub(crate) fn run(&mut self, bytes: u64, at: u64) -> Result<(), ReadError> {
        self.open.add(bytes).map_err(|l| limit_refused(at, l))
    }

    /// Counts `n` elements of `width` bytes each of a primitive run the
    /// pure-Python reader reads one by one: `n` values against
    /// `max_values_per_object`, and their bytes as the open's work.
    pub(crate) fn elements(&mut self, n: u64, width: u64, at: u64) -> Result<(), ReadError> {
        self.count(n, n.saturating_mul(width), at)
    }

    /// Starts reading a child whose name a sibling repeats: until the
    /// matching [`Self::leave_repeated`], [`Self::kept`] counts every value
    /// against `max_values_per_object`.
    pub(crate) fn enter_repeated(&mut self) {
        self.repeated = self.repeated.saturating_add(1);
    }

    pub(crate) fn leave_repeated(&mut self) {
        self.repeated = self.repeated.saturating_sub(1);
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
    /// began (plan section 9, Q15 (c)), when the read did work it keeps
    /// nothing of since `before` (the [`Self::discards`] mark taken before
    /// the element): it skipped a child, a sibling of the same name replaced
    /// a value, or, in the pure-Python reader, a negative `TypelessData`
    /// length took the position back ([`Self::discard`]). Such elements do
    /// work that neither the object's bytes nor the meter bound, so a count
    /// in a few bytes could ask any number of them. Every other element
    /// keeps what it builds, each value in a slot charged to the meter (a
    /// list's item, a dict's entry), and [`VALUE_WORK`] of the open's work a
    /// value: an array of a class with no fields reads no byte per element,
    /// and is read at any count the meter holds, as UnityPy reads it (review
    /// round 9's sweep found it refused past 2^20). What an element really
    /// read is the test, not its tree's
    /// [`crate::typetree::NodeInfo::min_size`]: which children a read skips
    /// depends on the registry in scope, and the pure-Python reader reads a
    /// malformed `pair` or `Array` UnityPyBoost refuses (review round 4:
    /// both were refused past the bound where UnityPy read them).
    pub(crate) fn zero_size(&mut self, before: u64, at: u64) -> Result<(), ReadError> {
        if self.discards == before {
            return Ok(());
        }
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
