//! Independent Python hashlib/zlib vectors for synthetic byte patterns.
use unityrs_batch::digest::{Hashes, Hex};
#[test]
fn multipart_boundaries_have_no_empty_trailing_part() {
    for (n, md5, crc, etag) in [
        (
            0,
            "d41d8cd98f00b204e9800998ecf8427e",
            0,
            "d41d8cd98f00b204e9800998ecf8427e",
        ),
        (
            16_777_215,
            "59bc7c3cf08fcff32cf523fcd58df01d",
            1_398_466_313,
            "59bc7c3cf08fcff32cf523fcd58df01d",
        ),
        (
            16_777_216,
            "f4820540fc0ac02750739896fe028d56",
            2_436_389_888,
            "f4820540fc0ac02750739896fe028d56",
        ),
        (
            16_777_217,
            "c2fa6bae9682ffe64b4d07bd5a7d02ac",
            3_894_838_815,
            "48e71d30757e1eff6c4e8a55fe9782fa-2",
        ),
        (
            33_554_432,
            "bc3d7c2ff64219e33239f2e13c2d21db",
            1_244_851_471,
            "93eeb5efa9d8c6cd4bb3b63ef5565684-2",
        ),
        (
            33_554_433,
            "e3e67aa924d2ad82c3e19f4f9bce93da",
            2_017_628_479,
            "78f6b3cc565d1920c53294ee0fbae1d5-3",
        ),
    ] {
        let h = Hashes::of(&vec![b'a'; n]);
        assert_eq!(Hex(&h.md5).to_string(), md5, "size {n}");
        assert_eq!(h.crc32, crc, "size {n}");
        assert_eq!(h.etag.to_string(), etag, "size {n}");
        assert_eq!(h.etag.is_multipart(), n > 16_777_216);
    }
}

#[test]
fn multipart_hashes_distinct_full_parts_in_order() {
    // Independently computed with Python hashlib and zlib, not Hashes::of:
    // P = 16 * 1024 * 1024; parts = [b'a' * P, b'b' * P] (+ [b'c']).
    // Their MD5 hex values are, in order:
    // f4820540fc0ac02750739896fe028d56
    // eb34153e9ed1e774db28cbbe4090a449
    // 4a8a08f09d37b73795649038408b5f33 (the one-byte tail).
    // ETag = md5(b''.join(bytes.fromhex(h) for h in part_hashes)).hexdigest()
    //        + '-' + str(len(part_hashes)); whole MD5/CRC use b''.join(parts).
    const PART: usize = 16 * 1024 * 1024;
    for (tail, md5, crc, etag) in [
        (
            false,
            "a05595467198d7fd344a5b257670595b",
            2_947_091_683,
            "280f90c005d489a4309c9ad711b0af48-2",
        ),
        (
            true,
            "6200a143941affa73ae1705f19881c0a",
            1_058_391_457,
            "48efdb9ee097d9d7b999803826530992-3",
        ),
    ] {
        let mut bytes = vec![b'a'; 2 * PART + usize::from(tail)];
        bytes[PART..2 * PART].fill(b'b');
        if tail {
            bytes[2 * PART] = b'c';
        }
        let h = Hashes::of(&bytes);
        assert_eq!(Hex(&h.md5).to_string(), md5, "tail {tail}");
        assert_eq!(h.crc32, crc, "tail {tail}");
        assert_eq!(h.etag.to_string(), etag, "tail {tail}");
        assert!(h.etag.is_multipart());
    }
}
