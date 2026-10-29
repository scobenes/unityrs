//! Hard bounds on what untrusted input can make the library allocate or do.
//!
//! UnityPy has none of these: where it would exhaust memory or the C stack,
//! unityrs stops with [`crate::Error::Limit`] (or the walker's `other` kind) and
//! `PARITY.md` lists the case as a safety divergence (plan section 3.15).
//!
//! Owner: the scaffold (implemented). Packages read these fields; a new
//! limit is added only through the integration package.

/// Bounds on allocation, recursion and work per input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Limits {
    /// One block's actual decoded output (blocks-info's included). LZMA
    /// decodes to the end of its input whatever blocks-info says
    /// (`helpers/CompressionHelper.py:17-44`), so a bomb is stopped here.
    /// Default 1 GiB.
    pub max_block_output: u64,
    /// The decoded data of one bundle, all blocks and blocks-info together:
    /// the declared total is checked at open, and the running *actual* total
    /// as blocks decode, since an LZMA block can outgrow its declaration
    /// (plan section 3.6). Default 4 GiB, so one input stays well inside a
    /// 12 GiB process (plan section 3.15).
    pub max_bundle_uncompressed: u64,
    /// One gzip stream inflated by detection (rule 7, streamed into a scratch
    /// buffer and never held) or a `WebFile`. Default 4 GiB.
    pub max_inflate: u64,
    /// Records in one file's metadata tables, all tables together: a
    /// `SerializedFile`'s types, objects, script types, externals and ref
    /// types, or a bundle's blocks and directory entries. A count never sizes
    /// an allocation (plan section 3.3), so the bytes already bound the
    /// tables; this bounds what they cost in memory, since a record can be as
    /// small as one input byte (an external below format 5) and its parsed
    /// form tens of bytes. Default 2^24.
    pub max_metadata_records: u64,
    /// Depth of a compiled typetree. Default 256.
    pub max_tree_depth: u32,
    /// Runtime nesting while reading a value, `ReferencedObject` data
    /// included (UnityPyBoost recurses on the C stack, unbounded). Default 512.
    pub max_value_depth: u32,
    /// Values built by one object read. A dict entry costs about 48 bytes,
    /// so the default 2^26 holds one object's value to about 3 GiB.
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
            max_metadata_records: 1 << 24,
            max_tree_depth: 256,
            max_value_depth: 512,
            max_values_per_object: 1 << 26,
            max_zero_size_elements: 1 << 20,
            max_prealloc: 1 << 16,
        }
    }
}
