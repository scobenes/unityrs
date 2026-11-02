//! The compile's work on its rows' strings (review round 13): a name is
//! read once per buffer and counted as the list's work before it is read
//! (`names`'s docs, "Work"), and a type's kind reads at most
//! [`super::KIND_SCAN`] bytes of it. Each test sees the work through the
//! list's meter (plan section 9, "How a test sees bounded work") and the
//! bytes really read through the compile's own counts.

use std::collections::HashMap;
use std::sync::Arc;

use bytes::Bytes;
use unityrs_testkit::{ByteWriter, Rng};

use super::names::READ;
use super::{KIND_SCAN, KINDS, Kind, SCANNED, Tree, kind_of, py_kind_of};
use crate::budget::{Reservation, Unbounded};
use crate::error::LimitKind;
use crate::limits::{Limits, Meter, MeteredVec};
use crate::raw::RawStr;
use crate::stream::{Endian, Reader};
use crate::typetree::parse::parse_blob;
use crate::typetree::{NodeId, NodeList, NodeRow, TreeError};

/// The longest string a blob's row can name: `parse_blob`'s window.
const WINDOW: usize = 32_767;

fn read_now() -> u64 {
    READ.with(std::cell::Cell::get)
}

fn scanned_now() -> u64 {
    SCANNED.with(std::cell::Cell::get)
}

fn input_meter(limits: &Limits) -> Arc<Meter> {
    Meter::for_input(limits, Arc::new(Unbounded), Reservation::Input)
}

/// A list on `meter` of a `Base` root and one child per name in `names`,
/// each child's type and name that one string.
fn list_on(meter: &Arc<Meter>, names: impl IntoIterator<Item = RawStr>) -> Arc<NodeList> {
    let mut rows = MeteredVec::new(meter);
    rows.push(NodeRow::new(0, "Base", "Base")).unwrap();
    for s in names {
        rows.push(NodeRow::new(1, s.clone(), s)).unwrap();
    }
    Arc::new(NodeList::from_rows(rows).unwrap())
}

/// A format-22 blob over `buffer`: a root naming offset 0 for its type and
/// name, then a child at level 1 naming offset `o` for both, for each `o`
/// of `children` (written here: a `NodeSpec` a row would copy its strings).
fn blob(buffer: &[u8], children: &[u32]) -> Vec<u8> {
    let mut w = ByteWriter::le();
    let rows = i32::try_from(children.len() + 1).unwrap();
    w.i32(rows).i32(i32::try_from(buffer.len()).unwrap());
    for (i, (level, offset)) in std::iter::once((0, 0))
        .chain(children.iter().map(|o| (1, *o)))
        .enumerate()
    {
        // version, level, type flags, type, name, byte size, index, meta
        // flag, ref type hash.
        w.i16(1).u8(level).u8(0).u32(offset).u32(offset);
        w.i32(4).i32(i32::try_from(i).unwrap()).i32(0).u64(0);
    }
    w.bytes(buffer);
    w.into_inner()
}

/// Review round 13, the reviewer's size: a blob of 100,000 rows that name
/// one of two 32,766-byte strings of its buffer for their type and name
/// (32 bytes of input a row), parsed as the file's parse reads it. The
/// compile hashed and compared every row's name and searched every row's
/// type to its NUL, twice: four passes of 32,766 bytes a row, which nothing
/// counted. Now the rows that name one offset hold one buffer, which is read
/// once whatever their number (the two strings: one length, two hashes),
/// each type's kind reads at most [`KIND_SCAN`] bytes, and every name byte
/// read is counted on the list's meter: the open's work stays within twice
/// the string buffer, the parse's one pass included.
#[test]
fn rows_naming_one_long_string_read_it_at_most_once() {
    const R: usize = 100_000;
    const L: usize = WINDOW - 1;
    let mut buffer = b"Base\0".to_vec();
    let a = u32::try_from(buffer.len()).unwrap();
    buffer.extend(std::iter::repeat_n(b'A', L).chain([0]));
    let b = u32::try_from(buffer.len()).unwrap();
    buffer.extend(std::iter::repeat_n(b'B', L).chain([0]));
    let children: Vec<u32> = (0..R).map(|i| if i % 2 == 0 { a } else { b }).collect();
    let data = Bytes::from(blob(&buffer, &children));
    let limits = Limits::default();
    let meter = input_meter(&limits);
    let mut r = Reader::new(&data, Endian::Little);
    let rows = parse_blob(&mut r, &data, 22, &meter, &limits, None).unwrap();
    let list = Arc::new(NodeList::from_rows(rows).unwrap());
    let parsed = meter.decoded();
    assert_eq!(
        parsed,
        u64::try_from(buffer.len()).unwrap(),
        "the parse's pass"
    );
    let (read, scanned) = (read_now(), scanned_now());
    let tree = Tree::compile(list, &limits).unwrap();
    let (read, scanned) = (read_now() - read, scanned_now() - scanned);
    let compiled = meter.decoded() - parsed;
    let len = u64::try_from(L).unwrap();
    assert_eq!(read, 2 * len, "the two strings, hashed once each");
    assert_eq!(compiled, read, "every byte read is counted");
    assert!(
        parsed + compiled <= 2 * u64::try_from(buffer.len()).unwrap(),
        "the open's work: {parsed} + {compiled}"
    );
    let rows = u64::try_from(R + 1).unwrap();
    let per_row = u64::try_from(2 * KIND_SCAN).unwrap();
    assert!(
        scanned <= per_row * rows,
        "{scanned} type bytes read for {rows} rows"
    );
    // The names still key as a dict keys them.
    let info = |i: u32| *tree.info(NodeId(i)).unwrap();
    assert!(info(0).dup_names);
    assert_eq!((info(1).key, info(1).slot), (NodeId(1), 0));
    assert_eq!((info(2).key, info(2).slot), (NodeId(2), 1));
    let final_row = u32::try_from(R).unwrap();
    assert_eq!((info(final_row).key, info(final_row).slot), (NodeId(2), 1));
    assert_eq!(info(final_row - 1).key, NodeId(1));
}

/// Review round 13: rows naming R distinct 32,767-byte windows of one
/// buffer of 32,767 + R bytes (R offsets of a blob's string buffer, 32
/// bytes of input a row) hold R names that are equal and that no index of
/// the buffer can tell apart without reading them: the compile hashed and
/// compared each, R x 32,767 bytes twice over, and counted none of it. It
/// is the list's work now, each byte counted before it is read, so an
/// allowance below it refuses the compile ([`LimitKind::DecodeWork`]):
/// under one a byte short of the hashes, it reads nothing; under one that
/// holds the hashes and ten and a half comparisons, it reads the hashes and
/// ten comparisons, counts exactly those, and is refused at the eleventh. Above it, the compile reads each window
/// twice (its hash, its comparison with the first), counts exactly that,
/// and keys every window to the first.
#[test]
fn names_the_compile_reads_are_the_lists_work_and_stop_at_its_allowance() {
    const R: usize = 1_000;
    let buffer = Bytes::from(vec![b'A'; WINDOW + R]);
    let windows = || (0..R).map(|i| RawStr::from_shared(buffer.slice(i..i + WINDOW)));
    let (r, window) = (u64::try_from(R).unwrap(), u64::try_from(WINDOW).unwrap());
    let max = r * window - 1;
    let limits = Limits {
        max_decode_work: max,
        ..Limits::default()
    };
    let meter = input_meter(&limits);
    let before = read_now();
    let e = Tree::compile(list_on(&meter, windows()), &limits).map(|_| ());
    assert_eq!((read_now() - before, meter.decoded()), (0, 0));
    let refused = crate::LimitError {
        kind: LimitKind::DecodeWork,
        value: r * window,
        max,
    };
    assert_eq!(e, Err(TreeError::Limit(refused)));
    let max = (r + 10) * window + window / 2;
    let limits = Limits {
        max_decode_work: max,
        ..Limits::default()
    };
    let meter = input_meter(&limits);
    let before = read_now();
    let Err(e) = Tree::compile(list_on(&meter, windows()), &limits) else {
        panic!("{R} rows of 32,767-byte names compiled within {max} bytes of work");
    };
    let read = read_now() - before;
    assert_eq!(read, (r + 10) * window, "the hashes and ten comparisons");
    assert_eq!(meter.decoded(), read, "counted as read");
    assert_eq!(
        e,
        TreeError::Limit(crate::LimitError {
            kind: LimitKind::DecodeWork,
            value: read + window,
            max
        })
    );

    let meter = input_meter(&Limits::default());
    let before = read_now();
    let tree = Tree::compile(list_on(&meter, windows()), &Limits::default()).unwrap();
    let read = read_now() - before;
    assert_eq!(read, (2 * r - 1) * window, "{read}");
    assert_eq!(meter.decoded(), read, "counted as read");
    for i in 1..=u32::try_from(R).unwrap() {
        let info = tree.info(NodeId(i)).unwrap();
        assert_eq!((info.key, info.slot), (NodeId(1), 0), "child {i}");
    }
}

/// Random classes whose names are shared buffers, distinct buffers of one
/// content, and distinct contents of one length and of others (empty names
/// among them), compiled under the compile's hash and under one that makes
/// every name collide: every child is keyed and slotted as a Python dict
/// keys it (its name's first sibling, slots in first-sighting order),
/// marked `repeated` exactly when a sibling shares its name, and every name
/// byte the compile reads is counted on the list's meter.
#[test]
fn names_number_as_a_dict_keys_them_and_every_byte_read_is_counted() {
    let mut rng = Rng::new(13);
    let pool: Vec<Vec<u8>> = (0..12)
        .map(|i| match i % 4 {
            0 => vec![],
            1 => format!("n{}", i / 4).into_bytes(),
            2 => vec![b'x'; 3],
            _ => format!("{i:03}").into_bytes(),
        })
        .collect();
    let mut checked = 0;
    for case in 0..400 {
        // Each name in its own buffer or in one of the pool's shared ones.
        let shared: Vec<Bytes> = pool.iter().map(|p| Bytes::from(p.clone())).collect();
        let meter = Meter::unlimited();
        let mut rows = MeteredVec::new(&meter);
        rows.push(NodeRow::new(0, "Base", "Base")).unwrap();
        let classes = 1 + rng.below(4);
        for _ in 0..classes {
            rows.push(NodeRow::new(1, "Foo", "c")).unwrap();
            for _ in 0..rng.below(12) {
                let k = rng.below(pool.len());
                let name = if rng.below(2) == 0 {
                    RawStr::from_shared(shared[k].clone())
                } else {
                    RawStr::from(pool[k].as_slice())
                };
                rows.push(NodeRow::new(2, "int", name)).unwrap();
            }
        }
        let list = Arc::new(NodeList::from_rows(rows).unwrap());
        for collide in [false, true] {
            let before = (read_now(), meter.decoded());
            let tree = if collide {
                let (ids, _) = super::names::name_ids(&list, |_| 0).unwrap();
                for (a, x) in list.rows().iter().zip(ids.as_slice()) {
                    for (b, y) in list.rows().iter().zip(ids.as_slice()) {
                        assert_eq!(a.name == b.name, x == y, "case {case}");
                    }
                }
                Tree::compile(list.clone(), &Limits::default()).unwrap()
            } else {
                Tree::compile(list.clone(), &Limits::default()).unwrap()
            };
            let read = read_now() - before.0;
            assert_eq!(meter.decoded() - before.1, read, "case {case}");
            for p in 0..u32::try_from(list.len()).unwrap() {
                let kids = list.children(NodeId(p));
                let mut first: HashMap<&[u8], (NodeId, u32)> = HashMap::new();
                for c in kids {
                    let next = u32::try_from(first.len()).unwrap();
                    first.entry(list.name(*c)).or_insert((*c, next));
                }
                for c in kids {
                    let want = first[list.name(*c)];
                    let got = tree.info(*c).unwrap();
                    assert_eq!((got.key, got.slot), want, "case {case}: child {c:?}");
                    let repeated = kids.iter().filter(|k| list.name(**k) == list.name(*c));
                    assert_eq!(got.repeated, repeated.count() > 1, "case {case}");
                    checked += 1;
                }
                let dup = first.len() < kids.len();
                assert_eq!(tree.info(NodeId(p)).unwrap().dup_names, dup, "case {case}");
            }
        }
    }
    assert!(checked > 5_000, "{checked}");
}

/// The kind a type name's whole C string has: UnityPyBoost's table read to
/// its first NUL, however far (the rule [`kind_of`] reads at most
/// [`KIND_SCAN`] bytes of).
fn c_kind(type_name: &[u8]) -> Kind {
    let c = type_name
        .iter()
        .position(|b| *b == 0)
        .map_or(type_name, |n| &type_name[..n]);
    if c.starts_with(b"PPtr<") {
        return Kind::PPtr;
    }
    KINDS
        .iter()
        .find(|(name, _)| *name == c)
        .map_or(Kind::Unknown, |(_, k)| *k)
}

/// The kind of the pure-Python reader's whole-string comparison.
fn py_kind(type_name: &[u8]) -> Kind {
    if type_name.contains(&0) {
        return Kind::Unknown;
    }
    match c_kind(type_name) {
        Kind::PPtr => Kind::Unknown,
        k => k,
    }
}

/// [`kind_of`] and [`py_kind_of`] read at most [`KIND_SCAN`] bytes of a type
/// name, and give the whole name's kind: every kind name and `PPtr<`, each
/// followed by nothing, by a NUL at every position to 40 bytes past it,
/// and by 32,767 bytes with no NUL or a NUL at their end.
#[test]
fn a_kind_reads_at_most_its_first_bytes_and_is_the_whole_names() {
    let mut names: Vec<Vec<u8>> = Vec::new();
    let mut heads: Vec<&[u8]> = KINDS.iter().map(|(n, _)| *n).collect();
    heads.extend([b"PPtr<".as_slice(), b"PPtr<X>", b"", b"x"]);
    for head in heads {
        names.push(head.to_vec());
        for tail in 0..40 {
            let mut n = head.to_vec();
            n.extend(std::iter::repeat_n(b'x', tail));
            n.push(0);
            n.extend(b"int");
            names.push(n);
        }
        for end in [vec![], vec![0]] {
            let mut n = head.to_vec();
            n.extend(std::iter::repeat_n(b'y', WINDOW));
            n.extend(end);
            names.push(n);
        }
    }
    assert_eq!(
        KINDS.iter().map(|(n, _)| n.len()).max(),
        Some(KIND_SCAN - 1),
        "KIND_SCAN is one past the longest kind name"
    );
    for name in &names {
        let before = scanned_now();
        assert_eq!(
            kind_of(name),
            c_kind(name),
            "{:?}",
            &name[..name.len().min(48)]
        );
        assert!(scanned_now() - before <= u64::try_from(KIND_SCAN).unwrap());
        let before = scanned_now();
        assert_eq!(
            py_kind_of(name),
            py_kind(name),
            "{:?}",
            &name[..name.len().min(48)]
        );
        assert!(scanned_now() - before <= 2 * u64::try_from(KIND_SCAN).unwrap());
    }
}

/// A [`super::NameTable`] over strings (two entries of one content among
/// them, each in a buffer of its own): a list whose names are all clones of
/// its strings compiles through it reading no name, keyed, slotted and
/// marked as the plain compile does it; a list with one name it does not
/// hold (an equal string in a buffer of its own) is compiled as the plain
/// compile compiles it, reading its names.
#[test]
fn a_name_table_knows_its_strings_buffers_and_nothing_else() {
    let strings: Vec<RawStr> = ["Base", "int", "a", "a", "b"]
        .into_iter()
        .map(RawStr::from)
        .collect();
    let table = super::NameTable::new(&strings, &Meter::unlimited()).unwrap();
    let list = |names: Vec<RawStr>| {
        let mut rows = vec![NodeRow::new(0, strings[0].clone(), strings[0].clone())];
        rows.extend(
            names
                .into_iter()
                .map(|n| NodeRow::new(1, strings[1].clone(), n)),
        );
        Arc::new(NodeList::trusted(rows).unwrap())
    };
    let s = |i: usize| strings[i].clone();
    let ours = list(vec![s(2), s(3), s(4), s(2)]);
    let outside = list(vec![s(2), RawStr::from("a"), s(4)]);
    for (what, list, reads) in [("ours", ours, false), ("outside", outside, true)] {
        let before = read_now();
        let with = Tree::compile_with_names(list.clone(), &Limits::default(), &table).unwrap();
        assert_eq!(read_now() > before, reads, "{what}");
        let plain = Tree::compile(list.clone(), &Limits::default()).unwrap();
        for i in 0..u32::try_from(list.len()).unwrap() {
            assert_eq!(with.info(NodeId(i)), plain.info(NodeId(i)), "{what}: {i}");
        }
        let key = |i: u32| with.info(NodeId(i)).unwrap().key;
        assert_eq!((key(1), key(2)), (NodeId(1), NodeId(1)), "{what}");
    }
}
