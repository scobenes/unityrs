// Ported from Arm astcenc commit 631fb04 (Apache-2.0): Source/astcenc_partition_tables.cpp:114-263,283-302, hash52 and select_partition, and row-major assignments. See THIRD_PARTY_LICENSES.
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

/// Only the per-texel assignments needed by decoding, indexed by partition
/// count (2..4), seed (0..1023), then row-major texel. A single partition
/// always maps to zero and occupies no table storage.
pub(super) struct PartitionTable {
    width: u8,
    height: u8,
    data: MeteredVec<u8>,
}
const _: () = crate::send_sync::<PartitionTable>();

impl PartitionTable {
    /// Bound storage to the 4..12 sides in the ASTC specification; the actual
    /// supported footprint list will be checked by T2b's format selection.
    pub(super) fn charge_bytes(width: u8, height: u8) -> Option<u64> {
        if !(4..=12).contains(&width) || !(4..=12).contains(&height) {
            return None;
        }
        Some(heap_cost(3 * 1024 * u64::from(width) * u64::from(height)))
    }

    /// Invalid dimensions return Ok(None); allocation or meter refusals
    /// remain `ChargeError`, with every charge released before returning.
    pub(super) fn new(
        width: u8,
        height: u8,
        meter: &Arc<Meter>,
    ) -> Result<Option<Self>, ChargeError> {
        if Self::charge_bytes(width, height).is_none() {
            return Ok(None);
        }
        let texels = usize::from(width) * usize::from(height);
        let mut data = MeteredVec::with_capacity(meter, 3 * 1024 * texels)?;
        for count in 2..=4 {
            for seed in 0..1024 {
                for y in 0..height {
                    for x in 0..width {
                        // All select's input bounds were checked above.
                        data.push(select(seed, count, x, y, texels < 32).unwrap_or(0))?;
                    }
                }
            }
        }
        Ok(Some(Self {
            width,
            height,
            data,
        }))
    }

    pub(super) fn get(&self, seed: u16, count: u8, x: u8, y: u8) -> Option<u8> {
        if seed >= 1024 || !(1..=4).contains(&count) || x >= self.width || y >= self.height {
            return None;
        }
        if count == 1 {
            return Some(0);
        }
        let texels = usize::from(self.width) * usize::from(self.height);
        let table = usize::from(count - 2) * 1024 + usize::from(seed);
        self.data
            .as_slice()
            .get(table * texels + usize::from(y) * usize::from(self.width) + usize::from(x))
            .copied()
    }
}
