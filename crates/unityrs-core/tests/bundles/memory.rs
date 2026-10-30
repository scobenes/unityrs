//! Decode work and the input's meter, over several files and hostile
//! directories.

use std::io::Write as _;

use flate2::Compression;
use flate2::write::GzEncoder;

use super::{ctx, ctx_with, meter_for, open_with};
use unityrs_core::budget::{Reservation, Unbounded};
use unityrs_core::detect::DetectError;
use unityrs_core::files::File;
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

/// Claude round 3's C LOW: the open's work allowance is inclusive, as
/// `Meter::count_decode` admits work. A read whose decodes bring the open's
/// work exactly to `max_decode_work` succeeds, where it was counted and then
/// refused at publish with `limit DecodeWork: N > N`; one byte less refuses
/// it with a note whose work is past the bound. A gzip detection that
/// starts with the allowance exactly spent is refused by its first chunk,
/// and its note counts that chunk: no refusal names work at the bound.
#[test]
fn work_that_lands_exactly_on_the_allowance_is_within_it() {
    let past = |r: &Error| matches!(r, Error::Limit(l) if l.kind == LimitKind::DecodeWork && l.value > l.max);
    let entry: Vec<u8> = (0..100u8).collect();
    let data = build(&BundleSpec {
        entries: vec![EntrySpec {
            name: b"a".to_vec(),
            data: entry.clone(),
            flags: 0,
        }],
        ..BundleSpec::default()
    });
    // The open's work, then the read's, under the default allowance.
    let c = ctx();
    let m = meter_for(&c);
    let b = open_with(&data, &c, &m).unwrap();
    let opened = m.decoded();
    b.entry_bytes(0).unwrap();
    let work = m.decoded();
    assert!(work > opened, "{opened}, then {work}");
    for limit in [work - 1, work, work + 1] {
        let ctx = ctx_with(|c| c.limits.max_decode_work = limit);
        let m = meter_for(&ctx);
        let b = open_with(&data, &ctx, &m).unwrap();
        match b.entry_bytes(0) {
            Ok(got) if limit >= work => assert_eq!(&got[..], &entry[..]),
            Err(e) if limit < work => assert!(past(&e), "limit {limit}: {e}"),
            got => panic!("limit {limit} for work {work}: {got:?}"),
        }
        if limit >= work {
            assert_eq!(m.decoded(), work);
            assert_eq!(m.work_used_up(), None, "limit {limit}");
            // Kept: read again, it is the run's slice, and no more work.
            assert_eq!(&b.entry_bytes(0).unwrap()[..], &entry[..]);
            assert_eq!(m.decoded(), work);
        }
    }

    // Two rows over one gzip stream: the open and row 0's detection under
    // the default allowance, then an allowance of exactly that much.
    let data = rows_over_one_gzip_stream(2);
    let c = ctx();
    let m = meter_for(&c);
    let b = open_with(&data, &c, &m).unwrap();
    assert!(b.entry(0).is_ok());
    let work = m.decoded();
    let ctx = ctx_with(|c| c.limits.max_decode_work = work);
    let m = meter_for(&ctx);
    let b = open_with(&data, &ctx, &m).unwrap();
    assert!(matches!(b.entry(0), Ok(File::Raw(_))));
    assert_eq!(m.decoded(), work);
    match b.entry(1) {
        Err(e) => assert!(past(&e), "{e}"),
        Ok(_) => panic!("row 1 detected past the allowance"),
    }
    assert!(m.decoded() > work, "the refused chunk is counted");
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

/// Claude round 3's C MEDIUM: a stored block is held to `max_block_output`
/// on every path, as `load_all` holds it (its codec's range is checked
/// against the cap, `codec::decode_block`). Block 0 is a stored block of 100
/// bytes, past a cap of 95, and entry 0 lies in it alone; entry 1 lies after
/// it. Lazily the block was taken as a slice of the input, its length as
/// what was read, with no cap, while `load_all` decoded it through slot 0
/// and failed; a bundle of stored blocks only never met the cap at all. Now
/// every read, in any order, before or after `load_all`, fails at block 0 as
/// `load_all` does, whether the block after it is LZ4 or stored.
#[test]
fn a_stored_block_past_the_block_cap_fails_lazily_as_eagerly() {
    let ctx = ctx_with(|c| c.limits.max_block_output = 95);
    let entries = vec![
        EntrySpec {
            name: b"a".to_vec(),
            data: (0..100u8).collect(),
            flags: 0,
        },
        EntrySpec {
            name: b"b".to_vec(),
            data: vec![7; 30],
            flags: 0,
        },
    ];
    let mixed = build(&BundleSpec {
        info_codec: TestCodec::Lz4,
        data_codec: TestCodec::Lz4,
        block_size: 100,
        entries: entries.clone(),
        block_overrides: vec![BlockOverride {
            block: 0,
            codec: Some(TestCodec::None),
            ..BlockOverride::default()
        }],
        ..BundleSpec::default()
    });
    let stored = build(&BundleSpec {
        info_codec: TestCodec::Lz4,
        data_codec: TestCodec::None,
        block_size: 100,
        entries,
        ..BundleSpec::default()
    });
    let want = "limit BlockOutput: 100 > 95";
    let refused = |r: &Result<(), String>| r.as_ref().is_err_and(|e| e.contains(want));
    for (name, data) in [("mixed", mixed), ("stored", stored)] {
        let fresh = || open_with(&data, &ctx, &meter_for(&ctx)).unwrap();
        let b = fresh();
        let loaded = b.load_all().map_err(|e| e.to_string());
        assert!(refused(&loaded), "{name}: load_all {loaded:?}");
        let after = super::all_entries(&b, &[0, 1]);
        for r in &after {
            let r = r.as_ref().map(|_| ()).map_err(Clone::clone);
            assert!(refused(&r), "{name}: after load_all {r:?}");
        }
        for order in [[0, 1], [1, 0]] {
            let b = fresh();
            assert_eq!(
                super::all_entries(&b, &order),
                after,
                "{name}: order {order:?}"
            );
            let loaded = b.load_all().map_err(|e| e.to_string());
            assert!(refused(&loaded), "{name}: load_all after {order:?}");
        }
    }
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

/// Claude round 2's C HIGH: every directory row names one small gzip
/// stream that inflates to 1 MiB. Detection inflates each stream within
/// `max_inflate`, and every row's inflation is the open's work, so the rows
/// together inflate no more than `max_decode_work` (plus the one chunk that
/// passed it): the row that would pass it is refused with the open's
/// `DecodeWork` limit, and so is every row after it, lazily and eagerly.
#[test]
fn gzip_detection_across_the_rows_of_one_open_draws_on_its_work() {
    const ROWS: usize = 64;
    const MIB: u64 = 1 << 20;
    let data = rows_over_one_gzip_stream(ROWS);
    let ctx = ctx_with(|c| c.limits.max_decode_work = 4 * MIB);
    assert!(ctx.limits.max_inflate >= MIB, "each stream inflates whole");

    // Lazily, in directory order: the rows that fit, then refusals.
    let meter = meter_for(&ctx);
    let b = open_with(&data, &ctx, &meter).unwrap();
    let got: Vec<Result<File, Error>> = (0..ROWS).map(|i| b.entry(i)).collect();
    let fit = got.iter().take_while(|r| r.is_ok()).count();
    assert!((1..=4).contains(&fit), "{fit} rows detected");
    assert!(got[..fit].iter().all(|r| matches!(r, Ok(File::Raw(_)))));
    for r in &got[fit..] {
        assert!(
            matches!(r, Err(Error::Limit(l)) if l.kind == LimitKind::DecodeWork && l.max == 4 * MIB),
            "{r:?}"
        );
    }
    assert!(
        meter.decoded() <= 4 * MIB + (64 << 10),
        "{}",
        meter.decoded()
    );

    // Eagerly: the load fails with the same limit.
    let meter = meter_for(&ctx);
    let b = open_with(&data, &ctx, &meter).unwrap();
    let got = b.load_all();
    assert!(
        matches!(&got, Err(Error::Limit(l)) if l.kind == LimitKind::DecodeWork),
        "{got:?}"
    );
    assert!(
        meter.decoded() <= 4 * MIB + (64 << 10),
        "{}",
        meter.decoded()
    );

    // Under the default allowance every row detects, as UnityPy's does.
    let c = super::ctx();
    let b = open_with(&data, &c, &meter_for(&c)).unwrap();
    b.load_all().unwrap();
}

/// A bundle of `rows` directory rows that all name one gzip stream of 1 MiB
/// of zeros: row 0 holds it, every other row is empty, then pointed at it.
fn rows_over_one_gzip_stream(rows: usize) -> Vec<u8> {
    let mut gz = GzEncoder::new(Vec::new(), Compression::best());
    gz.write_all(&vec![0u8; 1 << 20]).unwrap();
    let stream = gz.finish().unwrap();
    let entries: Vec<EntrySpec> = (0..rows)
        .map(|k| EntrySpec {
            name: format!("gz-{k:02}").into_bytes(),
            data: if k == 0 { stream.clone() } else { Vec::new() },
            flags: 0,
        })
        .collect();
    let mut data = build(&BundleSpec {
        info_codec: TestCodec::None,
        data_codec: TestCodec::Lz4,
        entries,
        ..BundleSpec::default()
    });
    let size = i64::try_from(stream.len()).unwrap();
    for k in 1..rows {
        let key = format!("gz-{k:02}\0").into_bytes();
        let at = data.windows(key.len()).position(|w| w == key).unwrap();
        data[at - 20..at - 12].copy_from_slice(&0_i64.to_be_bytes());
        data[at - 12..at - 4].copy_from_slice(&size.to_be_bytes());
    }
    data
}

/// A detection refused at its own stream's bound (`max_inflate`) still
/// inflated the chunk that passed it, and that chunk is the open's work as
/// well: rows whose streams each pass `max_inflate` in their first chunk
/// draw the open's allowance down until the open's `DecodeWork` limit
/// refuses the rest, instead of each inflating a chunk the open never
/// counts.
#[test]
fn a_detection_refused_at_its_inflate_bound_still_draws_on_the_opens_work() {
    const ROWS: usize = 64;
    const MIB: u64 = 1 << 20;
    let data = rows_over_one_gzip_stream(ROWS);
    let ctx = ctx_with(|c| {
        c.limits.max_inflate = 1;
        c.limits.max_decode_work = MIB;
    });
    let meter = meter_for(&ctx);
    let b = open_with(&data, &ctx, &meter).unwrap();
    let got: Vec<Result<File, Error>> = (0..ROWS).map(|i| b.entry(i)).collect();
    let inflate = |r: &Result<File, Error>| {
        matches!(r, Err(Error::Entry { source, .. })
            if matches!(&**source, Error::Detect(DetectError::InflateLimit(l))
                if l.kind == LimitKind::Inflate && l.max == 1))
    };
    let first = got.iter().take_while(|r| inflate(r)).count();
    // Each refused row inflated one chunk (at most 64 KiB, here all of it)
    // and every one is counted, beside the block decodes, so the allowance
    // runs out within the rows; uncounted, every row is refused at Inflate.
    assert!(
        (1..ROWS).contains(&first),
        "{first} rows refused at Inflate"
    );
    for r in &got[first..] {
        assert!(
            matches!(r, Err(Error::Limit(l)) if l.kind == LimitKind::DecodeWork && l.max == MIB),
            "{r:?}"
        );
    }
    assert!(meter.decoded() <= MIB + (64 << 10), "{}", meter.decoded());
}
