//! The work an entry's parse does, against the open's allowance (round 4's
//! and round 5's inputs): directory rows over one span whose parse fails, or
//! whose AssetBundle read builds a value its container does not keep, stop at
//! the open's `DecodeWork` limit instead of repeating the work once per row,
//! and an entry whose blob names the same NUL-less string on every row costs
//! one search of its string buffer per parse, whatever its rows.

use unityrs_core::files::File;
use unityrs_core::walk::{ParseError, ReadDetail};
use unityrs_core::{Error, FileContext, LimitKind};
use unityrs_testkit::blob::NodeSpec;
use unityrs_testkit::bundle::{BundleSpec, EntrySpec, TestCodec, build};
use unityrs_testkit::serialized::{self, SerializedSpec, no_nul_strings, one_object};

use super::{ctx, ctx_with, meter_for, open_with};

/// The bytes of one directory row named `CAB-%08d` (`offset`, `size`,
/// `flags`, the name and its NUL).
const ROW: usize = 8 + 8 + 4 + 13;

/// Round 4's bundle: UnityFS 6, engine 2018.4.0f1, a stored blocks-info and
/// `entry` in one data block, and `rows` directory rows `CAB-%08d`, every one
/// at offset 0 with the whole entry's size.
pub(super) fn rows_over_one_entry(entry: &[u8], rows: usize) -> Vec<u8> {
    rows_over(entry, rows, |_| entry.len())
}

/// [`rows_over_one_entry`], row `k` sized `size(k)` (row 0 keeps the
/// entry's size).
pub(super) fn rows_over(entry: &[u8], rows: usize, size: impl Fn(usize) -> usize) -> Vec<u8> {
    rows_over_engine(b"2018.4.0f1", entry, rows, size)
}

/// [`rows_over`] in a bundle of engine string `engine`.
fn rows_over_engine(
    engine: &[u8],
    entry: &[u8],
    rows: usize,
    size: impl Fn(usize) -> usize,
) -> Vec<u8> {
    let entries: Vec<EntrySpec> = (0..rows)
        .map(|k| EntrySpec {
            name: format!("CAB-{k:08}").into_bytes(),
            data: if k == 0 { entry.to_vec() } else { Vec::new() },
            flags: 4,
        })
        .collect();
    let mut data = build(&BundleSpec {
        format: 6,
        version_engine: engine.to_vec(),
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
    for k in 1..rows {
        let at = first + k * ROW;
        assert_eq!(&data[at..at + 12], format!("CAB-{k:08}").as_bytes());
        data[at - 20..at - 12].copy_from_slice(&0_i64.to_be_bytes());
        data[at - 12..at - 4].copy_from_slice(&i64::try_from(size(k)).unwrap().to_be_bytes());
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
/// under `Lazy`). Rows over one span are twins now (`bundle` module docs,
/// "Twins"; round 6) and parse it once. Rows over spans of it that differ
/// (here by bytes of `0x7F` past its end, which only make the dependency
/// count huge, so the read still runs past the end) each parse, and what a
/// failing parse was granted is the open's work: each parse here counts at
/// least its R rows' 32 bytes each, so the allowance admits at most
/// `allowance / (32 R) + 1` parses, and every later row is refused with
/// `DecodeWork` without parsing. Before round 4's fix all K rows parsed.
#[test]
fn rows_over_one_failing_entry_stop_at_the_opens_work() {
    const R: usize = 20_000;
    const K: usize = 200;
    const ALLOWANCE: u64 = 16 << 20;
    let entry = no_nul_strings(R, false);
    // Twins: one parse, whatever K.
    let ctx = ctx_with(|c| c.limits.max_decode_work = ALLOWANCE);
    let parse_rows = |rows: usize| {
        let meter = meter_for(&ctx);
        let b = open_with(&rows_over_one_entry(&entry, rows), &ctx, &meter).unwrap();
        let before = meter.decoded();
        for i in 0..rows {
            let got = b.entry(i);
            assert!(failed_parse(&got), "row {i}: {got:?}");
        }
        meter.decoded() - before
    };
    let one = parse_rows(1);
    assert!(
        one >= 32 * u64::try_from(R).unwrap(),
        "a parse counted {one}"
    );
    let all = parse_rows(K);
    assert!(all < 2 * one, "{K} twins counted {all}, one parse {one}");
    // Spans that differ: the allowance.
    let mut padded = entry.clone();
    padded.resize(entry.len() + K, 0x7F);
    let data = rows_over(&padded, K, |k| entry.len() + k);
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
            matches!(g, Err(Error::Limit(l)) if l.kind() == LimitKind::DecodeWork),
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
/// and its version's packing (round 7: `2018.4.0f1`'s 10 bytes and 9 limbs,
/// `version` module docs, "Work"), the same for 1,000 rows as for 20,000.
#[test]
fn an_entrys_blob_strings_cost_one_search_of_its_buffer_whatever_its_rows() {
    const PER_PARSE: u64 = 65_536 + 12 + (10 + 9 * 8);
    for r in [1_000, 20_000] {
        assert_eq!(eager_parse_work(r, 3), 3 * PER_PARSE, "{r} blob rows");
    }
}

/// A `PPtr<Object>` named `name` at `level`.
fn pptr(level: u8, name: &str) -> [NodeSpec; 3] {
    [
        NodeSpec::new(level, "PPtr<Object>", name).size(12),
        NodeSpec::new(level + 1, "int", "m_FileID").size(4),
        NodeSpec::new(level + 1, "SInt64", "m_PathID").size(8),
    ]
}

/// A `vector` of `PPtr<Object>` named `name`, a child of the root.
fn pointers(name: &str) -> Vec<NodeSpec> {
    let mut n = vec![
        NodeSpec::new(1, "vector", name),
        NodeSpec::new(2, "Array", "Array").size(-1),
        NodeSpec::new(3, "int", "size").size(4),
    ];
    n.extend(pptr(3, "data"));
    n
}

/// Round 5's entry: one AssetBundle (class 142) whose `m_PreloadTable` and
/// `m_Container` are empty and whose `m_Junk` holds `n` null pointers, read
/// whole at load (`SerializedFile.py:316-324`) and dropped but for the two
/// empty tables. With `cut`, the object's data stops before the last
/// pointer, so the read fails after building the rest.
fn junk_asset_bundle(n: usize, cut: bool) -> Vec<u8> {
    let mut nodes = vec![NodeSpec::new(0, "AssetBundle", "Base")];
    nodes.extend(pointers("m_PreloadTable"));
    nodes.extend([
        NodeSpec::new(1, "map", "m_Container"),
        NodeSpec::new(2, "Array", "Array").size(-1),
        NodeSpec::new(3, "int", "size").size(4),
        NodeSpec::new(3, "pair", "data"),
        NodeSpec::new(4, "string", "first"),
        NodeSpec::new(5, "Array", "Array").size(-1).aligned(),
        NodeSpec::new(6, "int", "size").size(4),
        NodeSpec::new(6, "char", "data").size(1),
        NodeSpec::new(4, "AssetInfo", "second"),
        NodeSpec::new(5, "int", "preloadIndex").size(4),
        NodeSpec::new(5, "int", "preloadSize").size(4),
    ]);
    nodes.extend(pptr(5, "asset"));
    nodes.extend(pointers("m_Junk"));
    let mut data = Vec::new();
    data.extend_from_slice(&0_i32.to_le_bytes());
    data.extend_from_slice(&0_i32.to_le_bytes());
    data.extend_from_slice(&i32::try_from(n).unwrap().to_le_bytes());
    data.resize(data.len() + 12 * n, 0);
    if cut {
        data.truncate(data.len() - 12);
    }
    serialized::build(&one_object(142, nodes, data))
}

/// Whether `ctx` reads every row's file, and its container, before the
/// allowance runs out: `(the rows read, the rows refused with DecodeWork)`.
/// Row `k` spans the entry and `k` bytes past it, so no two rows are twins.
/// A row is read when its entry parses and its container is read
/// (successfully, or with the read's own failure: `reads_ok` says which).
fn rows_over_one_asset_bundle(
    entry: &[u8],
    rows: usize,
    allowance: u64,
    eager: bool,
    reads_ok: bool,
) -> (usize, usize) {
    // Spans that differ by trailing bytes the file does not read: rows over
    // one span are twins, which share a failing parse (round 6).
    let mut padded = entry.to_vec();
    padded.resize(entry.len() + rows, 0);
    let data = rows_over(&padded, rows, |k| entry.len() + k);
    let ctx = ctx_with(|c: &mut FileContext| {
        c.limits.max_decode_work = allowance;
        c.eager_asset_bundle = eager;
    });
    let meter = meter_for(&ctx);
    let b = open_with(&data, &ctx, &meter).unwrap();
    let (mut read, mut refused) = (0, 0);
    for i in 0..rows {
        let container = match b.entry(i) {
            Ok(File::Serialized(f)) => f.container().map(|_| ()),
            Ok(other) => panic!("row {i}: {other:?}"),
            Err(e) => Err(e),
        };
        match container {
            Ok(()) if reads_ok => read += 1,
            Err(e) if work(&e) => refused += 1,
            // The read's own failure: at load it is the entry's.
            Err(Error::AssetBundle(_) | Error::Entry { .. }) if !reads_ok => read += 1,
            got => panic!("row {i}: {got:?}"),
        }
    }
    (read, refused)
}

/// A `DecodeWork` refusal: the open's, before a read starts, or a read's
/// own, under its entry, once its work took the open past its allowance
/// (`walk::OpenWork`).
fn work(e: &Error) -> bool {
    let read = |p: &ParseError| {
        matches!(p, ParseError::Read(r)
            if matches!(r.detail, ReadDetail::Limit(l) if l.kind() == LimitKind::DecodeWork))
    };
    match e {
        Error::Limit(l) => l.kind() == LimitKind::DecodeWork,
        Error::Entry { source, .. } => work(source),
        Error::AssetBundle(p) => read(p),
        _ => false,
    }
}

/// Round 5 (HIGH): K rows over one entry whose parse succeeds were K reads
/// of its AssetBundle, each building the whole value (here `n` pointers of
/// 12 bytes each; round 5's 1,000,000 cost about 0.25 s and 470 MB of grants
/// a row) and keeping only the empty container, so no limit saw them (64
/// rows of a 4,267-byte input took 15.5 s). What a container read was
/// granted beyond what its container keeps is now the open's work, so the
/// allowance admits at most `allowance / (12 n) + 1` reads (the value is at
/// least the object's own bytes), and every later row is refused with
/// `DecodeWork`, at load (the parse reads the AssetBundle) and lazily (the
/// first `container()` of each row's file reads it), a read that fails
/// counted as one that succeeds. Before the fix every row was read.
#[test]
fn rows_over_one_asset_bundle_stop_at_the_opens_work() {
    const N: usize = 20_000;
    const K: usize = 100;
    const ALLOWANCE: u64 = 16 << 20;
    let most = usize::try_from(ALLOWANCE).unwrap() / (12 * N) + 1;
    assert!(most < K);
    for cut in [false, true] {
        let entry = junk_asset_bundle(N, cut);
        for eager in [true, false] {
            // A failing read at load fails the parse, which the bundle
            // counted before (round 4); the success, and a lazy read either
            // way, are what round 5 found uncounted.
            let (read, refused) = rows_over_one_asset_bundle(&entry, K, ALLOWANCE, eager, !cut);
            assert!(
                (1..=most).contains(&read),
                "cut {cut}, eager {eager}: {read} reads, at most {most} allowed"
            );
            assert_eq!(read + refused, K, "cut {cut}, eager {eager}");
        }
    }
}

/// Review round 6 (LOW): the test above holds with the AssetBundle grant
/// count deleted, the read's walk counting a value's work besides. A read of
/// the container counts what it was granted beyond what its container keeps,
/// whatever its walk counts: here `n` null pointers, whose maps are granted
/// more than the walk counts for them, so a read that counted its walk alone
/// counts less than its grants. Without the count, the first assertion
/// fails.
#[test]
fn a_container_read_counts_what_it_was_granted_beyond_its_container() {
    const N: usize = 20_000;
    for (cut, eager) in [(false, false), (true, false), (false, true)] {
        let entry = junk_asset_bundle(N, cut);
        let data = rows_over_one_entry(&entry, 1);
        let ctx = ctx_with(|c: &mut FileContext| c.eager_asset_bundle = eager);
        let meter = meter_for(&ctx);
        let b = open_with(&data, &ctx, &meter).unwrap();
        b.entry_bytes(0).unwrap();
        let (granted, work, kept) = (meter.granted(), meter.decoded(), meter.used());
        // Lazily, the read is `container()`'s; eagerly, the parse's.
        let read = match b.entry(0) {
            Ok(File::Serialized(f)) if !eager => f.container().map(|_| ()),
            Ok(File::Serialized(_)) => Ok(()),
            got => panic!("{got:?}"),
        };
        assert_eq!(read.is_err(), cut, "cut {cut}: {read:?}");
        let granted = meter.granted() - granted;
        let work = meter.decoded() - work;
        let kept = meter.used() - kept;
        assert!(
            work >= granted - kept,
            "cut {cut}, eager {eager}: the read counted {work}, granted {granted}, kept {kept}"
        );
    }
}

/// Round 6 (group e's sweep, queued for package C): a `SerializedFile` with
/// no version string of its own takes its parent bundle's engine string
/// (chain 2), and every parse of an entry packed that string again,
/// uncounted, stepping through all 226 limbs of a fixed array for each 19
/// digits. Here the engine string is a 4,300-digit major of nines (4,306
/// bytes), whose packing counts 211,210 (`version::tests`, by a model of
/// the rule in Python ints), and its entries have no string of their own:
/// each parse of a row (rows over spans that differ, so no twins) counts
/// at least that packing as the open's work, so the allowance admits at
/// most `allowance / 211,210 + 1` parses and refuses every later row with
/// `DecodeWork`. Before round 7 each parse counted only its own few
/// strings, and every row parsed.
#[test]
fn each_entry_parse_counts_the_packing_of_its_parents_engine_string() {
    const PACKING: u64 = 211_210;
    const K: usize = 40;
    const ALLOWANCE: u64 = 10 * PACKING;
    let engine = format!("{}.0.0f1", "9".repeat(4300)).into_bytes();
    let mut entry = serialized::build(&SerializedSpec {
        unity_version: Vec::new(),
        ..SerializedSpec::default()
    });
    // Detection reads 128 bytes or more as a `SerializedFile` header.
    let len = entry.len().max(128);
    entry.resize(len + K, 0);
    let data = rows_over_engine(&engine, &entry, K, |k| len + k);
    let ctx = ctx_with(|c| c.limits.max_decode_work = ALLOWANCE);
    let meter = meter_for(&ctx);
    let b = open_with(&data, &ctx, &meter).unwrap();
    assert!(meter.decoded() >= PACKING, "the open packs it once");
    let mut parsed = 0_u64;
    for i in 0..K {
        let before = meter.decoded();
        match b.entry(i) {
            Ok(File::Serialized(sf)) => {
                let v = sf.version().unwrap();
                assert_eq!(v.version.raw().as_bytes(), &engine[..]);
                let counted = meter.decoded() - before;
                assert!(
                    counted >= PACKING,
                    "row {i}: its parse counted {counted}, its version's packing {PACKING}"
                );
                parsed += 1;
            }
            Err(e) if work(&e) => {}
            got => panic!("row {i}: {got:?}"),
        }
    }
    let most = ALLOWANCE / PACKING + 1;
    assert!(
        (1..=most).contains(&parsed),
        "{parsed} parses, at most {most} allowed"
    );
    assert!(meter.work_used_up().is_some() || parsed < K as u64);
}
