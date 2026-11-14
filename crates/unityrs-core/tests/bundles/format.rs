//! The format's edges: counts as Python ranges, decryption, unsupported
//! bundles, and mutated bundles.

use std::sync::Arc;

use super::{ctx, ctx_with, meter_for, open, open_with, small_entries};
use unityrs_core::codec::BlockDecryptor;
use unityrs_core::version::FlagsClass;
use unityrs_core::{Error, Unsupported};
use unityrs_testkit::Rng;
use unityrs_testkit::bundle::{BundleSpec, EntrySpec, TestCodec, build, matrix};

/// The block and directory counts as `range(count)` reads them (UnityPy
/// files/BundleFile.py:145-165): a negative count is no records, a huge one
/// fails at the end of the bytes with nothing allocated from the count.
#[test]
fn block_and_directory_counts_are_python_ranges() {
    // No entries and no blocks, blocks-info stored after the aligned header:
    // the hash (16 bytes), the block count, then the directory count.
    let base = build(&BundleSpec {
        info_codec: TestCodec::None,
        entries: Vec::new(),
        ..BundleSpec::default()
    });
    let nul = |from: usize| from + base[from..].iter().position(|c| *c == 0).unwrap() + 1;
    let start = (nul(nul(12)) + 20).next_multiple_of(16);
    assert_eq!(&base[start..start + 16], &[0; 16]);
    let empty = open(&base);
    assert!(empty.blocks().is_empty() && empty.directory().is_empty());
    for at in [start + 16, start + 20] {
        let mut neg = base.clone();
        neg[at..at + 4].copy_from_slice(&(-5i32).to_be_bytes());
        let b = open(&neg);
        assert!(b.blocks().is_empty() && b.directory().is_empty(), "{at}");
        let mut huge = base.clone();
        huge[at..at + 4].copy_from_slice(&i32::MAX.to_be_bytes());
        let c = ctx();
        let m = meter_for(&c);
        let got = open_with(&huge, &c, &m);
        assert!(got.is_err(), "{at}: {got:?}");
        assert_eq!(m.used(), 0, "everything returned");
        assert!(m.granted() < 4096, "{at}: {} granted", m.granted());
    }
}

/// A decryptor that XORs every byte with its key.
struct Xor(u8);

impl BlockDecryptor for Xor {
    fn decrypt(&self, block: &mut [u8], _index: usize) -> Result<(), Error> {
        for b in block {
            *b ^= self.0;
        }
        Ok(())
    }
}

/// A bundle of one LZ4 block of 20,000 bytes of noise (so the stored block
/// is as large as its output), the block's flags carrying `0x100`, and the
/// header's encryption flag set or not: under it the stored bytes are the
/// LZ4 block exclusive-or `0x5A`, for [`Xor`]; without it they are the
/// plain block. `(the bundle, the entry, the stored block's length)`.
fn flagged_block(encrypted: bool) -> (Vec<u8>, Vec<u8>, usize) {
    let mut rng = Rng::new(7);
    let entry: Vec<u8> = (0..20_000)
        .map(|_| rng.next_u64().to_le_bytes()[0])
        .collect();
    let mut data = build(&BundleSpec {
        info_codec: TestCodec::None,
        data_codec: TestCodec::Lz4,
        block_size: 1 << 20,
        entries: vec![EntrySpec {
            name: b"secret".to_vec(),
            data: entry.clone(),
            flags: 0,
        }],
        encrypted,
        ..BundleSpec::default()
    });
    let nul = |from: usize| from + data[from..].iter().position(|c| *c == 0).unwrap() + 1;
    // The one block's row: after the header (and the decryptor's 70 bytes),
    // aligned, the hash and the count.
    let header = nul(nul(12)) + 20 + if encrypted { 70 } else { 0 };
    let row = header.next_multiple_of(16) + 20;
    let stored = u32::from_be_bytes(data[row + 4..row + 8].try_into().unwrap()) as usize;
    let flags = u16::from_be_bytes([data[row + 8], data[row + 9]]) | 0x100;
    data[row + 8..row + 10].copy_from_slice(&flags.to_be_bytes());
    if encrypted {
        let len = data.len();
        for b in &mut data[len - stored..] {
            *b ^= 0x5A;
        }
    }
    (data, entry, stored)
}

/// UnityPy files/BundleFile.py:512-513: a block flagged 0x100 and not
/// stored, of a bundle whose header's encryption flag built a decryptor,
/// goes through the decryptor before its codec, in a charged copy that is
/// returned once the block is decoded. Without a decryptor such a bundle is
/// refused at open (UnityCN: P3).
#[test]
fn a_flagged_block_is_decrypted_in_a_charged_copy() {
    let (data, entry, stored) = flagged_block(true);
    let plain = ctx();
    assert!(matches!(
        open_with(&data, &plain, &meter_for(&plain)),
        Err(Error::UnityCnKeyMissing)
    ));
    let c = ctx_with(|c| c.decryptor = Some(Arc::new(Xor(0x5A))));
    let m = meter_for(&c);
    let b = open_with(&data, &c, &m).unwrap();
    assert!(b.header().encrypted);
    let held = m.used();
    let got = b.entry_bytes(0).unwrap();
    assert_eq!(got, entry);
    // The run is held; the decrypted copy (as large) was returned.
    assert!(stored > 15_000, "{stored}");
    assert!(m.used() >= held + 20_000);
    assert!(
        m.used() < held + 20_000 + u64::try_from(stored / 2).unwrap(),
        "{} over {held}, stored {stored}",
        m.used()
    );
}

/// Round 5 (MEDIUM): UnityPy's bundle has a decryptor only when its header's
/// encryption flag built one (`files/BundleFile.py:25`, `:123-124`), and
/// `decompress_data` decrypts only with one (`:512`), so the key a caller
/// set (`set_assetbundle_decrypt_key`, global) never touches a bundle whose
/// flag is clear: its block flagged `0x100` is decoded as stored, and the
/// entry is the plain bytes (UnityPy 1.25.2 on this input, with a key set:
/// `decryptor None`, the 20,000 bytes of the entry). The decryptor ran on it
/// before, and the LZ4 decode of the scrambled block failed, lazily and
/// eagerly.
#[test]
fn a_decryptor_reaches_only_a_bundle_whose_header_built_one() {
    let (data, entry, _) = flagged_block(false);
    let c = ctx_with(|c| c.decryptor = Some(Arc::new(Xor(0x5A))));
    let b = open_with(&data, &c, &meter_for(&c)).unwrap();
    assert!(!b.header().encrypted);
    assert_eq!(b.entry_bytes(0).unwrap(), entry);
    let b = open_with(&data, &c, &meter_for(&c)).unwrap();
    b.load_all().unwrap();
    assert_eq!(b.entry_bytes(0).unwrap(), entry);
}

/// A bundle of one block of 20,000 bytes of noise in `codec`, whose header,
/// of `engine`'s flag set, carries the encryption flag and `extra_flags`, its
/// block's flags `0x100` where `flagged`, every byte stored as written (none
/// scrambled for [`Xor`]). `(the bundle, the entry, where blocks-info ends)`.
fn encrypted_header(
    engine: &[u8],
    codec: TestCodec,
    flagged: bool,
    extra_flags: u32,
) -> (Vec<u8>, Vec<u8>, usize) {
    let mut rng = Rng::new(7);
    let entry: Vec<u8> = (0..20_000)
        .map(|_| rng.next_u64().to_le_bytes()[0])
        .collect();
    let mut data = build(&BundleSpec {
        version_engine: engine.to_vec(),
        info_codec: TestCodec::None,
        data_codec: codec,
        block_size: 1 << 20,
        entries: vec![EntrySpec {
            name: b"secret".to_vec(),
            data: entry.clone(),
            flags: 0,
        }],
        extra_flags,
        encrypted: true,
        ..BundleSpec::default()
    });
    let nul = |from: usize| from + data[from..].iter().position(|c| *c == 0).unwrap() + 1;
    // The header's fields after the engine string, then the decryptor's 70
    // bytes; blocks-info, stored, after the alignment, its size the header's.
    let head = nul(nul(12));
    let info = u32::from_be_bytes(data[head + 8..head + 12].try_into().unwrap()) as usize;
    let start = (head + 20 + 70).next_multiple_of(16);
    if flagged {
        // The one block's row: after the hash and the count.
        let row = start + 20;
        let flags = u16::from_be_bytes([data[row + 8], data[row + 9]]) | 0x100;
        data[row + 8..row + 10].copy_from_slice(&flags.to_be_bytes());
    }
    (data, entry, start + info)
}

/// UnityPy files/BundleFile.py:512-513: a bundle whose header built a
/// decryptor decrypts a block only when its flags carry `0x100` and it is
/// not stored. An LZ4 block without the flag, a stored block with it, and
/// stored blocks-info under data flags with it (`decompress_data` is given
/// the data flags for blocks-info, `:141`) are read as stored, lazily and
/// after `load_all` (review round 16: dropping either half of the
/// condition passed every test).
#[test]
fn only_a_flagged_block_that_is_not_stored_is_decrypted() {
    let c = ctx_with(|c| c.decryptor = Some(Arc::new(Xor(0x5A))));
    for (codec, flagged, extra) in [
        (TestCodec::Lz4, false, 0),
        (TestCodec::None, true, 0),
        (TestCodec::Lz4, false, 0x100),
    ] {
        let (data, entry, _) = encrypted_header(b"2022.3.0f1", codec, flagged, extra);
        let at = format!("{codec:?}, flagged {flagged}, data flags {extra:#x}");
        for eager in [false, true] {
            let b = open_with(&data, &c, &meter_for(&c));
            let b = b.unwrap_or_else(|e| panic!("{at}: {e}"));
            assert!(b.header().encrypted, "{at}");
            if eager {
                b.load_all().unwrap_or_else(|e| panic!("{at}, eager: {e}"));
            }
            let got = b.entry_bytes(0);
            assert!(
                got.as_deref() == Ok(&entry[..]),
                "{at}, eager {eager}: not the stored bytes: {:?}",
                got.map(|g| g.len())
            );
        }
    }
}

/// UnityPy files/BundleFile.py:170-171: the padding before the blocks is
/// read only under the new flag set, where `0x200` asks for it. Under
/// `ArchiveFlagsOld` (an engine before 2020.3.34 and the like, `:113-121`)
/// `0x200` is the encryption flag, and the blocks start where blocks-info
/// ends, here off a 16-byte boundary (review round 16: dropping the flag
/// set's half of the condition passed every test).
#[test]
fn an_old_flags_encrypted_bundle_reads_no_padding_before_its_blocks() {
    let (data, entry, info_end) = encrypted_header(b"2019.4.0f1", TestCodec::None, false, 0);
    assert_ne!(info_end % 16, 0, "blocks-info ends on a 16-byte boundary");
    let c = ctx_with(|c| c.decryptor = Some(Arc::new(Xor(0x5A))));
    let b = open_with(&data, &c, &meter_for(&c)).unwrap();
    assert_eq!(b.header().flags_class, Some(FlagsClass::Old));
    assert_eq!(b.header().data_flags.map(|f| f & 0x200), Some(0x200));
    assert!(b.header().encrypted);
    let got = b.entry_bytes(0);
    assert!(
        got.as_deref() == Ok(&entry[..]),
        "not the entry: {:?}",
        got.map(|g| g.len())
    );
}

#[test]
fn unsupported_bundles_are_named() {
    let base = BundleSpec {
        entries: small_entries(),
        ..BundleSpec::default()
    };
    let c = ctx();
    let m = meter_for(&c);
    let web7 = unityrs_testkit::bundle::build_with_signature(
        &BundleSpec {
            format: 7,
            ..base.clone()
        },
        b"UnityWeb",
    );
    assert!(matches!(
        open_with(&web7, &c, &m),
        Err(Error::Unsupported(Unsupported::WebRawLayout))
    ));
    let archive = unityrs_testkit::bundle::build_with_signature(&base, b"UnityArchive");
    assert!(matches!(
        open_with(&archive, &c, &m),
        Err(Error::Unsupported(Unsupported::UnityArchive))
    ));
    let enc = build(&BundleSpec {
        extra_flags: 0x1000,
        ..base.clone()
    });
    assert!(matches!(
        open_with(&enc, &c, &m),
        Err(Error::UnityCnKeyMissing)
    ));
    let lzham = build(&BundleSpec {
        info_codec: TestCodec::Raw(4),
        ..base
    });
    assert!(matches!(
        open_with(&lzham, &c, &m),
        Err(Error::Unsupported(Unsupported::Lzham))
    ));
}

#[test]
fn mutated_bundles_never_panic() {
    let inputs: Vec<Vec<u8>> = matrix()
        .into_iter()
        .filter(|(n, _)| n.ends_with(".bundle"))
        .map(|(_, b)| b)
        .collect();
    let mut rng = Rng::new(0x5EED);
    let ctx = ctx_with(|c| {
        c.limits.max_input_memory = 64 << 20;
        c.limits.max_block_output = 1 << 20;
    });
    for round in 0..10_000 {
        let mut data = inputs[round % inputs.len()].clone();
        match rng.below(3) {
            0 => {
                let n = 1 + rng.below(8);
                rng.flip_bits(&mut data, n);
            }
            1 => rng.truncate(&mut data),
            _ => {
                rng.flip_bits(&mut data, 1);
                rng.truncate(&mut data);
            }
        }
        let m = meter_for(&ctx);
        if let Ok(b) = open_with(&data, &ctx, &m) {
            for i in 0..b.directory().len().min(4) {
                let _ = b.entry_bytes(i);
                let _ = b.entry(i);
                let _ = b.entry_range(i, 1..9);
            }
            let _ = b.load_all();
        }
    }
}
