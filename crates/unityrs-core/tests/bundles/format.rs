//! The format's edges: counts as Python ranges, decryption, unsupported
//! bundles, and mutated bundles.

use std::sync::Arc;

use super::{ctx, ctx_with, meter_for, open, open_with, small_entries};
use unityrs_core::codec::BlockDecryptor;
use unityrs_core::{Error, Unsupported};
use unityrs_testkit::Rng;
use unityrs_testkit::bundle::{BundleSpec, EntrySpec, TestCodec, build, matrix};

/// The block and directory counts as `range(count)` reads them (UnityPy
/// files/BundleFile.py:145-164): a negative count is no records, a huge one
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

/// UnityPy files/BundleFile.py:512-513: a block flagged 0x100 and not
/// stored goes through the decryptor before its codec, in a charged copy
/// that is returned once the block is decoded.
#[test]
fn a_flagged_block_is_decrypted_in_a_charged_copy() {
    // Noise, so the stored block is as large as its output.
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
        ..BundleSpec::default()
    });
    let nul = |from: usize| from + data[from..].iter().position(|c| *c == 0).unwrap() + 1;
    // The one block's row: after the aligned header, the hash and the count.
    let row = (nul(nul(12)) + 20).next_multiple_of(16) + 20;
    let stored = u32::from_be_bytes(data[row + 4..row + 8].try_into().unwrap()) as usize;
    let flags = u16::from_be_bytes([data[row + 8], data[row + 9]]) | 0x100;
    data[row + 8..row + 10].copy_from_slice(&flags.to_be_bytes());
    let len = data.len();
    for b in &mut data[len - stored..] {
        *b ^= 0x5A;
    }
    let plain = ctx();
    assert!(
        open_with(&data, &plain, &meter_for(&plain))
            .unwrap()
            .entry_bytes(0)
            .is_err()
    );
    let c = ctx_with(|c| c.decryptor = Some(Arc::new(Xor(0x5A))));
    let m = meter_for(&c);
    let b = open_with(&data, &c, &m).unwrap();
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
        Err(Error::Unsupported(Unsupported::Encrypted))
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
