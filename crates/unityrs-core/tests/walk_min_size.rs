//! A compiled node's `min_size` (plan 3.10, "Bounds") against the bytes each
//! reader really consumes, over generated trees of every shape the compile
//! makes: primitives, `string`, `TypelessData`, a `pair` of two children and
//! malformed ones of one and three, vectors of every element (nested ones
//! included) and malformed ones whose `Array` has one child or three,
//! classes, a class holding a registry, read with a registry in scope and
//! without, `ReferencedObject`s and `PPtr`s, with and without align flags.
//!
//!     scripts/dev cargo test -p unityrs-core --test walk_min_size
//!
//! `min_size` sizes a list's first reservation, so it must never pass what
//! a successful read of the node consumes, in either reader, whatever
//! registry is in scope; where nothing can be skipped or padded it is
//! exactly the least a read consumes, which an input of zeros (every length
//! 0) makes it read. Review round 4: a malformed `pair` or `Array` had 0,
//! though the pure-Python reader reads it. The zero-size bound no longer
//! reads `min_size` (it counts what each element read,
//! `tests/walk_elements.rs`), but an estimate too low would still reserve
//! for elements the bytes cannot hold.

use std::collections::BTreeSet;
use std::sync::Arc;

use unityrs_core::files::SerializedType;
use unityrs_core::limits::{Limits, Meter};
use unityrs_core::stream::Endian;
use unityrs_core::typetree::{NodeList, NodeRow, Shape, Tree};
use unityrs_core::walk::{self, ReadCtx, ReadError, ReadOptions, RefTypes};
use unityrs_core::{Bytes, Parsed, RawStr};
use unityrs_testkit::Rng;

const ALIGN: i32 = 0x4000;

/// Builds random trees, rows in pre-order.
struct Gen {
    rng: Rng,
    rows: Vec<(i32, String, String, i32)>,
    /// Align flags on some nodes.
    flags: bool,
    /// The trees' features that make `min_size` a strict bound: a string
    /// (always aligned by UnityPyBoost), a flag, a registry or a
    /// `ReferencedObject` (whose children a read may skip).
    loose: bool,
}

impl Gen {
    fn push(&mut self, level: i32, ty: &str, flag: bool) {
        let meta = if flag && self.flags && self.rng.below(3) == 0 {
            self.loose = true;
            ALIGN
        } else {
            0
        };
        let name = format!("f{}", self.rows.len());
        self.rows.push((level, ty.to_owned(), name, meta));
    }

    /// Renames the last node: a `ReferencedObject` reads its `type` and
    /// that one's `class`, `ns` and `asm` by name.
    fn name(&mut self, name: &str) {
        if let Some(r) = self.rows.last_mut() {
            name.clone_into(&mut r.2);
        }
    }

    fn leaf(&mut self, level: i32, ty: &str) {
        self.push(level, ty, true);
    }

    fn node(&mut self, level: i32, depth: u32) {
        const LEAVES: [&str; 9] = [
            "UInt8",
            "SInt16",
            "int",
            "SInt64",
            "float",
            "double",
            "bool",
            "string",
            "TypelessData",
        ];
        let pick = self.rng.below(if depth == 0 { 9 } else { 19 });
        if let Some(ty) = LEAVES.get(pick) {
            self.loose |= *ty == "string";
            return self.leaf(level, ty);
        }
        match pick {
            // A `pair` of two, one and three children.
            9..=11 => {
                self.push(level, "pair", true);
                for _ in 0..[2, 1, 3][pick - 9] {
                    self.node(level + 1, depth - 1);
                }
            }
            // A vector whose `Array` has two children, one and three.
            12..=14 => {
                self.push(level, "vector", true);
                self.push(level + 1, "Array", true);
                self.leaf(level + 2, "int");
                for _ in 0..[1, 0, 2][pick - 12] {
                    self.node(level + 2, depth - 1);
                }
            }
            // A class of up to three children.
            15 | 16 => {
                self.push(level, "Foo", true);
                for _ in 0..self.rng.below(4) {
                    self.node(level + 1, depth - 1);
                }
            }
            // A class whose registry is read, or skipped in scope.
            17 => {
                self.loose = true;
                self.push(level, "Foo", true);
                self.node(level + 1, depth - 1);
                self.push(level + 1, "ManagedReferencesRegistry", true);
                self.leaf(level + 2, "int");
                self.node(level + 1, depth - 1);
            }
            // `PPtr<..>` or a `ReferencedObject` whose data an empty class skips.
            _ => {
                if self.rng.below(2) == 0 {
                    self.push(level, "PPtr<Foo>", true);
                    self.leaf(level + 1, "int");
                    self.leaf(level + 1, "SInt64");
                } else {
                    self.loose = true;
                    self.push(level, "ReferencedObject", true);
                    self.leaf(level + 1, "SInt64");
                    self.push(level + 1, "ReferencedManagedType", true);
                    self.name("type");
                    for name in ["class", "ns", "asm"] {
                        self.leaf(level + 2, "string");
                        self.name(name);
                    }
                    self.push(level + 1, "ReferencedObjectData", true);
                    self.node(level + 2, depth - 1);
                }
            }
        }
    }
}

fn compile(rows: &[(i32, String, String, i32)]) -> Arc<Tree> {
    let rows = rows
        .iter()
        .map(|(l, t, n, m)| NodeRow::new(*l, t.as_str(), n.as_str()).with_meta_flag(*m))
        .collect();
    Tree::compile(
        Arc::new(NodeList::trusted(rows).unwrap()),
        &Limits::default(),
    )
    .unwrap()
}

fn shape_name(s: Shape) -> &'static str {
    match s {
        Shape::Prim => "prim",
        Shape::Str => "string",
        Shape::Bytes => "bytes",
        Shape::Pair => "pair",
        Shape::BadPair => "bad pair",
        Shape::RefObject => "ReferencedObject",
        Shape::Vector { run: true, .. } => "run",
        Shape::Vector { run: false, .. } => "vector",
        Shape::BadVector => "bad vector",
        Shape::Class => "class",
    }
}

/// Both readers' reads of the whole of `data` from 0.
fn reads(
    data: &Bytes,
    tree: &Arc<Tree>,
    refs: &[SerializedType],
) -> [(&'static str, Result<Parsed, ReadError>); 2] {
    let options = ReadOptions::default();
    let meter = Meter::unlimited();
    let ctx = ReadCtx {
        endian: Endian::Little,
        ref_types: RefTypes::List(refs),
        options: &options,
        meter: &meter,
    };
    [
        ("boost", walk::boost::read(data, tree, &ctx)),
        ("python", walk::python::read(data, 0, tree, &ctx)),
    ]
}

/// Little-endian words of 0 or 1: lengths and counts of 0 or 1, so most
/// reads succeed, none goes back, and none reads past the end.
fn small_words(rng: &mut Rng) -> Bytes {
    let mut out = Vec::new();
    for _ in 0..512 {
        out.extend_from_slice(&u32::try_from(rng.below(2)).unwrap().to_le_bytes());
    }
    Bytes::from(out)
}

#[test]
fn min_size_never_passes_what_a_read_consumes_and_is_the_least_one_does() {
    let mut rng = Rng::new(0x6d69_6e5f_7369_7a65);
    // A ref type no class names, so a non-empty list where an empty class
    // skips `data` in both readers.
    let refs = [SerializedType::ref_type(
        114,
        RawStr::from("Z"),
        RawStr::from("Z"),
        RawStr::from("Z"),
        None,
    )];
    let zeros = Bytes::from(vec![0_u8; 2048]);
    let mut bounded = BTreeSet::new();
    let mut exact = BTreeSet::new();
    for i in 0..4000 {
        let mut g = Gen {
            rng: Rng::new(rng.next_u64()),
            rows: Vec::new(),
            flags: i % 2 == 0,
            loose: false,
        };
        let scope = i % 3 == 0;
        if scope {
            // `Base { ManagedReferencesRegistry reg { int }; <node> }`: the
            // node is read with a registry in scope, 4 bytes in.
            g.push(0, "Base", false);
            g.push(1, "ManagedReferencesRegistry", false);
            g.push(2, "int", false);
            g.node(1, 3);
        } else {
            g.node(0, 3);
        }
        let loose = g.loose;
        let tree = compile(&g.rows);
        let node = if scope {
            tree.list().children(tree.root())[1]
        } else {
            tree.root()
        };
        let info = *tree.info(node).unwrap();
        let skip = if scope { 4 } else { 0 };
        let small = small_words(&mut g.rng);
        for (data, zero) in [(&zeros, true), (&small, false)] {
            for (reader, got) in reads(data, &tree, &refs) {
                let Ok(p) = got else { continue };
                let consumed = p.bytes_read - skip;
                let shape = shape_name(info.shape);
                assert!(
                    consumed >= i64::from(info.min_size),
                    "{reader} read {consumed} bytes of a node whose min_size is {} ({shape}): {:?}",
                    info.min_size,
                    g.rows
                );
                bounded.insert((reader, shape));
                if zero && !loose {
                    assert_eq!(
                        consumed,
                        i64::from(info.min_size),
                        "{reader} read zeros through a node ({shape}) whose min_size is not \
                         the least a read consumes: {:?}",
                        g.rows
                    );
                    exact.insert((reader, shape));
                }
            }
        }
    }
    // Every shape was read, by each reader that reads it.
    for reader in ["boost", "python"] {
        for shape in [
            "prim",
            "string",
            "bytes",
            "pair",
            "ReferencedObject",
            "run",
            "vector",
            "class",
        ] {
            assert!(
                bounded.contains(&(reader, shape)),
                "{reader} never read a {shape}"
            );
        }
        for shape in ["prim", "bytes", "pair", "run", "vector", "class"] {
            assert!(
                exact.contains(&(reader, shape)),
                "{reader} never read zeros through a {shape}"
            );
        }
    }
    // UnityPyBoost refuses a malformed `pair` or `Array`; the pure-Python reader reads them.
    for shape in ["bad pair", "bad vector"] {
        assert!(
            exact.contains(&("python", shape)),
            "python never read a {shape}"
        );
        assert!(!bounded.contains(&("boost", shape)), "boost read a {shape}");
    }
}
