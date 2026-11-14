//! The block decoder's own rules: the UNORM8 store, void extent and error
//! blocks. Whole blocks against astcenc are the oracle's
//! (`tests/astc_oracle.rs`).

use super::super::context::{Footprint, Tables};
use super::{ERROR, MAX_TEXELS, decode, lerp};
use unityrs_core::limits::Meter;

/// astcenc_vecmathlib.h:496-524, `unorm16_to_sf16`, for one lane.
fn unorm16_to_sf16(p: u32) -> u32 {
    if p == 0xffff {
        return 0x3c00;
    }
    if p < 4 {
        return p << 8;
    }
    let lz = p.leading_zeros() - 16;
    let p = ((p << (lz + 1)) & 0xffff) >> 6;
    p | ((14 - lz) << 10)
}

/// An FP16 bit pattern as a float, exactly (`sf16_to_float`).
fn sf16_to_float(h: u32) -> f32 {
    let (exp, man) = ((h >> 10) & 31, h & 1023);
    match exp {
        0 => f32::from(u16::try_from(man).unwrap()) * 2f32.powi(-24),
        _ => f32::from_bits(((exp + 112) << 23) | (man << 13)),
    }
}

/// astcenc_image.cpp:391-394 for one channel of value `v16`: through FP16,
/// then `(int)(min(f, 1) * 255 + 0.5)`, the multiply and add fused or not
/// (astcenc_vecmathlib_sse_4.h:939-943 truncates after the add).
#[allow(
    clippy::cast_possible_truncation,
    reason = "C's `(int)` conversion, truncating toward 0, is what the store does"
)]
fn store(v16: u32, fused: bool) -> u8 {
    let f = sf16_to_float(unorm16_to_sf16(v16)).min(1.0);
    let scaled = if fused {
        f.mul_add(255.0, 0.5)
    } else {
        f * 255.0 + 0.5
    };
    u8::try_from(scaled as i32).unwrap()
}

/// `decode_unorm8` hands the store a byte replicated to 16 bits
/// (astcenc_decompress_symbolic.cpp:52-53), and the store gives it back,
/// so the decoder writes the byte.
#[test]
fn the_unorm8_store_keeps_the_byte() {
    for v in 0..=255u32 {
        assert_eq!(u32::from(store(v * 257, false)), v);
        assert_eq!(u32::from(store(v * 257, true)), v);
    }
    // The conversion itself: 1.0, the largest value below it, and a
    // denormal-range input.
    assert_eq!(unorm16_to_sf16(0xffff), 0x3c00);
    assert_eq!(sf16_to_float(unorm16_to_sf16(0x8080)), 0.501_953_1);
    assert_eq!(unorm16_to_sf16(2), 0x200);
    // And the interpolation's top byte.
    assert_eq!(lerp(0, 0xffff, 0), 0);
    assert_eq!(lerp(0, 0xffff, 64), 255);
    assert_eq!(lerp(0, 0xffff, 32), 128);
    assert_eq!(lerp(0x1010, 0x1010, 17), 0x10);
}

fn block(fp: (u8, u8), bytes: [u8; 16]) -> Vec<[u8; 4]> {
    let fp = Footprint::new(fp.0, fp.1).unwrap();
    let mut tables = Tables::new(fp, &Meter::unlimited()).unwrap();
    let mut out = [7; 4 * MAX_TEXELS];
    decode(&mut tables, &bytes, &mut out, ERROR);
    out.as_chunks::<4>()
        .0
        .iter()
        .take(fp.texels())
        .copied()
        .collect()
}

/// A void-extent block: its UNORM16 colour's top bytes, with extents
/// all ones or ordered; an error block when its reserved bits are not 3,
/// when an extent is empty, or when its colour is FP16.
#[test]
fn void_extent_blocks() {
    let colour = [0x00, 0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde];
    let mut ones = [0xff; 16];
    ones[0] = 0xfc;
    ones[1] = 0xfd;
    ones[8..].copy_from_slice(&colour);
    assert!(
        block((6, 5), ones)
            .iter()
            .all(|p| *p == [0x12, 0x56, 0x9a, 0xde])
    );
    // FP16 (bit 9).
    let mut hdr = ones;
    hdr[1] |= 0x02;
    assert!(block((4, 4), hdr).iter().all(|p| *p == ERROR));
    // Reserved bits 10 and 11 not both set.
    let mut reserved = ones;
    reserved[1] &= !0x08;
    assert!(block((4, 4), reserved).iter().all(|p| *p == ERROR));
    // Extents: S from 0x1000 and T from 0x1c03 up to 0x1fff are ordered;
    // T up to 0x1f is empty.
    let mut ordered = ones;
    ordered[1] = 0x0d;
    ordered[2] = 0;
    ordered[5] = 0;
    assert!(
        block((4, 4), ordered)
            .iter()
            .all(|p| *p == [0x12, 0x56, 0x9a, 0xde])
    );
    let mut empty = ones;
    empty[7] = 0;
    assert!(block((4, 4), empty).iter().all(|p| *p == ERROR));
}

/// Reserved and footprint-illegal block modes are error blocks.
#[test]
fn illegal_modes_are_error_blocks() {
    // Mode 0: reserved; a 12x2 grid (mode 4) does not fit 4x4.
    for mode in [0u16, 4, 0x1fd] {
        let mut b = [0x5a; 16];
        b[0] = (mode & 0xff) as u8;
        b[1] = (b[1] & !7) | (mode >> 8) as u8;
        assert!(block((4, 4), b).iter().all(|p| *p == ERROR), "mode {mode}");
    }
    let meter = Meter::unlimited();
    let mut small = Tables::new(Footprint::new(4, 4).unwrap(), &meter).unwrap();
    let mut large = Tables::new(Footprint::new(12, 12).unwrap(), &meter).unwrap();
    assert!(small.mode(4).is_none() && large.mode(4).is_some());
}
