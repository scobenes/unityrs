// Ported from Little CMS 2.15 (MIT): src/cmsintrp.c's TetrahedralInterp16,
// src/lcms2_internal.h's fixed-domain and eight-bit conversion macros.
// Copyright (c) 1998-2023 Marti Maria Saguer. See LICENSE-LCMS.
// Pillow 9.5.0's _imagingcms.c supplies the LAB formatter and default profiles.
//! The pinned wheel's 33-cubed CLUT, extracted by `tools/fixtures/set_image_lab.py`.
//! Integer tetrahedral interpolation avoids a runtime colour-library dependency.
const TABLE: &[u8; 215_622] = include_bytes!("lab.bin");

pub(super) fn convert(l: u8, a: u8, b: u8) -> [u8; 4] {
    let fixed = |v: u8| {
        let x = u32::from(v) * 257 * 32;
        x + (x + 0x7fff) / 0xffff
    };
    // Pillow's raw LAB packer offsets signed a/b by 128; ImageCms sees
    // the internal unsigned channel bytes before that packing.
    let positions = [fixed(l), fixed(a ^ 128), fixed(b ^ 128)];
    // Three-element sorting network, entirely on the stack.
    let fraction = |axis| positions.get(axis).copied().unwrap_or(0) & 0xffff;
    let (mut first, mut second, mut third) = (0usize, 1, 2);
    if fraction(first) < fraction(second) {
        std::mem::swap(&mut first, &mut second);
    }
    if fraction(second) < fraction(third) {
        std::mem::swap(&mut second, &mut third);
    }
    if fraction(first) < fraction(second) {
        std::mem::swap(&mut first, &mut second);
    }
    let order = [first, second, third];
    let strides = [33 * 33 * 3, 33 * 3, 3];
    let base = positions
        .iter()
        .zip(strides)
        .map(|(&v, stride)| (v >> 16) as usize * stride)
        .sum::<usize>();
    let at = |n: usize| {
        i64::from(u16::from_le_bytes([
            TABLE.get(n * 2).copied().unwrap_or(0),
            TABLE.get(n * 2 + 1).copied().unwrap_or(0),
        ]))
    };
    let channel = |c| {
        let c0 = at(base + c);
        let mut offset = base + c;
        let mut previous = c0;
        let mut rest = 0x8001i64;
        for axis in order {
            let position = positions.get(axis).copied().unwrap_or(0);
            if position & 0xffff != 0 {
                offset += strides.get(axis).copied().unwrap_or(0);
            }
            let next = at(offset);
            rest += (next - previous) * i64::from(position & 0xffff);
            previous = next;
        }
        let v = (c0 + ((rest + (rest >> 16)) >> 16)).rem_euclid(65536);
        u8::try_from((v * 65_281 + 8_388_608) >> 24).unwrap_or(0)
    };
    [channel(0), channel(1), channel(2), 255]
}

#[cfg(test)]
mod tests {
    use sha2::{Digest, Sha256};
    #[test]
    fn all_lab_colours_replay_pillows_default_transform() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/set_image.json")).unwrap();
        let mut hash = Sha256::new();
        for l in 0..=255u8 {
            for a in 0..=255u8 {
                let mut row = [0u8; 1024];
                for b in 0..=255u8 {
                    row[usize::from(b) * 4..usize::from(b) * 4 + 4]
                        .copy_from_slice(&super::convert(l, a, b));
                }
                hash.update(row);
            }
        }
        assert_eq!(
            hash.finalize().as_slice(),
            fixture["lab_rgba"]["sha256"]
                .as_str()
                .unwrap()
                .as_bytes()
                .as_chunks::<2>()
                .0
                .iter()
                .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
                .collect::<Vec<_>>()
        );
    }
}
