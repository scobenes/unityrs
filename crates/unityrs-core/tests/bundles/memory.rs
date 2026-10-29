//! Decode work and the input's meter, over several files and hostile
//! directories.

use super::{ctx, ctx_with, meter_for, open_with};
use unityrs_core::budget::{Reservation, Unbounded};
use unityrs_core::{Error, FileContext, LimitKind};
use unityrs_testkit::bundle::{BlockOverride, BundleSpec, EntrySpec, TestCodec, build};
use unityrs_testkit::serialized::build as file;
use unityrs_testkit::{blob::NodeSpec, serialized::one_object};

#[test]
fn decode_work_stops_a_lazy_read_and_a_reopen_on_the_same_meter_shares_it() {
    let ctx = ctx_with(|c| {
        c.limits.max_block_output = 4096;
        c.limits.max_decode_work = 64 * 4096;
    });
    let data = build(&BundleSpec {
        block_size: 4096,
        entries: vec![
            EntrySpec {
                name: b"first".to_vec(),
                data: vec![1; 4096 * 100],
                flags: 0,
            },
            EntrySpec {
                name: b"last".to_vec(),
                data: vec![2; 10],
                flags: 0,
            },
        ],
        ..BundleSpec::default()
    });
    let meter = meter_for(&ctx);
    let b = open_with(&data, &ctx, &meter).unwrap();
    let held = meter.used();
    // The last entry: every block before it measured (decoded to the block
    // cap and dropped) until the work bound stops the read.
    let got = b.entry_bytes(1);
    assert!(
        matches!(&got, Err(Error::Limit(l)) if l.kind == LimitKind::DecodeWork),
        "{got:?}"
    );
    assert_eq!(
        meter.used(),
        held,
        "measuring decodes returned their memory"
    );
    // Opened again on the same meter (no restart), the allowance is spent:
    // even a read of ten blocks stops.
    let ten = 0..4096 * 10;
    let again = open_with(&data, &ctx, &meter).unwrap();
    let got = again.entry_range(0, ten.clone());
    assert!(
        matches!(&got, Err(Error::Limit(l)) if l.kind == LimitKind::DecodeWork),
        "{got:?}"
    );
    // A reopen (a new open of the input) has its own allowance.
    let fresh = open_with(&data, &ctx, &meter.reopen()).unwrap();
    assert_eq!(fresh.entry_range(0, ten).unwrap().len(), 4096 * 10);
}

#[test]
fn each_block_of_a_run_is_capped_on_its_own() {
    let ctx = ctx_with(|c| c.limits.max_block_output = 4096);
    let entry: Vec<u8> = (0..3 * 4096 + 100).map(|i: u32| (i % 251) as u8).collect();
    let spec = BundleSpec {
        block_size: 4096,
        entries: vec![EntrySpec {
            name: b"three-blocks".to_vec(),
            data: entry.clone(),
            flags: 0,
        }],
        ..BundleSpec::default()
    };
    // A run of four blocks passes the cap; each block is within it.
    let b = open_with(&build(&spec), &ctx, &meter_for(&ctx)).unwrap();
    assert_eq!(b.entry_bytes(0).unwrap(), entry);
    b.load_all().unwrap();
    // One block of 8 KiB does not.
    let one = build(&BundleSpec {
        block_size: 8192,
        ..spec
    });
    let b = open_with(&one, &ctx, &meter_for(&ctx)).unwrap();
    let got = b.entry_bytes(0);
    assert!(
        matches!(&got, Err(Error::Limit(l)) if l.kind == LimitKind::BlockOutput),
        "{got:?}"
    );
}

#[test]
fn an_lzma_block_that_outgrows_its_declaration_is_charged_until_the_last_slice_drops() {
    let entry: Vec<u8> = (0..100_000u32).map(|i| (i % 7) as u8).collect();
    let data = build(&BundleSpec {
        data_codec: TestCodec::Lzma,
        block_size: 1 << 20,
        entries: vec![EntrySpec {
            name: b"grown".to_vec(),
            data: entry.clone(),
            flags: 0,
        }],
        block_overrides: vec![BlockOverride {
            block: 0,
            codec: None,
            uncompressed: Some(1000),
            compressed: None,
        }],
        ..BundleSpec::default()
    });
    let c = ctx();
    let meter = meter_for(&c);
    let b = open_with(&data, &c, &meter).unwrap();
    let held = meter.used();
    let slice = b.entry_bytes(0).unwrap();
    assert_eq!(
        slice, entry,
        "the block's actual output, not its declaration"
    );
    assert!(
        meter.used() >= held + 100_000,
        "charged at its actual length: {} over {held}",
        meter.used()
    );
    drop(b);
    assert!(meter.used() >= 100_000, "the slice keeps its run's charge");
    drop(slice);
    assert_eq!(meter.used(), 0, "returned with the last slice");
}

/// Round 2's second hostile input: rows whose spans are nested suffixes of
/// the joined data, the shortest first. Read lazily in directory order,
/// with every entry's bytes kept (as an Environment keeps what it parsed),
/// each read decodes its own run, charged, until the meter stops one with
/// `InputMemory`; the eager load, one run, fits.
#[test]
fn nested_suffix_spans_stop_a_lazy_read_at_the_meter_where_load_all_fits() {
    const N: usize = 32;
    const B: usize = 1 << 16;
    let entries: Vec<EntrySpec> = (0..N)
        .map(|k| EntrySpec {
            name: format!("suffix-{k:02}").into_bytes(),
            data: (0..B)
                .map(|i| ((i * 31 + k) % 251).to_le_bytes()[0])
                .collect(),
            flags: 0,
        })
        .collect();
    let mut data = build(&BundleSpec {
        info_codec: TestCodec::None,
        data_codec: TestCodec::Lz4,
        block_size: B,
        entries,
        ..BundleSpec::default()
    });
    for k in 0..N {
        let key = format!("suffix-{k:02}\0").into_bytes();
        let at = data.windows(key.len()).position(|w| w == key).unwrap();
        let offset = i64::try_from((N - 1 - k) * B).unwrap();
        let size = i64::try_from((k + 1) * B).unwrap();
        data[at - 20..at - 12].copy_from_slice(&offset.to_be_bytes());
        data[at - 12..at - 4].copy_from_slice(&size.to_be_bytes());
    }
    let ctx = ctx_with(|c| c.limits.max_input_memory = 6 << 20);
    let eager = open_with(&data, &ctx, &meter_for(&ctx)).unwrap();
    eager.load_all().unwrap();
    drop(eager);

    let meter = meter_for(&ctx);
    let lazy = open_with(&data, &ctx, &meter).unwrap();
    let mut kept = Vec::new();
    let mut last = meter.used();
    let stopped = loop {
        assert!(kept.len() < N, "every read fit");
        match lazy.entry_bytes(kept.len()) {
            Ok(bytes) => {
                assert_eq!(bytes.len(), (kept.len() + 1) * B);
                assert!(meter.used() > last, "each re-decode is charged");
                last = meter.used();
                kept.push(bytes);
            }
            Err(e) => break e,
        }
    };
    assert!(
        matches!(&stopped, Error::Limit(l) if l.kind == LimitKind::InputMemory),
        "{stopped:?}"
    );
    assert!(kept.len() > 1 && meter.used() <= 6 << 20);
}

#[test]
fn a_meter_spans_every_file_of_one_bundle() {
    let one = file(&one_object(
        1,
        vec![
            NodeSpec::new(0, "GameObject", "Base"),
            NodeSpec::new(1, "int", "m_A").size(4),
        ],
        vec![0; 4],
    ));
    let entry = |n: &str| EntrySpec {
        name: n.as_bytes().to_vec(),
        data: one.clone(),
        flags: 4,
    };
    let single = build(&BundleSpec {
        entries: vec![entry("CAB-1")],
        ..BundleSpec::default()
    });
    let double = build(&BundleSpec {
        entries: vec![entry("CAB-1"), entry("CAB-2")],
        ..BundleSpec::default()
    });
    // What one file of one entry costs, opened and loaded.
    let c = ctx();
    let m = meter_for(&c);
    let b = open_with(&single, &c, &m).unwrap();
    b.load_all().unwrap();
    let cost = m.used();
    drop(b);
    // A meter that holds one file's load but not two.
    let limit = cost + cost / 3;
    let ctx = ctx_with(|c| c.limits.max_input_memory = limit);
    let m = meter_for(&ctx);
    let b = open_with(&single, &ctx, &m).unwrap();
    b.load_all().unwrap();
    drop(b);
    let m = meter_for(&ctx);
    let b = open_with(&double, &ctx, &m).unwrap();
    let got = b.load_all();
    assert!(
        matches!(&got, Err(Error::Limit(l)) if l.kind == LimitKind::InputMemory),
        "{got:?}"
    );
}

#[test]
fn many_rows_of_one_entry_stop_at_the_meter() {
    let one = file(&one_object(
        1,
        vec![NodeSpec::new(0, "GameObject", "Base")],
        vec![0; 4],
    ));
    let entries: Vec<EntrySpec> = (0..2000)
        .map(|i| EntrySpec {
            name: format!("CAB-{i}").into_bytes(),
            data: one.clone(),
            flags: 4,
        })
        .collect();
    let data = build(&BundleSpec {
        block_size: 1 << 17,
        entries,
        ..BundleSpec::default()
    });
    let ctx = ctx_with(|c| c.limits.max_input_memory = 2 << 20);
    let m = meter_for(&ctx);
    let b = open_with(&data, &ctx, &m).unwrap();
    let got = b.load_all();
    assert!(
        matches!(&got, Err(Error::Limit(l)) if l.kind == LimitKind::InputMemory),
        "{got:?}"
    );
    assert!(m.used() <= 2 << 20);
}

#[test]
fn the_unbounded_budget_is_the_default() {
    // UnityPy mode books nothing outside the meter.
    let c = FileContext::unitypy();
    assert!(
        c.budget
            .try_reserve(u64::MAX / 2, Reservation::Input)
            .is_ok()
    );
    let _ = Unbounded;
}
