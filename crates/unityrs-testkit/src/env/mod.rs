//! Multi-file scenes for the `Environment` tests: bundles whose objects
//! point into each other, resources in sibling entries and in other bundles.
//!
//! A scene is the files a caller loads, in order, and the files a resolver
//! can return (UnityPy's `env.path`). Every object is one of three shapes,
//! each with its own type record so a file may mix them freely:
//!
//! - a named object ([`Obj::named`]): `m_Name` and `m_Ptr0`, `m_Ptr1`, ...
//!   (`PPtr<Object>`), as class 1;
//! - a streamed texture ([`Obj::streamed`]): `m_Name` and `m_StreamData`
//!   (`offset`, `size`, `path`), as class 28;
//! - an AssetBundle ([`Obj::asset_bundle`]): `m_Name`, `m_PreloadTable`,
//!   `m_Container` and, unless left out, `m_MainAsset`, as class 142.
//!
//! `tools/fixtures/env.py` asks cargo for [`scenes`] and records what
//! UnityPy 1.25.2 does with each (`crates/unityrs-core/tests/fixtures/d/`).
//!
//! Owner: package D.

use crate::blob::NodeSpec;
use crate::bundle::{BlockOverride, BundleSpec, EntrySpec, TestCodec};
use crate::serialized::{ExternalSpec, ObjectSpec, SerializedSpec, TypeSpec};
use crate::writer::ByteWriter;

/// Named files, as a caller would load them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Scene {
    /// `(name, bytes)` in load order.
    pub files: Vec<(Vec<u8>, Vec<u8>)>,
    /// `(name, bytes)` a resolver may return (UnityPy's `env.path`): a
    /// `find_file` or a resource fallback asking for exactly `name` gets the
    /// file (UnityPy's case-insensitive search never matches a name without
    /// a directory, `helpers/ImportHelper.py:159-173`).
    pub resolver: Vec<(Vec<u8>, Vec<u8>)>,
}

/// One container row: the path, its preload span and the asset pointer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// The container path.
    pub path: Vec<u8>,
    /// `preloadIndex`.
    pub preload_index: i32,
    /// `preloadSize`.
    pub preload_size: i32,
    /// `asset` as `(m_FileID, m_PathID)`.
    pub asset: (i32, i64),
}

/// The AssetBundle's fields.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Bundle {
    /// `m_PreloadTable`.
    pub preload: Vec<(i32, i64)>,
    /// `m_Container`.
    pub rows: Vec<Row>,
    /// Leaves `m_MainAsset` out (UnityPy then builds an `UnknownObject`).
    pub no_main_asset: bool,
    /// Leaves `m_Container` out (object mode then has no such attribute).
    pub no_container: bool,
    /// Adds a second field named `m.Container` after `m_Container` (the two
    /// clean to one key; object mode keeps the last).
    pub shadow_container: Option<Vec<Row>>,
    /// Writes `m_Container` and `m_PreloadTable` as a damaged tree can hold
    /// them: each pair's key an `int` (the row's position) instead of a
    /// `string`, `preloadIndex` a `float`, `preloadSize` named `preloadSizX`,
    /// the asset's `m_FileID` a `float`, and each preload pointer with a
    /// third child, `m_Extra` (an `int`, written as 0). UnityPy still loads
    /// the file: its load reads only each asset's `path_id`.
    pub hostile: bool,
}

/// What an object holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Body {
    /// `m_Name` and pointers `m_Ptr0`, `m_Ptr1`, ...
    Named(Vec<(i32, i64)>),
    /// `m_Name` and `m_StreamData` (`offset`, `size`, `path`).
    Streamed {
        /// `m_StreamData.path`.
        path: Vec<u8>,
        /// `m_StreamData.offset`.
        offset: u64,
        /// `m_StreamData.size`.
        size: u32,
    },
    /// An AssetBundle.
    AssetBundle(Bundle),
}

/// One object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Obj {
    /// Its path id.
    pub path_id: i64,
    /// `m_Name`.
    pub name: Vec<u8>,
    /// Its fields.
    pub body: Body,
}

impl Obj {
    /// A named object (class 1) with pointers.
    #[must_use]
    pub fn named(path_id: i64, name: &str, ptrs: &[(i32, i64)]) -> Self {
        Self {
            path_id,
            name: name.as_bytes().to_vec(),
            body: Body::Named(ptrs.to_vec()),
        }
    }

    /// A streamed texture (class 28) reading `size` bytes at `offset` of
    /// `path`.
    #[must_use]
    pub fn streamed(path_id: i64, name: &str, path: &str, offset: u64, size: u32) -> Self {
        Self {
            path_id,
            name: name.as_bytes().to_vec(),
            body: Body::Streamed {
                path: path.as_bytes().to_vec(),
                offset,
                size,
            },
        }
    }

    /// An AssetBundle (class 142).
    #[must_use]
    pub fn asset_bundle(path_id: i64, bundle: Bundle) -> Self {
        Self {
            path_id,
            name: b"bundle".to_vec(),
            body: Body::AssetBundle(bundle),
        }
    }
}

/// A row pointing at `asset`, with its preload span.
#[must_use]
pub fn row(path: &str, preload: (i32, i32), asset: (i32, i64)) -> Row {
    Row {
        path: path.as_bytes().to_vec(),
        preload_index: preload.0,
        preload_size: preload.1,
        asset,
    }
}

fn string_nodes(level: u8, name: &str) -> Vec<NodeSpec> {
    vec![
        NodeSpec::new(level, "string", name),
        NodeSpec::new(level + 1, "Array", "Array")
            .size(-1)
            .aligned(),
        NodeSpec::new(level + 2, "int", "size").size(4),
        NodeSpec::new(level + 2, "char", "data").size(1),
    ]
}

fn pptr_nodes(level: u8, name: &str) -> Vec<NodeSpec> {
    vec![
        NodeSpec::new(level, "PPtr<Object>", name).size(12),
        NodeSpec::new(level + 1, "int", "m_FileID").size(4),
        NodeSpec::new(level + 1, "SInt64", "m_PathID").size(8),
    ]
}

fn asset_info_nodes(level: u8, name: &str) -> Vec<NodeSpec> {
    let mut n = vec![
        NodeSpec::new(level, "AssetInfo", name),
        NodeSpec::new(level + 1, "int", "preloadIndex").size(4),
        NodeSpec::new(level + 1, "int", "preloadSize").size(4),
    ];
    n.extend(pptr_nodes(level + 1, "asset"));
    n
}

/// [`Bundle::hostile`]'s `AssetInfo`.
fn hostile_asset_info_nodes(level: u8, name: &str) -> Vec<NodeSpec> {
    vec![
        NodeSpec::new(level, "AssetInfo", name),
        NodeSpec::new(level + 1, "float", "preloadIndex").size(4),
        NodeSpec::new(level + 1, "int", "preloadSizX").size(4),
        NodeSpec::new(level + 1, "PPtr<Object>", "asset").size(12),
        NodeSpec::new(level + 2, "float", "m_FileID").size(4),
        NodeSpec::new(level + 2, "SInt64", "m_PathID").size(8),
    ]
}

fn container_nodes(name: &str, hostile: bool) -> Vec<NodeSpec> {
    let mut n = vec![
        NodeSpec::new(1, "map", name),
        NodeSpec::new(2, "Array", "Array").size(-1),
        NodeSpec::new(3, "int", "size").size(4),
        NodeSpec::new(3, "pair", "data"),
    ];
    if hostile {
        n.push(NodeSpec::new(4, "int", "first").size(4));
        n.extend(hostile_asset_info_nodes(4, "second"));
    } else {
        n.extend(string_nodes(4, "first"));
        n.extend(asset_info_nodes(4, "second"));
    }
    n
}

fn put_string(w: &mut ByteWriter, s: &[u8]) {
    w.i32(i32::try_from(s.len()).unwrap_or(i32::MAX))
        .bytes(s)
        .align(4);
}

fn put_pptr(w: &mut ByteWriter, p: (i32, i64)) {
    w.i32(p.0).i64(p.1);
}

fn put_rows(w: &mut ByteWriter, rows: &[Row], hostile: bool) {
    w.i32(i32::try_from(rows.len()).unwrap_or(i32::MAX));
    for (i, r) in rows.iter().enumerate() {
        if hostile {
            // Small values: each is exact as an `f32`.
            #[allow(clippy::cast_precision_loss)]
            let (index, file_id) = (r.preload_index as f32, r.asset.0 as f32);
            w.i32(i32::try_from(i).unwrap_or(i32::MAX))
                .f32(index)
                .i32(r.preload_size)
                .f32(file_id)
                .i64(r.asset.1);
        } else {
            put_string(w, &r.path);
            w.i32(r.preload_index).i32(r.preload_size);
            put_pptr(w, r.asset);
        }
    }
}

/// The class id, tree and little-endian bytes of `o`.
fn shape(o: &Obj) -> (i32, Vec<NodeSpec>, Vec<u8>) {
    let mut w = ByteWriter::le();
    match &o.body {
        Body::Named(ptrs) => {
            let mut nodes = vec![NodeSpec::new(0, "GameObject", "Base")];
            nodes.extend(string_nodes(1, "m_Name"));
            put_string(&mut w, &o.name);
            for (i, p) in ptrs.iter().enumerate() {
                nodes.extend(pptr_nodes(1, &format!("m_Ptr{i}")));
                put_pptr(&mut w, *p);
            }
            (1, nodes, w.into_inner())
        }
        Body::Streamed { path, offset, size } => {
            let mut nodes = vec![NodeSpec::new(0, "Texture2D", "Base")];
            nodes.extend(string_nodes(1, "m_Name"));
            nodes.push(NodeSpec::new(1, "StreamingInfo", "m_StreamData"));
            nodes.push(NodeSpec::new(2, "UInt64", "offset").size(8));
            nodes.push(NodeSpec::new(2, "unsigned int", "size").size(4));
            nodes.extend(string_nodes(2, "path"));
            put_string(&mut w, &o.name);
            w.u64(*offset).u32(*size);
            put_string(&mut w, path);
            (28, nodes, w.into_inner())
        }
        Body::AssetBundle(b) => {
            let mut nodes = vec![NodeSpec::new(0, "AssetBundle", "Base")];
            nodes.extend(string_nodes(1, "m_Name"));
            put_string(&mut w, &o.name);
            nodes.push(NodeSpec::new(1, "vector", "m_PreloadTable"));
            nodes.push(NodeSpec::new(2, "Array", "Array").size(-1));
            nodes.push(NodeSpec::new(3, "int", "size").size(4));
            nodes.extend(pptr_nodes(3, "data"));
            if b.hostile {
                nodes.push(NodeSpec::new(4, "int", "m_Extra").size(4));
            }
            w.i32(i32::try_from(b.preload.len()).unwrap_or(i32::MAX));
            for p in &b.preload {
                put_pptr(&mut w, *p);
                if b.hostile {
                    w.i32(0);
                }
            }
            if !b.no_container {
                nodes.extend(container_nodes("m_Container", b.hostile));
                put_rows(&mut w, &b.rows, b.hostile);
            }
            if let Some(shadow) = &b.shadow_container {
                nodes.extend(container_nodes("m.Container", b.hostile));
                put_rows(&mut w, shadow, b.hostile);
            }
            if !b.no_main_asset {
                nodes.extend(asset_info_nodes(1, "m_MainAsset"));
                w.i32(0).i32(0);
                put_pptr(&mut w, (0, 0));
            }
            (142, nodes, w.into_inner())
        }
    }
}

/// A `SerializedFile` (format 22, little-endian, type trees on) holding
/// `objects`, each with its own type record, and the given external paths.
#[must_use]
pub fn serialized(externals: &[&str], objects: &[Obj]) -> Vec<u8> {
    let mut types = Vec::new();
    let mut objs = Vec::new();
    for (i, o) in objects.iter().enumerate() {
        let (class_id, nodes, data) = shape(o);
        types.push(TypeSpec {
            class_id,
            nodes,
            ..TypeSpec::default()
        });
        objs.push(ObjectSpec {
            path_id: o.path_id,
            type_id: i32::try_from(i).unwrap_or(i32::MAX),
            class_id: u16::try_from(class_id).unwrap_or(0),
            data,
            ..ObjectSpec::default()
        });
    }
    crate::serialized::build(&SerializedSpec {
        types,
        objects: objs,
        externals: externals
            .iter()
            .map(|p| ExternalSpec {
                path: p.as_bytes().to_vec(),
                ..ExternalSpec::default()
            })
            .collect(),
        ..SerializedSpec::default()
    })
}

/// A UnityFS bundle (LZ4 blocks of 160 bytes) of `entries`.
#[must_use]
pub fn bundle(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
    crate::bundle::build(&bundle_spec(entries))
}

fn bundle_spec(entries: &[(&str, Vec<u8>)]) -> BundleSpec {
    BundleSpec {
        block_size: 160,
        entries: entries
            .iter()
            .map(|(name, data)| EntrySpec {
                name: name.as_bytes().to_vec(),
                data: data.clone(),
                flags: 4,
            })
            .collect(),
        ..BundleSpec::default()
    }
}

/// Raw bytes for a `.resS` entry: `n` bytes counting from `seed`.
#[must_use]
pub fn res_bytes(n: usize, seed: u8) -> Vec<u8> {
    (0..n)
        .map(|i| seed.wrapping_add(u8::try_from(i % 256).unwrap_or(0)))
        .collect()
}

fn named(name: &str, data: Vec<u8>) -> (Vec<u8>, Vec<u8>) {
    (name.as_bytes().to_vec(), data)
}

/// Two bundles: an object in the first points (`m_FileID` 1) at an object in
/// the second's CAB; the first also holds a `.resS` the object reads.
#[must_use]
pub fn cross_bundle_pointer() -> Scene {
    let cab_a = serialized(
        &["archive:/CAB-B/CAB-B"],
        &[
            Obj::named(1, "a1", &[(1, 7), (0, 2)]),
            Obj::streamed(2, "tex", "archive:/CAB-A/CAB-A.resS", 4, 8),
            Obj::asset_bundle(
                3,
                Bundle {
                    preload: vec![(1, 7), (0, 2)],
                    rows: vec![row("assets/a1.prefab", (0, 2), (0, 1))],
                    ..Bundle::default()
                },
            ),
        ],
    );
    let cab_b = serialized(&[], &[Obj::named(7, "b7", &[])]);
    Scene {
        files: vec![
            named(
                "a.bundle",
                bundle(&[("CAB-A", cab_a), ("CAB-A.resS", res_bytes(32, 0))]),
            ),
            named("b.bundle", bundle(&[("CAB-B", cab_b)])),
        ],
        resolver: Vec::new(),
    }
}

/// Two loose `SerializedFile`s loaded at top level under names with
/// directories (`a/CAB-A`, `b/CAB-B`): an object in A points (`m_FileID` 1,
/// external `archive:/CAB-B/CAB-B`) at an object in B. The pointer resolves,
/// as in UnityPy, whose Environment is a top-level file's container
/// (`find_file` finds `cab-b`); without B it is `DerefError::NotFound`.
#[must_use]
pub fn loose_external_pointer() -> Scene {
    Scene {
        files: vec![
            named(
                "a/CAB-A",
                serialized(&["archive:/CAB-B/CAB-B"], &[Obj::named(1, "a1", &[(1, 5)])]),
            ),
            named("b/CAB-B", serialized(&[], &[Obj::named(5, "b5", &[])])),
        ],
        resolver: Vec::new(),
    }
}

/// One bundle whose first CAB holds a pointer for every dereference rule:
/// null, a missing path id, `m_FileID` past the end, a negative id in range
/// and one below it, `i32::MIN`, an external nobody has, a raw sibling, a
/// sibling matched by its lower-cased name, and one found by `find_file`
/// under a key with a directory.
#[must_use]
pub fn deref_rules() -> Scene {
    let externals = [
        "archive:/CAB-RAW/CAB-RAW.resS",
        "archive:/CAB-MISSING/CAB-MISSING",
        "archive:/CAB-E2/Cab-E2",
        "archive:/elsewhere/CAB-D",
    ];
    let cab_e = serialized(
        &externals,
        &[Obj::named(
            1,
            "e1",
            &[
                (0, 0),
                (0, 99),
                (5, 1),
                (-1, 11),
                (-5, 1),
                (i32::MIN, 1),
                (2, 1),
                (1, 1),
                (3, 9),
                (3, 10),
                (4, 11),
                (0, 1),
            ],
        )],
    );
    Scene {
        files: vec![named(
            "e.bundle",
            bundle(&[
                ("CAB-E", cab_e),
                ("CAB-RAW.resS", res_bytes(16, 3)),
                ("CAB-E2", serialized(&[], &[Obj::named(9, "e9", &[])])),
                ("dir/CAB-D", serialized(&[], &[Obj::named(11, "d11", &[])])),
            ]),
        )],
        resolver: Vec::new(),
    }
}

/// A bundle of a good CAB, its `.resS` and a CAB that fails to parse: the
/// load fails, `files` stays empty, and the first two are registered.
#[must_use]
pub fn bad_cab() -> Scene {
    let good = serialized(
        &[],
        &[
            Obj::named(1, "g1", &[]),
            Obj::streamed(2, "tex", "archive:/CAB-G/CAB-G.resS", 0, 4),
        ],
    );
    // An AssetBundle without `m_Container`: UnityPy's object-mode read at
    // parse (`files/SerializedFile.py:316-321`) fails the file.
    let bad = serialized(
        &[],
        &[Obj::asset_bundle(
            1,
            Bundle {
                no_container: true,
                ..Bundle::default()
            },
        )],
    );
    Scene {
        files: vec![named(
            "g.bundle",
            bundle(&[
                ("CAB-G", good),
                ("CAB-G.resS", res_bytes(16, 7)),
                ("CAB-BAD", bad),
            ]),
        )],
        resolver: Vec::new(),
    }
}

/// A bundle whose second entry is a bundle under the path of the first,
/// a CAB (plan 3.13 (b)).
#[must_use]
pub fn nested_after_cab() -> Scene {
    let inner = bundle(&[("CAB-INNER", serialized(&[], &[Obj::named(2, "inner", &[])]))]);
    Scene {
        files: vec![named(
            "n.bundle",
            bundle(&[
                ("CAB-N", serialized(&[], &[Obj::named(1, "n1", &[])])),
                ("CAB-N", inner),
            ]),
        )],
        resolver: Vec::new(),
    }
}

/// A bundle with a block after its last entry's bytes that fails to decode
/// (plan 3.13 (c)).
#[must_use]
pub fn last_block_fails() -> Scene {
    let mut spec = bundle_spec(&[("CAB-L", serialized(&[], &[Obj::named(1, "l1", &[(0, 1)])]))]);
    let blocks = spec
        .entries
        .iter()
        .map(|e| e.data.len())
        .sum::<usize>()
        .div_ceil(spec.block_size);
    spec.block_overrides.push(BlockOverride {
        block: blocks,
        // An id no codec has: UnityPy's `CompressionFlags(9)` raises.
        codec: Some(TestCodec::Raw(9)),
        uncompressed: Some(16),
        compressed: None,
    });
    Scene {
        files: vec![named("l.bundle", crate::bundle::build(&spec))],
        resolver: Vec::new(),
    }
}

/// A container whose preload table points every way the preload skips:
/// past the externals, at a file nobody has, at a missing object, null,
/// and rows whose spans fall outside the table.
#[must_use]
pub fn preload_skips() -> Scene {
    let skipped = serialized(
        &["archive:/CAB-NONE/CAB-NONE"],
        &[
            Obj::named(1, "p1", &[]),
            Obj::asset_bundle(
                2,
                Bundle {
                    preload: vec![(0, 1), (9, 1), (1, 1), (0, 77), (0, 0)],
                    rows: vec![
                        row("assets/p1.prefab", (0, 5), (0, 1)),
                        row("assets/out.prefab", (3, 9), (0, 1)),
                        row("assets/neg.prefab", (-1, 2), (0, 1)),
                    ],
                    ..Bundle::default()
                },
            ),
        ],
    );
    Scene {
        files: vec![named("p.bundle", bundle(&[("CAB-P", skipped)]))],
        resolver: Vec::new(),
    }
}

/// A preload pointer below the externals: `IndexError`, raised.
#[must_use]
pub fn preload_below_start() -> Scene {
    let file = serialized(
        &["archive:/CAB-NONE/CAB-NONE"],
        &[
            Obj::named(1, "q1", &[]),
            Obj::asset_bundle(
                2,
                Bundle {
                    preload: vec![(-3, 1)],
                    rows: vec![row("assets/q1.prefab", (0, 1), (0, 1))],
                    ..Bundle::default()
                },
            ),
        ],
    );
    Scene {
        files: vec![named("q.bundle", bundle(&[("CAB-Q", file)]))],
        resolver: Vec::new(),
    }
}

/// A preload pointer at a raw sibling: `AttributeError`, raised.
#[must_use]
pub fn preload_raw_target() -> Scene {
    let file = serialized(
        &["archive:/CAB-R/CAB-R.resS"],
        &[
            Obj::named(1, "r1", &[]),
            Obj::asset_bundle(
                2,
                Bundle {
                    preload: vec![(1, 1)],
                    rows: vec![row("assets/r1.prefab", (0, 1), (0, 1))],
                    ..Bundle::default()
                },
            ),
        ],
    );
    Scene {
        files: vec![named(
            "r.bundle",
            bundle(&[("CAB-R", file), ("CAB-R.resS", res_bytes(8, 1))]),
        )],
        resolver: Vec::new(),
    }
}

/// `AssetBundle`s read in object mode: two fields whose names clean to one
/// key (the last wins), one without `m_MainAsset` (`UnknownObject`), and
/// one without `m_Container` (the file fails).
#[must_use]
pub fn asset_bundle_modes() -> Scene {
    let shadow = serialized(
        &[],
        &[
            Obj::named(1, "s1", &[]),
            Obj::asset_bundle(
                2,
                Bundle {
                    rows: vec![row("assets/first.prefab", (0, 0), (0, 1))],
                    shadow_container: Some(vec![
                        row("assets/second.prefab", (0, 0), (0, 1)),
                        row("assets/third.prefab", (0, 0), (0, 2)),
                    ]),
                    ..Bundle::default()
                },
            ),
        ],
    );
    let unknown = serialized(
        &[],
        &[
            Obj::named(1, "u1", &[]),
            Obj::asset_bundle(
                2,
                Bundle {
                    rows: vec![row("assets/u1.prefab", (0, 0), (0, 1))],
                    no_main_asset: true,
                    ..Bundle::default()
                },
            ),
        ],
    );
    let missing = serialized(
        &[],
        &[
            Obj::named(1, "m1", &[]),
            Obj::asset_bundle(
                2,
                Bundle {
                    no_container: true,
                    ..Bundle::default()
                },
            ),
        ],
    );
    Scene {
        files: vec![
            named("shadow.bundle", bundle(&[("CAB-S", shadow)])),
            named("unknown.bundle", bundle(&[("CAB-U", unknown)])),
            named("missing.bundle", bundle(&[("CAB-M", missing)])),
        ],
        resolver: Vec::new(),
    }
}

/// A loaded bundle whose CAB points at, and reads a resource from, files
/// only the resolver has: `find_file` loads `cab-dep`'s bundle as a
/// dependency (a container, so the pointer has no target), whose own copy of
/// `CAB-H` then replaces the loaded one's key, and `cab-loose`, a
/// `SerializedFile` it resolves into; the resource fallback loads
/// `CAB-H2.resS` under the name it asked for, twice (the first and last
/// candidates are one name).
#[must_use]
pub fn resolver_loads() -> Scene {
    let cab_h = |name: &str| {
        serialized(
            &["archive:/CAB-DEP/CAB-DEP", "archive:/CAB-LOOSE/CAB-LOOSE"],
            &[
                Obj::named(1, name, &[(1, 3), (2, 5)]),
                Obj::streamed(2, "tex", "archive:/CAB-H2/CAB-H2.resS", 2, 6),
                Obj::asset_bundle(
                    4,
                    Bundle {
                        rows: vec![row(&format!("assets/{name}.prefab"), (0, 0), (0, 1))],
                        ..Bundle::default()
                    },
                ),
            ],
        )
    };
    let dep = bundle(&[
        ("CAB-DEP", serialized(&[], &[Obj::named(3, "dep3", &[])])),
        ("CAB-H", cab_h("h-copy")),
    ]);
    Scene {
        files: vec![named("h.bundle", bundle(&[("CAB-H", cab_h("h1"))]))],
        resolver: vec![
            // Lower-case: `find_file` asks for the cleaned (lower-cased)
            // external name, and UnityPy finds only an exact file name
            // (its walk keys a lower-cased path its open then misses).
            named("cab-dep", dep),
            named(
                "cab-loose",
                serialized(&[], &[Obj::named(5, "loose5", &[])]),
            ),
            named("CAB-H2.resS", res_bytes(24, 9)),
        ],
    }
}

/// A loose file whose preload pointer names a CAB only the resolver has:
/// the container pass's `find_file` registers it, and UnityPy's loop over
/// `env.cabs` raises `RuntimeError` (the dict changed size); the next pass
/// ends.
#[must_use]
pub fn preload_loads_new_cab() -> Scene {
    let file = serialized(
        &["archive:/CAB-NEW/CAB-NEW"],
        &[
            Obj::named(1, "x1", &[]),
            Obj::asset_bundle(
                2,
                Bundle {
                    preload: vec![(1, 3)],
                    rows: vec![row("assets/x1.prefab", (0, 1), (0, 1))],
                    ..Bundle::default()
                },
            ),
        ],
    );
    Scene {
        files: vec![named("x/CAB-X", file)],
        resolver: vec![named(
            "cab-new",
            serialized(&[], &[Obj::named(3, "new3", &[])]),
        )],
    }
}

/// Container paths (`obj.container`): rows whose spans overlap (the first
/// row to reach an object gives its path), a row's own asset (which beats
/// any span), two rows of one asset (the last wins), and a span reaching an
/// object in a sibling CAB.
#[must_use]
pub fn preload_paths() -> Scene {
    let pp = serialized(
        &["archive:/CAB-PQ/CAB-PQ"],
        &[
            Obj::named(1, "p1", &[]),
            Obj::named(2, "p2", &[]),
            Obj::named(3, "p3", &[]),
            Obj::named(4, "p4", &[]),
            Obj::named(5, "p5", &[]),
            Obj::asset_bundle(
                9,
                Bundle {
                    preload: vec![(0, 1), (0, 2), (0, 3), (1, 7), (0, 4)],
                    rows: vec![
                        row("assets/a.prefab", (0, 3), (0, 1)),
                        row("assets/b.prefab", (1, 4), (0, 2)),
                        row("assets/c.prefab", (0, 0), (0, 5)),
                        row("assets/d.prefab", (0, 0), (0, 5)),
                    ],
                    ..Bundle::default()
                },
            ),
        ],
    );
    let pq = serialized(&[], &[Obj::named(7, "q7", &[])]);
    Scene {
        files: vec![named(
            "pp.bundle",
            bundle(&[("CAB-PP", pp), ("CAB-PQ", pq)]),
        )],
        resolver: Vec::new(),
    }
}

/// A container row whose asset is in another file (`m_FileID` 1): UnityPy
/// keys its path by path id alone, so the object of that path id in the
/// container's own file gets it; the core keys only a row whose asset is in
/// the container's own file (`PARITY.md` section 19.2), so neither file's
/// object does.
#[must_use]
pub fn container_other_file() -> Scene {
    let of = serialized(
        &["archive:/CAB-OG/CAB-OG"],
        &[
            Obj::named(1, "of1", &[]),
            Obj::asset_bundle(
                9,
                Bundle {
                    rows: vec![row("assets/other.prefab", (0, 0), (1, 1))],
                    ..Bundle::default()
                },
            ),
        ],
    );
    let og = serialized(&[], &[Obj::named(1, "og1", &[])]);
    Scene {
        files: vec![named(
            "of.bundle",
            bundle(&[("CAB-OF", of), ("CAB-OG", og)]),
        )],
        resolver: Vec::new(),
    }
}

/// A resource read's fallback loads, through the resolver, a file whose
/// object points (`m_FileID` 1) at a CAB of the loaded bundle. UnityPy's
/// `load_file` parses the fallback's file with the Environment as its
/// container (`environment.py:107-108`, `:154`), so the pointer resolves as
/// a top-level file's does.
#[must_use]
pub fn fallback_file_pointer() -> Scene {
    let f = serialized(
        &["LOOSE-T"],
        &[Obj::streamed(1, "tex", "archive:/CAB-R/CAB-R.resS", 0, 4)],
    );
    let t = serialized(&[], &[Obj::named(5, "t5", &[])]);
    Scene {
        files: vec![named("f.bundle", bundle(&[("CAB-F", f), ("CAB-T", t)]))],
        resolver: vec![named(
            "LOOSE-T",
            serialized(&["archive:/CAB-T/CAB-T"], &[Obj::named(4, "l4", &[(1, 5)])]),
        )],
    }
}

/// A bundle of two nested bundles that each hold a CAB of one name: an
/// eager load registers both in directory order, so the second's is the
/// key's value; under `LoadMode::Lazy` each is registered when its nested
/// bundle is first asked for, in either order with the same result.
#[must_use]
pub fn nested_duplicates() -> Scene {
    let inner = |name: &str| bundle(&[("CAB-DUP", serialized(&[], &[Obj::named(1, name, &[])]))]);
    Scene {
        files: vec![named(
            "dup.bundle",
            bundle(&[("N1", inner("from-n1")), ("N2", inner("from-n2"))]),
        )],
        resolver: Vec::new(),
    }
}

/// Every scene, by name: what `tools/fixtures/env.py` records.
#[must_use]
pub fn scenes() -> Vec<(&'static str, Scene)> {
    vec![
        ("cross_bundle_pointer", cross_bundle_pointer()),
        ("loose_external_pointer", loose_external_pointer()),
        ("deref_rules", deref_rules()),
        ("bad_cab", bad_cab()),
        ("nested_after_cab", nested_after_cab()),
        ("last_block_fails", last_block_fails()),
        ("preload_skips", preload_skips()),
        ("preload_below_start", preload_below_start()),
        ("preload_raw_target", preload_raw_target()),
        ("asset_bundle_modes", asset_bundle_modes()),
        ("resolver_loads", resolver_loads()),
        ("preload_loads_new_cab", preload_loads_new_cab()),
        ("preload_paths", preload_paths()),
        ("container_other_file", container_other_file()),
        ("fallback_file_pointer", fallback_file_pointer()),
        ("nested_duplicates", nested_duplicates()),
    ]
}

#[cfg(test)]
mod tests {
    use super::scenes;

    #[test]
    fn every_scene_has_files_with_distinct_names() {
        for (name, scene) in scenes() {
            assert!(!scene.files.is_empty(), "{name}");
            let mut names: Vec<_> = scene.files.iter().map(|f| &f.0).collect();
            names.sort();
            names.dedup();
            assert_eq!(names.len(), scene.files.len(), "{name}");
        }
    }
}
