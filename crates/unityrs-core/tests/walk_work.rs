//! Every read and every write of a value counts its work toward its open's
//! `Limits::max_decode_work` (`walk`'s module docs, "Work"; plan 3.10,
//! "Bounds"; `PARITY.md` 19.3, "decode work"), and tests see that work
//! through the open's meter (plan section 9, "How a test sees bounded
//! work").
//!
//! Objects may share their bytes, and a read refused by a bound of its own
//! keeps nothing, so K objects over one span repeated it K times with no
//! bound on the input (fix pass 4's sweep: every object of a small file
//! over one vector of 2^22 empty structs, each refused after 2^20 of them,
//! the time growing with the objects). Every object also searches its
//! file's one list of ref types again, so K objects over R ref types took
//! K x R steps no value counted (fix pass 5's sweep). Now each read's and
//! write's work counts, its lookups' included, and once the open's work has
//! passed its allowance every further read or write of it is refused
//! before it starts.
//!
//!     scripts/dev cargo test -p unityrs-core --test walk_work

use std::sync::Arc;

use unityrs_core::budget::{DecodeBuf, Reservation};
use unityrs_core::files::SerializedType;
use unityrs_core::limits::{Limits, Meter};
use unityrs_core::stream::Endian;
use unityrs_core::typetree::{NodeList, NodeRow, Tree};
use unityrs_core::walk::{
    self, ParseError, ReadCtx, ReadDetail, ReadError, ReadErrorKind, ReadOptions, RefTypes,
    VALUE_WORK, WriteCtx, WriteError, WriteRules,
};
use unityrs_core::{Bytes, FileContext, LimitError, LimitKind, Parsed, RawStr, SerializedFile};
use unityrs_testkit::blob::NodeSpec;
use unityrs_testkit::serialized::{ObjectSpec, build, one_object};

fn tree(rows: &[(i32, &str, &str)]) -> Arc<Tree> {
    let rows = rows
        .iter()
        .map(|&(level, ty, name)| NodeRow::new(level, ty, name))
        .collect();
    Tree::compile(
        Arc::new(NodeList::trusted(rows).unwrap()),
        &Limits::default(),
    )
    .unwrap()
}

/// `Base { int a; vector<int> b; vector<UInt8> u; vector<Empty> e }`.
fn mixed() -> Arc<Tree> {
    tree(&[
        (0, "Base", "Base"),
        (1, "int", "a"),
        (1, "vector", "b"),
        (2, "Array", "Array"),
        (3, "int", "size"),
        (3, "int", "data"),
        (1, "vector", "u"),
        (2, "Array", "Array"),
        (3, "int", "size"),
        (3, "UInt8", "data"),
        (1, "vector", "e"),
        (2, "Array", "Array"),
        (3, "int", "size"),
        (3, "Empty", "data"),
    ])
}

/// `a` = 7, `b` = [1, 2, 3], `u` = 5 bytes, `e` = two empty structs (no
/// node is flagged, so nothing aligns).
fn mixed_bytes() -> Vec<u8> {
    let mut d = 7_i32.to_le_bytes().to_vec();
    d.extend(3_i32.to_le_bytes());
    for x in [1_i32, 2, 3] {
        d.extend(x.to_le_bytes());
    }
    d.extend(5_i32.to_le_bytes());
    d.extend([1, 2, 3, 4, 5]);
    d.extend(2_i32.to_le_bytes());
    d
}

fn work_meter(max_decode_work: u64) -> Arc<Meter> {
    Meter::with_decode_work(
        u64::MAX,
        max_decode_work,
        Arc::new(unityrs_core::budget::Unbounded),
        Reservation::Input,
    )
}

fn read(
    reader: &str,
    data: &[u8],
    t: &Arc<Tree>,
    options: &ReadOptions,
    meter: &Arc<Meter>,
) -> Result<Parsed, ReadError> {
    read_with(reader, data, t, &[], options, meter)
}

fn read_with(
    reader: &str,
    data: &[u8],
    t: &Arc<Tree>,
    refs: &[SerializedType],
    options: &ReadOptions,
    meter: &Arc<Meter>,
) -> Result<Parsed, ReadError> {
    let data = Bytes::copy_from_slice(data);
    let ctx = ReadCtx {
        endian: Endian::Little,
        ref_types: RefTypes::List(refs),
        options,
        meter,
    };
    match reader {
        "boost" => walk::boost::read(&data, t, &ctx),
        _ => walk::python::read(&data, 0, t, &ctx),
    }
}

fn write(p: &Parsed, t: &Tree, limits: &Limits, out: &mut DecodeBuf) -> Result<(), WriteError> {
    write_with(p, t, &[], limits, out)
}

fn write_with(
    p: &Parsed,
    t: &Tree,
    refs: &[SerializedType],
    limits: &Limits,
    out: &mut DecodeBuf,
) -> Result<(), WriteError> {
    let ctx = WriteCtx {
        endian: Endian::Little,
        ref_types: RefTypes::List(refs),
        rules: WriteRules::UnityPy,
        limits,
    };
    walk::write::write(p, t, &ctx, out)
}

fn decode_work(e: &LimitError) -> bool {
    e.kind == LimitKind::DecodeWork
}

/// What one value costs is the size of the value that holds it.
#[test]
fn a_value_costs_the_size_of_its_value() {
    assert_eq!(
        VALUE_WORK,
        u64::try_from(size_of::<unityrs_core::Value>()).unwrap()
    );
}

/// A read counts [`VALUE_WORK`] for each value it builds and the width of
/// each element of a primitive run, in both readers; a write of the value
/// counts [`VALUE_WORK`] for each value it writes but a primitive, which
/// counts the bytes it writes.
#[test]
fn a_read_and_a_write_count_their_work_on_the_open() {
    let t = mixed();
    let data = mixed_bytes();
    // Values: the root, `a`, the three vectors and `e`'s two elements.
    // Runs: `b`'s three ints and `u`'s five bytes.
    let read_work = 7 * VALUE_WORK + 3 * 4 + 5;
    // Written: the root, the three vectors and `e`'s two elements are
    // values; `a`, `b`'s three ints and `u`'s five bytes are primitives.
    let write_work = 6 * VALUE_WORK + 4 + 3 * 4 + 5;
    for reader in ["boost", "python"] {
        let meter = work_meter(u64::MAX);
        let p = read(reader, &data, &t, &ReadOptions::default(), &meter).unwrap();
        assert_eq!(meter.decoded(), read_work, "{reader}: the read");
        if reader == "python" {
            continue;
        }
        let mut out = DecodeBuf::new(&meter, 0, u64::MAX, LimitKind::InputMemory).unwrap();
        write(&p, &t, &Limits::default(), &mut out).unwrap();
        assert_eq!(out.as_slice(), data.as_slice(), "the write's bytes");
        assert_eq!(
            meter.decoded(),
            read_work + write_work,
            "{reader}: the write"
        );
    }
}

/// A read or a write refused by a bound of its own counts the work it did
/// before the refusal, and none past it.
#[test]
fn a_refused_read_or_write_counts_what_it_did() {
    let t = mixed();
    let data = mixed_bytes();
    let mut limits = Limits::default();
    limits.max_values_per_object = 5;
    let options = ReadOptions {
        limits,
        ..ReadOptions::default()
    };
    for reader in ["boost", "python"] {
        let meter = work_meter(u64::MAX);
        let r = read(reader, &data, &t, &options, &meter);
        assert!(
            matches!(&r, Err(ReadError { detail: ReadDetail::Limit(l), .. })
                if l.kind == LimitKind::ValuesPerObject),
            "{reader}: {r:?}"
        );
        // Counted: the root, `a`, the vector `b`; then UnityPyBoost counts
        // `b`'s run of three at once, which would make six, and the
        // pure-Python reader reads it one by one, two of them before the
        // sixth value is refused.
        let run = if reader == "boost" { 0 } else { 2 * 4 };
        assert_eq!(meter.decoded(), 3 * VALUE_WORK + run, "{reader}");
    }
    let meter = work_meter(u64::MAX);
    let p = read("boost", &data, &t, &ReadOptions::default(), &meter).unwrap();
    let before = meter.decoded();
    let mut out = DecodeBuf::new(&meter, 0, u64::MAX, LimitKind::InputMemory).unwrap();
    let w = write(&p, &t, &limits, &mut out);
    assert!(
        matches!(&w, Err(WriteError::Limit(l)) if l.kind == LimitKind::ValuesPerObject),
        "{w:?}"
    );
    // Written: the root, `a` (4 bytes), the vector `b` and two of its ints;
    // refused: its third.
    assert_eq!(meter.decoded() - before, 2 * VALUE_WORK + 4 + 2 * 4);
}

/// Once the open's work has passed its allowance, a read or a write is
/// refused before it starts (`limit DecodeWork`, kind `other` for a read),
/// and counts nothing more.
#[test]
fn past_the_opens_work_a_read_or_a_write_is_refused_before_it_starts() {
    let t = mixed();
    let data = mixed_bytes();
    let read_work = 7 * VALUE_WORK + 3 * 4 + 5;
    for reader in ["boost", "python"] {
        // Room for one read and part of a second: the second is done, and
        // passes the allowance; the third is refused.
        let max = read_work + read_work / 2;
        let meter = work_meter(max);
        let options = ReadOptions::default();
        read(reader, &data, &t, &options, &meter).unwrap();
        read(reader, &data, &t, &options, &meter).unwrap();
        assert_eq!(meter.decoded(), 2 * read_work, "{reader}");
        let r = read(reader, &data, &t, &options, &meter);
        let want = LimitError {
            kind: LimitKind::DecodeWork,
            value: 2 * read_work,
            max,
        };
        assert_eq!(
            r.unwrap_err(),
            ReadError {
                kind: ReadErrorKind::Other,
                at: 0,
                detail: ReadDetail::Limit(want),
            },
            "{reader}"
        );
        assert_eq!(
            meter.decoded(),
            2 * read_work,
            "{reader}: a refusal counts nothing"
        );
    }
    let meter = work_meter(read_work);
    let p = read("boost", &data, &t, &ReadOptions::default(), &meter).unwrap();
    let mut out = DecodeBuf::new(&meter, 0, u64::MAX, LimitKind::InputMemory).unwrap();
    // The read used the allowance up exactly: within it.
    write(&p, &t, &Limits::default(), &mut out).unwrap();
    let after = meter.decoded();
    assert!(after > read_work);
    out.truncate(0);
    let w = write(&p, &t, &Limits::default(), &mut out);
    assert!(
        matches!(&w, Err(WriteError::Limit(l)) if decode_work(l) && l.value == after),
        "{w:?}"
    );
    assert!(out.as_slice().is_empty(), "a refused write writes nothing");
    assert_eq!(meter.decoded(), after, "a refusal counts nothing");
}

/// Fix pass 4's sweep, scaled down: `K` objects of a SerializedFile over one
/// 4-byte span, each a vector of empty structs one past the zero-size bound,
/// read through the file's objects on both readers (`byte_size` 4, and 0
/// for the pure-Python path). Each read is refused after the same number of
/// values, and counts them; once the open's work has passed its allowance,
/// every later object is refused before its read starts, so the input's
/// reads together stop at its allowance and one read past it, whatever `K`.
#[test]
fn objects_over_one_span_stop_at_the_opens_work() {
    const K: usize = 64;
    const ZERO: u64 = 1 << 10;
    // The root, the vector and every element, the one that trips included.
    let per_read = (ZERO + 3) * VALUE_WORK;
    for byte_size in [4, 0] {
        let reader = if byte_size == 4 { "boost" } else { "python" };
        let data = empties(K, ZERO + 1, byte_size);
        let all = |max_decode_work: u64| {
            let mut c = FileContext::unitypy();
            c.limits.max_zero_size_elements = ZERO;
            c.limits.max_decode_work = max_decode_work;
            let c = Arc::new(c);
            let m = Meter::for_input(&c.limits, c.budget.clone(), Reservation::Input);
            let f = SerializedFile::parse(
                RawStr::from("CAB-w"),
                Bytes::from(data.clone()),
                None,
                &m,
                &c,
            )
            .unwrap();
            let load = m.decoded();
            let outcomes: Vec<_> = f.objects().map(|o| o.parse_as_dict(true)).collect();
            (load, m.decoded() - load, outcomes)
        };
        // Unbounded: every read is refused at its own bound, and counted.
        let (load, reads, outcomes) = all(u64::MAX);
        assert_eq!(reads, u64::try_from(K).unwrap() * per_read, "{reader}");
        for o in &outcomes {
            assert_eq!(
                limit_of(o).map(|l| l.kind),
                Some(LimitKind::ZeroSizeElements),
                "{reader}"
            );
        }
        // Room for four reads and part of a fifth: five are done, and the
        // fifth passes the allowance; the rest are refused before they read.
        let max = load + 4 * per_read + per_read / 2;
        let (_, reads, outcomes) = all(max);
        assert_eq!(reads, 5 * per_read, "{reader}: the input's reads stop");
        let kinds: Vec<_> = outcomes.iter().map(|o| limit_of(o).unwrap()).collect();
        for (i, l) in kinds.iter().enumerate() {
            if i < 5 {
                assert_eq!(l.kind, LimitKind::ZeroSizeElements, "{reader}: object {i}");
            } else {
                assert_eq!(
                    *l,
                    LimitError {
                        kind: LimitKind::DecodeWork,
                        value: load + 5 * per_read,
                        max,
                    },
                    "{reader}: object {i}"
                );
            }
        }
    }
}

/// A SerializedFile of `k` objects at one offset, each of `byte_size`, each
/// `Base { vector v { Array { int size; Empty data } } }` of `count`
/// elements: one 4-byte span that every object reads.
fn empties(k: usize, count: u64, byte_size: u32) -> Vec<u8> {
    let nodes = vec![
        NodeSpec::new(0, "Base", "Base"),
        NodeSpec::new(1, "vector", "v"),
        NodeSpec::new(2, "Array", "Array"),
        NodeSpec::new(3, "int", "size"),
        NodeSpec::new(3, "Empty", "data"),
    ];
    let mut spec = one_object(1, nodes, Vec::new());
    let first = spec.objects[0].clone();
    let span = i32::try_from(count).unwrap().to_le_bytes().to_vec();
    spec.objects = (1..=k)
        .map(|i| ObjectSpec {
            path_id: i64::try_from(i).unwrap(),
            data: if i == k { span.clone() } else { Vec::new() },
            byte_size: Some(byte_size),
            ..first.clone()
        })
        .collect();
    build(&spec)
}

fn limit_of(o: &Result<Parsed, ParseError>) -> Option<LimitError> {
    match o {
        Err(ParseError::Read(ReadError {
            kind: ReadErrorKind::Other,
            detail: ReadDetail::Limit(l),
            ..
        })) => Some(*l),
        _ => None,
    }
}

/// `Base { ReferencedObject r; ReferencedObject s }`, each a `type` of three
/// names and its `data`.
fn two_refs() -> Arc<Tree> {
    let mut rows = vec![(0, "Base", "Base")];
    for name in ["r", "s"] {
        rows.extend([
            (1, "ReferencedObject", name),
            (2, "ReferencedManagedType", "type"),
            (3, "string", "class"),
            (3, "string", "ns"),
            (3, "string", "asm"),
            (2, "ReferencedObjectData", "data"),
        ]);
    }
    tree(&rows)
}

/// Both references name `("X", "n", "a")`, whose data is one `int`.
fn two_refs_bytes() -> Vec<u8> {
    let mut d = Vec::new();
    for _ in 0..2 {
        for s in ["X", "n", "a"] {
            d.extend(1_i32.to_le_bytes());
            d.extend(s.as_bytes());
            d.extend([0, 0, 0]);
        }
        d.extend(9_i32.to_le_bytes());
    }
    d
}

/// `fillers` ref types `("C0000", "n", "a")`, `("C0001", "n", "a")` and so
/// on, which no reference names, then `("X", "n", "a")`.
fn ref_list(fillers: usize) -> Vec<SerializedType> {
    let z = Arc::new(
        NodeList::trusted(vec![
            NodeRow::new(0, "X", "Base"),
            NodeRow::new(1, "int", "z"),
        ])
        .unwrap(),
    );
    let one = |class: String| {
        SerializedType::ref_type(
            114,
            RawStr::from(class.as_str()),
            RawStr::from("n"),
            RawStr::from("a"),
            Some(z.clone()),
        )
    };
    (0..fillers)
        .map(|i| one(format!("C{i:04}")))
        .chain([one("X".into())])
        .collect()
}

/// A ref type a lookup compares, a ref type its table indexes and a probe
/// of the table count [`VALUE_WORK`] and the name bytes they read, in both
/// readers and in a write. Of two references to the last of `R + 1` ref
/// types, the first scans the list; for the second, the scans have passed
/// the list's length, which is longer than 8, so the list's table is built
/// and probed once. A list of `X` alone is scanned twice.
#[test]
fn a_lookup_counts_the_ref_types_it_reads() {
    const R: u64 = 100;
    let t = two_refs();
    let data = two_refs_bytes();
    let (small, big) = (ref_list(0), ref_list(usize::try_from(R).unwrap()));
    // A filler's names are 7 bytes, `X`'s 3.
    let (filler, x) = (VALUE_WORK + 7, VALUE_WORK + 3);
    let big_refs = R * filler + x + R * filler + x + x;
    let small_refs = x + x;
    for reader in ["boost", "python"] {
        let mut work = Vec::new();
        for refs in [&small, &big] {
            let meter = work_meter(u64::MAX);
            let p = read_with(reader, &data, &t, refs, &ReadOptions::default(), &meter).unwrap();
            let read = meter.decoded();
            let mut out = DecodeBuf::new(&meter, 0, u64::MAX, LimitKind::InputMemory).unwrap();
            write_with(&p, &t, refs, &Limits::default(), &mut out).unwrap();
            assert_eq!(
                out.as_slice(),
                data.as_slice(),
                "{reader}: the write's bytes"
            );
            work.push((read, meter.decoded() - read));
        }
        let more = big_refs - small_refs;
        assert_eq!(work[1].0 - work[0].0, more, "{reader}: the read");
        assert_eq!(work[1].1 - work[0].1, more, "{reader}: the write");
    }
}

/// Objects that each search one long list of ref types: each read counts
/// its scan, so the open's reads together stop at its allowance and one
/// read past it, whatever the number of objects.
#[test]
fn objects_over_many_ref_types_stop_at_the_opens_work() {
    const K: usize = 64;
    const R: u64 = 4096;
    let t = two_refs();
    let data = two_refs_bytes();
    let refs = ref_list(usize::try_from(R).unwrap());
    for reader in ["boost", "python"] {
        let options = ReadOptions::default();
        let one = work_meter(u64::MAX);
        read_with(reader, &data, &t, &refs, &options, &one).unwrap();
        let per_read = one.decoded();
        assert!(per_read > 2 * R * VALUE_WORK, "{reader}: {per_read}");
        let max = 4 * per_read + per_read / 2;
        let meter = work_meter(max);
        let outcomes: Vec<_> = (0..K)
            .map(|_| read_with(reader, &data, &t, &refs, &options, &meter))
            .collect();
        assert_eq!(meter.decoded(), 5 * per_read, "{reader}: the reads stop");
        for (i, o) in outcomes.iter().enumerate() {
            if i < 5 {
                assert!(o.is_ok(), "{reader}: object {i}: {o:?}");
            } else {
                assert_eq!(
                    o.as_ref().unwrap_err().detail,
                    ReadDetail::Limit(LimitError {
                        kind: LimitKind::DecodeWork,
                        value: 5 * per_read,
                        max,
                    }),
                    "{reader}: object {i}"
                );
            }
        }
    }
}
