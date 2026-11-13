// Ported from texture2ddecoder 1.0.5 (MIT): src/Texture2DDecoder/etc.cpp, its ETC1, ETC2, ETC2 A1, ETC2 A8 and EAC block decoders with their tables (decode_etc1_block, decode_etc2_block, decode_etc2a1_block, decode_etc2a8_block, decode_eac_block, decode_eac_signed_block) and their block loops (decode_etc1 to decode_eacrg_signed), and src/Texture2DDecoder/color.h's copy_block_buffer; the code it carries from AssetStudio's Texture2DDecoderNative and mikunyan. See THIRD_PARTY_LICENSES.
// Ported from UnityPy 1.25.2 (MIT): UnityPy/export/Texture2DConverter.py's etc and eac, which read texture2ddecoder's BGRA output as RGBA. See THIRD_PARTY_LICENSES.
//! texture2ddecoder 1.0.5's ETC and EAC decoders, which UnityPy 1.25.2
//! calls for `ETC_RGB4`, `ETC2_RGB`, `ETC2_RGBA1`, `ETC2_RGBA8`, `EAC_R`,
//! `EAC_R_SIGNED`, `EAC_RG` and `EAC_RG_SIGNED` (`etc` and `eac`,
//! `export/Texture2DConverter.py:405-432`), on the image padded to whole
//! 4x4 blocks, reading the `BGRA` words they return with Pillow's raw
//! `BGRA` mode, so as `RGBA` pixels; UnityPy then crops and flips the
//! image as for every block format (`decode`'s module docs). Ported rule
//! for rule, with C's integer widths (`uint_fast8_t` is a byte, so a
//! colour assigned to one wraps):
//! - **ETC1** (`decode_etc1_block`): the individual mode (two 4-bit base
//!   colours, widened by replication) and the differential mode (a 5-bit
//!   base and a 3-bit signed delta, widened by replication; a delta past
//!   either end wraps, as the byte it is assigned to does), each with its
//!   two table codewords, the flip bit's two sub-blocks, and each pixel's
//!   modifier from its two index bits, added to every channel and clamped.
//! - **ETC2** (`decode_etc2_block`): ETC1's two modes, and where the
//!   differential mode's red, green or blue would leave 0 to 255, the T, H
//!   and planar modes in that order: T's and H's two colours and distance,
//!   H's distance bit from the order of its colours, and the planar mode's
//!   three colours interpolated per pixel, `(4 o + x (h - o) + y (v - o) +
//!   2) >> 2` clamped. Alpha 255.
//! - **ETC2 A1** (`decode_etc2a1_block`): ETC2 without the individual mode,
//!   the diff bit being the opaque bit. Where it is 0, the differential
//!   mode's index 0 and 2 modifiers are 0 and index 2 is transparent, as
//!   T's and H's index 2 is; the planar mode is opaque.
//! - **ETC2 A8** (`decode_etc2a8_block`): the colour block of ETC2 after
//!   an alpha block: a base, a multiplier and one of 16 modifier tables,
//!   each pixel's 3-bit index read from the 48 bits after them, big-endian,
//!   the result clamped; a multiplier of 0 gives every pixel the base.
//! - **EAC** (`decode_eac_block`, `decode_eac_signed_block`): the alpha
//!   block's layout as an 11-bit channel, `base * 8 + multiplier *
//!   modifier`, `+ 4` unsigned and `+ 1023` signed (the signed base a
//!   two's-complement byte, -128 read as -127: a patch, below), clamped to
//!   0 to 2047 and kept as its high 8 bits; a multiplier of 0 reads as 1/8.
//!   R alone, or R and then G, on (0, 0, 0, 255).
//!
//! **Patches.** `DESIGN.md` section 3 labels each change to a vendored or
//! ported decoder Tier A (the reference behaviour kept) or Tier C:
//! - Tier C, `ETC2_RGBA1`'s transparent pixels: texture2ddecoder keeps a
//!   transparent pixel's colour and clears only its alpha
//!   (`TRANSPARENT_MASK`, `applicate_color_alpha`); the Khronos Data
//!   Format Specification 1.3, section 22.9, makes it (0, 0, 0, 0), which
//!   [`decode()`] writes (`PARITY.md` section 19.1).
//! - Tier C, a signed EAC base of -128 (`EAC_R_SIGNED`, `EAC_RG_SIGNED`):
//!   texture2ddecoder computes with the byte's -128
//!   (`decode_eac_signed_block`); the Khronos Data Format Specification
//!   1.3, sections 22.7 and 22.8, reads it as -127, which [`decode()`]
//!   does: 8 more in 11 bits, so each pixel's byte is 1 higher, but where
//!   both clamp to 0 (no base of -127 reaches 2047) (`PARITY.md` section
//!   19.1).
//! - Tier A, the reads: texture2ddecoder checks neither the data's length
//!   nor a side, and reads past the data's end where it is short;
//!   [`crate::plan()`] refuses such data before any decode, so every block
//!   read here exists, and a missing one would leave its pixels 0 rather
//!   than read anything else (`PARITY.md` section 19.3).
//! - Tier A, the output: each pixel is written as the `RGBA` Pillow's
//!   `BGRA` mode reads from the word texture2ddecoder writes, straight to
//!   its cropped and flipped place, as [`crate::bcn`]'s decoders write.

/// One pixel, `[r, g, b, a]`.
type Rgba = [u8; 4];

/// A block's 16 pixels, row by row.
type Block = [Rgba; 16];

/// Which of texture2ddecoder's decoders.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// `decode_etc1`: `ETC_RGB4`.
    Etc1,
    /// `decode_etc2`: `ETC2_RGB`.
    Etc2,
    /// `decode_etc2a1`: `ETC2_RGBA1`, with the Tier C patch.
    Etc2A1,
    /// `decode_etc2a8`: `ETC2_RGBA8`.
    Etc2A8,
    /// `decode_eacr`: `EAC_R`.
    EacR,
    /// `decode_eacr_signed`: `EAC_R_SIGNED`.
    EacRSigned,
    /// `decode_eacrg`: `EAC_RG`.
    EacRg,
    /// `decode_eacrg_signed`: `EAC_RG_SIGNED`.
    EacRgSigned,
}

/// `WriteOrderTable`: the pixel of a block's `i`th index bits, which run
/// down its columns.
const WRITE_ORDER: [usize; 16] = [0, 4, 8, 12, 1, 5, 9, 13, 2, 6, 10, 14, 3, 7, 11, 15];
/// `WriteOrderTableRev`: the pixel of an alpha block's `i`th index from its
/// end.
const WRITE_ORDER_REV: [usize; 16] = [15, 11, 7, 3, 14, 10, 6, 2, 13, 9, 5, 1, 12, 8, 4, 0];
/// `Etc1ModifierTable`: each codeword's two modifiers.
const ETC1_MODIFIERS: [[u8; 2]; 8] = [
    [2, 8],
    [5, 17],
    [9, 29],
    [13, 42],
    [18, 60],
    [24, 80],
    [33, 106],
    [47, 183],
];
/// `Etc2aModifierTable`: ETC2 A1's, by the opaque bit: where it is 0 the
/// first modifier is 0.
const ETC2A_MODIFIERS: [[[u8; 2]; 8]; 2] = [
    [
        [0, 8],
        [0, 17],
        [0, 29],
        [0, 42],
        [0, 60],
        [0, 80],
        [0, 106],
        [0, 183],
    ],
    ETC1_MODIFIERS,
];
/// `Etc1SubblockTable`: each index's sub-block, by the flip bit.
const ETC1_SUBBLOCKS: [[usize; 16]; 2] = [
    [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1],
    [0, 0, 1, 1, 0, 0, 1, 1, 0, 0, 1, 1, 0, 0, 1, 1],
];
/// `Etc2DistanceTable`: the T and H modes' distances.
const ETC2_DISTANCES: [i32; 8] = [3, 6, 11, 16, 23, 32, 41, 64];
/// `Etc2AlphaModTable`: the alpha and EAC blocks' modifier tables.
const ETC2_ALPHA_MODS: [[i32; 8]; 16] = [
    [-3, -6, -9, -15, 2, 5, 8, 14],
    [-3, -7, -10, -13, 2, 6, 9, 12],
    [-2, -5, -8, -13, 1, 4, 7, 12],
    [-2, -4, -6, -13, 1, 3, 5, 12],
    [-3, -6, -8, -12, 2, 5, 7, 11],
    [-3, -7, -9, -11, 2, 6, 8, 10],
    [-4, -7, -8, -11, 3, 6, 7, 10],
    [-3, -5, -8, -11, 2, 4, 7, 10],
    [-2, -6, -8, -10, 1, 5, 7, 9],
    [-2, -5, -8, -10, 1, 4, 7, 9],
    [-2, -4, -8, -10, 1, 3, 7, 9],
    [-2, -5, -7, -10, 1, 4, 6, 9],
    [-3, -4, -7, -10, 2, 3, 6, 9],
    [-1, -2, -3, -10, 0, 1, 2, 9],
    [-4, -6, -8, -9, 3, 5, 7, 8],
    [-3, -5, -7, -9, 2, 4, 6, 8],
];

/// The Tier C patch's transparent pixel (module docs).
const TRANSPARENT: Rgba = [0, 0, 0, 0];

/// The `i`th entry of `table`, or the default where it has none (never, for
/// the indices the decoders compute).
fn pick<T: Copy + Default>(table: &[T], i: usize) -> T {
    table.get(i).copied().unwrap_or_default()
}

/// A count of bits or an index taken from a block (at most 16 bits).
fn index(v: u64) -> usize {
    usize::try_from(v).unwrap_or(0)
}

/// C's conversion of an `int` to a `uint_fast8_t`: its low byte.
fn low(v: i32) -> u8 {
    u8::try_from(v & 0xFF).unwrap_or(0)
}

/// `clamp`: an `int` to 0 to 255.
fn clamp(n: i32) -> u8 {
    u8::try_from(n.clamp(0, 255)).unwrap_or(u8::MAX)
}

/// `applicate_color`: the colour with `modifier` added to each channel,
/// clamped, opaque.
fn applicate(colour: [u8; 3], modifier: i32) -> Rgba {
    let [red, green, blue] = colour.map(|v| clamp(i32::from(v) + modifier));
    [red, green, blue, 255]
}

/// `applicate_color_raw`: the colour, opaque.
fn raw(colour: [u8; 3]) -> Rgba {
    let [red, green, blue] = colour;
    [red, green, blue, 255]
}

/// Sets the block's pixel `at`, which is always in it.
fn put(out: &mut Block, at: usize, px: Rgba) {
    if let Some(p) = out.get_mut(at) {
        *p = px;
    }
}

/// Decodes `data` (at least the padded image's blocks) into `out`, the
/// `w` x `h` image's rows top to bottom, as [`crate::bcn`]'s `decode` does:
/// block row `by` of the padded image holds source rows `4 by` to
/// `4 by + 3`, and source row `y` is output row `h - 1 - y`.
pub(crate) fn decode(kind: Kind, data: &[u8], w: usize, h: usize, out: &mut [u8]) {
    let block_bytes = match kind {
        Kind::Etc1 | Kind::Etc2 | Kind::Etc2A1 | Kind::EacR | Kind::EacRSigned => 8,
        Kind::Etc2A8 | Kind::EacRg | Kind::EacRgSigned => 16,
    };
    let (blocks_x, blocks_y) = (w.div_ceil(4), h.div_ceil(4));
    let row = w.saturating_mul(4);
    let mut blocks = data.chunks_exact(block_bytes);
    for by in 0..blocks_y {
        for bx in 0..blocks_x {
            let Some(src) = blocks.next() else {
                return;
            };
            let px = block(kind, half(src, 0), half(src, 8));
            for (n, p) in px.iter().enumerate() {
                let (x, y) = (bx * 4 + n % 4, by * 4 + n / 4);
                if x >= w || y >= h {
                    continue;
                }
                let at = (h - 1 - y).saturating_mul(row).saturating_add(x * 4);
                if let Some(dst) = out.get_mut(at..at.saturating_add(4)) {
                    dst.copy_from_slice(p);
                }
            }
        }
    }
}

/// One block's pixels: `first` is its first 8 bytes, `second` the next 8
/// of a 16-byte block (zeros for an 8-byte one).
fn block(kind: Kind, first: [u8; 8], second: [u8; 8]) -> Block {
    match kind {
        Kind::Etc1 => etc1(first),
        Kind::Etc2 => etc2(first, None),
        Kind::Etc2A1 => etc2(first, Some(first[3] >> 1 & 1 == 1)),
        Kind::Etc2A8 => {
            let mut px = etc2(second, None);
            etc2_alpha(first, &mut px);
            px
        }
        Kind::EacR | Kind::EacRSigned | Kind::EacRg | Kind::EacRgSigned => {
            let signed = matches!(kind, Kind::EacRSigned | Kind::EacRgSigned);
            let mut px = [[0, 0, 0, 255]; 16];
            eac(first, 0, signed, &mut px);
            if matches!(kind, Kind::EacRg | Kind::EacRgSigned) {
                eac(second, 1, signed, &mut px);
            }
            px
        }
    }
}

/// The 8 bytes of `src` from `at`, or zeros where it has none (an 8-byte
/// block's second half, never read).
fn half(src: &[u8], at: usize) -> [u8; 8] {
    src.get(at..at + 8)
        .and_then(|s| <[u8; 8]>::try_from(s).ok())
        .unwrap_or_default()
}

/// The pixel index bits of a colour block: `(lsb, msb)`, bit `i` of each
/// the `i`th index's (`j` and `k`).
fn index_bits(bytes: [u8; 8]) -> (u32, u32) {
    let [_, _, _, _, b4, b5, b6, b7] = bytes.map(u32::from);
    (b6 << 8 | b7, b4 << 8 | b5)
}

/// The individual and differential modes' 16 pixels: the sub-block colours
/// `colours`, the codewords of byte 3, and `modifier` for a sub-block's
/// codeword and an index's low bit, its high bit the sign; `transparent`
/// for an index's low and high bits says whether the pixel is transparent.
fn sub_blocks(
    bytes: [u8; 8],
    colours: [[u8; 3]; 2],
    modifier: impl Fn(usize, usize) -> u8,
    transparent: impl Fn(u32, u32) -> bool,
) -> Block {
    let code = [usize::from(bytes[3] >> 5), usize::from(bytes[3] >> 2 & 7)];
    let table = pick(&ETC1_SUBBLOCKS, usize::from(bytes[3] & 1));
    let (mut lsb, mut msb) = index_bits(bytes);
    let mut out = [[0; 4]; 16];
    for i in 0..16 {
        let sub = pick(&table, i);
        let size = i32::from(modifier(pick(&code, sub), index(u64::from(lsb & 1))));
        let px = applicate(pick(&colours, sub), if msb & 1 == 1 { -size } else { size });
        // The Tier C patch (module docs): (0, 0, 0, 0), not the colour.
        let px = if transparent(lsb & 1, msb & 1) {
            TRANSPARENT
        } else {
            px
        };
        put(&mut out, pick(&WRITE_ORDER, i), px);
        lsb >>= 1;
        msb >>= 1;
    }
    out
}

/// ETC1's and ETC2's modifier: `Etc1ModifierTable`.
fn etc1_modifier(code: usize, lsb: usize) -> u8 {
    pick(&pick(&ETC1_MODIFIERS, code), lsb)
}

/// Two colours from each channel's pair: `channel` maps a byte to its two
/// values.
fn pairs(bytes: [u8; 8], channel: impl Fn(u8) -> (u8, u8)) -> [[u8; 3]; 2] {
    let [(r0, r1), (g0, g1), (b0, b1)] = [bytes[0], bytes[1], bytes[2]].map(channel);
    [[r0, g0, b0], [r1, g1, b1]]
}

/// The individual mode's two colours: each channel's two nibbles, widened.
fn individual(bytes: [u8; 8]) -> [[u8; 3]; 2] {
    pairs(bytes, |byte| {
        let v = i32::from(byte);
        (low((v & 0xf0) | v >> 4), low((v & 0x0f) | v << 4))
    })
}

/// `decode_etc1_block`.
fn etc1(bytes: [u8; 8]) -> Block {
    let colours = if bytes[3] & 2 != 0 {
        // The differential mode: a delta past either end wraps, as the
        // `uint_fast8_t` it is assigned to does.
        pairs(bytes, |byte| {
            let v = i32::from(byte);
            let base = low(v & 0xf8);
            let other = low(i32::from(base) + (v << 3 & 0x18) - (v << 3 & 0x20));
            (base | base >> 5, other | other >> 5)
        })
    } else {
        individual(bytes)
    };
    sub_blocks(bytes, colours, etc1_modifier, |_, _| false)
}

/// The T and H modes' 16 pixels from their four colours: with the high
/// index bits shifted one left, the index is `(k & 2) | (j & 1)`, and with
/// `punch` index 2 is transparent.
fn four_colours(bytes: [u8; 8], set: [Rgba; 4], punch: bool) -> Block {
    let (mut lsb, msb) = index_bits(bytes);
    let mut msb = msb << 1;
    let mut out = [[0; 4]; 16];
    for i in 0..16 {
        let n = index(u64::from((msb & 2) | (lsb & 1)));
        // The Tier C patch (module docs): (0, 0, 0, 0), where
        // texture2ddecoder masks the alpha alone.
        let px = if punch && n == 2 {
            TRANSPARENT
        } else {
            pick(&set, n)
        };
        put(&mut out, pick(&WRITE_ORDER, i), px);
        lsb >>= 1;
        msb >>= 1;
    }
    out
}

/// The differential mode's delta of a channel's byte: its low 3 bits,
/// signed, times 8.
fn delta(v: i32) -> i32 {
    (v << 3 & 0x18) - (v << 3 & 0x20)
}

/// `decode_etc2_block`, or with `opaque` (the opaque bit) set,
/// `decode_etc2a1_block`.
fn etc2(bytes: [u8; 8], opaque: Option<bool>) -> Block {
    let punch = opaque == Some(false);
    if opaque.is_none() && bytes[3] & 2 == 0 {
        return sub_blocks(bytes, individual(bytes), etc1_modifier, |_, _| false);
    }
    let [red, green, blue] = [bytes[0], bytes[1], bytes[2]].map(|v| i32::from(v & 0xf8));
    let [dr, dg, db] = [bytes[0], bytes[1], bytes[2]].map(|v| delta(i32::from(v)));
    let out_of_range = |v: i32| !(0..=255).contains(&v);
    if out_of_range(red + dr) {
        four_colours(bytes, t_mode(bytes), punch)
    } else if out_of_range(green + dg) {
        four_colours(bytes, h_mode(bytes), punch)
    } else if out_of_range(blue + db) {
        // Opaque, with or without the opaque bit.
        planar(bytes)
    } else {
        let channel = |base: i32, delta: i32| {
            let base = low(base);
            let other = low(i32::from(base) + delta);
            (base | base >> 5, other | other >> 5)
        };
        let [(r0, r1), (g0, g1), (b0, b1)] =
            [(red, dr), (green, dg), (blue, db)].map(|(base, d)| channel(base, d));
        let colours = [[r0, g0, b0], [r1, g1, b1]];
        match opaque {
            None => sub_blocks(bytes, colours, etc1_modifier, |_, _| false),
            Some(opaque) => sub_blocks(
                bytes,
                colours,
                |code, lsb| {
                    pick(
                        &pick(&pick(&ETC2A_MODIFIERS, usize::from(opaque)), code),
                        lsb,
                    )
                },
                // `!obaq && (k & 1) && !(j & 1)`: index 2.
                |lsb, msb| punch && msb == 1 && lsb == 0,
            ),
        }
    }
}

/// The T mode's four colours.
fn t_mode(bytes: [u8; 8]) -> [Rgba; 4] {
    let [b0, b1, b2, b3, ..] = bytes.map(i32::from);
    let c0 = [
        low((b0 << 3 & 0xc0) | (b0 << 4 & 0x30) | (b0 >> 1 & 0xc) | (b0 & 3)),
        low((b1 & 0xf0) | b1 >> 4),
        low((b1 & 0x0f) | b1 << 4),
    ];
    let c1 = [
        low((b2 & 0xf0) | b2 >> 4),
        low((b2 & 0x0f) | b2 << 4),
        low((b3 & 0xf0) | b3 >> 4),
    ];
    let dist = pick(
        &ETC2_DISTANCES,
        index(u64::from(bytes[3] >> 1 & 6 | bytes[3] & 1)),
    );
    [raw(c0), applicate(c1, dist), raw(c1), applicate(c1, -dist)]
}

/// The H mode's four colours, its distance's low bit from their order.
fn h_mode(bytes: [u8; 8]) -> [Rgba; 4] {
    let [b0, b1, b2, b3, ..] = bytes.map(i32::from);
    let g0 = low((b0 << 5 & 0xe0) | (b1 & 0x10));
    let blue0 = low((b1 & 8) | (b1 << 1 & 6) | b2 >> 7);
    let g1 = low((b2 << 5 & 0xe0) | (b3 >> 3 & 0x10));
    let c0 = [
        low((b0 << 1 & 0xf0) | (b0 >> 3 & 0xf)),
        g0 | g0 >> 4,
        low(i32::from(blue0) | i32::from(blue0) << 4),
    ];
    let c1 = [
        low((b2 << 1 & 0xf0) | (b2 >> 3 & 0xf)),
        g1 | g1 >> 4,
        low((b3 << 1 & 0xf0) | (b3 >> 3 & 0xf)),
    ];
    let mut n = index(u64::from(bytes[3] & 4 | bytes[3] << 1 & 2));
    // `c0 > c1` channel by channel, or equal: C's comparison, in order.
    if c0 >= c1 {
        n += 1;
    }
    let dist = pick(&ETC2_DISTANCES, n);
    [
        applicate(c0, dist),
        applicate(c0, -dist),
        applicate(c1, dist),
        applicate(c1, -dist),
    ]
}

/// The planar mode: its origin, horizontal and vertical colours, each
/// pixel `(4 o + x (h - o) + y (v - o) + 2) >> 2`, clamped.
fn planar(bytes: [u8; 8]) -> Block {
    let [b0, b1, b2, b3, b4, b5, b6, b7] = bytes.map(i32::from);
    let blue_o = low((b1 << 7 & 0x80) | (b2 << 2 & 0x60) | (b2 << 3 & 0x18) | (b3 >> 5 & 4));
    let blue_h = low((b4 << 7 & 0x80) | (b5 >> 1 & 0x7c));
    let colours = [
        [
            low((b0 << 1 & 0xfc) | (b0 >> 5 & 3)),
            low((b0 << 7 & 0x80) | (b1 & 0x7e) | (b0 & 1)),
            blue_o | blue_o >> 6,
        ],
        [
            low((b3 << 1 & 0xf8) | (b3 << 2 & 4) | (b3 >> 5 & 3)),
            low((b4 & 0xfe) | b4 >> 7),
            blue_h | blue_h >> 6,
        ],
        [
            low((b5 << 5 & 0xe0) | (b6 >> 3 & 0x1c) | (b5 >> 1 & 3)),
            low((b6 << 3 & 0xf8) | (b7 >> 5 & 0x6) | (b6 >> 4 & 1)),
            low(b7 << 2 | (b7 >> 4 & 3)),
        ],
    ];
    let [origin, horizontal, vertical] = colours.map(|c| c.map(i32::from));
    let mut out = [[0; 4]; 16];
    for (i, px) in out.iter_mut().enumerate() {
        let x = i32::try_from(i % 4).unwrap_or(0);
        let y = i32::try_from(i / 4).unwrap_or(0);
        let channel = |n: usize| {
            let o = pick(&origin, n);
            clamp((x * (pick(&horizontal, n) - o) + y * (pick(&vertical, n) - o) + 4 * o + 2) >> 2)
        };
        *px = [channel(0), channel(1), channel(2), 255];
    }
    out
}

/// The 48 index bits of an alpha or EAC block, big-endian after its two
/// header bytes, as `bton64` reads the block: the `i`th from the end is
/// bits `3 i` to `3 i + 2`.
fn alpha_indices(bytes: [u8; 8]) -> u64 {
    u64::from_be_bytes(bytes)
}

/// `decode_etc2a8_block`: the alpha of `px` from the alpha block.
fn etc2_alpha(bytes: [u8; 8], px: &mut Block) {
    let base = i32::from(bytes[0]);
    if bytes[1] & 0xf0 != 0 {
        let multiplier = i32::from(bytes[1] >> 4);
        let table = pick(&ETC2_ALPHA_MODS, usize::from(bytes[1] & 0xf));
        let mut indices = alpha_indices(bytes);
        for i in 0..16 {
            let alpha = clamp(base + multiplier * pick(&table, index(indices & 7)));
            if let Some(p) = px.get_mut(pick(&WRITE_ORDER_REV, i)) {
                p[3] = alpha;
            }
            indices >>= 3;
        }
    } else {
        // A multiplier of 0: every pixel the base.
        for p in px.iter_mut() {
            p[3] = bytes[0];
        }
    }
}

/// `decode_eac_block` (`signed` false) and `decode_eac_signed_block`: the
/// `channel` of `px` from the EAC block (0, red, where texture2ddecoder
/// writes its `BGRA` word's byte 2; 1, green, its byte 1). A signed base of
/// -128 is -127 (the Tier C patch of the module docs).
fn eac(bytes: [u8; 8], channel: usize, signed: bool, px: &mut Block) {
    let (base, offset) = if signed {
        (i32::from(i8::from_ne_bytes([bytes[0]])).max(-127), 1023)
    } else {
        (i32::from(bytes[0]), 4)
    };
    let multiplier = match i32::from(bytes[1] >> 1 & 0x78) {
        0 => 1,
        m => m,
    };
    let table = pick(&ETC2_ALPHA_MODS, usize::from(bytes[1] & 0xf));
    let mut indices = alpha_indices(bytes);
    for i in 0..16 {
        let value = base * 8 + multiplier * pick(&table, index(indices & 7)) + offset;
        let v = if value < 0 {
            0
        } else if value >= 2048 {
            0xff
        } else {
            low(value >> 3)
        };
        if let Some(c) = px
            .get_mut(pick(&WRITE_ORDER_REV, i))
            .and_then(|p| p.get_mut(channel))
        {
            *c = v;
        }
        indices >>= 3;
    }
}

#[cfg(test)]
mod tests;
