//! One layout in every read order, and runs.

use std::sync::{Arc, Mutex};

use super::{all_entries, by_name, ctx_with, meter_for, open, open_with, small_entries};
use unityrs_core::budget::DecodeBuf;
use unityrs_core::codec::{BlockCodec, Codec, Codecs, Decoded, Lz4};
use unityrs_core::{Bytes, Error};
use unityrs_testkit::Rng;
use unityrs_testkit::bundle::{BlockOverride, BundleSpec, TestCodec, build};

/// Bundles whose blocks before an entry decode to other lengths than they
/// declare: every entry read lazily, in every order, equals `load_all`'s.
#[test]
fn one_layout_in_every_read_order() {
    let mut inputs: Vec<Vec<u8>> = [
        "lz4-declared-larger.bundle",
        "lzma-overlong.bundle",
        "stored-declared-differs.bundle",
        "stored-compressed-larger.bundle",
        "truncated.bundle",
        "tiny-blocks.bundle",
        "nested.bundle",
    ]
    .iter()
    .map(|n| by_name(n))
    .collect();
    for codec in [TestCodec::Lz4, TestCodec::Lzma, TestCodec::None] {
        inputs.push(build(&BundleSpec {
            data_codec: codec,
            block_size: 64,
            entries: small_entries(),
            block_overrides: vec![BlockOverride {
                block: 0,
                uncompressed: Some(1000),
                ..BlockOverride::default()
            }],
            ..BundleSpec::default()
        }));
    }
    let mut rng = Rng::new(7);
    for data in &inputs {
        let eager = open(data);
        let n = eager.directory().len();
        eager.load_all().unwrap();
        let want = all_entries(&eager, &(0..n).collect::<Vec<_>>());
        let mut orders: Vec<Vec<usize>> = vec![(0..n).collect(), (0..n).rev().collect()];
        for _ in 0..4 {
            let mut o: Vec<usize> = (0..n).collect();
            for i in (1..n).rev() {
                o.swap(i, rng.below(i + 1));
            }
            orders.push(o);
        }
        for order in orders {
            let lazy = open(data);
            assert_eq!(all_entries(&lazy, &order), want, "order {order:?}");
            // Entries parse the same way too.
            for i in 0..n {
                assert!(lazy.entry(i).is_ok());
            }
        }
    }
}

/// LZ4, counting the decodes of each block (by where its bytes start in
/// the input).
#[derive(Default)]
struct CountingLz4 {
    calls: Mutex<std::collections::HashMap<usize, u32>>,
    base: Mutex<usize>,
}

impl CountingLz4 {
    fn calls(&self, at: usize) -> u32 {
        let base = *self.base.lock().unwrap();
        self.calls
            .lock()
            .unwrap()
            .get(&(base + at))
            .copied()
            .unwrap_or(0)
    }
}

impl BlockCodec for CountingLz4 {
    fn decode(
        &self,
        input: &Bytes,
        uncompressed: u32,
        out: &mut DecodeBuf,
    ) -> Result<Decoded, Error> {
        *self
            .calls
            .lock()
            .unwrap()
            .entry(input.as_ptr() as usize)
            .or_default() += 1;
        Lz4.decode(input, uncompressed, out)
    }
}

#[test]
fn ranges_in_one_block_decode_it_once_and_a_straddling_read_decodes_again() {
    let spec = BundleSpec {
        data_codec: TestCodec::Lz4,
        info_codec: TestCodec::None,
        block_size: 64,
        entries: small_entries(),
        ..BundleSpec::default()
    };
    let data = build(&spec);
    let codec = Arc::new(CountingLz4::default());
    let mut codecs = Codecs::unitypy();
    codecs.set(Codec::Lz4, codec.clone()).unwrap();
    let ctx = ctx_with(|c| c.codecs = Arc::new(codecs));
    let joined: Vec<u8> = spec.entries.iter().flat_map(|e| e.data.clone()).collect();
    let b = open_with(&data, &ctx, &meter_for(&ctx)).unwrap();
    *codec.base.lock().unwrap() = b.source().as_ptr() as usize;
    // Each block's stored bytes, in order, follow the stored blocks-info.
    let starts: Vec<usize> = {
        let total: usize = b.blocks().iter().map(|x| x.bytes_read as usize).sum();
        let mut at = data.len() - total;
        b.blocks()
            .iter()
            .map(|x| {
                let s = at;
                at += x.bytes_read as usize;
                s
            })
            .collect()
    };
    let calls = |k: usize| codec.calls(starts[k]);
    // Two ranges of entry 1 in block 2 (joined 128..192): one decode of it.
    let r1 = b.entry_range(1, 30..40).unwrap();
    let r2 = b.entry_range(1, 50..60).unwrap();
    assert_eq!(&r1[..], &joined[130..140]);
    assert_eq!(&r2[..], &joined[150..160]);
    assert_eq!(
        calls(2),
        1,
        "a second range in the block is a slice of its run"
    );
    // Entry 0 is blocks 0 and 1; entry 1 (joined 100..400) starts in block 1,
    // which its read decodes again into its own run.
    let b = open_with(&data, &ctx, &meter_for(&ctx)).unwrap();
    *codec.base.lock().unwrap() = b.source().as_ptr() as usize;
    codec.calls.lock().unwrap().clear();
    assert_eq!(&b.entry_bytes(0).unwrap()[..], &joined[..100]);
    assert_eq!((calls(0), calls(1)), (1, 1));
    assert_eq!(&b.entry_bytes(1).unwrap()[..], &joined[100..400]);
    assert_eq!((calls(0), calls(1), calls(2)), (1, 2, 1));
    // `load_all` decodes every block once more into one run; every later
    // read is a slice of it.
    b.load_all().unwrap();
    let after: Vec<u32> = (0..starts.len()).map(calls).collect();
    b.entry_bytes(0).unwrap();
    b.entry_bytes(1).unwrap();
    b.entry_range(1, 5..6).unwrap();
    assert_eq!((0..starts.len()).map(calls).collect::<Vec<_>>(), after);
}
