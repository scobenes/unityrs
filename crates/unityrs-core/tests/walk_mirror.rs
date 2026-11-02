//! `WriteRules::MirrorReader` (plan 3.11) against UnityPy 1.25.2's two
//! readers: a value written under the mirror reads back, through the reader
//! that read it, to the same value.
//!
//!     scripts/dev cargo test -p unityrs-core --test walk_mirror
//!
//! The cases are `tools/fixtures/walk.py`'s (`tests/fixtures/walk/cases.json`),
//! each read by UnityPyBoost over the object's bytes and by the pure-Python
//! reader over the whole buffer, as the fixture recorded them
//! (`walk_fixtures.rs` replays those outcomes and UnityPy's writes of them).

use serde_json::{Value as Json, json};
use unityrs_core::limits::{Limits, Meter};
use unityrs_core::typetree::{NodeId, Tree};
use unityrs_core::walk::{self, ReadCtx, ReadDetail, ReadError, ReadOptions, WriteRules};
use unityrs_core::{Bytes, Parsed, ReadBy};

mod walk_common;

use walk_common::{Refs, compile, enc, endian, hex, load, unhex, write_out};

/// Whether no two children of a node share a name, in the tree and in each
/// ref type's own: a repeated name keeps its last value only, so no writer
/// gives back the others' bytes.
fn names_unique(tree: &Tree, refs: &Refs) -> bool {
    let unique = |t: &Tree| {
        (0..t.list().len()).all(|i| {
            let id = NodeId(u32::try_from(i).unwrap());
            t.info(id).is_none_or(|x| x.key == id)
        })
    };
    let ref_unique = match refs {
        Refs::List(l) => l.iter().all(|r| match r.tree(&Limits::default()) {
            Some(Ok(t)) => unique(&t),
            _ => true,
        }),
        _ => true,
    };
    unique(tree) && ref_unique
}

/// Why no writer gives back the bytes of a recorded case the pure-Python
/// reader read, if none does: its position moved other than forward over
/// the bytes the value holds (a negative `TypelessData` length moves it back,
/// one past the end of the bytes moves it past them, `CANONICAL_DUMP.md`
/// section 7), or it started at an odd position, aligning to the bytes'
/// start where the writer aligns to the object's. The test checks that each
/// case named here does not round-trip, so the list cannot hide a new one.
fn py_not_given_back(name: &str) -> Option<&'static str> {
    if name.starts_with("py_odd_start") {
        return Some("an odd start");
    }
    if name.starts_with("py_base_read_") || name == "py_vector_of_bytes_negative" {
        return Some("a negative TypelessData length");
    }
    (name == "r02_bytes_past_end").then_some("a TypelessData length past the end")
}

/// The two readers, each over a case's bytes as `walk.py` hands them over.
#[derive(Clone, Copy, Debug)]
enum Reader {
    Boost,
    Python,
}

impl Reader {
    /// The fixture's key for this reader's outcome.
    fn key(self) -> &'static str {
        match self {
            Self::Boost => "boost",
            Self::Python => "python",
        }
    }

    /// What a value this reader built says of it ([`Parsed::read_by`]).
    fn read_by(self) -> ReadBy {
        match self {
            Self::Boost => ReadBy::Boost,
            Self::Python => ReadBy::Python,
        }
    }

    fn read(
        self,
        data: &Bytes,
        start: usize,
        tree: &std::sync::Arc<Tree>,
        ctx: &ReadCtx<'_>,
    ) -> Result<Parsed, ReadError> {
        match self {
            Self::Boost => walk::boost::read(&data.slice(start..), tree, ctx),
            Self::Python => walk::python::read(data, i128::try_from(start).unwrap(), tree, ctx),
        }
    }
}

/// Each value a reader reads from a tree that repeats no name writes, under
/// the mirror, to bytes the same reader reads back (from the start of the
/// bytes written) to the same value and the same `bytes_read`, but the
/// pure-Python reads [`py_not_given_back`] names. Under `WriteRules::UnityPy`
/// some do not: a flagged run's elements each pad, an empty-class
/// `ReferencedObject` fails, a type name holding a NUL is a class, and so
/// on, so the mirror is what makes the round trip.
#[test]
fn mirror_reader_round_trips_what_each_reader_reads() {
    let cases = load("cases.json");
    for reader in [Reader::Boost, Reader::Python] {
        let (mut n, mut unitypy_differs, mut lossy) = (0, 0, 0);
        let mut bad = Vec::new();
        for c in cases.as_array().unwrap() {
            let name = c["name"].as_str().unwrap();
            let tree = compile(&c["nodes"]);
            let refs = Refs::of(c.get("ref_types"));
            let data = Bytes::from(unhex(c["hex"].as_str().unwrap()));
            let start =
                usize::try_from(c.get("start").and_then(Json::as_i64).unwrap_or(0)).unwrap();
            let options = ReadOptions::default();
            let meter = Meter::unlimited();
            let ctx = ReadCtx {
                endian: endian(c),
                ref_types: refs.get(),
                options: &options,
                meter: &meter,
            };
            let Ok(first) = reader.read(&data, start, &tree, &ctx) else {
                continue;
            };
            assert_eq!(first.read_by(), reader.read_by(), "{name}");
            if !names_unique(&tree, &refs) {
                continue;
            }
            n += 1;
            let not_given_back = match reader {
                Reader::Boost => None,
                Reader::Python => py_not_given_back(name),
            };
            lossy += usize::from(not_given_back.is_some());
            for rules in [WriteRules::MirrorReader, WriteRules::UnityPy] {
                let written = write_out(&first, &tree, endian(c), refs.get(), rules);
                let bytes = written["hex"].as_str().map(|h| Bytes::from(unhex(h)));
                let same = bytes.as_ref().is_some_and(|b| {
                    reader.read(b, 0, &tree, &ctx).is_ok_and(|p| {
                        enc(&p, p.value()) == enc(&first, first.value())
                            && p.bytes_read == first.bytes_read
                            && i64::try_from(b.len()).unwrap() == first.bytes_read
                    })
                });
                match (rules, not_given_back) {
                    (WriteRules::MirrorReader, None) if !same => {
                        bad.push(format!("{name}: {written}"));
                    }
                    (WriteRules::MirrorReader, Some(why)) if same => {
                        bad.push(format!("{name} round-trips, though listed ({why})"));
                    }
                    (WriteRules::UnityPy, None) if !same => unitypy_differs += 1,
                    _ => {}
                }
            }
        }
        assert!(n > 80, "{reader:?}: only {n} cases read");
        assert!(
            bad.is_empty(),
            "{reader:?}: {} of {n} do not round-trip:\n{}",
            bad.len(),
            bad.join("\n")
        );
        assert!(unitypy_differs > 0, "{reader:?}: no case needs the mirror");
        if let Reader::Python = reader {
            assert_eq!(lossy, 19, "the cases py_not_given_back names");
        }
    }
}

/// The `mirror_` cases (`tools/fixtures/walk_mirror.py`), trees each reader
/// reads its own way: each reader that reads one, by UnityPy 1.25.2's record
/// (`walk_fixtures.rs` replays every outcome, so the value is UnityPy's),
/// reads exactly its bytes, padding as zeros, so the mirror writes back the
/// very bytes that reader read: a pure-Python read as `read_value_array`
/// reads a flagged run (aligned once after; a `pair`'s primitive children
/// without their own flags; a class with no registry check; 16-bit
/// signedness swapped), an empty string unaligned, a type name holding a
/// NUL as a class; a UnityPyBoost read as UnityPyBoost reads (review round
/// 11: the mirror wrote every value as UnityPyBoost reads, and refused the
/// pure-Python value of a zero-byte object read through `int\0x`).
#[test]
fn mirror_reader_writes_back_the_bytes_each_reader_read() {
    let cases = load("cases.json");
    let (mut n, mut reads, mut unitypy_differs) = (0, [0; 2], 0);
    let mut bad = Vec::new();
    for c in cases.as_array().unwrap() {
        let name = c["name"].as_str().unwrap();
        if !name.starts_with("mirror_") {
            continue;
        }
        n += 1;
        let tree = compile(&c["nodes"]);
        let refs = Refs::of(c.get("ref_types"));
        let data = Bytes::from(unhex(c["hex"].as_str().unwrap()));
        assert!(c.get("start").is_none(), "{name} reads from 0");
        let options = ReadOptions::default();
        let meter = Meter::unlimited();
        let ctx = ReadCtx {
            endian: endian(c),
            ref_types: refs.get(),
            options: &options,
            meter: &meter,
        };
        for (i, reader) in [Reader::Boost, Reader::Python].into_iter().enumerate() {
            // UnityPy's outcome: where its read of these bytes ended.
            let Some(pos) = c[reader.key()]["pos"].as_i64() else {
                continue;
            };
            reads[i] += 1;
            let want = json!({ "hex": hex(&data[..usize::try_from(pos).unwrap()]) });
            let value = reader.read(&data, 0, &tree, &ctx).unwrap();
            let have = write_out(
                &value,
                &tree,
                endian(c),
                refs.get(),
                WriteRules::MirrorReader,
            );
            if have != want {
                bad.push(format!(
                    "{name} [{}]\n  want {want}\n  have {have}",
                    reader.key()
                ));
            }
            unitypy_differs += usize::from(c[format!("{}_write", reader.key())] != want);
        }
    }
    assert_eq!(n, 13, "the mirror_ cases");
    assert_eq!(
        reads[1], n,
        "the pure-Python reader reads every mirror_ case"
    );
    assert!(
        reads[0] >= 6,
        "UnityPyBoost reads only {} mirror_ cases",
        reads[0]
    );
    assert!(
        bad.is_empty(),
        "{} mirror writes differ:\n{}",
        bad.len(),
        bad.join("\n")
    );
    assert!(
        unitypy_differs >= 10,
        "only {unitypy_differs} need the mirror"
    );
}

/// A `ReferencedObject`'s data resolves, under the mirror, by the rule of
/// the reader that read the value (plan R14; `walk.py`'s `r14_` cases record
/// both on every kind of `class`): its `class` an `int` 0, UnityPyBoost
/// skips the data (the `int`'s size field is 0) where the pure-Python reader
/// finds no ref type equal to `0` and raises. A value read without the data
/// child, written through a tree with one, so writes no data from
/// UnityPyBoost's value, and is refused from the pure-Python reader's, as
/// each reader reads the bytes that write would give.
#[test]
fn mirror_reader_resolves_referenced_data_by_the_readers_rule() {
    let object = |data: bool| {
        let mut rows = vec![
            json!([0, "Base", "Base", 0]),
            json!([1, "ReferencedObject", "o", 0]),
            json!([2, "SInt64", "rid", 0]),
            json!([2, "ReferencedManagedType", "type", 0]),
            json!([3, "int", "class", 0]),
            json!([3, "string", "ns", 0]),
            json!([3, "string", "asm", 0]),
        ];
        if data {
            rows.push(json!([2, "ReferencedObjectData", "data", 0]));
        }
        compile(&Json::Array(rows))
    };
    let (read_tree, write_tree) = (object(false), object(true));
    let refs = Refs::of(Some(&json!([{
        "class": "C", "ns": "N", "asm": "A", "nodes": [[0, "Data", "Base", 0], [1, "int", "x", 0]]
    }])));
    let mut data = 7_i64.to_le_bytes().to_vec();
    for word in [0_u32, 1, u32::from(b'N'), 1, u32::from(b'A')] {
        data.extend(word.to_le_bytes());
    }
    let data = Bytes::from(data);
    let options = ReadOptions::default();
    let meter = Meter::unlimited();
    let ctx = ReadCtx {
        endian: unityrs_core::stream::Endian::Little,
        ref_types: refs.get(),
        options: &options,
        meter: &meter,
    };
    for reader in [Reader::Boost, Reader::Python] {
        let value = reader.read(&data, 0, &read_tree, &ctx).unwrap();
        let mirror = write_out(
            &value,
            &write_tree,
            ctx.endian,
            refs.get(),
            WriteRules::MirrorReader,
        );
        let again = reader.read(&data, 0, &write_tree, &ctx);
        match reader {
            Reader::Boost => {
                assert_eq!(mirror, json!({ "hex": hex(&data) }), "{reader:?}");
                assert_eq!(again.unwrap().bytes_read, 28, "{reader:?}");
            }
            Reader::Python => {
                assert_eq!(mirror, json!({ "err": true }), "{reader:?}");
                let refused = matches!(
                    &again,
                    Err(ReadError {
                        detail: ReadDetail::RefTypeNotFound,
                        ..
                    })
                );
                assert!(refused, "{reader:?}: {again:?}");
            }
        }
    }
}
