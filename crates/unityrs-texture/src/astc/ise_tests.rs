use super::super::{
    ise::{self, Quant},
    tables,
};
use super::put;

/// A test-only spec encoder: choose the packed trit/quint code with the
/// desired digit prefix, using zero for the absent high code bits. This
/// deliberately handles tails independently of the decoder's collection.
fn encode(q: Quant, values: &[u8], offset: usize) -> Vec<u8> {
    let mut out = vec![0; (offset + q.bit_count(values.len()).unwrap()).div_ceil(8)];
    let (bits, radix) = q.layout();
    let widths: &[usize] = match radix {
        3 => &[2, 2, 1, 2, 1],
        5 => &[3, 2, 2],
        _ => &[0],
    };
    let mut pos = offset;
    for group in values.chunks(widths.len()) {
        let nbits: usize = widths.iter().take(group.len()).sum();
        let code = match radix {
            3 => (0..1 << nbits)
                .find(|&code| {
                    group
                        .iter()
                        .zip(tables::TRITS_OF_INTEGER[code])
                        .all(|(&v, d)| v >> bits == d)
                })
                .unwrap(),
            5 => (0..1 << nbits)
                .find(|&code| {
                    group
                        .iter()
                        .zip(tables::QUINTS_OF_INTEGER[code])
                        .all(|(&v, d)| v >> bits == d)
                })
                .unwrap(),
            _ => 0,
        };
        let mut shift = 0;
        for (&v, &width) in group.iter().zip(widths) {
            put(
                &mut out,
                pos,
                usize::from(bits),
                u32::from(v & u8::try_from((1_u16 << bits) - 1).unwrap()),
            );
            pos += usize::from(bits);
            put(
                &mut out,
                pos,
                width,
                u32::try_from((code >> shift) & ((1 << width) - 1)).unwrap(),
            );
            pos += width;
            shift += width;
        }
    }
    out
}

#[test]
fn hand_decoded_bits_trits_quints() {
    // ASTC ISE tables: bit string 00000011 maps to trits 0,0,2,0,0,
    // not the base-3 digits of 3. Quint code 0000111 maps to 4,4,4.
    for (q, bytes, want) in [
        (0, vec![0b0101_1010], vec![0, 1, 0, 1, 1, 0, 1, 0]),
        (1, vec![3], vec![0, 0, 2, 0, 0]),
        (3, vec![7], vec![4, 4, 4]),
        (20, vec![0, 127, 255], vec![0, 127, 255]),
    ] {
        let mut out = vec![0; want.len()];
        ise::decode(Quant::new(q).unwrap(), &bytes, 0, &mut out).unwrap();
        assert_eq!(out, want);
    }
}

#[test]
fn round_trips_every_range_length_tail_and_bit_offset() {
    for index in 0..21 {
        let q = Quant::new(index).unwrap();
        for len in 0..=64 {
            for offset in 0..8 {
                let values: Vec<_> = (0..len)
                    .map(|i| u8::try_from((i * 37 + len * 11) % usize::from(q.levels())).unwrap())
                    .collect();
                let encoded = encode(q, &values, offset);
                let mut out = vec![0; len];
                ise::decode(q, &encoded, offset, &mut out).unwrap();
                assert_eq!(values, out, "q={index} n={len} offset={offset}");
                if len > 0 {
                    assert!(
                        ise::decode(q, &encoded[..encoded.len() - 1], offset, &mut out).is_none()
                    );
                }
            }
        }
    }
    assert_eq!(Quant::new(21), None);
    assert!(ise::decode(Quant::new(0).unwrap(), &[0; 16], 0, &mut [0; 65]).is_none());
}

#[test]
fn every_ise_code_including_redundant_codes() {
    // Independently compiled original C++ arrays, in stored row order.
    let digest = |values: &[u8]| {
        values.iter().fold(2_166_136_261_u32, |h, &v| {
            (h ^ u32::from(v)).wrapping_mul(16_777_619)
        })
    };
    assert_eq!(
        digest(tables::TRITS_OF_INTEGER.as_flattened()),
        2_537_031_502
    );
    assert_eq!(
        digest(tables::QUINTS_OF_INTEGER.as_flattened()),
        2_786_201_013
    );
    // The spec allows all 256 trit and 128 quint codes; the redundant
    // encodings must decode, not be treated as illegal ISE sequences.
    for (q, radix, width, count) in [(1, 3, 8, 5), (3, 5, 7, 3)] {
        let mut patterns = std::collections::BTreeSet::new();
        for code in 0..1 << width {
            let mut out = vec![0; count];
            ise::decode(
                Quant::new(q).unwrap(),
                &[u8::try_from(code).unwrap()],
                0,
                &mut out,
            )
            .unwrap();
            assert!(out.iter().all(|&v| v < radix));
            patterns.insert(out);
        }
        assert_eq!(
            patterns.len(),
            usize::from(radix).pow(u32::try_from(count).unwrap())
        );
    }
}

#[test]
fn weight_unquantization_covers_every_weight_range() {
    // astcenc_weight_quant_xfer_tables.cpp's decode maps, including the
    // 32->33 gap and the scrambled non-power-of-two indices.
    for index in 0..12 {
        let q = Quant::new(index).unwrap();
        let values: Vec<_> = (0..q.levels())
            .map(|v| q.unquant_weight(u8::try_from(v).unwrap()).unwrap())
            .collect();
        let mut sorted = values.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), usize::from(q.levels()));
        assert_eq!(sorted.first(), Some(&0));
        assert_eq!(sorted.last(), Some(&64));
        assert!(
            q.unquant_weight(u8::try_from(q.levels()).unwrap())
                .is_none()
        );
    }
    assert_eq!(Quant::new(2).unwrap().unquant_weight(2), Some(43));
    assert_eq!(Quant::new(4).unwrap().unquant_weight(1), Some(64));
    assert_eq!(Quant::new(11).unwrap().unquant_weight(16), Some(34));
    assert_eq!(Quant::new(12).unwrap().unquant_weight(0), None);
}
