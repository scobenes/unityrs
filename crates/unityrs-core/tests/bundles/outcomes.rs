//! Kept block outcomes, and refusals that are not kept.

use std::sync::Arc;

use super::{by_name, ctx_with, meter_for, open, open_with, small_entries};
use unityrs_core::budget::{Counting, MemoryBudget, Reservation};
use unityrs_core::codec::Codec;
use unityrs_core::{Error, LimitKind, Unsupported};
use unityrs_testkit::bundle::{BlockOverride, BundleSpec, EntrySpec, TestCodec, build};

#[test]
fn a_failing_block_is_decoded_once_and_a_late_one_fails_only_load_all() {
    let data = by_name("lz4-declared-smaller.bundle");
    let b = open(&data);
    let m = b.meter().clone();
    assert!(b.entry_bytes(0).is_err());
    let after = m.decoded();
    let again = b.entry_bytes(0);
    assert!(
        matches!(
            again,
            Err(Error::Decompress {
                codec: Codec::Lz4,
                ..
            })
        ),
        "{again:?}"
    );
    assert_eq!(m.decoded(), after, "the kept failure answers");
    assert!(b.load_all().is_err());

    // A late LZHAM block: the first entry reads lazily, `load_all` fails.
    let late = build(&BundleSpec {
        block_size: 160,
        entries: small_entries(),
        block_overrides: vec![BlockOverride {
            block: 2,
            codec: Some(TestCodec::Raw(4)),
            ..BlockOverride::default()
        }],
        ..BundleSpec::default()
    });
    let b = open(&late);
    assert_eq!(&b.entry_bytes(0).unwrap()[..], &small_entries()[0].data[..]);
    assert!(matches!(
        b.load_all(),
        Err(Error::Unsupported(Unsupported::Lzham))
    ));
    assert!(b.entry_bytes(1).is_err());
}

#[test]
fn a_bundle_of_corrupt_blocks_opens_and_decodes_nothing_until_read() {
    let spec = BundleSpec {
        info_codec: TestCodec::None,
        data_codec: TestCodec::Lz4,
        block_size: 32,
        entries: small_entries(),
        ..BundleSpec::default()
    };
    let mut data = build(&spec);
    // Every data block follows blocks-info: overwrite them all.
    let blocks_from = data.len() - 120;
    for b in &mut data[blocks_from..] {
        *b = 0xF0;
    }
    let b = open(&data);
    assert_eq!(b.directory().len(), 2);
    assert_eq!(b.files().len(), 2);
    assert_eq!(b.meter().decoded(), 0, "open decodes no data block");
    assert!(b.entry_bytes(1).is_err());
    assert!(b.load_all().is_err());
}

#[test]
fn a_refused_block_is_not_kept() {
    // The budget: K refusals of the same read count no decode work (the
    // allowance below holds one decode of the entry, not K), and the same
    // read succeeds once the pool has room.
    let pool = Arc::new(Counting::new(1 << 20));
    let ctx = ctx_with(|c| {
        c.budget = pool.clone();
        c.limits.max_decode_work = 350_000;
    });
    let data = build(&BundleSpec {
        block_size: 100_000,
        entries: vec![EntrySpec {
            name: b"big".to_vec(),
            data: vec![3; 300_000],
            flags: 0,
        }],
        ..BundleSpec::default()
    });
    let meter = meter_for(&ctx);
    let b = open_with(&data, &ctx, &meter).unwrap();
    // Another holder takes most of the pool.
    let other = pool
        .try_reserve((1 << 20) - pool.used() - 1000, Reservation::Input)
        .unwrap();
    // The work so far: the blocks-info decoded at open.
    let work = meter.decoded();
    for _ in 0..8 {
        let refused = b.entry_bytes(0);
        assert!(matches!(refused, Err(Error::Budget(_))), "{refused:?}");
    }
    assert_eq!(meter.decoded(), work, "a refused growth counts no work");
    drop(other);
    assert_eq!(
        b.entry_bytes(0).unwrap().len(),
        300_000,
        "the same read succeeds once there is room"
    );

    // The meter: two bundles of one open share it; while the first holds
    // its run the second's block is refused, and once the first drops, the
    // same read of the second succeeds.
    // One read of the entry peaks near 500 KB (its run grows 200 KB to
    // 300 KB holding both); two need far more.
    let ctx = ctx_with(|c| c.limits.max_input_memory = 700_000);
    let meter = meter_for(&ctx);
    let first = open_with(&data, &ctx, &meter).unwrap();
    let second = open_with(&data, &ctx, &meter).unwrap();
    let held = first.entry_bytes(0).unwrap();
    let refused = second.entry_bytes(0);
    assert!(
        matches!(&refused, Err(Error::Limit(l)) if l.kind == LimitKind::InputMemory),
        "{refused:?}"
    );
    drop((held, first));
    assert_eq!(second.entry_bytes(0).unwrap().len(), 300_000);
}
