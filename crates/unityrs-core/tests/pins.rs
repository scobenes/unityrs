//! What a value kept past its input's handles keeps alive (plan section 8,
//! R3; `unityrs_core::pin`), on testkit bundles of two entries laid back to
//! back: a `SerializedFile` of one object (a `string` and a `TypelessData`,
//! whose values are slices of the file) and a padding entry. Eagerly loaded,
//! the bundle's joined data is one decoded run, so the run's length `L` is
//! the two entries' lengths; the value's slices are `S` bytes (its string's
//! and its data's), and its tree's names `T` bytes: the distinct bytes its
//! rows' types and names name (`Base`/`Base`, one string the root's type and
//! name share, `string`/`m_Name`, `TypelessData`/`m_Data`: 34). A copy is
//! made when `L - S >= max(1 MiB, 4 S)`, and it holds each of those bytes
//! once, `heap_cost(S)` and its owner record (`OWNER_COST`), on the input's
//! meter.

use std::sync::Arc;

use unityrs_core::budget::{OWNER_COST, Reservation};
use unityrs_core::files::{BundleFile, SerializedFile};
use unityrs_core::limits::{Meter, heap_cost};
use unityrs_core::pin::{Copies, UNPIN_MIN, worth_copying};
use unityrs_core::walk::ParseError;
use unityrs_core::{Bytes, File, FileContext, Parsed, PrimArray, RawStr, Value};
use unityrs_testkit::blob::NodeSpec;
use unityrs_testkit::bundle::{BundleSpec, EntrySpec, TestCodec, build};
use unityrs_testkit::serialized::{build as build_file, one_object};
use unityrs_testkit::writer::ByteWriter;

/// The tree's names: the distinct bytes its rows name, `Base` once though
/// two slices name it (perf stage 2's review: each slice was copied on its
/// own, `Base` twice).
const T: u64 = 4 + 6 + 6 + 12 + 6;

fn bytes(n: u64, seed: u64) -> Vec<u8> {
    (0..n)
        .map(|i| u8::try_from((i * 13 + seed) % 251).unwrap())
        .collect()
}

/// A file of one object: `name` (a `string`) and `data` (`TypelessData`).
fn object_file(name: &[u8], data: &[u8]) -> Vec<u8> {
    let nodes = vec![
        NodeSpec::new(0, "Base", "Base"),
        NodeSpec::new(1, "string", "m_Name"),
        NodeSpec::new(1, "TypelessData", "m_Data"),
    ];
    let len = |b: &[u8]| i32::try_from(b.len()).unwrap().to_le_bytes();
    let mut body = Vec::new();
    body.extend_from_slice(&len(name));
    body.extend_from_slice(name);
    while body.len() % 4 != 0 {
        body.push(0);
    }
    body.extend_from_slice(&len(data));
    body.extend_from_slice(data);
    build_file(&one_object(1, nodes, body))
}

/// A bundle of `file` and `pad` padding bytes, cut into blocks of `block`.
fn bundle(file: &[u8], pad: u64, codec: TestCodec, block: u64) -> Vec<u8> {
    build(&BundleSpec {
        data_codec: codec,
        block_size: usize::try_from(block).unwrap(),
        entries: vec![
            EntrySpec {
                name: b"CAB-pins".to_vec(),
                data: file.to_vec(),
                flags: 4,
            },
            EntrySpec {
                name: b"CAB-pins.resS".to_vec(),
                data: bytes(pad, 7),
                flags: 0,
            },
        ],
        ..BundleSpec::default()
    })
}

struct Read {
    meter: Arc<Meter>,
    bundle: Arc<BundleFile>,
    file: Arc<SerializedFile>,
    value: Parsed,
    /// The work the object's read counted (its tree's compile and its walk).
    work: u64,
}

/// Opens `data` and loads it eagerly (one run) or reads only its file's
/// entry: the meter, the bundle and the file.
fn open(data: &[u8], eager: bool) -> (Arc<Meter>, Arc<BundleFile>, Arc<SerializedFile>) {
    let ctx = Arc::new(FileContext::unitypy());
    let meter = Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input);
    let bundle = BundleFile::open(
        RawStr::from("pins.bundle"),
        Bytes::copy_from_slice(data),
        None,
        &meter,
        &ctx,
    )
    .unwrap();
    if eager {
        bundle.load_all().unwrap();
    }
    let File::Serialized(file) = bundle.entry(0).unwrap() else {
        panic!("not a serialized file");
    };
    (meter, bundle, file)
}

/// [`open`], then the object read.
fn read(data: &[u8], eager: bool) -> Read {
    let (meter, bundle, file) = open(data, eager);
    let before = meter.decoded();
    let value = file.objects().next().unwrap().parse_as_dict(true).unwrap();
    let work = meter.decoded() - before;
    Read {
        meter,
        bundle,
        file,
        value,
        work,
    }
}

/// Whether `b` starts in `file`'s bytes (the run they are a slice of).
fn in_run(file: &SerializedFile, b: &[u8]) -> bool {
    let run = file.data().as_ptr().addr()..file.data().as_ptr().addr() + file.data().len();
    run.contains(&b.as_ptr().addr())
}

/// The value's string and data.
fn slices(p: &Parsed) -> (Vec<u8>, Bytes) {
    let Value::Dict(m) = p.value() else {
        panic!("{:?}", p.value())
    };
    let [(_, Value::Str(s)), (_, Value::Bytes(b))] = m.entries() else {
        panic!("{m:?}")
    };
    (s.as_bytes().to_vec(), b.clone())
}

/// What `value` keeps alive once every handle of its input is dropped,
/// beyond its own charge and its node lists'.
fn kept_beyond_own(r: Read) -> u64 {
    let Read {
        meter,
        bundle,
        file,
        value,
        ..
    } = r;
    drop((file, bundle));
    let own = value.charge().bytes()
        + value
            .lists()
            .iter()
            .map(|l| l.charge().bytes())
            .sum::<u64>();
    meter.used() - own
}

fn copies(values: u64, value_bytes: u64, trees: u64, tree_bytes: u64) -> Copies {
    let mut c = Copies::default();
    (c.values, c.value_bytes, c.trees, c.tree_bytes) = (values, value_bytes, trees, tree_bytes);
    c
}

/// A small value (S = 100) in a run of 2 MiB and more: its slices and its
/// tree's names are copied out, each into one buffer, so once the handles
/// drop it keeps those two copies alive and nothing of the run. Before the
/// fix it kept the run (`heap_cost(L)` and more): a kept value held all of
/// its input's decoded bytes.
#[test]
fn a_kept_value_copies_its_slices_out_of_a_large_run() {
    let (name, data) = (bytes(60, 1), bytes(40, 2));
    let file = object_file(&name, &data);
    let r = read(&bundle(&file, 2 << 20, TestCodec::Lz4, 1 << 16), true);
    let l = u64::try_from(file.len()).unwrap() + (2 << 20);
    assert_eq!(r.file.pinned(), l, "the run of both entries");
    assert_eq!(r.file.copies(), copies(1, 100, 1, T));
    let (s, b) = slices(&r.value);
    assert_eq!((s.as_slice(), &b[..]), (name.as_slice(), data.as_slice()));
    assert!(!in_run(&r.file, &b), "the data is a copy");
    let kept = kept_beyond_own(r);
    assert_eq!(
        kept,
        heap_cost(100) + OWNER_COST + heap_cost(T) + OWNER_COST
    );
}

/// The threshold on the file's run: `L - S` of exactly `max(1 MiB, 4 S)`
/// copies, a byte less does not (the padding sets `L`). Under 1 MiB of
/// slices the floor is 1 MiB; at 512 KiB of slices it is four times them.
#[test]
fn a_value_is_copied_out_exactly_when_the_run_is_long_enough() {
    for data_len in [40_u64, (512 << 10) - 60] {
        let (name, data) = (bytes(60, 1), bytes(data_len, 2));
        let s = 60 + data_len;
        let file = object_file(&name, &data);
        let own = u64::try_from(file.len()).unwrap();
        let need = s + UNPIN_MIN.max(4 * s);
        for (pad, copied) in [(need - own, true), (need - own - 1, false)] {
            let r = read(&bundle(&file, pad, TestCodec::Lz4, 1 << 16), true);
            assert_eq!(r.file.pinned(), own + pad);
            assert_eq!(worth_copying(own + pad, s), copied);
            let c = r.file.copies();
            assert_eq!(
                (c.values, c.value_bytes),
                if copied { (1, s) } else { (0, 0) },
                "S {s}, L {}",
                own + pad
            );
            assert_eq!(
                (c.trees, c.tree_bytes),
                (1, T),
                "the names are copied at both"
            );
        }
    }
}

/// Read lazily, an entry is a slice of a run of its own blocks only, so
/// what its value keeps alive is that run (64 KiB blocks: under 1 MiB, no
/// copy). Of a stored bundle it is a slice of the bundle's own bytes, which
/// it keeps alive whole (2 MiB of padding: copied).
#[test]
fn what_a_slice_keeps_alive_is_its_run_or_the_bundles_own_bytes() {
    let file = object_file(&bytes(60, 1), &bytes(40, 2));
    let own = u64::try_from(file.len()).unwrap();
    let lz4 = read(&bundle(&file, 2 << 20, TestCodec::Lz4, 1 << 16), false);
    assert!(lz4.file.pinned() < UNPIN_MIN, "{}", lz4.file.pinned());
    assert!(lz4.file.pinned() >= own);
    assert_eq!(lz4.file.copies(), Copies::default());
    let data = bundle(&file, 2 << 20, TestCodec::None, 1 << 16);
    let stored = read(&data, false);
    assert_eq!(stored.file.pinned(), u64::try_from(data.len()).unwrap());
    assert_eq!(stored.file.copies(), copies(1, 100, 1, T));
}

/// Rows that name one string name one string of the copy (`unityrs_core::pin`,
/// "Each byte once"): 300 rows named by one 1,000-byte string, which the
/// blob holds once, as Unity's does. Their compile goes by buffer (rows that
/// share one cost what one row naming it costs), so the object's read counts
/// the same work whether the tree's names were copied out of a long run
/// (eager) or are slices of a short one (lazy: 64 KiB blocks, no copy). The
/// copy holds `Base`, `int` and the long name once: 1,007 bytes. Before,
/// each row's slice was copied on its own, 300 copies of the name, and the
/// eager read counted 611,048 bytes of work against the lazy read's 12,048.
#[test]
fn rows_that_name_one_string_name_one_string_of_the_copy() {
    let long = format!("m_{}", "x".repeat(998));
    let mut nodes = vec![NodeSpec::new(0, "Base", "Base")];
    nodes.extend((0..300).map(|_| NodeSpec::new(1, "int", &long)));
    let file = build_file(&one_object(1, nodes, vec![0; 300 * 4]));
    let data = bundle(&file, 2 << 20, TestCodec::Lz4, 1 << 16);
    let eager = read(&data, true);
    let lazy = read(&data, false);
    assert_eq!(eager.file.copies(), copies(0, 0, 1, 4 + 3 + 1000));
    assert_eq!(lazy.file.copies(), Copies::default());
    assert_eq!(eager.work, lazy.work, "the compile reads one buffer");
    for r in [&eager, &lazy] {
        let list = r.file.types()[0].nodes.clone().unwrap();
        let first = &list.rows()[1].name;
        assert_eq!(first.as_bytes(), long.as_bytes());
        assert!(
            list.rows()[1..]
                .iter()
                .all(|row| row.name.as_bytes().as_ptr() == first.as_bytes().as_ptr()),
            "the rows name one buffer"
        );
        assert_eq!(
            in_run(&r.file, first.as_bytes()),
            r.file.copies().trees == 0
        );
    }
}

/// `s` as the walk reads a `string`: its length, its bytes, aligned to 4.
fn string(w: &mut ByteWriter, s: &[u8]) {
    w.i32(i32::try_from(s.len()).unwrap()).bytes(s).align(4);
}

/// Every slice `v` holds: its strings and byte runs, depth first.
fn value_slices<'a>(v: &'a Value, out: &mut Vec<&'a [u8]>) {
    match v {
        Value::Str(s) => out.push(s.as_bytes()),
        Value::Bytes(b) | Value::Prims(PrimArray::U8(b)) => out.push(b),
        Value::List(items) => items.iter().for_each(|i| value_slices(i, out)),
        Value::Tuple(items) => items.iter().for_each(|i| value_slices(i, out)),
        Value::Dict(m) => m.entries().iter().for_each(|(_, i)| value_slices(i, out)),
        _ => {}
    }
}

/// A value keeps most of its strings in lists and pairs (an AssetBundle's
/// `m_Container` is a `vector<pair<string, AssetInfo>>`): a `vector<string>`
/// (a list of two), a `pair<string, int>` (a tuple), a `vector<UInt8>` (a
/// byte run) and a `vector<pair<string, int>>` (a list of two tuples), in a
/// run of 2 MiB and more. Every one of their slices is copied out, S being
/// all of their bytes: 5 + 9, 10, 12, 5 + 7 = 48.
#[test]
fn a_values_slices_are_copied_out_of_its_lists_tuples_and_byte_runs() {
    let nodes = vec![
        NodeSpec::new(0, "Base", "Base"),
        NodeSpec::new(1, "vector", "m_Names"),
        NodeSpec::new(2, "Array", "Array"),
        NodeSpec::new(3, "int", "size"),
        NodeSpec::new(3, "string", "data"),
        NodeSpec::new(1, "pair", "m_Pair"),
        NodeSpec::new(2, "string", "first"),
        NodeSpec::new(2, "int", "second"),
        NodeSpec::new(1, "vector", "m_Raw"),
        NodeSpec::new(2, "Array", "Array"),
        NodeSpec::new(3, "int", "size"),
        NodeSpec::new(3, "UInt8", "data"),
        NodeSpec::new(1, "vector", "m_Container"),
        NodeSpec::new(2, "Array", "Array"),
        NodeSpec::new(3, "int", "size"),
        NodeSpec::new(3, "pair", "data"),
        NodeSpec::new(4, "string", "first"),
        NodeSpec::new(4, "int", "second"),
    ];
    let mut w = ByteWriter::le();
    w.i32(2);
    string(&mut w, b"alpha");
    string(&mut w, b"bravo-two");
    string(&mut w, b"pair-first");
    w.i32(7);
    let raw = bytes(12, 3);
    w.i32(12).bytes(&raw);
    w.i32(2);
    for (s, n) in [(&b"c-one"[..], 1), (b"c-two-x", 2)] {
        string(&mut w, s);
        w.i32(n);
    }
    let file = build_file(&one_object(1, nodes, w.into_inner()));
    let r = read(&bundle(&file, 2 << 20, TestCodec::Lz4, 1 << 16), true);
    let Value::Dict(m) = r.value.value() else {
        panic!("{:?}", r.value.value())
    };
    let shapes: Vec<&str> = m
        .entries()
        .iter()
        .map(|(_, v)| match v {
            Value::List(_) => "list",
            Value::Tuple(_) => "tuple",
            Value::Prims(PrimArray::U8(_)) => "u8",
            _ => "other",
        })
        .collect();
    assert_eq!(shapes, ["list", "tuple", "u8", "list"]);
    let mut got = Vec::new();
    value_slices(r.value.value(), &mut got);
    let texts: Vec<&[u8]> = vec![
        b"alpha",
        b"bravo-two",
        b"pair-first",
        &raw,
        b"c-one",
        b"c-two-x",
    ];
    assert_eq!(got, texts);
    assert_eq!(r.file.copies().values, 1);
    assert_eq!(r.file.copies().value_bytes, 48, "every slice, once");
    for s in got {
        assert!(!in_run(&r.file, s), "{s:?} is a copy");
    }
}

/// `check_read`'s error carries the value the read made (what
/// `check_read=False` returns), which a caller may keep as it keeps any
/// value: its slices are copied out too. The object's `byte_size` covers 8
/// bytes past what its tree reads, so the read ends short of it.
#[test]
fn a_check_read_errors_value_is_copied_out_too() {
    let (name, data) = (bytes(60, 1), bytes(40, 2));
    let tree = vec![
        NodeSpec::new(0, "Base", "Base"),
        NodeSpec::new(1, "string", "m_Name"),
        NodeSpec::new(1, "TypelessData", "m_Data"),
    ];
    // `object_file`'s object, then 8 bytes its tree does not read.
    let mut w = ByteWriter::le();
    string(&mut w, &name);
    w.i32(40).bytes(&data).bytes(&[0; 8]);
    let file = build_file(&one_object(1, tree, w.into_inner()));
    let (_meter, _bundle, sf) = open(&bundle(&file, 2 << 20, TestCodec::Lz4, 1 << 16), true);
    let e = sf
        .objects()
        .next()
        .unwrap()
        .parse_as_dict(true)
        .unwrap_err();
    let ParseError::CheckRead { parsed, expected } = e else {
        panic!("{e:?}")
    };
    assert_eq!(i64::from(expected), parsed.bytes_read + 8);
    assert_eq!(sf.copies(), copies(1, 100, 1, T));
    let (s, b) = slices(&parsed);
    assert_eq!((s.as_slice(), &b[..]), (name.as_slice(), data.as_slice()));
    assert!(!in_run(&sf, &b), "the data is a copy");
}

/// A row may name a common string (a high-bit offset into UnityPy's table,
/// or a TPK's), which is not in the file and keeps nothing of it alive: the
/// copy leaves it out. `Base`, `int` (common, offset 222 of UnityPy's
/// table) and `m_Count`: T = 4 + 7. The file is written with `int` in its
/// own buffer, then the row's type offset is set to the common one.
#[test]
fn a_common_string_a_row_names_is_not_copied() {
    let nodes = vec![
        NodeSpec::new(0, "Base", "Base"),
        NodeSpec::new(1, "int", "m_Count"),
    ];
    let mut file = build_file(&one_object(1, nodes, 5_i32.to_le_bytes().to_vec()));
    // The row: version 1 (i16), level 1, flags 0, then the type's offset
    // (`int` at 5, after `Base\0`) and the name's (`m_Count` at 9).
    let row = [1, 0, 1, 0, 5, 0, 0, 0, 9, 0, 0, 0];
    let at: Vec<usize> = file
        .windows(row.len())
        .enumerate()
        .filter_map(|(i, w)| (w == row).then_some(i))
        .collect();
    assert_eq!(at.len(), 1, "one such row");
    // 222 = 0xDE.
    let common = unityrs_core::typetree::commonstring::common_string(222).unwrap();
    assert_eq!(common, b"int");
    file[at[0] + 4..at[0] + 8].copy_from_slice(&(0xDE_u32 | 0x8000_0000).to_le_bytes());
    let r = read(&bundle(&file, 2 << 20, TestCodec::Lz4, 1 << 16), true);
    assert_eq!(r.file.copies(), copies(0, 0, 1, 4 + 7));
    let list = r.file.types()[0].nodes.clone().unwrap();
    let ty = list.rows()[1].ty.as_bytes();
    assert_eq!(ty.as_ptr(), common.as_ptr(), "the table's own string");
    assert!(!in_run(&r.file, list.rows()[1].name.as_bytes()), "copied");
}
