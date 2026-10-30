//! Hard bounds on what untrusted input can make the library allocate or do,
//! and the one meter that enforces the memory bound.
//!
//! UnityPy has none of these: where it would exhaust memory or the C stack,
//! unityrs stops with [`crate::Error::Limit`] (or the walker's `other` kind) and
//! `PARITY.md` lists the case as a safety divergence (plan section 3.15).
//!
//! **One meter per input.** Everything the core allocates in proportion to a
//! size or count the input chose is charged to the input's [`Meter`] *before*
//! it is allocated: the input's own bytes when the core reads a path, every
//! block decode (a first decode, a lazy re-decode, a measuring decode, a
//! decode after eviction), every metadata table and typetree row of every
//! entry parse (an entry parsed twice, or two directory rows over the same
//! bytes, pay twice), every compiled tree, every parsed value and every
//! writer output. A charge the meter refuses allocates nothing. The charge is
//! a [`Charge`] that lives inside what it paid for, so the bytes return when
//! that drops: a [`crate::Parsed`] carries the charge for its values, a
//! decoded buffer's [`bytes::Bytes`] the charge for the buffer. So what one
//! input holds at any moment, retained values included, is at most
//! [`Limits::max_input_memory`] (plus the constant overhead plan section 3.15
//! states). A top-level load makes the meter; every file inside it (a
//! bundle's entries, nested bundles) charges the same one.
//!
//! **A meter is one open of its input.** Its memory is the input's: an
//! account every open of the input shares, so the bound holds across opens.
//! Its *work* ([`Limits::max_decode_work`]) is the open's own: the
//! Environment opens an evicted dependency again on [`Meter::reopen`], a new
//! meter on the same account with a fresh allowance, while a handle of the
//! old open that someone still holds (a nested bundle a resource read kept)
//! goes on counting against the old meter's, which nothing resets.
//!
//! **One accounting path.** The meter books every charge it grants with the
//! load's [`MemoryBudget`] as well ([`crate::FileContext::budget`]), so a
//! batch engine's budget sees exactly what every input holds, and a refusal
//! says which side refused: [`ChargeError::Limit`] (this input's bound) or
//! [`ChargeError::Budget`] (the shared budget, tagged with the meter's
//! [`Reservation`]).
//!
//! The other bounds are on work and nesting, per object read or per input.
//!
//! Owner: the scaffold (implemented). Packages read these fields and charge
//! through these types; a new limit is added only through the integration
//! package.
//!
//! [`MemoryBudget`]: crate::budget::MemoryBudget
//! [`Reservation`]: crate::budget::Reservation

use crate::budget::BudgetExceeded;
use crate::error::LimitError;

mod meter;
mod vec;

pub(crate) use meter::GrantScope;
pub use meter::{Charge, Meter};
pub use vec::MeteredVec;
pub(crate) use vec::{grow_charged, grow_counted};

/// Bounds on allocation, recursion and work per input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Limits {
    /// What the core may hold for one input at once, in bytes, charged to
    /// the input's [`Meter`] before every allocation whose size the input
    /// chooses (module docs). Default 8 GiB.
    pub max_input_memory: u64,
    /// The longest file [`crate::env::Source::Path`] reads, checked against
    /// the file's length before anything is read, and again as it is read
    /// ([`crate::env::read_path`]). Default 4 GiB.
    pub max_input_bytes: u64,
    /// One block's actual decoded output (blocks-info's included). LZMA
    /// decodes to the end of its input whatever blocks-info says
    /// (`helpers/CompressionHelper.py:17-44`), so a bomb is stopped here
    /// before the meter. Default 1 GiB.
    pub max_block_output: u64,
    /// The *work* one open of an input may cost, in bytes: every byte a
    /// block decode writes, kept or not (a first decode, a measuring decode
    /// whose output is dropped, a re-decode of a shared block, a decode that
    /// fails after it wrote), whether or not its buffer had to grow for it,
    /// and every byte the meter granted to a lazy computation, on the
    /// computation's own thread, that the meter itself then refused
    /// ([`LimitKind::InputMemory`]: a parse or compile that alone needs more
    /// than the meter, which the next ask runs again:
    /// [`crate::once::get_or_publish_result`]); and the bytes a parse
    /// searches or indexes (a string's search, a typetree blob's one pass
    /// over its string buffer, an old-format file's type index:
    /// `typetree::parse` and `files::serialized`), the bytes a bundle
    /// entry's gzip detection reads and inflates (`detect`), and every byte
    /// the meter granted an entry parse that ended in an error its cell
    /// keeps (`files::bundle`), all of which K directory rows over one span
    /// repeat K times (round 4); and the work of every read and write of an
    /// object's value, whatever its outcome ([`crate::walk::VALUE_WORK`] a
    /// value built, written or skipped, a primitive run's bytes: `walk`'s
    /// "Work"), which K objects over one span repeat K times, a read refused
    /// by its own bound keeping nothing (fix pass 4's sweep); past the
    /// allowance a read or a write is refused before it starts. The meter
    /// bounds what is
    /// held; this bounds what is done, so an input cannot make its readers
    /// decode or recompute without end while the meter stays empty
    /// (thousands of blocks that each decode to the block cap, measured and
    /// dropped by a lazy read of the last entry; one type whose compile alone
    /// needs more than the meter, asked by every object of the type). A
    /// write the meter or the budget refuses decodes nothing and counts
    /// nothing, and a lazy computation the budget, an I/O failure or a
    /// provider refused counts nothing either (those depend on what else is
    /// held or on caller code, and the engine bounds its retries), so
    /// contention never uses the allowance up; a write is counted atomically
    /// (between a growth's charge and its allocation when it grows), so
    /// decodes running at once never pass the bound
    /// ([`Meter::count_decode`]). It is counted
    /// per open: every file of one open shares its meter's count, and the
    /// Environment reopens an evicted dependency on [`Meter::reopen`], so
    /// eviction churn costs work in proportion to the reads that cause it
    /// and never exhausts a valid dependency. UnityPy decodes every block
    /// once; a lazy read of a bundle whose entries do not overlap decodes a
    /// block at most four times: measured, decoded for the entries wholly
    /// inside it, and decoded again for each of the two entries that
    /// straddle its edges (plan section 3.6, "Runs"). Default 32 GiB, four
    /// times [`Self::max_input_memory`].
    ///
    /// [`LimitKind::InputMemory`]: crate::error::LimitKind::InputMemory
    pub max_decode_work: u64,
    /// One gzip stream inflated by detection (rule 7, streamed into a fixed
    /// scratch buffer and never held). Default 4 GiB.
    pub max_inflate: u64,
    /// Containers nested in one input (a bundle entry that is itself a
    /// bundle, and so on): parsing one recurses. Default 16.
    pub max_container_depth: u32,
    /// Depth of a compiled typetree. Default 256.
    pub max_tree_depth: u32,
    /// Runtime nesting while reading or writing a value, `ReferencedObject`
    /// data included: the frames each reader and the writer recurse through
    /// (plan section 3.10, "Bounds"). UnityPyBoost's are its C recursion,
    /// which has no bound of its own; the pure-Python reader's and the
    /// writer's are the Python frames UnityPy's own pushes (`read_value`,
    /// `read_value_array`, each comprehension and `lambda`), so every value
    /// UnityPy refuses with `RecursionError` (Python's 1,000 frames) passes
    /// this first. What it costs in stack: [`Self::stack_needed`]. Default 512.
    pub max_value_depth: u32,
    /// Values built by one object read, each element of a primitive run
    /// included, and each child the read skips (a `ReferencedObject`'s data
    /// that does not resolve to a tree, a `ManagedReferencesRegistry` once
    /// one is in scope); likewise the values one write of an object writes
    /// and the children it skips. This bounds the *work* of one read or
    /// write, which their bytes do not: the pure-Python reader can read the
    /// same bytes again after a negative length, a class can skip many
    /// children for each element of an array, and a tree that repeats a name
    /// hands one value to each child of the name, which writes it whole. A
    /// write through the tree its value was read with counts no more than
    /// its read did unless the tree repeats a name
    /// (`tests/walk_bounds.rs`). The values' memory is the meter's, and the
    /// work of every read and write together the open's
    /// ([`Self::max_decode_work`]). Default 2^26.
    pub max_values_per_object: u64,
    /// Array elements whose read ended no further on than it began, per
    /// object: a vector of empty structs with a huge count reads no byte per
    /// element, nor does a pure-Python element whose negative `TypelessData`
    /// length takes the reader back to its start. An element that reads a
    /// byte is never counted, whatever its tree. Default 2^20.
    pub max_zero_size_elements: u64,
    /// Elements reserved before the bytes prove they exist. Default 2^16.
    pub max_prealloc: usize,
}

impl Limits {
    /// The stack a thread needs to read or write one object under these
    /// limits, from the frame that calls the reader or the writer: they
    /// recurse once per level they count against
    /// [`Self::max_value_depth`], and nothing else an object's bytes drive
    /// recurses deeper (a typetree compiles without recursion; a value is
    /// dropped one level per reader level). It grows with `max_value_depth`,
    /// and a debug build's frames are several times a release build's.
    ///
    /// At the defaults: 1.75 MiB in a release build, 9 MiB in a debug one.
    /// `tests/walk_stack.rs` reads, writes and drops the deepest object of
    /// every shape on a thread this size, and its `stack_measured` prints
    /// the least each needs (at most 0.96 MiB release, 6.8 MiB debug, with
    /// toolchain 1.99.1). Rust's spawned threads get 2 MiB unless told
    /// otherwise, and a main thread usually 8 MiB, so a debug build reads
    /// untrusted input on a thread built with
    /// `std::thread::Builder::stack_size(limits.stack_needed())`.
    #[must_use]
    pub fn stack_needed(&self) -> usize {
        let (per_level, base): (usize, usize) = if cfg!(debug_assertions) {
            (16 << 10, 1 << 20)
        } else {
            (3 << 10, 256 << 10)
        };
        usize::try_from(self.max_value_depth)
            .unwrap_or(usize::MAX)
            .saturating_mul(per_level)
            .saturating_add(base)
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_input_memory: 8 << 30,
            max_input_bytes: 4 << 30,
            max_block_output: 1 << 30,
            max_decode_work: 32 << 30,
            max_inflate: 4 << 30,
            max_container_depth: 16,
            max_tree_depth: 256,
            max_value_depth: 512,
            max_values_per_object: 1 << 26,
            max_zero_size_elements: 1 << 20,
            max_prealloc: 1 << 16,
        }
    }
}

/// Buckets a hash table is charged (or counted) per key: a doubling table
/// (hashbrown's, behind `std`'s `HashMap`) is at least 7/16 full once it has
/// grown, so it holds up to 16/7 buckets per key, and while it grows the old
/// table and the doubled one are both live, up to 24/7 buckets per key; four
/// cover both. A bucket is a key, a value and one control byte; the table's
/// fixed part (its control bytes' tail of one SIMD group, 16 bytes, and the
/// allocator's rounding of its one allocation) is a constant per table.
pub(crate) const HASH_TABLE_SHARES: u64 = 4;

/// The charge for a hash table of `len` keys (at least two keys' worth)
/// whose key and value take `entry` bytes together: [`HASH_TABLE_SHARES`]
/// buckets a key, each the entry and one control byte, plus the table's
/// fixed part, as one [`heap_cost`]. A table that grows one key at a time
/// charges `hash_table_cost(n + 1, _) - hash_table_cost(n, _)` before each
/// insert; the 4/7 of a bucket a key between the four charged and the 24/7
/// a doubling holds covers the rounding of the second allocation while
/// both are live.
pub(crate) fn hash_table_cost(len: usize, entry: usize) -> u64 {
    let bucket = (entry as u64).saturating_add(1);
    heap_cost(
        (len.max(2) as u64)
            .saturating_mul(HASH_TABLE_SHARES.saturating_mul(bucket))
            .saturating_add(16),
    )
}

/// glibc's default `M_MMAP_THRESHOLD`: a chunk of at least this many bytes
/// may be `mmap`ped (the threshold starts here and can only rise, which
/// serves more chunks from the heap, never fewer).
const MMAP_THRESHOLD: u64 = 128 << 10;

/// What one heap allocation of `bytes` costs, which is what a charge for it
/// is: glibc's chunk on x86-64. The request gets its 8-byte header and is
/// rounded to 16, with a 32-byte minimum (`request2size`); a chunk of
/// 128 KiB (glibc's default `M_MMAP_THRESHOLD`) or more may be `mmap`ped instead, which adds another
/// 8 bytes and rounds to a 4 KiB page (`sysmalloc`'s
/// `ALIGN_UP(nb + SIZE_SZ, pagesize)`), and that is the larger of the two.
/// So a charge covers the allocator's rounding too, and many small
/// allocations cannot hold more than they were charged. 0 for no allocation.
#[must_use]
pub const fn heap_cost(bytes: u64) -> u64 {
    if bytes == 0 {
        return 0;
    }
    let chunk = bytes.saturating_add(8 + 15) & !15;
    let chunk = if chunk < 32 { 32 } else { chunk };
    if chunk < MMAP_THRESHOLD {
        chunk
    } else {
        chunk.saturating_add(8 + 4095) & !4095
    }
}

/// A charge the meter or the budget refused. Nothing was allocated.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ChargeError {
    /// The input's own bound ([`Limits::max_input_memory`], or a buffer's
    /// cap such as [`Limits::max_block_output`]).
    #[error(transparent)]
    Limit(#[from] LimitError),
    /// The shared budget refused, or the allocator failed.
    #[error(transparent)]
    Budget(#[from] BudgetExceeded),
}

#[cfg(test)]
mod tests;
