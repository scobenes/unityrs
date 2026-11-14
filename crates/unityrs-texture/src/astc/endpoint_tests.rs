use super::super::{
    block_mode::{self, BlockMode, WeightMode},
    endpoint,
    ise::Quant,
};
use super::put;

fn weights(bits: u8, dual_plane: bool) -> WeightMode {
    WeightMode {
        x: 4,
        y: 4,
        quant: Quant::new(2).unwrap(),
        bits,
        dual_plane,
    }
}

#[test]
fn single_shared_and_split_endpoint_headers() {
    let mut data = [0; 16];
    put(&mut data, 13, 4, 0);
    let layout = endpoint::decode_layout(&data, weights(32, false)).unwrap();
    assert_eq!(
        (
            layout.partitions,
            layout.seed,
            layout.value_count,
            layout.value_offset,
            layout.quant.levels()
        ),
        (1, 0, 2, 17, 256)
    );
    put(&mut data, 17, 8, 11);
    put(&mut data, 25, 8, 239);
    assert_eq!(
        endpoint::decode_values(&data, &layout).unwrap()[0],
        [11, 239, 0, 0, 0, 0, 0, 0]
    );
    assert_eq!(
        endpoint::decode_layout(&data, weights(96, false))
            .unwrap()
            .quant
            .levels(),
        160
    );

    // 3 partitions with one shared luminance mode. Shared CEM returns
    // its provisional high bits to the endpoint budget.
    data = [0; 16];
    put(&mut data, 11, 2, 2);
    put(&mut data, 13, 10, 0x2ab);
    let layout = endpoint::decode_layout(&data, weights(32, false)).unwrap();
    assert_eq!(layout.modes, [0; 4]);
    assert_eq!(
        (
            layout.seed,
            layout.value_count,
            layout.value_offset,
            layout.below_weights
        ),
        (0x2ab, 6, 29, 96)
    );

    // Split CEM: baseclass=2, class selectors 0,1,0, low bits 0,1,2.
    // Encoded 1162 has low 6 bits 10 and high 5 bits 18, giving 4,9,6.
    put(&mut data, 23, 6, 0x48a & 0x3f);
    put(&mut data, 91, 5, 1162 >> 6);
    let layout = endpoint::decode_layout(&data, weights(32, false)).unwrap();
    assert_eq!(&layout.modes[..3], &[4, 9, 6]);
    assert_eq!(
        (
            layout.value_count,
            layout.quant.levels(),
            layout.below_weights
        ),
        (14, 20, 91)
    );
}

#[test]
fn endpoint_header_errors_and_precision_boundaries() {
    let mut data = [0; 16];
    put(&mut data, 13, 4, 15); // 8 values cannot fit even at QUANT_6.
    assert!(endpoint::decode_layout(&data, weights(96, false)).is_none());
    data = [0; 16];
    put(&mut data, 11, 2, 3);
    assert!(endpoint::decode_layout(&data, weights(32, true)).is_none());
    // Four shared RGBA modes need 32 values; the block limit is 18.
    put(&mut data, 23, 6, 12 << 2);
    assert!(endpoint::decode_layout(&data, weights(32, false)).is_none());
    // All 16 endpoint modes can be read in a one-partition header.
    for mode in 0..16 {
        data = [0; 16];
        put(&mut data, 13, 4, mode);
        let layout = endpoint::decode_layout(&data, weights(24, false)).unwrap();
        assert_eq!(u32::from(layout.modes[0]), mode);
        assert_eq!(
            layout.value_count,
            usize::try_from(2 * ((mode >> 2) + 1)).unwrap()
        );
    }
    // With the same bit budget, dual plane reserves two extra bits.
    data = [0; 16];
    assert_eq!(
        endpoint::decode_layout(&data, weights(96, true))
            .unwrap()
            .quant
            .levels(),
        80
    );
}

#[test]
fn unquantization_every_endpoint_range() {
    // FNV-1a over every value of each astcenc_quantization.cpp:651-778
    // decode map. Constants calculated from the original C++ arrays,
    // independently of the Rust generator and implementation.
    let checks = [
        (6, 2_997_285_158),
        (8, 1_699_164_693),
        (10, 2_093_761_398),
        (12, 2_518_164_693),
        (16, 4_279_471_893),
        (20, 2_726_045_039),
        (24, 1_386_137_521),
        (32, 1_629_109_461),
        (40, 2_832_363_365),
        (48, 3_055_249_321),
        (64, 2_396_432_677),
        (80, 3_023_725_421),
        (96, 1_551_310_573),
        (128, 2_039_184_965),
        (160, 1_785_777_925),
        (192, 181_322_309),
        (256, 2_426_689_733),
    ];
    for (index, (levels, want)) in (4..21).zip(checks) {
        let q = Quant::new(index).unwrap();
        assert_eq!(q.levels(), levels);
        let mut hash = 2_166_136_261_u32;
        for value in 0..levels {
            let byte = q.unquant_color(u8::try_from(value).unwrap()).unwrap();
            hash = (hash ^ u32::from(byte)).wrapping_mul(16_777_619);
            // Power-of-two ranges use bit replication, not rounding.
            if levels.is_power_of_two() {
                let bits = levels.ilog2();
                let mut replicated = u32::from(value) << (8 - bits);
                let mut shift = bits;
                while shift < 8 {
                    replicated |= replicated >> shift;
                    shift *= 2;
                }
                assert_eq!(u32::from(byte), replicated);
            }
        }
        assert_eq!(hash, want, "range {levels}");
        if levels < 256 {
            assert_eq!(q.unquant_color(u8::try_from(levels).unwrap()), None);
        }
    }
    // Hand values from the spec's trit and quint unquantization tables.
    assert_eq!(
        (0..6)
            .map(|v| Quant::new(4).unwrap().unquant_color(v).unwrap())
            .collect::<Vec<_>>(),
        [0, 255, 51, 204, 102, 153]
    );
    assert_eq!(Quant::new(6).unwrap().unquant_color(2), Some(28));
    assert_eq!(Quant::new(7).unwrap().unquant_color(2), Some(69));
    for index in 0..4 {
        assert_eq!(Quant::new(index).unwrap().unquant_color(0), None);
    }
}

#[test]
fn every_ldr_endpoint_mode_with_hand_values() {
    // Expected 8-bit endpoints before 16-bit replication (spec Endpoint
    // Decoding); include signed transfer, clamping and RGB scale's /256.
    for (mode, input, want) in [
        (0, vec![10, 200], [[10, 10, 10, 255], [200, 200, 200, 255]]),
        (
            1,
            vec![252, 255],
            [[255, 255, 255, 255], [255, 255, 255, 255]],
        ),
        (
            4,
            vec![10, 20, 30, 40],
            [[10, 10, 10, 30], [20, 20, 20, 40]],
        ),
        (5, vec![0, 126, 254, 128], [[0, 0, 0, 255], [0, 0, 0, 255]]),
        (
            6,
            vec![64, 128, 255, 128],
            [[32, 64, 127, 255], [64, 128, 255, 255]],
        ),
        (
            8,
            vec![10, 20, 30, 40, 50, 60],
            [[10, 30, 50, 255], [20, 40, 60, 255]],
        ),
        (
            9,
            vec![20, 2, 40, 4, 60, 6],
            [[10, 20, 30, 255], [11, 22, 33, 255]],
        ),
        (
            10,
            vec![64, 128, 255, 128, 9, 27],
            [[32, 64, 127, 9], [64, 128, 255, 27]],
        ),
        (
            12,
            vec![10, 20, 30, 40, 50, 60, 70, 80],
            [[10, 30, 50, 70], [20, 40, 60, 80]],
        ),
        (
            13,
            vec![20, 2, 40, 4, 60, 6, 80, 8],
            [[10, 20, 30, 40], [11, 22, 33, 44]],
        ),
    ] {
        assert_eq!(
            endpoint::unpack_ldr(mode, &input),
            Some(want.map(|v| v.map(|c| c * 257))),
            "mode {mode}"
        );
        assert_eq!(endpoint::unpack_ldr(mode, &input[..input.len() - 1]), None);
    }
    for mode in [2, 3, 7, 11, 14, 15] {
        assert_eq!(
            endpoint::unpack_ldr(mode, &[0; 8]),
            Some([[65535, 0, 65535, 65535]; 2])
        );
    }
    assert_eq!(endpoint::unpack_ldr(16, &[0; 8]), None);
}

#[test]
fn blue_contraction_swaps_alpha_and_clamps_after_uncontracting() {
    let to16 = |v: [[u16; 4]; 2]| v.map(|p| p.map(|c| c * 257));
    assert_eq!(
        endpoint::unpack_ldr(12, &[100, 20, 80, 40, 60, 10, 5, 9]),
        Some(to16([[15, 25, 10, 9], [80, 70, 60, 5]]))
    );
    // Deltas -1,-2,-3 trigger uncontraction and endpoint swap.
    assert_eq!(
        endpoint::unpack_ldr(13, &[20, 126, 40, 124, 60, 122, 80, 2]),
        Some(to16([[18, 22, 27, 41], [20, 25, 30, 40]]))
    );
    // Out-of-range red (-32) is averaged with blue (96) before clamping.
    assert_eq!(
        endpoint::unpack_ldr(9, &[0, 64, 0, 64, 254, 66]),
        Some(to16([[32, 32, 96, 255], [63, 63, 127, 255]]))
    );
}

#[test]
fn a_complete_component_path_uses_the_same_layout() {
    let mut data = [0; 16];
    put(&mut data, 0, 11, 19);
    put(&mut data, 13, 4, 0);
    put(&mut data, 17, 8, 23);
    put(&mut data, 25, 8, 200);
    let Some(BlockMode::Weights(w)) = block_mode::decode(19) else {
        panic!("weight mode")
    };
    let layout = endpoint::decode_layout(&data, w).unwrap();
    let values = endpoint::decode_values(&data, &layout).unwrap();
    assert_eq!(
        endpoint::unpack_ldr(layout.modes[0], &values[0]),
        Some([[5911, 5911, 5911, 65535], [51400, 51400, 51400, 65535]])
    );
}
