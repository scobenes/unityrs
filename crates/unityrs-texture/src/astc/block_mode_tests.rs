use super::super::block_mode::{self, BlockMode};

#[test]
fn every_spec_block_mode_class() {
    // Hand-decoded fields from the ASTC Block Mode tables. Each row is
    // mode, X, Y, dual-plane, quantization index, stored weight bits.
    // Covers the four low-bit classes (including both class-3 layouts),
    // all high-bit classes, the 10x6 transpose and the forced H=D=0 class.
    for [mode, x, y, dual, quant, bits] in [
        [19, 4, 2, 0, 5, 24],
        [6, 8, 2, 0, 2, 32],
        [10, 2, 8, 0, 2, 32],
        [14, 2, 6, 0, 2, 24],
        [351, 2, 4, 0, 5, 24],
        [4, 12, 2, 0, 0, 24],
        [132, 2, 12, 0, 0, 24],
        [260, 6, 6, 0, 0, 36],
        [388, 6, 10, 0, 0, 60],
        [420, 10, 6, 0, 0, 60],
        [0x104 | 0x400 | 0x200, 6, 9, 0, 0, 54],
        [0x13 | 0x400, 4, 2, 1, 5, 48],
        [0x13 | 0x200, 4, 2, 0, 11, 40],
    ] {
        let Some(BlockMode::Weights(w)) = block_mode::decode(mode) else {
            panic!("mode {mode}")
        };
        assert_eq!(
            [
                u16::from(w.x),
                u16::from(w.y),
                u16::from(w.dual_plane),
                u16::try_from(w.quant.index()).unwrap(),
                u16::from(w.bits)
            ],
            [x, y, dual, quant, bits],
            "mode {mode}"
        );
    }
}

#[test]
fn reserved_invalid_and_footprint_modes() {
    // Zero low nibble is reserved; class 7 with A=2 or 3 is reserved.
    for mode in [0, 16, 512, 1024, 452, 484, 0xffff] {
        assert_eq!(block_mode::decode(mode), None, "{mode}");
    }
    // Too few weight bits; too many weights (dual 12x4); too many bits.
    for mode in [1, 1092, 540] {
        assert_eq!(block_mode::decode(mode), None, "{mode}");
    }
    assert_eq!(block_mode::for_footprint(4, 4, 4), None);
    assert!(block_mode::for_footprint(19, 4, 4).is_some());
    for mode in [0x1fc, 0x3fc, 0x5fc, 0x7fc] {
        assert_eq!(block_mode::decode(mode), Some(BlockMode::VoidExtent));
    }
}

#[test]
fn exhaustive_mode_table_matches_reference() {
    // FNV-1a of [x,y,dual,quant,bits] for modes 0..2048, computed by
    // compiling astcenc_block_sizes.cpp:36-137 unchanged. Illegal modes
    // encode [255,0,0,0,0], void markers [254,0,0,0,0]. This checks every
    // reserved value, precision and count boundary, not only sample classes.
    let mut hash = 2_166_136_261_u32;
    let mut valid = 0;
    for mode in 0..2048 {
        let row = match block_mode::decode(mode) {
            None => [255, 0, 0, 0, 0],
            Some(BlockMode::VoidExtent) => [254, 0, 0, 0, 0],
            Some(BlockMode::Weights(w)) => {
                valid += 1;
                [
                    w.x,
                    w.y,
                    u8::from(w.dual_plane),
                    u8::try_from(w.quant.index()).unwrap(),
                    w.bits,
                ]
            }
        };
        for value in row {
            hash = (hash ^ u32::from(value)).wrapping_mul(16_777_619);
        }
    }
    assert_eq!(valid, 775);
    assert_eq!(hash, 1_086_902_382);
}
