// Ported from Arm astcenc commit 631fb04 (Apache-2.0): decode-side foundations from Source/astcenc_block_sizes.cpp, astcenc_integer_sequence.cpp, astcenc_partition_tables.cpp, astcenc_symbolic_physical.cpp and astcenc_color_unquantize.cpp. See THIRD_PARTY_LICENSES.
// Copyright 2011-2024 Arm Limited.
//! ASTC 2D LDR foundations. Reference source: Arm astcenc at commit
//! `631fb04e1c4bca002a8b653f34e044dbb35b32b4` (between 4.8.0 and 5.0.0),
//! as bundled by astc-encoder-py 0.1.12.
//! <https://github.com/ARM-software/astc-encoder/tree/631fb04e1c4bca002a8b653f34e044dbb35b32b4/Source>.
//! All source citations in this module and its children refer to that commit.
//!
//! This module deliberately has no texture dispatch or image store. Invalid
//! modes return `None`; T2b must turn these into astcenc's error colour.
//! Void-extent markers are classified only; their payload belongs to T2b.
//! The pinned `astcenc_decompress_symbolic.cpp` handles structural error blocks
//! at :190-204, constant colours at :206-257, and texel interpolation at
//! :274-309; these operations are outside this module's foundations.
//!
//! Small decode maps are constants (see tables.rs and its reproducible
//! generator). The only heap storage here is a compact partition cache:
//! 3 * 1024 * width * height bytes, at most 442,368 bytes for 12x12.
//! `PartitionTable::charge_bytes` gives its charge before allocation.
//! No encoder-only partition coverage, canonicalization or SIMD padding is
//! needed by a scalar decoder. Decimation storage is deferred to T2b.

// T2b will consume these foundations; keeping them compiled now checks lints.
#![allow(dead_code)]

pub(super) mod block_mode;
pub(super) mod endpoint;
pub(super) mod ise;
pub(super) mod partition;
mod tables;

/// A bounded bit read, little endian within each byte. Unlike the C++ reader
/// (astcenc_integer_sequence.cpp:478-490), it never overreads the input.
pub(super) fn read_bits(data: &[u8], offset: usize, count: usize) -> Option<u32> {
    if count > 16 || offset.checked_add(count)? > data.len().checked_mul(8)? {
        return None;
    }
    let mut value = 0;
    for bit in 0..count {
        let pos = offset + bit;
        value |= u32::from((data.get(pos / 8)? >> (pos % 8)) & 1) << bit;
    }
    Some(value)
}

#[cfg(test)]
mod tests;
