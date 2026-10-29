//! `ObjectReader` (`files/ObjectReader.py:44-312`): one object of a
//! `SerializedFile`, read on demand.
//!
//! An [`ObjectRef`] is a cheap handle: an `Arc` on the file and the row's
//! index in the file's object table, which it reads on demand. It owns no
//! copy of the row and nothing else, so a batch can hold millions and clone
//! them freely: every handle of a file shares the one table the file's
//! parse charged to the input's meter (plan section 3.12). Reading goes
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

/// One object of a `SerializedFile`: the file and the index of its row
/// (module docs).
#[derive(Clone, Debug)]
pub struct ObjectRef {
    file: Arc<SerializedFile>,
    index: u32,
}

/// What [`ObjectRef::info`] would answer for an index [`ObjectRef::new`] did
/// not check. Never returned: `new` checks the index, and a file's object
/// table never changes after its parse.
static NO_ROW: ObjectInfo = ObjectInfo {
    path_id: 0,
    byte_start: 0,
    byte_size: 0,
    type_id: 0,
    class_id: 0,
    type_index: None,
    is_destroyed: None,
    is_stripped: None,
    script_type_index: None,
};

impl ObjectRef {
    /// The object at `index` in [`SerializedFile::object_infos`], or `None`
    /// past the table's end.
    #[must_use]
    pub fn new(file: Arc<SerializedFile>, index: u32) -> Option<Self> {
        file.object_infos().get(usize::try_from(index).ok()?)?;
        Some(Self { file, index })
    }

    /// The file it belongs to.
    pub fn file(&self) -> &Arc<SerializedFile> {
        &self.file
    }

    /// Its index in [`SerializedFile::object_infos`].
    pub fn index(&self) -> u32 {
        self.index
    }

    /// Its table row, read from the file's table (no copy).
    pub fn info(&self) -> &ObjectInfo {
        usize::try_from(self.index)
            .ok()
            .and_then(|i| self.file.object_infos().get(i))
            .unwrap_or(&NO_ROW)
    }

    /// `path_id`.
    pub fn path_id(&self) -> i64 {
        self.info().path_id
    }

    /// `class_id`.
    pub fn class_id(&self) -> i32 {
        self.info().class_id
    }

    /// The type record it resolves to, if any.
    pub fn serialized_type(&self) -> Option<&SerializedType> {
        self.file.types().get(self.info().type_index?)
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

#[cfg(test)]
mod tests {
    use std::mem::size_of;
    use std::sync::Arc;

    use super::ObjectRef;
    use crate::files::{ObjectInfo, SerializedFile};

    #[test]
    fn a_handle_holds_the_file_and_an_index_and_reads_its_row_from_the_table() {
        // Round 4 (Codex): every handle held its own copy of the row, so
        // handles kept or cloned by a caller duplicated the object table
        // outside the meter that paid for it.
        assert_eq!(
            size_of::<ObjectRef>(),
            size_of::<Arc<SerializedFile>>() + size_of::<u64>()
        );
        let row = |path_id| ObjectInfo {
            path_id,
            byte_start: 0,
            byte_size: 4,
            type_id: 0,
            class_id: 114,
            type_index: None,
            is_destroyed: None,
            is_stripped: None,
            script_type_index: None,
        };
        let file = SerializedFile::for_tests(vec![row(7), row(-3)]);
        let o = ObjectRef::new(file.clone(), 1).unwrap();
        assert!(
            std::ptr::eq(o.info(), &raw const file.object_infos()[1]),
            "the file's own row"
        );
        assert_eq!((o.path_id(), o.class_id()), (-3, 114));
        assert!(ObjectRef::new(file.clone(), 2).is_none());
        let ids: Vec<i64> = file.objects().map(|o| o.path_id()).collect();
        assert_eq!(ids, [7, -3]);
    }
}
