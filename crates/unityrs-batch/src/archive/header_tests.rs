use super::*;
use crate::{phase::Account, winners::layout::Layout};

fn bytes(hex: &str) -> Vec<u8> {
    let (pairs, remainder) = hex.as_bytes().as_chunks::<2>();
    assert_eq!(remainder, b"");
    pairs
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

#[test]
fn local_and_central_extras_at_all_size_and_offset_edges() {
    // Complete, independent section-8 fixtures (Python struct formats
    // <I5H3I2H and <I6H3I5H2I). Extra ID, data length, order and every
    // surrounding field are compared. No payloads of these sizes are allocated.
    // (size, local-header offset, local bytes, central bytes)
    let cases = [
        (
            0xffff_fffe,
            0xffff_fffe,
            "504b03041400000800000000210078563412fefffffffeffffff0100000061",
            "504b010214001400000800000000210078563412fefffffffeffffff0100000000000000000000000000feffffff61",
        ),
        (
            0xffff_ffff,
            0xffff_fffe,
            "504b03042d00000800000000210078563412ffffffffffffffff010014006101001000ffffffff00000000ffffffff00000000",
            "504b010214002d00000800000000210078563412ffffffffffffffff0100140000000000000000000000feffffff6101001000ffffffff00000000ffffffff00000000",
        ),
        (
            0x1_0000_0000,
            0xffff_fffe,
            "504b03042d00000800000000210078563412ffffffffffffffff01001400610100100000000000010000000000000001000000",
            "504b010214002d00000800000000210078563412ffffffffffffffff0100140000000000000000000000feffffff610100100000000000010000000000000001000000",
        ),
        (
            0xffff_fffe,
            0xffff_ffff,
            "504b03041400000800000000210078563412fefffffffeffffff0100000061",
            "504b010214002d00000800000000210078563412fefffffffeffffff01000c0000000000000000000000ffffffff6101000800ffffffff00000000",
        ),
        (
            0xffff_ffff,
            0xffff_ffff,
            "504b03042d00000800000000210078563412ffffffffffffffff010014006101001000ffffffff00000000ffffffff00000000",
            "504b010214002d00000800000000210078563412ffffffffffffffff01001c0000000000000000000000ffffffff6101001800ffffffff00000000ffffffff00000000ffffffff00000000",
        ),
        (
            0x1_0000_0000,
            0xffff_ffff,
            "504b03042d00000800000000210078563412ffffffffffffffff01001400610100100000000000010000000000000001000000",
            "504b010214002d00000800000000210078563412ffffffffffffffff01001c0000000000000000000000ffffffff610100180000000000010000000000000001000000ffffffff00000000",
        ),
        (
            0xffff_fffe,
            0x1_0000_0000,
            "504b03041400000800000000210078563412fefffffffeffffff0100000061",
            "504b010214002d00000800000000210078563412fefffffffeffffff01000c0000000000000000000000ffffffff61010008000000000001000000",
        ),
        (
            0xffff_ffff,
            0x1_0000_0000,
            "504b03042d00000800000000210078563412ffffffffffffffff010014006101001000ffffffff00000000ffffffff00000000",
            "504b010214002d00000800000000210078563412ffffffffffffffff01001c0000000000000000000000ffffffff6101001800ffffffff00000000ffffffff000000000000000001000000",
        ),
        (
            0x1_0000_0000,
            0x1_0000_0000,
            "504b03042d00000800000000210078563412ffffffffffffffff01001400610100100000000000010000000000000001000000",
            "504b010214002d00000800000000210078563412ffffffffffffffff01001c0000000000000000000000ffffffff6101001800000000000100000000000000010000000000000001000000",
        ),
    ];
    let account = Account::new(1 << 20);
    let mut out = Buffer::new(account.meter(), 4096).unwrap();
    for (size, offset, local_hex, central_hex) in cases {
        let mut layout = Layout::seed(offset, 0, 0).unwrap();
        let entry = layout.member(1, size).unwrap();
        local(&mut out, "a", size, 0x1234_5678, entry).unwrap();
        assert_eq!(&*out, bytes(local_hex), "local size={size} offset={offset}");
        central(&mut out, "a", size, 0x1234_5678, entry).unwrap();
        assert_eq!(
            &*out,
            bytes(central_hex),
            "central size={size} offset={offset}"
        );
    }
}

#[test]
fn end_fields_saturate_independently_and_locator_is_logical() {
    // Complete independent EOCD/ZIP64 EOCD/locator bytes from section 8
    // (<I4H2IH, <IQ2H2I4Q, <IIQI). Non-target fields stay distinct and
    // nonzero, so saturating unrelated fields also fails the comparison.
    // (central offset, central size, member count, complete end bytes)
    let cases = [
        (
            0xffff_fffe,
            0x13,
            0x7,
            "504b0506000000000700070013000000feffffff0000",
        ),
        (
            0xffff_ffff,
            0x13,
            0x7,
            "504b06062c000000000000002d002d000000000000000000070000000000000007000000000000001300000000000000ffffffff00000000504b060700000000120000000100000001000000504b0506000000000700070013000000ffffffff0000",
        ),
        (
            0x1_0000_0000,
            0x13,
            0x7,
            "504b06062c000000000000002d002d0000000000000000000700000000000000070000000000000013000000000000000000000001000000504b060700000000130000000100000001000000504b0506000000000700070013000000ffffffff0000",
        ),
        (
            0x17,
            0xffff_fffe,
            0x7,
            "504b05060000000007000700feffffff170000000000",
        ),
        (
            0x17,
            0xffff_ffff,
            0x7,
            "504b06062c000000000000002d002d00000000000000000007000000000000000700000000000000ffffffff000000001700000000000000504b060700000000160000000100000001000000504b05060000000007000700ffffffff170000000000",
        ),
        (
            0x17,
            0x1_0000_0000,
            0x7,
            "504b06062c000000000000002d002d0000000000000000000700000000000000070000000000000000000000010000001700000000000000504b060700000000170000000100000001000000504b05060000000007000700ffffffff170000000000",
        ),
        (
            0x17,
            0x13,
            0xfffe,
            "504b050600000000fefffeff13000000170000000000",
        ),
        (
            0x17,
            0x13,
            0xffff,
            "504b06062c000000000000002d002d000000000000000000ffff000000000000ffff00000000000013000000000000001700000000000000504b0607000000002a0000000000000001000000504b050600000000ffffffff13000000170000000000",
        ),
        (
            0x17,
            0x13,
            0x1_0000,
            "504b06062c000000000000002d002d0000000000000000000000010000000000000001000000000013000000000000001700000000000000504b0607000000002a0000000000000001000000504b050600000000ffffffff13000000170000000000",
        ),
    ];
    let account = Account::new(1 << 20);
    let mut out = Buffer::new(account.meter(), 4096).unwrap();
    for (offset, size, count, expected) in cases {
        let layout = Layout::seed(offset, size, count).unwrap();
        end(&mut out, layout.finish().unwrap()).unwrap();
        assert_eq!(
            &*out,
            bytes(expected),
            "offset={offset} size={size} count={count}"
        );
    }
}

#[test]
fn ordinary_headers_equal_independent_complete_bytes() {
    let account = Account::new(1 << 20);
    let mut out = Buffer::new(account.meter(), 4096).unwrap();
    let mut layout = Layout::default();
    let e = layout.member(1, 1).unwrap();
    local(&mut out, "a", 1, 0x8cdc_1683, e).unwrap();
    assert_eq!(
        &*out,
        &[
            0x50, 0x4b, 3, 4, 20, 0, 0, 8, 0, 0, 0, 0, 0x21, 0, 0x83, 0x16, 0xdc, 0x8c, 1, 0, 0, 0,
            1, 0, 0, 0, 1, 0, 0, 0, b'a',
        ]
    );
    central(&mut out, "a", 1, 0x8cdc_1683, e).unwrap();
    assert_eq!(
        &*out,
        &[
            0x50, 0x4b, 1, 2, 20, 0, 20, 0, 0, 8, 0, 0, 0, 0, 0x21, 0, 0x83, 0x16, 0xdc, 0x8c, 1,
            0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, b'a',
        ]
    );
    end(&mut out, layout.finish().unwrap()).unwrap();
    assert_eq!(
        &*out,
        &[
            0x50, 0x4b, 5, 6, 0, 0, 0, 0, 1, 0, 1, 0, 47, 0, 0, 0, 32, 0, 0, 0, 0, 0
        ]
    );
}

#[test]
fn exact_final_publication_limit_is_allowed() {
    use crate::winners::layout::LIMIT;
    for size in [LIMIT - 1, LIMIT, LIMIT + 1] {
        let mut layout = Layout::default();
        layout.member(1, size - 216).unwrap();
        let end = layout.finish().unwrap();
        assert_eq!(end.size, size);
        assert_eq!(end.oversized(), size > LIMIT);
    }
}
