//! Hard bounds on what untrusted input can make the library allocate or do.
//!
//! UnityPy has none of these: where it would exhaust memory or the C stack,
//! unityrs stops with [`crate::Error::Limit`] (or the walker's `other` kind) and
//! `PARITY.md` lists the case as a safety divergence (plan section 3.15).
//!
//! **Per input.** The bounds on what parsing keeps (metadata records,
//! typetree rows, decoded bundle bytes) hold for one *input*: a top-level
//! load and every file inside it, a bundle's entries and nested bundles
//! included. One [`Meter`] per top-level load counts them, shared through
//! [`crate::files::Parent`], so a bundle of a thousand `SerializedFile`s gets
//! one budget of records, not a thousand. The value bounds hold per object
//! read.
//!
//! Owner: the scaffold (implemented). Packages read these fields; a new
//! limit is added only through the integration package.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::{LimitError, LimitKind};

/// Bounds on allocation, recursion and work per input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Limits {
    /// One block's actual decoded output (blocks-info's included). LZMA
    /// decodes to the end of its input whatever blocks-info says
    /// (`helpers/CompressionHelper.py:17-44`), so a bomb is stopped here.
    /// Default 1 GiB.
    pub max_block_output: u64,
    /// The decoded data of one input, every block and blocks-info of every
    /// bundle in it, each block counted once at its actual length: the
    /// declared total is checked at open, and the running *actual* total
    /// ([`Metered::DecodedBytes`]) as output grows, before it is allocated,
    /// since an LZMA block can outgrow its declaration (plan section 3.6).
    /// Default 4 GiB (plan section 3.15).
    pub max_bundle_uncompressed: u64,
    /// One gzip stream inflated by detection (rule 7, streamed into a scratch
    /// buffer and never held) or a `WebFile`. Default 4 GiB.
    pub max_inflate: u64,
    /// Records in one input's metadata tables ([`Metered::MetadataRecords`]),
    /// every table of every file together: `SerializedFile` types, objects,
    /// script types, externals and ref types, and bundle blocks and directory
    /// entries. A count never sizes an allocation (plan section 3.3), so the
    /// bytes already bound the tables; this bounds what they cost in memory,
    /// since a record can be as small as one input byte (an external below
    /// format 5) and its parsed form is up to `size_of::<SerializedType>()`.
    /// Default 2^22.
    pub max_metadata_records: u64,
    /// Typetree rows one input stores ([`Metered::TreeRows`]): every row of
    /// every type and ref-type tree of every file, counted by the parser
    /// before the row is kept. A blob row is 24 input bytes and an LZMA block
    /// compresses repeated rows about 7,000:1, so without this bound a small
    /// input could make the parser build hundreds of millions of rows.
    /// Default 2^22.
    pub max_tree_rows: u64,
    /// Depth of a compiled typetree. Default 256.
    pub max_tree_depth: u32,
    /// Runtime nesting while reading a value, `ReferencedObject` data
    /// included (UnityPyBoost recurses on the C stack, unbounded). Default 512.
    pub max_value_depth: u32,
    /// Values built by one object read, each element of a primitive run
    /// included. A value costs at most 48 bytes (a dict entry; a list slot is
    /// 40, a run element at most 8), so the default 2^26 holds one object's
    /// value to 3 GiB (plan section 3.15).
    pub max_values_per_object: u64,
    /// Array elements whose minimum size is 0 (a vector of empty structs with
    /// a huge count does no I/O per element). Default 2^20 per object.
    pub max_zero_size_elements: u64,
    /// Elements reserved before the bytes prove they exist. Default 2^16.
    pub max_prealloc: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_block_output: 1 << 30,
            max_bundle_uncompressed: 4 << 30,
            max_inflate: 4 << 30,
            max_metadata_records: 1 << 22,
            max_tree_rows: 1 << 22,
            max_tree_depth: 256,
            max_value_depth: 512,
            max_values_per_object: 1 << 26,
            max_zero_size_elements: 1 << 20,
            max_prealloc: 1 << 16,
        }
    }
}

/// What a [`Meter`] counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Metered {
    /// Against [`Limits::max_metadata_records`].
    MetadataRecords,
    /// Against [`Limits::max_tree_rows`].
    TreeRows,
    /// Against [`Limits::max_bundle_uncompressed`]: decoded bytes, each block
    /// at its actual length, once.
    DecodedBytes,
}

impl Metered {
    /// The limit kind a refusal reports.
    #[must_use]
    pub fn kind(self) -> LimitKind {
        match self {
            Self::MetadataRecords => LimitKind::MetadataRecords,
            Self::TreeRows => LimitKind::TreeRows,
            Self::DecodedBytes => LimitKind::BundleUncompressed,
        }
    }

    /// The bound in `limits`.
    #[must_use]
    pub fn max(self, limits: &Limits) -> u64 {
        match self {
            Self::MetadataRecords => limits.max_metadata_records,
            Self::TreeRows => limits.max_tree_rows,
            Self::DecodedBytes => limits.max_bundle_uncompressed,
        }
    }
}

/// What one input has cost so far, against its [`Limits`] (module docs). A
/// parser charges *before* it keeps what it charges for, so a refused charge
/// allocates nothing. Thread-safe: a lazily read bundle's entries charge one
/// meter from any thread.
#[derive(Debug, Default)]
pub struct Meter {
    metadata_records: AtomicU64,
    tree_rows: AtomicU64,
    decoded_bytes: AtomicU64,
}

impl Meter {
    /// A meter at zero.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn counter(&self, what: Metered) -> &AtomicU64 {
        match what {
            Metered::MetadataRecords => &self.metadata_records,
            Metered::TreeRows => &self.tree_rows,
            Metered::DecodedBytes => &self.decoded_bytes,
        }
    }

    /// Adds `n`, or refuses (and adds nothing) when the total would pass the bound.
    pub fn charge(&self, what: Metered, n: u64, limits: &Limits) -> Result<(), LimitError> {
        let max = what.max(limits);
        self.counter(what)
            .try_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(n).filter(|t| *t <= max)
            })
            .map(|_| ())
            .map_err(|used| LimitError {
                kind: what.kind(),
                value: used.saturating_add(n),
                max,
            })
    }

    /// Gives back `n` charged and not kept (a decode buffer cut after it grew).
    pub fn refund(&self, what: Metered, n: u64) {
        // Saturating: a refund never takes the count below zero.
        let _ = self
            .counter(what)
            .try_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                Some(used.saturating_sub(n))
            });
    }

    /// The total charged so far.
    #[must_use]
    pub fn used(&self, what: Metered) -> u64 {
        self.counter(what).load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::{Limits, Meter, Metered};
    use crate::error::LimitKind;

    #[test]
    fn every_metered_bound_trips_and_keeps_nothing() {
        let limits = Limits {
            max_metadata_records: 10,
            max_tree_rows: 20,
            max_bundle_uncompressed: 30,
            ..Limits::default()
        };
        let m = Meter::new();
        for (what, max, kind) in [
            (Metered::MetadataRecords, 10, LimitKind::MetadataRecords),
            (Metered::TreeRows, 20, LimitKind::TreeRows),
            (Metered::DecodedBytes, 30, LimitKind::BundleUncompressed),
        ] {
            m.charge(what, max - 1, &limits).unwrap();
            m.charge(what, 1, &limits).unwrap();
            let e = m.charge(what, 1, &limits).unwrap_err();
            assert_eq!((e.kind, e.value, e.max), (kind, max + 1, max));
            assert_eq!(m.used(what), max, "a refused charge adds nothing");
            m.refund(what, 5);
            assert_eq!(m.used(what), max - 5);
            m.refund(what, u64::MAX);
            assert_eq!(m.used(what), 0);
        }
    }

    #[test]
    fn the_row_bound_defaults_below_the_memory_model() {
        let l = Limits::default();
        assert_eq!(l.max_tree_rows, 1 << 22);
        assert_eq!(l.max_metadata_records, 1 << 22);
    }
}
