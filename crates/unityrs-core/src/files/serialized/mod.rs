// Ported from UnityPy 1.25.2 (MIT): UnityPy/files/SerializedFile.py, UnityPy/files/ObjectReader.py. See THIRD_PARTY_LICENSES.
//! `SerializedFile` metadata (`files/SerializedFile.py:206-360`,
//! `files/ObjectReader.py:67-130`), ported from UnityPy, bytes-native.
//!
//! Rules ported exactly (plan section 3.7):
//! - the header (`SerializedFile.py:255-269`): the endian byte at offset 16
//!   from format 9, else at `file_size - metadata_size` (absolute); the
//!   format-22 fields replace the legacy `u32`s;
//! - gates (`:271-288`): the version string from format 7, the target platform
//!   from 8, `enable_type_tree` from 13 (else true), `big_id_enabled` at 7-13;
//! - type records (`:117-148`): `is_stripped_type` from 16, `script_type_index`
//!   from 17 (else -1), `script_id` and `old_type_hash` from 13 under
//!   UnityPy's condition, trees only when `enable_type_tree`, blobs at format
//!   10 and 12 and up; at 21 ref types read three names and plain types a
//!   dependency list;
//! - objects (`ObjectReader.py:73-130`): path id as `i64` under bigID, `i32`
//!   below 14, else aligned `i64`; `byte_start` `u32` (or `i64` from 22) plus
//!   `data_offset`; below 16 the type is the first whose `class_id` equals the
//!   object's `type_id`, from 16 `types[type_id]` with Python indexing (a
//!   negative id counts from the end; out of range fails the file); at 11-16 the
//!   object's script index is written back into its type, the last object
//!   winning; the `objects` dict keeps a repeated path id at its first position
//!   with its last entry;
//! - `read_string_to_null`'s 32,767-byte window for every metadata string;
//! - every table is built with `range(count)` (`SerializedFile.py:282-308`):
//!   a negative count is zero records, and a table grows record by record, so
//!   no count sizes an allocation;
//! - with [`FileContext::eager_asset_bundle`], the first AssetBundle in dict
//!   order is read at load with `check_read`, and a failure fails the file
//!   (`SerializedFile.py:316-324`). UnityPy reads it in object mode
//!   (`parse_as_object`); the core reads it in dict mode, and package D's
//!   fixtures pin that the two agree (plan section 3.7).
//!
//! **Memory.** Every table (types, their dependency lists and trees, objects,
//! the dict order, scripts, externals, ref types) is built in a
//! [`crate::limits::MeteredVec`] on the input's meter, and the file keeps the
//! charges: two directory rows over the same bytes parse twice and pay twice.
//! Strings are zero-copy slices of the file.
//!
//! **Work.** The container read ([`SerializedFile::container`]) builds the
//! whole AssetBundle value and keeps only its rows and preload table, so
//! two directory rows over the same bytes would repeat a read whose memory
//! is returned: what the meter granted the read on its own thread, beyond
//! what its [`Container`] keeps, counts as the open's work
//! ([`crate::error::LimitKind::DecodeWork`]), and so does all it was
//! granted when it fails with an error its cell keeps. At load, a failure
//! fails the parse, whose grants its bundle counts instead
//! (`files::bundle`, "Memory and work"), so they count once (round 5).
//!
//! Owner: package C. The accessors and [`SerializedType`]'s constructors and
//! [`SerializedType::tree`] are implemented by the scaffold, so package B can
//! build ref types for its tests before C lands.

use std::sync::{Arc, OnceLock};

use bytes::Bytes;

use crate::container::{Container, read_container};
use crate::error::Error;
use crate::files::Parent;
use crate::limits::{Charge, GrantScope, Limits, Meter};
use crate::object::ObjectRef;
use crate::once::{CellError, ResultCell, get_or_publish_result};
use crate::options::FileContext;
use crate::raw::RawStr;
use crate::stream::{Endian, ShortArray};
use crate::typetree::{NodeList, Tree, TreeError};
use crate::version::ResolvedVersion;
use crate::walk::RefTypes;

mod parse;

/// The header, as read.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct SerializedHeader {
    /// `metadata_size`.
    pub metadata_size: u32,
    /// `file_size`.
    pub file_size: i64,
    /// The format number (UnityPy's `header.version`).
    pub version: u32,
    /// `data_offset`.
    pub data_offset: i64,
    /// The byte order.
    pub endian: Endian,
    /// The three reserved bytes (format 9 and up).
    pub reserved: Option<[u8; 3]>,
    /// Format 22's trailing `i64` (UnityPy's `unknown`), else 0.
    pub unknown: i64,
}

/// A type record (`SerializedType`), plain or ref type. Not `Clone`: it
/// lives in its file's table, which paid for it.
#[derive(Debug)]
#[non_exhaustive]
pub struct SerializedType {
    /// `class_id`.
    pub class_id: i32,
    /// Format 16 and up.
    pub is_stripped_type: Option<bool>,
    /// Read from format 17, else -1; at 11-16 the last object's write-back.
    pub script_type_index: i16,
    /// `script_id`, under UnityPy's condition (format 13 and up): the bytes
    /// `read_bytes(16)` returned, short at a negative position or the end.
    pub script_id: Option<ShortArray<16>>,
    /// `old_type_hash` (format 13 and up), as `script_id`.
    pub old_type_hash: Option<ShortArray<16>>,
    /// The stored tree, when type trees are enabled.
    pub nodes: Option<Arc<NodeList>>,
    /// Plain types, format 21 and up, type trees enabled.
    pub type_dependencies: Option<Box<[i32]>>,
    /// Ref types, format 21 and up, type trees enabled: `m_ClassName`.
    pub class_name: Option<RawStr>,
    /// `m_NameSpace`.
    pub namespace: Option<RawStr>,
    /// `m_AssemblyName`.
    pub assembly: Option<RawStr>,
    tree: ResultCell<Arc<Tree>, TreeError>,
}

impl SerializedType {
    /// A plain type record: `class_id` and its stored tree, every other
    /// field as a format below 13 leaves it (`script_type_index` -1).
    #[must_use]
    pub fn new(class_id: i32, nodes: Option<Arc<NodeList>>) -> Self {
        Self {
            class_id,
            is_stripped_type: None,
            script_type_index: -1,
            script_id: None,
            old_type_hash: None,
            nodes,
            type_dependencies: None,
            class_name: None,
            namespace: None,
            assembly: None,
            tree: OnceLock::new(),
        }
    }

    /// A ref type (format 21 and up): its class, namespace and assembly
    /// names and its stored tree.
    #[must_use]
    pub fn ref_type(
        class_id: i32,
        class_name: RawStr,
        namespace: RawStr,
        assembly: RawStr,
        nodes: Option<Arc<NodeList>>,
    ) -> Self {
        Self {
            class_name: Some(class_name),
            namespace: Some(namespace),
            assembly: Some(assembly),
            ..Self::new(class_id, nodes)
        }
    }

    /// The compiled tree: `Tree::compile` over [`Self::nodes`] (package B),
    /// computed on first use and published once, so every object and every
    /// reference of the type reuses one compile. The compile runs outside
    /// the cell ([`get_or_publish_result`]): two first callers may both
    /// compile, and the loser's tree is dropped. Its rows are charged to the
    /// meter the `NodeList` was built on. A compile the meter or the budget
    /// refuses is returned and not kept, so the next call compiles again;
    /// a failure the rows decide (depth) is kept. `None` when the record
    /// stores no tree.
    pub fn tree(&self, limits: &Limits) -> Option<Result<Arc<Tree>, TreeError>> {
        self.tree_with(|nodes| Tree::compile(nodes, limits))
    }

    fn tree_with(
        &self,
        compile: impl FnOnce(Arc<NodeList>) -> Result<Arc<Tree>, TreeError>,
    ) -> Option<Result<Arc<Tree>, TreeError>> {
        let nodes = self.nodes.as_ref()?;
        Some(get_or_publish_result(&self.tree, nodes.meter(), || compile(nodes.clone())).cloned())
    }
}

/// One row of the object table.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ObjectInfo {
    /// `path_id`.
    pub path_id: i64,
    /// Absolute within the `SerializedFile`: the stored `u32` (or format 22's
    /// `i64`) plus `data_offset`, added as Python adds them, so it can pass
    /// `i64::MAX` (`2^63 - 1 + 144` loads in UnityPy) or go negative; a read
    /// from it is a Python slice ([`crate::stream::py_read`]).
    pub byte_start: i128,
    /// `byte_size`.
    pub byte_size: u32,
    /// `type_id` as stored.
    pub type_id: i32,
    /// Below format 16 the stored `u16`; from 16 the type's `class_id`.
    pub class_id: i32,
    /// The index into `types` this object resolves to, if any.
    pub type_index: Option<usize>,
    /// Below format 11.
    pub is_destroyed: Option<u16>,
    /// Formats 15 and 16.
    pub is_stripped: Option<i8>,
    /// Formats 11-16.
    pub script_type_index: Option<i16>,
}

/// One external file reference.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct External {
    /// `temp_empty` (format 6 and up).
    pub temp_empty: Option<RawStr>,
    /// `guid` (format 5 and up): the bytes `read_bytes(16)` returned, short
    /// at a negative position or the end.
    pub guid: Option<ShortArray<16>>,
    /// `type` (format 5 and up).
    pub kind: Option<i32>,
    /// `path`.
    pub path: RawStr,
}

/// One script type reference (`LocalSerializedObjectIdentifier`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ScriptRef {
    /// `local_serialized_file_index`.
    pub file_index: i32,
    /// `local_identifier_in_file`.
    pub identifier: i64,
}

/// A parsed `SerializedFile`: metadata and object table. Values are read on
/// demand through [`ObjectRef`].
pub struct SerializedFile {
    name: RawStr,
    data: Bytes,
    header: SerializedHeader,
    unity_version: Option<RawStr>,
    version: Option<ResolvedVersion>,
    target_platform: Option<i32>,
    enable_type_tree: bool,
    big_id_enabled: i32,
    types: Box<[SerializedType]>,
    ref_types: Option<Box<[SerializedType]>>,
    script_types: Box<[ScriptRef]>,
    externals: Box<[External]>,
    user_information: Option<RawStr>,
    object_infos: Box<[ObjectInfo]>,
    object_order: Box<[u32]>,
    /// The dict's entries (indexes into `object_infos`) sorted by path id,
    /// for [`Self::object`].
    by_path: Box<[u32]>,
    /// Published by [`get_or_publish_result`]: a refused charge is not kept.
    container: ResultCell<Container, Error>,
    parent: Option<Parent>,
    /// The handle and every table above, charged to the input, with the
    /// resolved version's heap ([`crate::version::UnityVersion::heap_bytes`], charged
    /// by the chain before it was allocated;
    /// its string is the file's own slice, or the parent's, never a copy).
    charge: Charge,
    ctx: Arc<FileContext>,
}

impl std::fmt::Debug for SerializedFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SerializedFile")
            .field("name", &self.name)
            .field("header", &self.header)
            .field("unity_version", &self.unity_version)
            .field("types", &self.types.len())
            .field("objects", &self.object_order.len())
            .finish_non_exhaustive()
    }
}

impl SerializedFile {
    /// The meter of the input it belongs to: what reading its objects
    /// charges ([`crate::walk::ReadCtx::meter`]).
    pub fn meter(&self) -> &Arc<Meter> {
        self.charge.meter()
    }

    /// The name it was loaded under.
    pub fn name(&self) -> &RawStr {
        &self.name
    }

    /// The whole file's bytes.
    pub fn data(&self) -> &Bytes {
        &self.data
    }

    /// The header.
    pub fn header(&self) -> &SerializedHeader {
        &self.header
    }

    /// The version string as read (format 7 and up).
    pub fn unity_version(&self) -> Option<&RawStr> {
        self.unity_version.as_ref()
    }

    /// Chain 2's result (format 7 and up).
    pub fn version(&self) -> Option<&ResolvedVersion> {
        self.version.as_ref()
    }

    /// The raw target platform (format 8 and up).
    pub fn target_platform(&self) -> Option<i32> {
        self.target_platform
    }

    /// Whether type records carry trees.
    pub fn enable_type_tree(&self) -> bool {
        self.enable_type_tree
    }

    /// `big_id_enabled` (formats 7-13, else 0).
    pub fn big_id_enabled(&self) -> i32 {
        self.big_id_enabled
    }

    /// Type records, in file order, script indexes written back.
    pub fn types(&self) -> &[SerializedType] {
        &self.types
    }

    /// Ref types (format 20 and up).
    pub fn ref_types(&self) -> Option<&[SerializedType]> {
        self.ref_types.as_deref()
    }

    /// The reader's view of the ref types.
    pub fn ref_types_view(&self) -> RefTypes<'_> {
        match &self.ref_types {
            Some(t) => RefTypes::List(t),
            None => RefTypes::Absent,
        }
    }

    /// Script type references.
    pub fn script_types(&self) -> &[ScriptRef] {
        &self.script_types
    }

    /// External file references.
    pub fn externals(&self) -> &[External] {
        &self.externals
    }

    /// `userInformation` (format 5 and up).
    pub fn user_information(&self) -> Option<&RawStr> {
        self.user_information.as_ref()
    }

    /// The object table in file order, repeated path ids kept.
    pub fn object_infos(&self) -> &[ObjectInfo] {
        &self.object_infos
    }

    /// UnityPy's `objects` dict order: indexes into [`Self::object_infos`] of
    /// each path id's last entry, at its first position.
    pub fn object_order(&self) -> &[u32] {
        &self.object_order
    }

    /// The objects, in dict order.
    pub fn objects(self: &Arc<Self>) -> impl Iterator<Item = ObjectRef> + '_ {
        self.object_order
            .iter()
            .filter_map(|i| ObjectRef::new(self.clone(), *i))
    }

    /// The objects in ascending signed path id (UnityPy's
    /// `sorted(objects.values(), key=lambda o: o.path_id)`): the table
    /// [`Self::object`] searches, kept and charged with the file, so a
    /// caller orders them without a copy or a sort.
    pub fn objects_by_path(self: &Arc<Self>) -> impl Iterator<Item = ObjectRef> + '_ {
        self.by_path
            .iter()
            .filter_map(|i| ObjectRef::new(self.clone(), *i))
    }

    /// The object `objects[path_id]` holds.
    pub fn object(self: &Arc<Self>, path_id: i64) -> Option<ObjectRef> {
        let at = self
            .by_path
            .binary_search_by_key(&path_id, |i| {
                usize::try_from(*i)
                    .ok()
                    .and_then(|i| self.object_infos.get(i))
                    .map_or(i64::MIN, |o| o.path_id)
            })
            .ok()?;
        ObjectRef::new(self.clone(), *self.by_path.get(at)?)
    }

    /// The container rows of the first AssetBundle in dict order, read once
    /// ([`crate::container::read_container`]) and published
    /// ([`get_or_publish_result`]: the read runs outside the cell, and a
    /// refused charge is returned, not kept). What the read was granted and
    /// its container does not keep is the open's work (module docs, "Work").
    pub fn container(self: &Arc<Self>) -> Result<&Container, Error> {
        self.container_counted(true)
    }

    /// [`Self::container`] as the load reads it (`SerializedFile.py:316-324`):
    /// a failure fails the parse, whose caller counts its grants.
    fn container_at_load(self: &Arc<Self>) -> Result<&Container, Error> {
        self.container_counted(false)
    }

    /// The container, its read's grants counted as work: on success what its
    /// container does not keep, on a failure its cell keeps all of it when
    /// `failure_counts`. A refusal the cell does not keep is
    /// [`get_or_publish_result`]'s to count.
    fn container_counted(self: &Arc<Self>, failure_counts: bool) -> Result<&Container, Error> {
        let meter = self.meter();
        get_or_publish_result(&self.container, meter, || {
            let scope = GrantScope::enter(meter);
            let got = read_container(self);
            let granted = scope.finish(meter);
            match &got {
                Ok(c) => meter.add_work(granted.saturating_sub(c.charge().bytes())),
                Err(e) if failure_counts && !e.is_transient() => meter.add_work(granted),
                Err(_) => {}
            }
            got
        })
    }

    /// The container this file is an entry of.
    pub fn parent(&self) -> Option<&Parent> {
        self.parent.as_ref()
    }

    /// The context it was loaded with.
    pub fn context(&self) -> &Arc<FileContext> {
        &self.ctx
    }

    /// A file of `rows` and nothing else, for the crate's tests before
    /// package C's `parse` lands: dict order is file order.
    #[cfg(test)]
    pub(crate) fn for_tests(rows: Vec<ObjectInfo>) -> Arc<Self> {
        let meter = Meter::unlimited();
        let order = (0..u32::try_from(rows.len()).unwrap()).collect();
        Arc::new(Self {
            name: RawStr::from_static(b"CAB-test"),
            data: Bytes::new(),
            header: SerializedHeader {
                metadata_size: 0,
                file_size: 0,
                version: 22,
                data_offset: 0,
                endian: Endian::Little,
                reserved: None,
                unknown: 0,
            },
            unity_version: None,
            version: None,
            target_platform: None,
            enable_type_tree: true,
            big_id_enabled: 0,
            types: Box::new([]),
            ref_types: None,
            script_types: Box::new([]),
            externals: Box::new([]),
            user_information: None,
            by_path: Box::new([]),
            object_infos: rows.into_boxed_slice(),
            object_order: order,
            container: ResultCell::new(),
            parent: None,
            charge: Charge::zero(&meter),
            ctx: Arc::new(FileContext::unitypy()),
        })
    }
}

#[cfg(test)]
mod tests;
