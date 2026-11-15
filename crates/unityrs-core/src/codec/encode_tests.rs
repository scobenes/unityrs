use super::*;
use crate::budget::{Reservation, Unbounded};
use crate::codec::{BlockCodec, Decoded, Lz4, Lzma};
use crate::limits::ChargeError;

#[test]
fn every_packer_roundtrips_and_releases_its_charge() {
    let data = b"Bundle payload with a repeated repeated repeated tail.".repeat(1024);
    for codec in [Codec::None, Codec::Lzma, Codec::Lz4, Codec::Lz4Hc] {
        let meter = Meter::unlimited();
        let packed = compress(&data, codec, &meter).unwrap();
        assert!(meter.used() > 0);
        if codec == Codec::None {
            assert_eq!(packed.as_ref(), data);
        } else {
            let mut out =
                DecodeBuf::new(&meter, 0, data.len() as u64, LimitKind::BlockOutput).unwrap();
            let decoder: &dyn BlockCodec = if codec == Codec::Lzma { &Lzma } else { &Lz4 };
            assert_eq!(
                decoder
                    .decode(&packed, u32::try_from(data.len()).unwrap(), &mut out)
                    .unwrap(),
                Decoded::Written
            );
            assert_eq!(out.as_slice(), data);
        }
        drop(packed);
        assert_eq!(meter.used(), 0, "{codec:?}");
    }
}

#[test]
fn chunk_rows_preserve_flags_and_store_only_growing_chunks() {
    let meter = Meter::unlimited();
    let mut data = vec![0; 0x20000];
    data.push(7);
    let result = chunk_based_compress(&data, 0x243, &meter).unwrap();
    assert_eq!(result.blocks.len(), 2);
    let rows = result.blocks.as_slice();
    assert_eq!(rows[0].uncompressed, 0x20000);
    assert_eq!(rows[0].flags, 0x243);
    assert!(rows[0].compressed < rows[0].uncompressed);
    assert_eq!(
        rows[1],
        BlockInfo {
            uncompressed: 1,
            compressed: 1,
            flags: 0x240
        }
    );
    assert_eq!(result.data.last(), Some(&7));
    drop(result);
    assert_eq!(meter.used(), 0);
}

#[test]
fn empty_payload_rows_and_unsupported_codecs_follow_the_packer() {
    let meter = Meter::unlimited();
    let stored = chunk_based_compress(&[], 0x240, &meter).unwrap();
    assert_eq!(
        stored.blocks.as_slice(),
        &[BlockInfo {
            uncompressed: 0,
            compressed: 0,
            flags: 0x240
        }]
    );
    for flags in [1, 2, 3] {
        let packed = chunk_based_compress(&[], flags, &meter).unwrap();
        assert!(packed.data.is_empty());
        assert!(packed.blocks.is_empty());
    }
    for codec in [Codec::Lzham, Codec::Other(5), Codec::Other(1)] {
        assert_eq!(
            compress(&[], codec, &meter),
            Err(SaveError::UnsupportedCodec(codec))
        );
    }
    assert!(matches!(
        chunk_based_compress(&[], 4, &meter),
        Err(SaveError::UnsupportedCodec(Codec::Lzham))
    ));
}

#[test]
fn refused_encoder_state_returns_all_charges() {
    for codec in [Codec::Lzma, Codec::Lz4, Codec::Lz4Hc] {
        let meter = Meter::new(1024, Arc::new(Unbounded), Reservation::Input);
        assert!(matches!(
            compress(b"payload", codec, &meter),
            Err(SaveError::Charge(ChargeError::Limit(_)))
        ));
        assert_eq!(meter.used(), 0);
    }
}

#[test]
fn refused_work_stops_before_compression_and_releases_state() {
    for codec in [Codec::Lzma, Codec::Lz4, Codec::Lz4Hc] {
        let meter = Meter::with_decode_work(u64::MAX, 3, Arc::new(Unbounded), Reservation::Input);
        assert!(matches!(
            compress(b"payload", codec, &meter),
            Err(SaveError::Charge(ChargeError::Limit(_)))
        ));
        assert_eq!(meter.used(), 0);
    }
}

#[test]
fn hc_state_zeroing_is_charged_as_writer_work() {
    let state_bytes = u64::try_from(unityrs_compress_sys::lz4_state_words().unwrap()).unwrap() * 8;
    let meter = Meter::with_decode_work(
        u64::MAX,
        state_bytes - 1,
        Arc::new(Unbounded),
        Reservation::Input,
    );
    assert!(matches!(
        compress(b"", Codec::Lz4Hc, &meter),
        Err(SaveError::Charge(ChargeError::Limit(_)))
    ));
    assert_eq!(meter.used(), 0);
}
