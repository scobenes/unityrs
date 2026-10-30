//! The work an entry's parse does, against the open's allowance (round 4's
//! inputs): directory rows over one span whose parse fails stop at the
//! open's `DecodeWork` limit instead of repeating the parse once per row,
//! and an entry whose blob names the same NUL-less string on every row costs
//! one search of its string buffer per parse, whatever its rows.

use unityrs_core::files::File;
use unityrs_core::{Error, LimitKind};
use unityrs_testkit::bundle::{BundleSpec, EntrySpec, TestCodec, build};
use unityrs_testkit::serialized::no_nul_strings;

use super::{ctx, ctx_with, meter_for, open_with};

/// The bytes of one directory row named `CAB-%08d` (`offset`, `size`,
/// `flags`, the name and its NUL).
const ROW: usize = 8 + 8 + 4 + 13;

/// Round 4's bundle: UnityFS 6, engine 2018.4.0f1, a stored blocks-info and
/// `entry` in one data block, and `rows` directory rows `CAB-%08d`, every one
/// at offset 0 with the whole entry's size.
fn rows_over_one_entry(entry: &[u8], rows: usize) -> Vec<u8> {
    let entries: Vec<EntrySpec> = (0..rows)
        .map(|k| EntrySpec {
            name: format!("CAB-{k:08}").into_bytes(),
            data: if k == 0 { entry.to_vec() } else { Vec::new() },
            flags: 4,
        })
        .collect();
    let mut data = build(&BundleSpec {
        format: 6,
        version_engine: b"2018.4.0f1".to_vec(),
        info_codec: TestCodec::None,
        data_codec: TestCodec::None,
        block_size: entry.len(),
        entries,
        ..BundleSpec::default()
    });
    // The rows follow each other in the stored blocks-info: every row after
    // the first moves to offset 0, with the entry's size.
    let first = data
        .windows(13)
        .position(|w| w == b"CAB-00000000\0")
        .unwrap();
    let size = i64::try_from(entry.len()).unwrap().to_be_bytes();
    for k in 1..rows {
        let at = first + k * ROW;
        assert_eq!(&data[at..at + 12], format!("CAB-{k:08}").as_bytes());
        data[at - 20..at - 12].copy_from_slice(&0_i64.to_be_bytes());
        data[at - 12..at - 4].copy_from_slice(&size);
    }
    data
}

/// Whether `got` is the entry's own parse failure: its type's dependency
/// read past the end of the file.
fn failed_parse(got: &Result<File, Error>) -> bool {
    match got {
        Err(Error::Entry { source, .. }) => matches!(**source, Error::OutOfBounds { .. }),
        _ => false,
    }
}

/// Round 4 (HIGH): K rows over one entry whose parse fails after reading R
/// rows of blob were K parses, each returning what it was granted, so no
/// limit saw them (100,000 rows over a 3.2 MB entry ran about 41 hours
/// under `Lazy`). What a failing parse was granted is now the open's work:
/// each parse here counts at least its R rows' 32 bytes each, so the
/// allowance admits at most `allowance / (32 R) + 1` parses, and every
/// later row is refused with `DecodeWork` without parsing. Before the fix
/// all K rows parsed.
#[test]
fn rows_over_one_failing_entry_stop_at_the_opens_work() {
    const R: usize = 20_000;
    const K: usize = 200;
    const ALLOWANCE: u64 = 16 << 20;
    let entry = no_nul_strings(R, false);
    let data = rows_over_one_entry(&entry, K);
    let ctx = ctx_with(|c| c.limits.max_decode_work = ALLOWANCE);
    let meter = meter_for(&ctx);
    let b = open_with(&data, &ctx, &meter).unwrap();
    let got: Vec<Result<File, Error>> = (0..K).map(|i| b.entry(i)).collect();
    let parsed = got.iter().take_while(|g| failed_parse(g)).count();
    let most = usize::try_from(ALLOWANCE).unwrap() / (32 * R) + 1;
    assert!(
        (1..=most).contains(&parsed),
        "{parsed} parses, at most {most} allowed"
    );
    for (i, g) in got.iter().enumerate().skip(parsed) {
        assert!(
            matches!(g, Err(Error::Limit(l)) if l.kind == LimitKind::DecodeWork),
            "row {i}: {g:?}"
        );
    }
    assert!(meter.work_used_up().is_some());
    // The row a refusal is kept for stays refused: asked again, nothing is
    // parsed again.
    let before = meter.decoded();
    assert!(b.entry(K - 1).is_err());
    assert_eq!(meter.decoded(), before);
}

/// The counted work of parsing `rows` directory rows over one complete
/// entry of `r` blob rows, eagerly: every block decoded first, then each
/// entry parsed, as `load_all` does. Its reads are not the parses' work:
/// `load_all`'s read of every block and each row's read are a cached run's
/// slice here (one stored block, one run), each costing what a read of row
/// 0 then costs, its steps (plan 3.6 "Runs"), which are taken off.
fn eager_parse_work(r: usize, rows: usize) -> u64 {
    let entry = no_nul_strings(r, true);
    let data = rows_over_one_entry(&entry, rows);
    let ctx = ctx();
    let meter = meter_for(&ctx);
    let b = open_with(&data, &ctx, &meter).unwrap();
    b.entry_bytes(0).unwrap();
    let before = meter.decoded();
    b.load_all().unwrap();
    for i in 0..rows {
        assert!(matches!(b.entry(i), Ok(File::Serialized(_))), "row {i}");
    }
    let work = meter.decoded() - before;
    let before = meter.decoded();
    b.entry_bytes(0).unwrap();
    let read = meter.decoded() - before;
    assert!((1..64).contains(&read), "a read counted {read}");
    work - (1 + u64::try_from(rows).unwrap()) * read
}

/// Round 4 (MEDIUM): a 700-byte bundle of 3 rows over an entry of 100,000
/// blob rows whose strings are all offset 0 of a 64 KiB buffer with no NUL
/// cost 1.66 s per row on the eager path, searching 2 x 32,767 bytes per
/// blob row, none of it counted. Each parse now searches its string buffer
/// once and counts it: the buffer, plus the two metadata strings it reads
/// to their NUL (`2018.4.0f1` and the empty user information, 12 bytes),
/// the same for 1,000 rows as for 20,000.
#[test]
fn an_entrys_blob_strings_cost_one_search_of_its_buffer_whatever_its_rows() {
    const PER_PARSE: u64 = 65_536 + 12;
    for r in [1_000, 20_000] {
        assert_eq!(eager_parse_work(r, 3), 3 * PER_PARSE, "{r} blob rows");
    }
}
