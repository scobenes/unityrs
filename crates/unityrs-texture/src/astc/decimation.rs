// Ported from Arm astcenc commit 631fb04 (Apache-2.0): Source/astcenc_block_sizes.cpp:252-345, init_decimation_info_2d's bilinear infill weights; astcenc_decompress_symbolic.cpp:89-155, unpack_weights. See THIRD_PARTY_LICENSES.
// Copyright 2011-2024 Arm Limited.
//! Decimation: how a block's weight grid, smaller than or equal to its
//! footprint, is spread over the footprint's texels.
//!
//! astcenc gives each texel up to four grid weights and their contributions
//! (in sixteenths), the "truncated-precision bilinear interpolation" of
//! astcenc_block_sizes.cpp:286-307, keeping only the non-zero ones in
//! order (astcenc_block_sizes.cpp:309-321 and astcenc_block_sizes.cpp:331-344).
//! A texel's weight is then `(8 + sum(weight[index] * contribution)) >> 4`
//! (astcenc_decompress_symbolic.cpp:104-120, and for two planes
//! astcenc_decompress_symbolic.cpp:133-153). Skipping the zero contributions keeps every stored index
//! inside the grid, so no lookup reads past the decoded weights
//! (`every_stored_index_is_inside_its_grid`).

/// The bytes one texel's entry takes: four grid indices, then their four
/// contributions, the unused ones 0.
pub(super) const ENTRY: usize = 8;

/// The infill entry of texel (`x`, `y`) for a `grid_x` x `grid_y` weight
/// grid over a `width` x `height` footprint, or `None` for arguments no
/// footprint and grid of astcenc's have (a side below 2, a grid larger than
/// the footprint).
pub(super) fn entry(
    (width, height): (u8, u8),
    (grid_x, grid_y): (u8, u8),
    texel_x: u8,
    texel_y: u8,
) -> Option<[u8; ENTRY]> {
    let (x, y) = (texel_x, texel_y);
    if width < 2 || height < 2 || grid_x < 2 || grid_y < 2 || grid_x > width || grid_y > height {
        return None;
    }
    if x >= width || y >= height {
        return None;
    }
    let (fw, fh, gx, gy) = (
        u32::from(width),
        u32::from(height),
        u32::from(grid_x),
        u32::from(grid_y),
    );
    // astcenc_block_sizes.cpp:286-298: each axis in 1/16 of a grid step.
    let xw = (((1024 + fw / 2) / (fw - 1)) * u32::from(x) * (gx - 1) + 32) >> 6;
    let yw = (((1024 + fh / 2) / (fh - 1)) * u32::from(y) * (gy - 1) + 32) >> 6;
    let (xf, yf) = (xw & 15, yw & 15);
    let q0 = (xw >> 4) + (yw >> 4) * gx;
    let index = [q0, q0 + 1, q0 + gx, q0 + gx + 1];
    // astcenc_block_sizes.cpp:300-307.
    let w3 = (xf * yf + 8) >> 4;
    let weight = [
        (16 + w3).checked_sub(xf + yf)?,
        xf.checked_sub(w3)?,
        yf.checked_sub(w3)?,
        w3,
    ];
    let mut out = [0; ENTRY];
    let mut n = 0;
    for (&i, &c) in index.iter().zip(&weight) {
        if c != 0 {
            *out.get_mut(n)? = u8::try_from(i).ok()?;
            *out.get_mut(4 + n)? = u8::try_from(c).ok()?;
            n += 1;
        }
    }
    Some(out)
}

/// A texel's weight (0..64) from its entry and the grid's unquantised
/// weights: astcenc_decompress_symbolic.cpp:106-119, rounded by its initial
/// 8 and its `lsr<4>`.
#[inline]
pub(super) fn infill(entry: [u8; ENTRY], grid: &[u8; 64]) -> u8 {
    let [i0, i1, i2, i3, c0, c1, c2, c3] = entry;
    let at = |i: u8| u32::from(grid.get(usize::from(i)).copied().unwrap_or(0));
    let sum = 8
        + at(i0) * u32::from(c0)
        + at(i1) * u32::from(c1)
        + at(i2) * u32::from(c2)
        + at(i3) * u32::from(c3);
    // At most (8 + 64 * 16) >> 4 = 64.
    u8::try_from(sum >> 4).unwrap_or(64)
}

#[cfg(test)]
mod tests {
    use super::{ENTRY, entry, infill};

    const FOOTPRINTS: [(u8, u8, u8); 14] = super::super::context::FOOTPRINTS;

    /// Contributions sum to 16, and an index with a non-zero contribution
    /// lies inside its grid, for every footprint and grid astcenc builds.
    #[test]
    fn every_stored_index_is_inside_its_grid() {
        for (w, h, _) in FOOTPRINTS {
            for gx in 2..=w {
                for gy in 2..=h {
                    for y in 0..h {
                        for x in 0..w {
                            let e = entry((w, h), (gx, gy), x, y).unwrap();
                            let sum: u32 = e[4..].iter().map(|&c| u32::from(c)).sum();
                            assert_eq!(sum, 16, "{w}x{h} grid {gx}x{gy} at {x},{y}");
                            for i in 0..4 {
                                if e[4 + i] != 0 {
                                    assert!(e[i] < gx * gy, "{w}x{h} grid {gx}x{gy} at {x},{y}");
                                }
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(entry((4, 4), (5, 4), 0, 0), None);
        assert_eq!(entry((4, 4), (4, 4), 4, 0), None);
        assert_eq!(entry((1, 4), (1, 4), 0, 0), None);
    }

    /// A full-size grid maps each texel to its own weight, and a 2x2 grid
    /// over 5x5 puts the centre texel at an equal mix of all four, as the
    /// Khronos Data Format Specification's infill procedure does.
    #[test]
    fn identity_and_bilinear_infill() {
        for (w, h, _) in FOOTPRINTS {
            for y in 0..h {
                for x in 0..w {
                    let e = entry((w, h), (w, h), x, y).unwrap();
                    assert_eq!(e, [y * w + x, 0, 0, 0, 16, 0, 0, 0], "{w}x{h} at {x},{y}");
                }
            }
        }
        assert_eq!(entry((5, 5), (2, 2), 2, 2), Some([0, 1, 2, 3, 4, 4, 4, 4]));
        let mut grid = [0; 64];
        grid[..4].copy_from_slice(&[0, 64, 64, 0]);
        assert_eq!(infill(entry((5, 5), (2, 2), 2, 2).unwrap(), &grid), 32);
        assert_eq!(infill([0; ENTRY], &grid), 0);
        assert_eq!(infill([1, 0, 0, 0, 16, 0, 0, 0], &grid), 64);
    }
}
