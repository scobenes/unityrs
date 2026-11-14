// Ported from Arm astcenc commit 631fb04 (Apache-2.0): Source/astcenc_partition_tables.cpp:114-263,283-302, hash52 and select_partition, and row-major assignments, which every seed's partitioning keeps for decoding (astcenc_partition_tables.cpp:382-457). See THIRD_PARTY_LICENSES.
// Copyright 2011-2023 Arm Limited.
use std::sync::Arc;

use unityrs_core::limits::{ChargeError, Meter, MeteredVec, heap_cost};

/// The hash's overflow is intentional 32-bit unsigned arithmetic.
/// astcenc_partition_tables.cpp:114-128.
pub(super) fn hash52(mut value: u32) -> u32 {
    value ^= value >> 15;
    value = value.wrapping_mul(0xeede_0891);
    value ^= value >> 5;
    value = value.wrapping_add(value << 16);
    value ^= value >> 7;
    value ^= value >> 3;
    value ^= value << 6;
    value ^= value >> 17;
    value
}

/// 2D scalar specialization of `select_partition` (z = 0). Inputs are
/// checked before any shifts or products; small blocks double coordinates.
/// astcenc_partition_tables.cpp:142-263. Ties prefer the lowest partition.
pub(super) fn select(seed: u16, count: u8, x: u8, y: u8, small: bool) -> Option<u8> {
    if seed >= 1024 || !(1..=4).contains(&count) || x >= 12 || y >= 12 {
        return None;
    }
    let seed = u32::from(seed) + u32::from(count - 1) * 1024;
    let random = hash52(seed);
    let sh1 = if seed & 1 != 0 {
        if seed & 2 != 0 { 4 } else { 5 }
    } else if count == 3 {
        6
    } else {
        5
    };
    let sh2 = if seed & 1 == 0 {
        if seed & 2 != 0 { 4 } else { 5 }
    } else if count == 3 {
        6
    } else {
        5
    };
    let scale = if small { 2 } else { 1 };
    let x = u32::from(x) * scale;
    let y = u32::from(y) * scale;
    let mut best_score = 0;
    let mut best_partition = 0;
    for partition in 0..count {
        let shift = u32::from(partition) * 8;
        let sx = (random >> shift) & 15;
        let sy = (random >> (shift + 4)) & 15;
        let bias_shift = 14 - 4 * u32::from(partition);
        let score = (((sx * sx) >> sh1) * x + ((sy * sy) >> sh2) * y + (random >> bias_shift)) & 63;
        if score > best_score {
            best_score = score;
            best_partition = partition;
        }
    }
    Some(best_partition)
}

/// Only the per-texel assignments needed by decoding, for partition counts
/// 2..4 and seeds 0..1023, each row row-major by texel. A single partition
/// always maps to zero and occupies no table storage. The whole table's
/// storage is charged and allocated up front; a seed's row is computed and
/// appended the first time a block names it, so a decode of a few blocks
/// does not pay for 3,072 rows it never reads.
pub(super) struct PartitionTable {
    width: u8,
    height: u8,
    data: MeteredVec<u8>,
    /// Each row's place in `data`, in rows, by `(count - 2) * 1024 + seed`;
    /// [`UNBUILT`] until the row is computed.
    at: [u16; ROWS],
}
const _: () = crate::send_sync::<PartitionTable>();

/// The rows a table holds: 3 partition counts x 1,024 seeds.
const ROWS: usize = 3 * 1024;

/// A row not yet computed.
const UNBUILT: u16 = u16::MAX;

impl PartitionTable {
    /// Bound storage to the 4..12 sides in the ASTC specification; the
    /// supported footprints are checked by `context::Footprint`.
    pub(super) fn charge_bytes(width: u8, height: u8) -> Option<u64> {
        if !(4..=12).contains(&width) || !(4..=12).contains(&height) {
            return None;
        }
        Some(heap_cost(3 * 1024 * u64::from(width) * u64::from(height)))
    }

    /// Invalid dimensions return Ok(None); allocation or meter refusals
    /// remain `ChargeError`, with every charge released before returning.
    #[cfg(test)]
    pub(super) fn new(
        width: u8,
        height: u8,
        meter: &Arc<Meter>,
    ) -> Result<Option<Self>, ChargeError> {
        if Self::charge_bytes(width, height).is_none() {
            return Ok(None);
        }
        Self::alloc(width, height, meter).map(Some)
    }

    /// [`Self::new`] for sides the caller has checked (a `Footprint`'s).
    pub(super) fn alloc(width: u8, height: u8, meter: &Arc<Meter>) -> Result<Self, ChargeError> {
        let len = ROWS * usize::from(width) * usize::from(height);
        Ok(Self {
            width,
            height,
            data: MeteredVec::with_capacity(meter, len)?,
            at: [UNBUILT; ROWS],
        })
    }

    /// The row-major partition of every texel for `count` partitions (2..4)
    /// and `seed`, computed on first use.
    pub(super) fn row(&mut self, seed: u16, count: u8) -> Option<&[u8]> {
        if seed >= 1024 || !(2..=4).contains(&count) {
            return None;
        }
        let texels = usize::from(self.width) * usize::from(self.height);
        let at = self
            .at
            .get_mut(usize::from(count - 2) * 1024 + usize::from(seed))?;
        if *at == UNBUILT {
            let small = texels < 32;
            let width = usize::from(self.width);
            let mut row = [0; 144];
            for (i, part) in row.iter_mut().take(texels).enumerate() {
                let (x, y) = (u8::try_from(i % width).ok()?, u8::try_from(i / width).ok()?);
                *part = select(seed, count, x, y, small)?;
            }
            *at = u16::try_from(self.data.len() / texels).ok()?;
            // Within the capacity charged up front: no allocation.
            self.data.extend_from_slice(row.get(..texels)?).ok()?;
        }
        let start = usize::from(*at) * texels;
        self.data.as_slice().get(start..start + texels)
    }

    #[cfg(test)]
    pub(super) fn get(&mut self, seed: u16, count: u8, x: u8, y: u8) -> Option<u8> {
        if seed >= 1024 || !(1..=4).contains(&count) || x >= self.width || y >= self.height {
            return None;
        }
        if count == 1 {
            return Some(0);
        }
        let at = usize::from(y) * usize::from(self.width) + usize::from(x);
        self.row(seed, count)?.get(at).copied()
    }
}
