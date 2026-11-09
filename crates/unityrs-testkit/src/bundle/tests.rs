use super::{BlockOverride, BundleSpec, EntrySpec, TestCodec, build, compress, new_flags};

fn spec(codec: TestCodec) -> BundleSpec {
    BundleSpec {
        data_codec: codec,
        info_codec: codec,
        block_size: 64,
        entries: vec![
            EntrySpec {
                name: b"CAB-a".to_vec(),
                data: (0..100u8).collect(),
                flags: 4,
            },
            EntrySpec {
                name: b"CAB-a.resS".to_vec(),
                data: vec![9; 50],
                flags: 0,
            },
        ],
        ..BundleSpec::default()
    }
}

#[test]
fn the_header_is_big_endian_and_aligned_at_format_7() {
    let b = build(&spec(TestCodec::None));
    assert!(b.starts_with(b"UnityFS\0\0\0\0\x07"));
    let at = 8 + 4 + 6 + 11;
    let total = i64::from_be_bytes(b[at..at + 8].try_into().unwrap());
    assert_eq!(usize::try_from(total).unwrap(), b.len());
    let flags = u32::from_be_bytes(b[at + 16..at + 20].try_into().unwrap());
    assert_eq!(flags, 0x40);
    // Header 49 bytes, aligned to 64; blocks-info: 16 + 4 + 3 * 10 + 4
    // + 2 rows; then 150 data bytes.
    let info = 16 + 4 + 30 + 4 + (20 + 6) + (20 + 11);
    assert_eq!(b.len(), 64 + info + 150);
}

/// `encrypted` sets the flag set's encryption bit and writes the
/// decryptor's 70 bytes (a `u32`, two vectors of 16 + 16 bytes and a skipped
/// byte) after the data flags, before the alignment.
#[test]
fn an_encrypted_header_holds_the_decryptors_bytes() {
    let at = 8 + 4 + 6 + 11;
    let flags = |b: &[u8]| u32::from_be_bytes(b[at + 16..at + 20].try_into().unwrap());
    for (engine, bit) in [(&b"2022.3.0f1"[..], 0x1000), (&b"2019.4.0f1"[..], 0x200)] {
        let plain = BundleSpec {
            version_engine: engine.to_vec(),
            ..spec(TestCodec::None)
        };
        let encrypted = build(&BundleSpec {
            encrypted: true,
            ..plain.clone()
        });
        let plain = build(&plain);
        assert_eq!(flags(&encrypted), flags(&plain) | bit);
        assert_eq!(&encrypted[at + 20..at + 90], &[0; 70]);
        // 49 + 70 bytes of header align to 128, where 49 aligned to 64.
        assert_eq!(encrypted.len(), plain.len() + 64);
    }
}

#[test]
fn every_codec_round_trips() {
    let data: Vec<u8> = (0..5000u32).map(|i| u8::try_from(i % 7).unwrap()).collect();
    let lz4 = compress(TestCodec::Lz4, &data);
    assert_eq!(lz4_flex::block::decompress(&lz4, data.len()).unwrap(), data);
    let lzma = compress(TestCodec::Lzma, &data);
    assert_eq!(lzma[0], 0x5D);
    assert!(lzma.len() < data.len());
    for c in [TestCodec::Lzma, TestCodec::Lz4, TestCodec::Lz4Hc] {
        let b = build(&spec(c));
        assert!(b.len() > 64, "{c:?}");
    }
}

#[test]
fn overrides_and_truncation() {
    let mut s = spec(TestCodec::None);
    s.block_overrides = vec![BlockOverride {
        block: 1,
        codec: Some(TestCodec::Raw(9)),
        uncompressed: Some(7),
        compressed: None,
    }];
    s.truncate_to = Some(100);
    assert_eq!(build(&s).len(), 100);
    assert!(new_flags(b"2022.3.0f1"));
    assert!(!new_flags(b"2020.3.33f1"));
    assert!(!new_flags(b"2019.4.40f1"));
    assert!(new_flags(b"2021.3.2f1"));
}

#[test]
fn an_lzma_dictionary_size_is_a_field_memo_or_not() {
    use super::{LZMA_DICT, compress};
    use crate::writer::{Field, with_fields};
    let data: Vec<u8> = (0..300u32).map(|i| (i % 7) as u8).collect();
    let dict = Field {
        width: 4,
        big_endian: false,
        value: u64::from(LZMA_DICT),
    };
    // Encoded, then served from the memo: the same bytes and fields.
    let (first, f1) = with_fields(&[], || compress(TestCodec::Lzma, &data));
    let (again, f2) = with_fields(&[], || compress(TestCodec::Lzma, &data));
    assert_eq!((&first, &f1), (&again, &f2));
    assert_eq!(f1, [dict]);
    assert_eq!(first[1..5], LZMA_DICT.to_le_bytes());
    // Replaced, it is written as given, the stream unchanged.
    let (huge, _) = with_fields(&[(0, 0xFFFF_FFFF)], || compress(TestCodec::Lzma, &data));
    assert_eq!(huge[1..5], [0xFF; 4]);
    assert_eq!((huge[0], &huge[5..]), (first[0], &first[5..]));
}
