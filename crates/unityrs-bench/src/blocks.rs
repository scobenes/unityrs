//! Texture data, legal by construction (plan section 2).
//!
//! - DXT, BC4, BC5, EAC, ETC2, PVRTC and ATC decode any bit pattern, so
//!   their blocks, and the raw formats' pixels, are seeded bytes.
//! - ETC1 (`ETC_RGB4`): seeded blocks, but a differential block's base plus
//!   its delta stays within 0..31 in each channel ([`etc1_block`]); outside
//!   it the specification leaves the colour undefined.
//! - BC7: seeded blocks whose mode byte names mode 1, 5 or 6, 40%, 30% and
//!   30% of the blocks in a fixed cycle ([`BC7_CYCLE`]); never the reserved
//!   mode.
//! - BC6H: seeded blocks whose mode bits are one of the 14 legal modes
//!   ([`BC6H_MODES`]).
//! - ASTC: the tiles of [`crate::astc`].

use unityrs_testkit::Rng;

/// How a format's data is made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Uncompressed pixels of this many bytes, seeded.
    Raw(usize),
    /// 4x4 blocks of this many bytes, seeded.
    Seeded(usize),
    /// ETC1 blocks with legal differential endpoints.
    Etc1,
    /// BC7 blocks of modes 1, 5 and 6.
    Bc7,
    /// BC6H blocks of legal modes.
    Bc6h,
    /// ASTC blocks of this footprint, tiled from the oracle-checked pool.
    Astc(u8, u8),
}

impl Kind {
    /// The block's sides in pixels (1 x 1 for raw pixels).
    #[must_use]
    pub fn block(self) -> (usize, usize) {
        match self {
            Self::Raw(_) => (1, 1),
            Self::Astc(w, h) => (usize::from(w), usize::from(h)),
            _ => (4, 4),
        }
    }

    /// The bytes of one block (or pixel).
    #[must_use]
    pub fn block_bytes(self) -> usize {
        match self {
            Self::Raw(n) | Self::Seeded(n) => n,
            Self::Etc1 => 8,
            Self::Bc7 | Self::Bc6h | Self::Astc(..) => 16,
        }
    }

    /// How many blocks a `width` x `height` image holds.
    #[must_use]
    pub fn blocks(self, width: usize, height: usize) -> usize {
        let (bw, bh) = self.block();
        width.div_ceil(bw) * height.div_ceil(bh)
    }
}

/// BC7's modes, block by block, cycled: four of mode 1, three each of 5
/// and 6.
pub const BC7_CYCLE: [u8; 10] = [1, 5, 6, 1, 5, 6, 1, 5, 6, 1];

/// BC6H's legal modes as (bits, value) of the block's low bits: the two
/// 2-bit modes and the twelve 5-bit ones (the four whose low bits are
/// `0b11` and top bit set are reserved).
pub const BC6H_MODES: [(u8, u8); 14] = [
    (2, 0b00),
    (2, 0b01),
    (5, 0b00010),
    (5, 0b00110),
    (5, 0b01010),
    (5, 0b01110),
    (5, 0b10010),
    (5, 0b10110),
    (5, 0b11010),
    (5, 0b11110),
    (5, 0b00011),
    (5, 0b00111),
    (5, 0b01011),
    (5, 0b01111),
];

fn seeded<const N: usize>(rng: &mut Rng) -> [u8; N] {
    let mut out = [0; N];
    for chunk in out.chunks_mut(8) {
        let bytes = rng.next_u64().to_le_bytes();
        chunk.copy_from_slice(bytes.get(..chunk.len()).unwrap_or_default());
    }
    out
}

/// A 3-bit two's-complement delta.
fn signed3(v: u8) -> i16 {
    let v = i16::from(v & 7);
    if v >= 4 { v - 8 } else { v }
}

/// An ETC1 block from `rng`: a differential block (byte 3's bit 1) gets
/// each channel's delta moved so base plus delta stays within 0..31.
#[must_use]
pub fn etc1_block(rng: &mut Rng) -> [u8; 8] {
    let mut b: [u8; 8] = seeded(rng);
    if b[3] & 2 != 0 {
        for c in b.iter_mut().take(3) {
            let base = i16::from(*c >> 3);
            let sum = (base + signed3(*c)).clamp(0, 31);
            let delta = u8::try_from((sum - base) & 7).unwrap_or(0);
            *c = (*c & 0xf8) | delta;
        }
    }
    b
}

/// Whether an ETC1 block is individual, or differential with every
/// channel's base plus delta within 0..31.
#[must_use]
pub fn etc1_legal(b: &[u8; 8]) -> bool {
    b[3] & 2 == 0
        || b.iter()
            .take(3)
            .all(|c| (0..=31).contains(&(i16::from(c >> 3) + signed3(*c))))
}

/// BC7 block `i` from `rng`: mode [`BC7_CYCLE`]`[i % 10]`.
#[must_use]
pub fn bc7_block(i: usize, rng: &mut Rng) -> [u8; 16] {
    let mut b: [u8; 16] = seeded(rng);
    let mode = BC7_CYCLE.get(i % BC7_CYCLE.len()).copied().unwrap_or(6);
    let low = (1u8 << (mode + 1)) - 1;
    b[0] = (b[0] & !low) | (1 << mode);
    b
}

/// A BC7 block's mode (the lowest set bit of its first byte), `None` for
/// the reserved mode (a zero byte).
#[must_use]
pub fn bc7_mode(b: &[u8; 16]) -> Option<u8> {
    let z = b[0].trailing_zeros();
    u8::try_from(z).ok().filter(|m| *m < 8)
}

/// A BC6H block from `rng` with one of [`BC6H_MODES`].
#[must_use]
pub fn bc6h_block(rng: &mut Rng) -> [u8; 16] {
    let mut b: [u8; 16] = seeded(rng);
    let (bits, value) = BC6H_MODES
        .get(rng.below(BC6H_MODES.len()))
        .copied()
        .unwrap_or((2, 0));
    let low = (1u8 << bits) - 1;
    b[0] = (b[0] & !low) | value;
    b
}

/// The data of one `width` x `height` image of `kind`, from `rng`.
#[must_use]
pub fn data(kind: Kind, width: usize, height: usize, rng: &mut Rng) -> Vec<u8> {
    let n = kind.blocks(width, height);
    let mut out = Vec::with_capacity(n * kind.block_bytes());
    match kind {
        Kind::Raw(_) | Kind::Seeded(_) => {
            let len = n * kind.block_bytes();
            while out.len() < len {
                out.extend_from_slice(&rng.next_u64().to_le_bytes());
            }
            out.truncate(len);
        }
        Kind::Etc1 => (0..n).for_each(|_| out.extend_from_slice(&etc1_block(rng))),
        Kind::Bc7 => (0..n).for_each(|i| out.extend_from_slice(&bc7_block(i, rng))),
        Kind::Bc6h => (0..n).for_each(|_| out.extend_from_slice(&bc6h_block(rng))),
        Kind::Astc(..) => {}
    }
    out
}
