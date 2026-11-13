//! Blocks built by hand, each checked against what the Khronos Data Format
//! Specification 1.3 (sections 22.1 to 22.9) gives for it, worked out
//! here from its bit layout, not from the port; `tests/etc_oracle.rs`
//! replays what texture2ddecoder gives through UnityPy.

use super::{Block, TRANSPARENT, eac, etc1, etc2};

/// ETC1's individual mode: base colours (0xAA, 0x33, 0x00) and (0x55, 0xCC,
/// 0x00) from byte 0 to 2's nibbles, codewords 1 and 6, the flip bit 0
/// (two 2x4 sub-blocks side by side). Every index is 0 (`+a`), but at
/// (1, 2), where it is 3 (`-b`): bit 6 of both index words, column 1's
/// third pixel.
#[test]
fn etc1_individual_mode_is_the_specification() {
    let px = etc1([0xA5, 0x3C, 0x00, 0x38, 0x00, 0x40, 0x00, 0x40]);
    for (n, p) in px.iter().enumerate() {
        let (x, y) = (n % 4, n / 4);
        let want = match (x, y) {
            (1, 2) => [0xAA - 17, 0x33 - 17, 0, 255],
            (0 | 1, _) => [0xAA + 5, 0x33 + 5, 5, 255],
            _ => [0x55 + 33, 0xCC + 33, 33, 255],
        };
        assert_eq!(*p, want, "({x}, {y})");
    }
}

/// An ETC2 A1 block in the T mode: red 0xF8 with a delta of +3 leaves
/// 0 to 255. C0 = (0xFF, 0x88, 0x00), C1 = (0x44, 0x00, 0x22), distance
/// index 2 (11). Column 0 holds indices 2, 1, 3 and 0, the rest 0.
const T_BLOCK: [u8; 8] = [0xFB, 0x80, 0x40, 0x24, 0x00, 0x05, 0x00, 0x06];

fn column(px: &Block) -> [[u8; 4]; 4] {
    [px[0], px[4], px[8], px[12]]
}

#[test]
fn etc2_t_mode_is_the_specification() {
    let p0 = [0xFF, 0x88, 0x00, 255];
    let p1 = [0x44 + 11, 11, 0x22 + 11, 255];
    let p2 = [0x44, 0x00, 0x22, 255];
    let p3 = [0x44 - 11, 0, 0x22 - 11, 255];
    // Opaque: index 2 is C1 itself.
    let mut opaque = T_BLOCK;
    opaque[3] |= 2;
    let px = etc2(opaque, Some(true));
    assert_eq!(column(&px), [p2, p1, p3, p0]);
    for (n, p) in px.iter().enumerate() {
        if ![0, 4, 8].contains(&n) {
            assert_eq!(*p, p0, "{n}");
        }
    }
    // ETC2_RGB reads the same block (its diff bit set) the same way.
    assert_eq!(etc2(opaque, None), px);
    // The opaque bit clear: index 2 is transparent, (0, 0, 0, 0) by
    // section 22.9 (the Tier C patch), where texture2ddecoder gives
    // (0x44, 0x00, 0x22, 0).
    let px = etc2(T_BLOCK, Some(false));
    assert_eq!(column(&px), [TRANSPARENT, p1, p3, p0]);
}

/// An ETC2 A1 block in the differential mode, the opaque bit clear: base
/// colour (0x84, 0x84, 0x84) from 5-bit 16, deltas 0, codewords 0. Index 0
/// gives the base (its modifier is 0), index 1 `+8`, index 2 transparent,
/// index 3 `-8`.
#[test]
fn etc2_punch_through_differential_mode_is_the_specification() {
    // Pixel 0 index 2, pixel 1 index 1, pixel 2 index 3, the rest 0.
    let px = etc2(
        [0x80, 0x80, 0x80, 0x00, 0x00, 0x05, 0x00, 0x06],
        Some(false),
    );
    let base = [0x84, 0x84, 0x84, 255];
    assert_eq!(
        column(&px),
        [
            TRANSPARENT,
            [0x8C, 0x8C, 0x8C, 255],
            [0x7C, 0x7C, 0x7C, 255],
            base
        ]
    );
}

/// EAC: a multiplier of 0 is 1/8 of a step; the unsigned base 100 with
/// modifier -3 at a multiplier of 1 is `(800 - 3 + 4) >> 3 = 100`; signed,
/// `+ 1023` in place of `+ 4`, and the base two's complement, so -128 and
/// -127 at that modifier both give 0 (texture2ddecoder does not otherwise
/// read -128 as -127).
#[test]
fn eac_bases_and_multipliers_are_the_specification() {
    let read = |d: [u8; 8], signed: bool| {
        let mut px = [[0, 0, 0, 255]; 16];
        eac(d, 0, signed, &mut px);
        px.map(|p| p[0])
    };
    let solid = |base: u8, header: u8| [base, header, 0, 0, 0, 0, 0, 0];
    assert_eq!(read(solid(100, 0x00), false), [100; 16]);
    // Signed base 100 at modifier -3 and a multiplier of 1:
    // (800 - 3 + 1023) >> 3 = 227; base -100: (-800 - 3 + 1023) >> 3 = 27.
    assert_eq!(read(solid(100, 0x00), true), [227; 16]);
    assert_eq!(read(solid(0x9C, 0x00), true), [27; 16]);
    // -127 and -128 at -3: below 0, so both clamp to 0.
    assert_eq!(read(solid(0x80, 0x00), true), [0; 16]);
    assert_eq!(read(solid(0x81, 0x00), true), [0; 16]);
    // A multiplier of 15 (8 x 15 = 120) at table 0's -3: (800 - 360 + 4) >> 3.
    assert_eq!(read(solid(100, 0xF0), false), [55; 16]);
    // Index 7 everywhere (all 48 bits set): table 0's +14, clamped at 2047.
    let mut ones = solid(255, 0xF0);
    ones[2..].copy_from_slice(&[0xFF; 6]);
    assert_eq!(read(ones, false), [255; 16]);
}

/// The Tier C patch: a signed base of -128 is -127 (the Khronos Data Format
/// Specification 1.3, sections 22.7 and 22.8), where texture2ddecoder
/// 1.0.5 computes with -128. Index 7 everywhere is table 0's +14; with a
/// multiplier of 0 (read as 1) and of 1 (8):
/// - texture2ddecoder: (-1024 + 14 + 1023) >> 3 = 1 and
///   (-1024 + 112 + 1023) >> 3 = 13;
/// - the specification: (-1016 + 14 + 1023) >> 3 = 2 and
///   (-1016 + 112 + 1023) >> 3 = 14, which is also what a base of -127 (0x81)
///   gives. `tests/etc_oracle.rs`
///   `signed_eac_reads_a_base_of_minus_128_as_minus_127` pins texture2ddecoder's
///   1 and 13 against the oracle.
#[test]
fn a_signed_eac_base_of_minus_128_is_minus_127() {
    for (header, unitypy, khronos) in [(0x00, 1, 2), (0x10, 13, 14)] {
        let block = |base: u8| [base, header, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF];
        // EAC_R_SIGNED: the red channel.
        let mut px = [[0, 0, 0, 255]; 16];
        eac(block(0x80), 0, true, &mut px);
        assert_eq!(px, [[khronos, 0, 0, 255]; 16], "{header:#x}");
        assert_ne!(khronos, unitypy);
        let mut minus_127 = [[0, 0, 0, 255]; 16];
        eac(block(0x81), 0, true, &mut minus_127);
        assert_eq!(minus_127, px);
        // EAC_RG_SIGNED: R from the first block, G from the second; -128
        // in either is -127.
        let mut rg = [[0, 0, 0, 255]; 16];
        eac(block(0x80), 0, true, &mut rg);
        eac(block(0x80), 1, true, &mut rg);
        assert_eq!(rg, [[khronos, khronos, 0, 255]; 16], "{header:#x}");
        // Unsigned, 0x80 is 128: no patch.
        let mut unsigned = [[0, 0, 0, 255]; 16];
        eac(block(0x80), 0, false, &mut unsigned);
        let multiplier = if header == 0 { 1 } else { 8 };
        let want = u8::try_from((128 * 8 + 14 * multiplier + 4) >> 3).unwrap();
        assert_eq!(unsigned, [[want, 0, 0, 255]; 16]);
    }
}
