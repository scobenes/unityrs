use super::{Kind, decode};

/// One pixel of `kind` from `stored`.
fn one(kind: Kind, stored: &[u8], out_bytes: usize) -> Vec<u8> {
    let mut out = vec![0xAA; out_bytes];
    decode(kind, stored, 1, 1, &mut out);
    out
}

/// Each unpacker on one pixel, as the module docs say (the oracle's own
/// checks, every value of each field, are `tests/raw_oracle.rs`').
#[test]
fn each_unpacker_moves_the_bands_its_docs_say() {
    assert_eq!(one(Kind::A, &[7], 4), [0, 0, 0, 7]);
    assert_eq!(one(Kind::R, &[9], 3), [9, 0, 0]);
    assert_eq!(one(Kind::Bgr, &[1, 2, 3], 3), [3, 2, 1]);
    assert_eq!(one(Kind::Bgra, &[1, 2, 3, 4], 4), [3, 2, 1, 4]);
    assert_eq!(one(Kind::Argb, &[1, 2, 3, 4], 4), [2, 3, 4, 1]);
    assert_eq!(one(Kind::Rgb, &[1, 2, 3], 3), [1, 2, 3]);
    assert_eq!(one(Kind::Rgba, &[1, 2, 3, 4], 4), [1, 2, 3, 4]);
    // 0x4321: r 1, g 2, b 3, a 4, each times 17.
    assert_eq!(
        one(Kind::Rgba4BArgb, &[0x21, 0x43], 4),
        [0x33, 0x22, 0x11, 0x44]
    );
    assert_eq!(
        one(Kind::Rgba4BRgba, &[0x21, 0x43], 4),
        [0x44, 0x33, 0x22, 0x11]
    );
    // Red 16 is 16 * 255 / 31 = 131, where bit replication gives 132.
    assert_eq!(one(Kind::Bgr16, &[0x00, 0x80], 3), [131, 0, 0]);
    assert_eq!(one(Kind::Bgr16, &[0xFF, 0xFF], 3), [255, 255, 255]);
    // Green 1 is 255 / 63 = 4; blue 1 is 255 / 31 = 8.
    assert_eq!(one(Kind::Bgr16, &[0x21, 0x00], 3), [0, 4, 8]);
}

/// Rows are flipped, and only `h` rows of the data are read.
#[test]
fn rows_are_flipped_and_the_rest_ignored() {
    let mut out = vec![0; 2 * 2 * 3];
    decode(Kind::R, &[1, 2, 3, 4, 5, 6], 2, 2, &mut out);
    assert_eq!(out, [3, 0, 0, 4, 0, 0, 1, 0, 0, 2, 0, 0]);
}
