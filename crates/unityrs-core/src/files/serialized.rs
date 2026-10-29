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
//! Owner: package C. The accessors and [`SerializedType`]'s constructors and
//! [`SerializedType::tree`] are implemented by the scaffold, so package B can
//! build ref types for its tests before C lands.

// STUB(C): delete this allow when the module is implemented.
#![allow(
    clippy::todo,
    clippy::needless_pass_by_value,
    clippy::must_use_candidate,
    clippy::missing_panics_doc,
    unused_variables,
    dead_code
)]

use std::sync::{Arc, OnceLock};

use bytes::Bytes;

use crate::container::Container;
use crate::error::Error;
use crate::files::Parent;
use crate::limits::{Charge, Limits, Meter};
use crate::object::ObjectRef;
use crate::once::{ResultCell, get_or_publish_result};
use crate::options::FileContext;
use crate::raw::RawStr;
use crate::stream::Endian;
use crate::typetree::{NodeList, Tree, TreeError};
use crate::version::ResolvedVersion;
use crate::walk::RefTypes;

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
    /// `script_id`, under UnityPy's condition (format 13 and up).
    pub script_id: Option<[u8; 16]>,
    /// `old_type_hash` (format 13 and up).
    pub old_type_hash: Option<[u8; 16]>,
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
    /// `guid` (format 5 and up).
    pub guid: Option<[u8; 16]>,
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
    /// Published by [`get_or_publish_result`]: a refused charge is not kept.
    container: ResultCell<Container, Error>,
    parent: Option<Parent>,
    /// The handle and every table above, charged to the input, with the
    /// resolved version's heap ([`crate::version::UnityVersion::heap_bytes`];
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
    /// `SerializedFile.__init__`. Everything it keeps is charged to `meter`,
    /// the input's.
    pub fn parse(
        name: RawStr,
        data: Bytes,
        parent: Option<Parent>,
        meter: &Arc<Meter>,
        ctx: &Arc<FileContext>,
    ) -> Result<Arc<Self>, Error> {
        todo!()
    }

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

    /// The object `objects[path_id]` holds.
    pub fn object(self: &Arc<Self>, path_id: i64) -> Option<ObjectRef> {
        todo!()
    }

    /// The container rows of the first AssetBundle in dict order, read once
    /// ([`crate::container::read_container`]) and published
    /// ([`get_or_publish_result`]: the read runs outside the cell, and a
    /// refused charge is returned, not kept).
    pub fn container(self: &Arc<Self>) -> Result<&Container, Error> {
        todo!()
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
mod tests {
    use std::sync::Arc;

    use super::{ObjectInfo, SerializedType};
    use crate::raw::RawStr;
    use crate::typetree::{NodeList, NodeRow};

    #[test]
    fn package_b_can_build_ref_types_without_package_c() {
        let nodes = Arc::new(
            NodeList::trusted(vec![
                NodeRow::new(0, "Foo", "Base"),
                NodeRow::new(1, "int", "x"),
            ])
            .unwrap(),
        );
        let t = SerializedType::ref_type(
            114,
            RawStr::from("Foo"),
            RawStr::from("Ns"),
            RawStr::from("Asm.dll"),
            Some(nodes),
        );
        assert_eq!(
            t.class_name.as_ref().map(RawStr::as_bytes),
            Some(&b"Foo"[..])
        );
        assert_eq!(t.script_type_index, -1);
        let plain = SerializedType::new(1, None);
        assert!(plain.class_name.is_none());
        assert!(
            plain.tree(&crate::limits::Limits::default()).is_none(),
            "no stored tree, nothing to compile"
        );
    }

    #[test]
    fn a_refused_compile_is_not_kept() {
        use crate::budget::{Counting, Reservation};
        use crate::limits::{ChargeError, Meter, MeteredVec};
        use crate::typetree::{Tree, TreeError};
        // Round 3: a compile the pool refused (while another reader held it)
        // was published, failing every object of the type for good.
        let pool = Arc::new(Counting::new(4096));
        let meter = Meter::new(u64::MAX, pool.clone(), Reservation::Dependency);
        let rows = MeteredVec::from_vec(vec![NodeRow::new(0, "Base", "Base")], &meter).unwrap();
        let t = SerializedType::new(1, Some(Arc::new(NodeList::from_rows(rows).unwrap())));
        let compile = |nodes: Arc<NodeList>| {
            let _scratch = nodes.meter().charge(1024).map_err(TreeError::Charge)?;
            Ok(Tree::for_tests(nodes))
        };
        let other = meter.charge(4096 - pool.used()).unwrap();
        let refused = t.tree_with(compile).unwrap();
        assert!(
            matches!(&refused, Err(TreeError::Charge(ChargeError::Budget(_)))),
            "{refused:?}"
        );
        drop(other);
        assert!(
            t.tree_with(compile).unwrap().is_ok(),
            "compiled again once the pool had room"
        );
        // A failure the rows decide is kept.
        let deep = SerializedType::new(2, t.nodes.clone());
        let too_deep = |_: Arc<NodeList>| {
            Err(TreeError::Limit(crate::LimitError {
                kind: crate::LimitKind::TreeDepth,
                value: 2,
                max: 1,
            }))
        };
        assert!(deep.tree_with(too_deep).unwrap().is_err());
        assert!(
            deep.tree_with(compile).unwrap().is_err(),
            "the kept error answers"
        );
    }

    #[test]
    // The conversion keeps the test meaningful against a narrower field type.
    #[allow(clippy::useless_conversion)]
    fn byte_start_holds_what_python_adds() {
        // A format-22 object stored at i64::MAX in a file with data_offset 144
        // loads in UnityPy 1.25.2 with byte_start 9223372036854775951.
        let start = i128::from(i64::MAX) + 144;
        let info = ObjectInfo {
            path_id: 1,
            byte_start: start.try_into().unwrap(),
            byte_size: 4,
            type_id: 0,
            class_id: 1,
            type_index: Some(0),
            is_destroyed: None,
            is_stripped: None,
            script_type_index: None,
        };
        assert_eq!(info.byte_start, start);
    }
}
