// Ported from Arm astcenc commit 631fb04 (Apache-2.0): Source/astcenc_block_sizes.cpp:36-137, 2D block modes; astcenc_symbolic_physical.cpp:300-304, void-extent marker. See THIRD_PARTY_LICENSES.
// Copyright 2011-2023 Arm Limited.
use super::ise::Quant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct WeightMode {
    pub(super) x: u8,
    pub(super) y: u8,
    pub(super) dual_plane: bool,
    pub(super) quant: Quant,
    pub(super) bits: u8,
}
const _: () = crate::send_sync::<WeightMode>();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BlockMode {
    Weights(WeightMode),
    VoidExtent,
}
const _: () = crate::send_sync::<BlockMode>();

/// `None` is an illegal/reserved block mode, which the image decoder must
/// render as astcenc's error colour. This only recognizes void extent; it
/// does not validate its reserved bits or extent payload (T2b).
pub(super) fn decode(mode: u16) -> Option<BlockMode> {
    if mode > 0x7ff {
        return None;
    }
    if mode & 0x1ff == 0x1fc {
        return Some(BlockMode::VoidExtent);
    }
    let mut base = (mode >> 4) & 1;
    let mut high = (mode >> 9) & 1;
    let mut dual = (mode >> 10) & 1;
    let a = (mode >> 5) & 3;
    let (x, y) = if mode & 3 != 0 {
        base |= (mode & 3) << 1;
        let b = (mode >> 7) & 3;
        match (mode >> 2) & 3 {
            0 => (b + 4, a + 2),
            1 => (b + 8, a + 2),
            2 => (a + 2, b + 8),
            _ if mode & 0x100 != 0 => ((b & 1) + 2, a + 2),
            _ => (a + 2, (b & 1) + 6),
        }
    } else {
        base |= ((mode >> 2) & 3) << 1;
        if base < 2 {
            return None;
        }
        let b = (mode >> 9) & 3;
        match (mode >> 7) & 3 {
            0 => (12, a + 2),
            1 => (a + 2, 12),
            2 => {
                dual = 0;
                high = 0;
                (a + 6, b + 6)
            }
            _ => match a {
                0 => (6, 10),
                1 => (10, 6),
                _ => return None,
            },
        }
    };
    let quant = Quant::new(u8::try_from(base.checked_sub(2)? + 6 * high).ok()?)?;
    let count = usize::from(x * y * (dual + 1));
    let bits = quant.bit_count(count)?;
    // astcenc_internal.h:87-96: at most 64 weights, 24..96 stored bits.
    if count > 64 || !(24..=96).contains(&bits) {
        return None;
    }
    Some(BlockMode::Weights(WeightMode {
        x: u8::try_from(x).ok()?,
        y: u8::try_from(y).ok()?,
        dual_plane: dual != 0,
        quant,
        bits: u8::try_from(bits).ok()?,
    }))
}

/// A mode legal in the abstract can still have a grid larger than the
/// footprint: astcenc_block_sizes.cpp:864-868 excludes it from the descriptor.
pub(super) fn for_footprint(mode: u16, width: u8, height: u8) -> Option<BlockMode> {
    match decode(mode)? {
        BlockMode::Weights(w) if w.x > width || w.y > height => None,
        valid => Some(valid),
    }
}
