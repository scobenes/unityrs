// Ported from Arm astcenc commit 631fb04 (Apache-2.0): Source/astcenc_percentile_tables.cpp:1201-1226, the legal 2D block sizes; astcenc_block_sizes.cpp:792-949, the block-mode and decimation tables of a block size descriptor, for a context that keeps every legal mode (astcenc_entry.cpp:708-710). See THIRD_PARTY_LICENSES.
// Copyright 2011-2024 Arm Limited.
//! The tables one decode builds for its footprint, as astcenc's block size
//! descriptor holds them for a context: which of the 2,048 block modes are
//! legal and what they decode to, the decimation entries of each weight
//! grid a legal mode names, and the partition assignments.
//!
//! **Which modes.** UnityPy's context sets no `SELF_DECOMPRESS_ONLY` flag,
//! so astcenc may not omit modes (astcenc_entry.cpp:708-710) and its
//! descriptor keeps every mode `decode_block_mode_2d` accepts whose grid
//! fits the footprint (astcenc_block_sizes.cpp:845-868); the further
//! "physically encodable" skip (astcenc_block_sizes.cpp:876-893) never
//! fires, a weight field being at most 96 bits. Every other mode is
//! `BLOCK_BAD_BLOCK_MODE`, an error block (astcenc_symbolic_physical.cpp:372-377).
//!
//! **Memory.** Each table is charged to the decode's meter before it is
//! allocated (`MeteredVec`, fallible), and [`Tables::charge`] gives the sum
//! before anything is read, for `plan()`. The decimation and partition rows
//! are computed the first time a block names them, so a decode costs work
//! in proportion to what its blocks use; their storage is charged whole.

use std::sync::Arc;

use unityrs_core::limits::{ChargeError, Meter, MeteredVec, heap_cost};

use super::block_mode::{self, BlockMode, WeightMode};
use super::decimation::{self, ENTRY};
use super::partition::PartitionTable;

/// astcenc's legal 2D block sizes, width by height
/// (astcenc_percentile_tables.cpp:1201-1226), each with the number of weight
/// grids its legal block modes name (`grid_counts` in the tests counts
/// them from the block-mode table).
pub(crate) const FOOTPRINTS: [(u8, u8, u8); 14] = [
    (4, 4, 9),
    (5, 4, 12),
    (5, 5, 16),
    (6, 5, 20),
    (6, 6, 25),
    (8, 5, 28),
    (8, 6, 35),
    (8, 8, 49),
    (10, 5, 36),
    (10, 6, 45),
    (10, 8, 60),
    (10, 10, 71),
    (12, 10, 79),
    (12, 12, 87),
];

/// One of [`FOOTPRINTS`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Footprint {
    width: u8,
    height: u8,
    grids: u8,
}
const _: () = crate::send_sync::<Footprint>();

impl Footprint {
    /// The footprint `width` x `height`, if astcenc takes it.
    pub(crate) fn new(width: u8, height: u8) -> Option<Self> {
        FOOTPRINTS
            .iter()
            .find(|f| (f.0, f.1) == (width, height))
            .map(|&(width, height, grids)| Self {
                width,
                height,
                grids,
            })
    }

    pub(crate) fn width(self) -> u8 {
        self.width
    }

    pub(crate) fn height(self) -> u8 {
        self.height
    }

    pub(crate) fn texels(self) -> usize {
        usize::from(self.width) * usize::from(self.height)
    }

    /// The decimation table's bytes: [`ENTRY`] a texel a grid.
    fn grid_bytes(self) -> usize {
        usize::from(self.grids) * self.texels() * ENTRY
    }
}

/// The grids a block mode can name, by `(y - 2) * 11 + (x - 2)`.
const GRIDS: usize = 11 * 11;

fn grid_slot(x: u8, y: u8) -> Option<usize> {
    let (x, y) = (
        usize::from(x.checked_sub(2)?),
        usize::from(y.checked_sub(2)?),
    );
    (x < 11 && y < 11).then_some(y * 11 + x)
}

/// A table entry not yet computed.
const UNBUILT: u16 = u16::MAX;

/// A block mode's entry: not yet looked up, or its weights (`None` for an
/// error block).
type ModeEntry = Option<Option<WeightMode>>;

/// The per-decode tables of one footprint, each row computed the first
/// time a block names it.
pub(crate) struct Tables {
    fp: Footprint,
    /// By block mode (11 bits).
    modes: MeteredVec<ModeEntry>,
    /// [`ENTRY`] bytes a texel, a grid's texels in row-major order, the
    /// grids in the order blocks first name them.
    grids: MeteredVec<u8>,
    /// Each grid's place in `grids`, in grids, by [`grid_slot`];
    /// [`UNBUILT`] until it is computed.
    grid_at: [u16; GRIDS],
    partitions: PartitionTable,
}
const _: () = crate::send_sync::<Tables>();

impl Tables {
    /// What [`Self::new`] charges for `fp`: the [`heap_cost`] of each table.
    pub(crate) fn charge(fp: Footprint) -> u64 {
        let modes = heap_cost(2048 * size_of::<ModeEntry>() as u64);
        let grids = heap_cost(u64::try_from(fp.grid_bytes()).unwrap_or(u64::MAX));
        let partitions = PartitionTable::charge_bytes(fp.width, fp.height).unwrap_or(0);
        modes + grids + partitions
    }

    /// The tables, each charged to `meter` before it is allocated.
    pub(crate) fn new(fp: Footprint, meter: &Arc<Meter>) -> Result<Self, ChargeError> {
        let mut modes = MeteredVec::with_capacity(meter, 2048)?;
        for _ in 0..8 {
            modes.extend_from_slice(&[None; 256])?;
        }
        let grids = MeteredVec::with_capacity(meter, fp.grid_bytes())?;
        // Every footprint is within the partition table's 4..12 sides.
        let partitions = PartitionTable::alloc(fp.width, fp.height, meter)?;
        Ok(Self {
            fp,
            modes,
            grids,
            grid_at: [UNBUILT; GRIDS],
            partitions,
        })
    }

    pub(crate) fn footprint(&self) -> Footprint {
        self.fp
    }

    /// A block mode's weights, or `None` where astcenc's descriptor has
    /// `BLOCK_BAD_BLOCK_MODE`.
    pub(super) fn mode(&mut self, mode: u16) -> Option<WeightMode> {
        let entry = self.modes.as_mut_slice().get_mut(usize::from(mode))?;
        if let Some(weights) = *entry {
            return weights;
        }
        let weights = match block_mode::for_footprint(mode, self.fp.width, self.fp.height) {
            Some(BlockMode::Weights(w)) => Some(w),
            _ => None,
        };
        *entry = Some(weights);
        weights
    }

    /// The decimation entries of grid `x` x `y` and, for `partition`'s
    /// (seed, count of 2 to 4), the partition of every texel (empty for one
    /// partition), each row computed on first use.
    pub(super) fn rows(
        &mut self,
        (x, y): (u8, u8),
        partition: Option<(u16, u8)>,
    ) -> Option<(&[u8], &[u8])> {
        let Self {
            fp,
            grids,
            grid_at,
            partitions,
            ..
        } = self;
        let len = fp.texels() * ENTRY;
        let at = grid_at.get_mut(grid_slot(x, y)?)?;
        if *at == UNBUILT {
            let mut grid = [0; 144 * ENTRY];
            let w = usize::from(fp.width);
            let entries = grid.as_chunks_mut::<ENTRY>().0.iter_mut();
            for (i, out) in entries.take(fp.texels()).enumerate() {
                let (tx, ty) = (u8::try_from(i % w).ok()?, u8::try_from(i / w).ok()?);
                *out = decimation::entry((fp.width, fp.height), (x, y), tx, ty)?;
            }
            *at = u16::try_from(grids.len() / len).ok()?;
            // Within the capacity charged up front: a legal mode's grid is
            // one of the footprint's `grids`.
            grids.extend_from_slice(grid.get(..len)?).ok()?;
        }
        let start = usize::from(*at) * len;
        let grid = grids.as_slice().get(start..start + len)?;
        let parts = match partition {
            Some((seed, count)) => partitions.row(seed, count)?,
            None => &[],
        };
        Some((grid, parts))
    }
}

#[cfg(test)]
mod tests {
    use super::super::block_mode::{self, BlockMode};
    use super::{FOOTPRINTS, Footprint, GRIDS, Tables, grid_slot};
    use unityrs_core::limits::Meter;

    /// The distinct grids the legal modes of each footprint name.
    fn grid_counts() -> Vec<(u8, u8, u8)> {
        FOOTPRINTS
            .iter()
            .map(|&(w, h, _)| {
                let mut used = [false; GRIDS];
                for mode in 0..2048 {
                    if let Some(BlockMode::Weights(g)) = block_mode::for_footprint(mode, w, h) {
                        used[grid_slot(g.x, g.y).unwrap()] = true;
                    }
                }
                (
                    w,
                    h,
                    u8::try_from(used.iter().filter(|u| **u).count()).unwrap(),
                )
            })
            .collect()
    }

    /// The tables charge what [`Tables::charge`] says, and return it; every
    /// grid a legal mode names fits the decimation table without growing
    /// it. The sizes are the ones `docs/plans/rest.md` section 2.1 records.
    #[test]
    fn the_tables_charge_what_the_plan_says() {
        assert_eq!(grid_counts(), FOOTPRINTS);
        let meter = Meter::unlimited();
        let mut sizes = Vec::new();
        for (w, h, _) in FOOTPRINTS {
            let fp = Footprint::new(w, h).unwrap();
            let mut tables = Tables::new(fp, &meter).unwrap();
            assert_eq!(meter.used(), Tables::charge(fp), "{w}x{h}");
            let capacity = tables.grids.capacity();
            for mode in 0..2048 {
                if let Some(g) = tables.mode(mode) {
                    assert!(tables.rows((g.x, g.y), Some((mode % 1024, 2))).is_some());
                }
            }
            assert_eq!(tables.grids.len(), capacity, "{w}x{h}");
            assert_eq!(meter.used(), Tables::charge(fp), "{w}x{h}");
            sizes.push((w, h, capacity, Tables::charge(fp)));
            drop(tables);
            assert_eq!(meter.used(), 0);
        }
        // The decimation tables' bytes per footprint (8 a texel a grid),
        // and all the tables' charge.
        assert_eq!(
            sizes,
            [
                (4, 4, 1_152, 60_592),
                (5, 4, 1_920, 73_648),
                (5, 5, 3_200, 90_288),
                (6, 5, 4_800, 107_248),
                (6, 6, 7_200, 128_080),
                (8, 5, 8_960, 142_128),
                (8, 6, 13_440, 175_264),
                (8, 8, 25_088, 236_064),
                (10, 5, 14_400, 180_320),
                (10, 6, 21_600, 220_288),
                (10, 8, 38_400, 298_528),
                (10, 10, 56_800, 378_368),
                (12, 10, 75_840, 458_848),
                (12, 12, 100_224, 556_960),
            ]
        );
        assert_eq!(Footprint::new(4, 6), None);
        assert_eq!(Footprint::new(13, 13), None);
    }
}
