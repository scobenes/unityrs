use super::unpack;

#[test]
fn luminance_ranges_and_default_hdr_alpha() {
    // Hand values from ASTC endpoint decoding: large-range ordering adds
    // a half-step when reversed; small-range stores base plus a delta.
    for (mode, input, low, high) in [
        (2, [10, 200], 2560, 51200),
        (2, [200, 10], 2688, 51072),
        (2, [10, 10], 2560, 2560),
        (3, [10, 33], 8512, 8544),
        (3, [138, 33], 8832, 8896),
        (3, [127, 255], 65504, 65520),
        (3, [255, 255], 65472, 65520),
    ] {
        assert_eq!(
            unpack(mode, &input),
            Some((
                [[low, low, low, 0x7800], [high, high, high, 0x7800],],
                [true; 4]
            )),
            "mode {mode}, {input:?}"
        );
    }
}

#[test]
fn rgb_offset_all_six_submodes_and_major_components() {
    // Deliberately small differentials distinguish the scale and each
    // component rotation without hiding mistakes behind the zero clamp.
    for (input, low, high) in [
        ([20, 2, 3, 1], [608, 544, 512], [640, 576, 544]),
        ([84, 2, 3, 1], [608, 544, 512], [640, 576, 544]),
        ([148, 2, 3, 1], [1216, 1088, 1024], [1280, 1152, 1088]),
        ([212, 2, 3, 1], [2432, 2176, 2048], [2560, 2304, 2176]),
        ([20, 130, 131, 1], [4864, 4352, 4096], [5120, 4608, 4352]),
        ([212, 130, 131, 1], [9728, 512, 1024], [10240, 1024, 1536]),
        ([20, 130, 3, 1], [544, 608, 512], [576, 640, 544]),
        ([20, 2, 131, 1], [512, 544, 608], [544, 576, 640]),
        ([0, 31, 31, 31], [0, 0, 0], [0, 0, 0]),
    ] {
        let [r0, g0, b0] = low;
        let [r1, g1, b1] = high;
        assert_eq!(
            unpack(7, &input),
            Some(([[r0, g0, b0, 0x7800], [r1, g1, b1, 0x7800],], [true; 4])),
            "{input:?}"
        );
    }
}

#[test]
fn rgb_direct_all_eight_submodes_sign_extend_deltas() {
    // The final 127 contains a negative signed delta in every submode.
    // In submodes 3..7 its upper bits also contribute to A or C.
    for (input, low, high) in [
        (
            [100, 10, 3, 5, 1, 127],
            [11520, 11008, 11008],
            [12800, 12416, 12160],
        ),
        (
            [100, 138, 3, 5, 1, 127],
            [11520, 11008, 0],
            [12800, 12416, 0],
        ),
        (
            [100, 10, 131, 5, 1, 127],
            [5760, 5504, 5504],
            [6400, 6208, 6080],
        ),
        (
            [100, 138, 131, 5, 1, 127],
            [1664, 1408, 1408],
            [6400, 6208, 6080],
        ),
        (
            [100, 10, 3, 133, 1, 127],
            [35648, 35520, 31424],
            [35968, 35872, 31712],
        ),
        (
            [100, 138, 3, 133, 1, 127],
            [832, 704, 704],
            [3200, 3104, 3040],
        ),
        (
            [100, 10, 131, 133, 1, 127],
            [16800, 16736, 16736],
            [17984, 17936, 17904],
        ),
        (
            [100, 138, 131, 133, 1, 127],
            [416, 352, 352],
            [1600, 1552, 1520],
        ),
    ] {
        let [r0, g0, b0] = low;
        let [r1, g1, b1] = high;
        assert_eq!(
            unpack(11, &input),
            Some(([[r0, g0, b0, 0x7800], [r1, g1, b1, 0x7800],], [true; 4])),
            "{input:?}"
        );
    }
}

#[test]
fn rgb_direct_major_rotation_and_flat_encoding() {
    for (input, low, high) in [
        (
            [100, 10, 3, 5, 129, 127],
            [11008, 11520, 11008],
            [12416, 12800, 12160],
        ),
        (
            [100, 10, 3, 5, 1, 255],
            [11008, 11008, 11520],
            [12160, 12416, 12800],
        ),
        (
            [10, 20, 30, 40, 133, 135],
            [2560, 7680, 2560],
            [5120, 10240, 3584],
        ),
    ] {
        let [r0, g0, b0] = low;
        let [r1, g1, b1] = high;
        assert_eq!(
            unpack(11, &input),
            Some(([[r0, g0, b0, 0x7800], [r1, g1, b1, 0x7800],], [true; 4])),
            "{input:?}"
        );
    }
}

#[test]
fn mixed_alpha_replication_and_hdr_alpha_selectors() {
    assert_eq!(
        unpack(14, &[10, 20, 30, 40, 133, 135, 9, 27]),
        Some((
            [[2560, 7680, 2560, 2313], [5120, 10240, 3584, 6939],],
            [true, true, true, false]
        ))
    );
    for (alpha, low, high) in [
        ([10, 2], 2560, 3072),
        ([138, 31], 1280, 1152),
        ([10, 159], 8832, 8768),
        ([138, 148], 5120, 10240),
        ([0, 32], 0, 0),
        ([127, 127], 65280, 65024),
        ([127, 95], 65280, 65520),
    ] {
        let [a0, a1] = alpha;
        assert_eq!(
            unpack(15, &[10, 20, 30, 40, 133, 135, a0, a1]),
            Some((
                [[2560, 7680, 2560, low], [5120, 10240, 3584, high],],
                [true; 4]
            )),
            "{alpha:?}"
        );
    }
}

#[test]
fn ldr_endpoints_keep_linear_replication_and_no_hdr_lanes() {
    assert_eq!(
        unpack(0, &[10, 200]),
        Some((
            [[2570, 2570, 2570, 65535], [51400, 51400, 51400, 65535],],
            [false; 4]
        ))
    );
    assert_eq!(
        unpack(12, &[10, 20, 30, 40, 50, 60, 70, 80]),
        Some((
            [[2570, 7710, 12850, 17990], [5140, 10280, 15420, 20560],],
            [false; 4]
        ))
    );
}

#[test]
fn invalid_modes_and_every_truncated_endpoint_are_rejected() {
    for mode in 0..=15 {
        let count = usize::from((mode >> 2) + 1) * 2;
        for len in 0..count {
            assert_eq!(unpack(mode, &[0; 8][..len]), None, "mode {mode}, len {len}");
        }
        assert!(unpack(mode, &[0; 8]).is_some(), "mode {mode}");
    }
    for mode in 16..=255 {
        assert_eq!(unpack(mode, &[0; 8]), None);
    }
}
