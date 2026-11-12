//! Round 6 (HIGH): directory rows over a nested bundle whose LZ4 scan fails.
//! liblz4's control flow is scanned once before anything is written
//! (`codec::Lz4`), so a block that fails at its end wrote nothing and was
//! granted nothing, and no count saw the scan; and each row opens its nested
//! bundle anew, so K rows scanned it K times (4,096 rows of a 4 KB input ran
//! 29 s under `Lazy`). A block's stored bytes are now the open's work before
//! a codec reads them, and rows over one span share their kept failure
//! (`bundle` module docs, "Twins"): K such rows scan it once, rows over
//! different spans of it stop at the open's `DecodeWork` limit, and so do
//! the rows of nested bundles whose data block fails the same way.

use unityrs_core::files::File;
use unityrs_core::{Error, LimitKind};
use unityrs_testkit::bundle::{BundleSpec, EntrySpec, TestCodec, build, compress};

use super::work::{rows_over, rows_over_one_entry};
use super::{ctx, ctx_with, meter_for, open_with};

/// The scan's length: what each failing scan reads.
const S: usize = 1 << 20;

/// A UnityFS 6 bundle (engine 2018.4.0f1) whose blocks-info is flagged LZ4
/// and holds `0xF0` then [`S`] bytes of `0xFF`: liblz4 reads every `0xFF` as a
/// literal-length byte and fails at the end of the input, having written
/// nothing.
fn failing_info() -> Vec<u8> {
    let mut v = b"UnityFS\0".to_vec();
    v.extend_from_slice(&6_u32.to_be_bytes());
    v.extend_from_slice(b"5.x.x\0");
    v.extend_from_slice(b"2018.4.0f1\0");
    let total = v.len() + 8 + 12 + 1 + S;
    v.extend_from_slice(&i64::try_from(total).unwrap().to_be_bytes());
    v.extend_from_slice(&u32::try_from(1 + S).unwrap().to_be_bytes());
    v.extend_from_slice(&16_u32.to_be_bytes());
    v.extend_from_slice(&0x42_u32.to_be_bytes());
    v.push(0xF0);
    v.resize(v.len() + S, 0xFF);
    v
}

/// A nested bundle whose blocks-info is stored and whose one data block is
/// flagged LZ4 and holds `0xF0` then [`S`] bytes of `0xFF`.
fn failing_block() -> Vec<u8> {
    let mut d = vec![0xF0_u8];
    d.resize(1 + S, 0xFF);
    build(&BundleSpec {
        format: 6,
        version_engine: b"2018.4.0f1".to_vec(),
        info_codec: TestCodec::None,
        data_codec: TestCodec::Raw(2),
        block_size: d.len(),
        entries: vec![EntrySpec {
            name: b"inner".to_vec(),
            data: d,
            flags: 4,
        }],
        ..BundleSpec::default()
    })
}

/// liblz4's verdict on the scan: python-lz4's `LZ4BlockError` once the scan
/// reaches its length-byte limit, 13 bytes before the input's end (UnityPy
/// gave `Error code: 16777204` for 2^24 bytes, review round 6).
fn lz4_failure(e: &Error) -> bool {
    matches!(e, Error::Decompress { detail, .. }
        if detail.starts_with("LZ4BlockError") && detail.ends_with(&format!("Error code: {}", S - 12)))
}

fn work_refused(e: &Error) -> bool {
    matches!(e, Error::Limit(l) if l.kind() == LimitKind::DecodeWork)
}

/// K rows over one span (offset 0, the nested bundle's size) are twins: the
/// first to fail keeps its failure for the others, whichever row is asked
/// first, so the scan runs once and the open counts it once, its [`S`] + 1
/// stored bytes, against K scans before (and none of them counted: the old
/// code counted under 1 KiB a row). Every row answers the nested bundle's
/// LZ4 failure under its own name.
#[test]
fn rows_over_one_failing_nested_bundle_scan_it_once() {
    const K: usize = 64;
    let data = rows_over_one_entry(&failing_info(), K);
    let ctx = ctx();
    let meter = meter_for(&ctx);
    let b = open_with(&data, &ctx, &meter).unwrap();
    let before = meter.decoded();
    let order: Vec<usize> = std::iter::once(K - 1).chain(0..K - 1).collect();
    for &i in &order {
        match b.entry(i) {
            Err(Error::Entry { name, source }) => {
                assert_eq!(name.as_bytes(), format!("CAB-{i:08}").as_bytes());
                assert!(lz4_failure(&source), "row {i}: {source:?}");
            }
            got => panic!("row {i}: {got:?}"),
        }
    }
    let work = meter.decoded() - before;
    let scan = u64::try_from(S + 1).unwrap();
    assert!(
        (scan..2 * scan).contains(&work),
        "{K} rows counted {work}: one scan is {scan}"
    );
    // Asked again, every row answers from its cell.
    let again = meter.decoded();
    for i in 0..K {
        assert!(b.entry(i).is_err());
    }
    assert_eq!(meter.decoded(), again);
}

/// Rows over different spans of the nested bundle (sizes that differ by a
/// byte of padding) are not twins, and each opens it and scans: each scan
/// counts [`S`] + 1 bytes before it reads them, so the allowance admits at
/// most `allowance / (S + 1)` scans, and every later row is refused with
/// `DecodeWork` without scanning. Before the fix all K rows scanned.
#[test]
fn rows_over_spans_of_a_failing_nested_bundle_stop_at_the_opens_work() {
    const K: usize = 64;
    const ALLOWANCE: u64 = 16 << 20;
    let mut entry = failing_info();
    entry.resize(entry.len() + K, 0);
    let data = rows_over(&entry, K, |k| entry.len() - k);
    let ctx = ctx_with(|c| c.limits.max_decode_work = ALLOWANCE);
    let meter = meter_for(&ctx);
    let b = open_with(&data, &ctx, &meter).unwrap();
    let (mut scanned, mut refused) = (0, 0);
    for i in 0..K {
        match b.entry(i) {
            Err(Error::Entry { source, .. }) if lz4_failure(&source) => scanned += 1,
            Err(e) if work_refused(&e) => refused += 1,
            got => panic!("row {i}: {got:?}"),
        }
    }
    let most = usize::try_from(ALLOWANCE).unwrap() / (S + 1);
    assert!(
        (1..=most).contains(&scanned),
        "{scanned} scans, at most {most} allowed"
    );
    assert_eq!(scanned + refused, K);
}

/// Rows over a nested bundle that opens, whose one data block fails the
/// scan: each row's nested bundle is its own (its file carries the row's
/// name), and a read of its entry scans the block, so the rows' reads stop
/// at the open's `DecodeWork` limit, at most `allowance / (S + 1)` scans.
/// Before the fix every row's read scanned.
#[test]
fn reads_of_failing_blocks_through_rows_stop_at_the_opens_work() {
    const K: usize = 64;
    const ALLOWANCE: u64 = 16 << 20;
    let data = rows_over_one_entry(&failing_block(), K);
    let ctx = ctx_with(|c| c.limits.max_decode_work = ALLOWANCE);
    let meter = meter_for(&ctx);
    let b = open_with(&data, &ctx, &meter).unwrap();
    let (mut scanned, mut refused) = (0, 0);
    for i in 0..K {
        let read = match b.entry(i) {
            Ok(File::Bundle(nested)) => nested.entry_bytes(0).map(|_| ()),
            Ok(other) => panic!("row {i}: {other:?}"),
            Err(e) => Err(e),
        };
        match read {
            Err(e) if lz4_failure(&e) => scanned += 1,
            Err(e) if work_refused(&e) => refused += 1,
            got => panic!("row {i}: {got:?}"),
        }
    }
    let most = usize::try_from(ALLOWANCE).unwrap() / (S + 1);
    assert!(
        (1..=most).contains(&scanned),
        "{scanned} scans, at most {most} allowed"
    );
    assert_eq!(scanned + refused, K);
}

/// Rows over one span whose names disagree on the raw rule (a `.resS`
/// suffix keeps an `AssetsFile` raw, `detect::parsed_as`) parse the same
/// bytes differently, so they are not twins: a row named for a resource is
/// UnityPy's raw reader however its twin by span failed, and the other way
/// round. Every row with the plain name shares the first one's failure.
#[test]
fn rows_whose_names_disagree_on_the_raw_rule_are_not_twins() {
    // A serialized file whose type's dependency read runs past its end.
    let entry = unityrs_testkit::serialized::no_nul_strings(100, false);
    let names = ["a", "a.resS", "b", "b.resS"];
    let entries: Vec<EntrySpec> = names
        .iter()
        .enumerate()
        .map(|(k, n)| EntrySpec {
            name: n.as_bytes().to_vec(),
            data: if k == 0 { entry.clone() } else { Vec::new() },
            flags: 4,
        })
        .collect();
    let mut data = build(&BundleSpec {
        format: 6,
        version_engine: b"2018.4.0f1".to_vec(),
        info_codec: TestCodec::None,
        data_codec: TestCodec::None,
        entries,
        ..BundleSpec::default()
    });
    for n in &names[1..] {
        // Every row at offset 0, with the entry's size.
        let key = [n.as_bytes(), b"\0"].concat();
        let at = data.windows(key.len()).position(|w| w == key).unwrap();
        data[at - 20..at - 12].copy_from_slice(&0_i64.to_be_bytes());
        data[at - 12..at - 4].copy_from_slice(&i64::try_from(entry.len()).unwrap().to_be_bytes());
    }
    let raw = |got: &Result<File, Error>| matches!(got, Ok(File::Raw(_)));
    let failed = |got: &Result<File, Error>, name: &str| {
        matches!(got, Err(Error::Entry { name: n, source })
            if n.as_bytes() == name.as_bytes() && matches!(**source, Error::OutOfBounds { .. }))
    };
    for order in [[0, 1, 2, 3], [3, 2, 1, 0], [1, 2, 0, 3]] {
        let c = ctx();
        let b = open_with(&data, &c, &meter_for(&c)).unwrap();
        for i in order {
            let got = b.entry(i);
            if names[i].ends_with(".resS") {
                assert!(raw(&got), "{order:?}, row {i}: {got:?}");
            } else {
                assert!(failed(&got, names[i]), "{order:?}, row {i}: {got:?}");
            }
        }
    }
}

/// The stored bytes a block's decode counts are credit for its output, so a
/// decode counts the larger of the two: a block that compresses counts its
/// output, as before round 6, and one whose stored bytes outnumber its
/// output (LZ4 of bytes that do not compress) counts its stored bytes. Each
/// read here is one entry in one block, so its steps (`runs`) cost the same
/// in both, and what is left is the decode. Counting both, the compressed
/// read counts its stored bytes more; counting the output alone, the other
/// read counts fewer than it read.
#[test]
fn a_decode_counts_the_larger_of_its_stored_bytes_and_its_output() {
    const N: usize = 1 << 16;
    let mut x = 1_u32;
    let noise: Vec<u8> = (0..N)
        .map(|_| {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            x.to_be_bytes()[0]
        })
        .collect();
    let read = |data: Vec<u8>| {
        let stored = compress(TestCodec::Lz4, &data).len();
        let bundle = build(&BundleSpec {
            info_codec: TestCodec::None,
            data_codec: TestCodec::Lz4,
            block_size: N,
            entries: vec![EntrySpec {
                name: b"e".to_vec(),
                data,
                flags: 0,
            }],
            ..BundleSpec::default()
        });
        let c = ctx();
        let meter = meter_for(&c);
        let b = open_with(&bundle, &c, &meter).unwrap();
        let before = meter.decoded();
        assert_eq!(b.entry_bytes(0).unwrap().len(), N);
        (u64::try_from(stored).unwrap(), meter.decoded() - before)
    };
    let n = u64::try_from(N).unwrap();
    let (zeros_stored, zeros) = read(vec![0; N]);
    let (noise_stored, noise) = read(noise);
    assert!(
        zeros_stored < n && noise_stored > n,
        "{zeros_stored}, {noise_stored}"
    );
    assert!(zeros >= n, "{zeros}");
    // The same steps besides the decode.
    assert_eq!(zeros - n, noise - noise_stored, "{zeros}, {noise}");
}
