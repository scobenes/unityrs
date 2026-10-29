//! `ObjectReader` (`files/ObjectReader.py:44-312`): one object of a
//! `SerializedFile`, read on demand.
//!
//! An [`ObjectRef`] is a cheap handle (an `Arc` on the file plus the table
//! row); it owns nothing else, so a batch can hold millions. Reading goes
//! through [`crate::walk::read_typetree`] with the tree that
//! [`ObjectRef::tree`] resolves (plan section 3.12: explicit, embedded, then
//! the provider chain with the class and packed version as its key).
//!
//! Owner: package D. The constructor and getters are implemented by the
//! scaffold.

// STUB(D): delete this allow when the module is implemented.
#![allow(
    clippy::todo,
    clippy::needless_pass_by_value,
    clippy::must_use_candidate,
    clippy::missing_panics_doc,
    unused_variables
)]

use std::sync::Arc;

use bytes::Bytes;

use crate::files::{ObjectInfo, SerializedFile, SerializedType};
use crate::provider::TreeRef;
use crate::typetree::Tree;
use crate::value::Parsed;
use crate::walk::{ParseError, WriteError, WriteRules};

/// One object of a `SerializedFile`.
#[derive(Clone, Debug)]
pub struct ObjectRef {
    file: Arc<SerializedFile>,
    index: u32,
    info: ObjectInfo,
}

impl ObjectRef {
    /// The object at `index` in [`SerializedFile::object_infos`].
    #[must_use]
    pub fn new(file: Arc<SerializedFile>, index: u32) -> Option<Self> {
        let info = file
            .object_infos()
            .get(usize::try_from(index).ok()?)?
            .clone();
        Some(Self { file, index, info })
    }

    /// The file it belongs to.
    pub fn file(&self) -> &Arc<SerializedFile> {
        &self.file
    }

    /// Its index in [`SerializedFile::object_infos`].
    pub fn index(&self) -> u32 {
        self.index
    }

    /// Its table row.
    pub fn info(&self) -> &ObjectInfo {
        &self.info
    }

    /// `path_id`.
    pub fn path_id(&self) -> i64 {
        self.info.path_id
    }

    /// `class_id`.
    pub fn class_id(&self) -> i32 {
        self.info.class_id
    }

    /// The type record it resolves to, if any.
    pub fn serialized_type(&self) -> Option<&SerializedType> {
        self.file.types().get(self.info.type_index?)
    }

    /// `get_raw_data`: `byte_size` bytes from `byte_start`, cut short at the
    /// end of the file (a slice of the file's buffer, no copy).
    pub fn raw_data(&self) -> Bytes {
        todo!()
    }

    /// The tree a read would use: `nodes` when given, else the type record's,
    /// else the provider chain's class tree (`ObjectReader.py:285-307`), with
    /// no script (`TreeRequest::script` is `None`). `Ok(None)` is UnityPy's
    /// `TypeTreeError`. The `MonoBehaviour` script hook needs the Environment
    /// to dereference `m_Script`: [`crate::env::Environment::object_tree`].
    pub fn tree(&self, nodes: Option<&Arc<Tree>>) -> Result<Option<TreeRef>, ParseError> {
        todo!()
    }

    /// `read_typetree(nodes, wrap=False, check_read)`.
    pub fn read_typetree(
        &self,
        nodes: Option<&Arc<Tree>>,
        check_read: bool,
    ) -> Result<Parsed, ParseError> {
        todo!()
    }

    /// `parse_as_dict(check_read=check_read)`: [`Self::read_typetree`] with
    /// the resolved tree.
    pub fn parse_as_dict(&self, check_read: bool) -> Result<Parsed, ParseError> {
        self.read_typetree(None, check_read)
    }

    /// `save_typetree`'s bytes: UnityPy's writer (or `rules`) over `value`,
    /// written into a buffer charged to the file's input meter, whose charge
    /// rides in the returned `Bytes`.
    pub fn write_typetree(
        &self,
        value: &Parsed,
        nodes: Option<&Arc<Tree>>,
        rules: WriteRules,
    ) -> Result<Bytes, WriteError> {
        todo!()
    }
}
