// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/ImportHelper.py. See THIRD_PARTY_LICENSES.
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

mod catalog;

pub use catalog::{
    FindAsk, FindScene, asset_bundle_modes, bad_cab, container_other_file, cross_bundle_pointer,
    dependency_reads, dependency_scenes, deref_rules, deref_sibling_nested, fallback_file_pointer,
    find_file_kinds, find_file_listing, find_scenes, last_block_fails, listings, listings_raced,
    loose_external_pointer, nested_after_cab, nested_between_loads, nested_dropped_evicted,
    nested_duplicates, nested_evicted, nested_raw_replaced, nested_reloaded, nested_reloaded_spans,
    nested_reloaded_spans_raced, nested_replaced_unread, preload_below_start,
    preload_loads_new_cab, preload_nested_external, preload_paths, preload_raw_target,
    preload_revisits_after_load, preload_revisits_ahead, preload_skips, race_scenes,
    resolver_loads, resource_in_nested_cab, resource_in_own_nested, scenes, web_file,
};

use crate::blob::NodeSpec;
use crate::bundle::{BundleSpec, EntrySpec};
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

/// A scene some of whose loads are dependencies (UnityPy's
/// `load_file(..., is_dependency=True)`, the core's
/// `Environment::load_dependency`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DepScene {
    /// The files, in load order.
    pub scene: Scene,
    /// The indexes of `scene.files` loaded as dependencies.
    pub dependencies: Vec<usize>,
}

/// A scene whose container preload walk races a load another thread makes:
/// `other` is loaded at one point of the walk, each in turn
/// (`tools/fixtures/env.py` records UnityPy's walk with the load made at
/// each point and, for a load of more than one key, with only its first key
/// made there). Exactly one file of the scene holds an AssetBundle, so the
/// walk is of one table; `other` registers only keys a file of the scene
/// registered first, so UnityPy's pass, which fails when its dict of CABs
/// grows under it, finishes wherever the load is made.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RaceScene {
    /// The files loaded before the walk, and the resolver's.
    pub scene: Scene,
    /// `(name, bytes)` of the load made while the walk runs.
    pub other: (Vec<u8>, Vec<u8>),
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
