// Ported from UnityPy 1.25.2 (MIT): UnityPy/files/ObjectReader.py. See THIRD_PARTY_LICENSES.
//! `ObjectReader` (`files/ObjectReader.py:44-311`): one object of a
//! `SerializedFile`, read on demand.
//!
//! An [`ObjectRef`] is a cheap handle: an `Arc` on the file and the row's
//! index in the file's object table, which it reads on demand. It owns no
//! copy of the row and nothing else, so a batch can hold millions and clone
//! them freely: every handle of a file shares the one table the file's
//! parse charged to the input's meter (plan section 3.12). Reading goes
//! through [`crate::walk::read_typetree_with_reader`], by the rules of the
//! file's reader ([`SerializedFile::reader_kind`]), with the tree that
//! [`ObjectRef::tree`] resolves (plan section 3.12: explicit, embedded, then
//! the provider chain with the class and packed version as its key).
//!
//! Owner: package D. The constructor and getters are implemented by the
//! scaffold.

use std::sync::Arc;

use bytes::Bytes;

use crate::budget::DecodeBuf;
use crate::error::{Error, LimitKind};
use crate::files::{ObjectInfo, SerializedFile, SerializedType};
use crate::provider::{NoTree, TreeRef, TreeRequest, TreeSource};
use crate::typetree::Tree;
use crate::value::Parsed;
use crate::walk::{
    ParseError, ReadCtx, WriteCtx, WriteError, WriteRules, read_typetree_with_reader,
};

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
    #[must_use]
    pub fn file(&self) -> &Arc<SerializedFile> {
        &self.file
    }

    /// Its index in [`SerializedFile::object_infos`].
    #[must_use]
    pub fn index(&self) -> u32 {
        self.index
    }

    /// Its table row, read from the file's table (no copy).
    #[must_use]
    pub fn info(&self) -> &ObjectInfo {
        usize::try_from(self.index)
            .ok()
            .and_then(|i| self.file.object_infos().get(i))
            .unwrap_or(&NO_ROW)
    }

    /// `path_id`.
    #[must_use]
    pub fn path_id(&self) -> i64 {
        self.info().path_id
    }

    /// `class_id`.
    #[must_use]
    pub fn class_id(&self) -> i32 {
        self.info().class_id
    }

    /// The type record it resolves to, if any.
    #[must_use]
    pub fn serialized_type(&self) -> Option<&SerializedType> {
        self.file.types().get(self.info().type_index?)
    }

    /// `get_raw_data`: `byte_size` bytes from `byte_start`, cut short at the
    /// end of the file (a slice of the file's buffer, no copy), by the
    /// file's reader's rules ([`SerializedFile::reader_kind`]): the stream
    /// reader (a file loaded from a path) refuses a negative `byte_start`, or
    /// one at or above 2^63, as UnityPy's does on every machine, where the
    /// memory reader slices from the end or reads nothing. A `byte_start`
    /// past the largest file the file system takes is UnityPy's `OSError`
    /// on that machine; the stream reader takes it and reads nothing
    /// ([`crate::stream::ReaderKind`], `PARITY.md` section 19.3).
    pub fn raw_data(&self) -> Result<Bytes, Error> {
        // UnityPy files/ObjectReader.py:278-283: `reset()`, then
        // `read_bytes(byte_size)`.
        let info = self.info();
        let data = self.file.data();
        let kind = self.file.reader_kind();
        let start = kind.seek(info.byte_start)?;
        let (range, _) = kind.read(data.len(), start, i64::from(info.byte_size))?;
        Ok(data.slice(range))
    }

    /// The tree a read would use: `nodes` when given, else the type record's,
    /// else the provider chain's class tree (`ObjectReader.py:285-307`), with
    /// no script (`TreeRequest::script` is `None`). `Ok(None)` where UnityPy
    /// raises: [`Self::why_no_tree`] says which exception. The
    /// `MonoBehaviour` script hook needs the Environment to dereference
    /// `m_Script`: [`crate::env::Environment::object_tree`].
    pub fn tree(&self, nodes: Option<&Arc<Tree>>) -> Result<Option<TreeRef>, ParseError> {
        // UnityPy files/ObjectReader.py:285-307
        if let Some(tree) = nodes {
            return Ok(Some(TreeRef {
                tree: tree.clone(),
                source: TreeSource::Explicit,
            }));
        }
        // :296-297: the type record's own tree, when it has one.
        if let Some(st) = self.serialized_type()
            && let Some(tree) = st.tree(&self.file.context().limits)
        {
            return Ok(Some(TreeRef {
                tree: tree?,
                source: TreeSource::Embedded,
            }));
        }
        // :298-299: `get_typetree_node(class_id, version)`. A file below
        // format 7 has no version: UnityPy's `assets_file.version` is unset,
        // so the `ObjectReader.version` property's `AttributeError` falls to
        // `ObjectReader.__getattr__` (:223-225), which returns `None`, and
        // `get_typetree_node` raises with that `None` (`why_no_tree` names
        // which exception: `NoTree::NoVersion` for the comparison). No tree.
        let Some(version) = self.file.version() else {
            return Ok(None);
        };
        let req = TreeRequest {
            class_id: self.class_id(),
            version: &version.version,
            script: None,
        };
        Ok(self.file.context().providers.lookup(&req)?)
    }

    /// Why [`Self::tree`] finds none for this object without `nodes` (its
    /// type record holds none, and no provider has its class's): the
    /// exception UnityPy raises there ([`NoTree`],
    /// [`ProviderChain::why_none`] for the file's version, or for none below
    /// format 7). `None` where no provider says. What
    /// [`ParseError::NoTypeTree`] and [`WriteError::NoTypeTree`] carry.
    ///
    /// [`ProviderChain::why_none`]: crate::provider::ProviderChain::why_none
    #[must_use]
    pub fn why_no_tree(&self) -> Option<NoTree> {
        let version = self.file.version().map(|v| &v.version);
        self.file
            .context()
            .providers
            .why_none(self.class_id(), version)
    }

    /// `read_typetree(nodes, wrap=False, check_read)`.
    pub fn read_typetree(
        &self,
        nodes: Option<&Arc<Tree>>,
        check_read: bool,
    ) -> Result<Parsed, ParseError> {
        // UnityPy files/ObjectReader.py:244-262
        let Some(tree) = self.tree(nodes)? else {
            return Err(ParseError::NoTypeTree {
                class_id: self.class_id(),
                why: self.why_no_tree(),
            });
        };
        let file = &self.file;
        let options = file.context().read_options();
        let ctx = ReadCtx {
            endian: file.header().endian,
            ref_types: file.ref_types_view(),
            options: &options,
            meter: file.meter(),
        };
        let info = self.info();
        read_typetree_with_reader(
            file.data(),
            info.byte_start,
            info.byte_size,
            &tree.tree,
            &ctx,
            check_read,
            file.reader_kind(),
        )
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
        // UnityPy files/ObjectReader.py:264-276
        // `_get_typetree_node` raised: the writer never ran.
        let tree = match self.tree(nodes) {
            Ok(Some(t)) => t.tree,
            Ok(None) => {
                return Err(WriteError::NoTypeTree {
                    class_id: self.class_id(),
                    why: self.why_no_tree(),
                });
            }
            Err(e) => return Err(WriteError::Tree(Box::new(e))),
        };
        let file = &self.file;
        let limits = file.context().limits;
        let ctx = WriteCtx {
            endian: file.header().endian,
            ref_types: file.ref_types_view(),
            rules,
            limits: &limits,
        };
        let mut out = DecodeBuf::new(file.meter(), 0, u64::MAX, LimitKind::InputMemory)
            .map_err(WriteError::Charge)?;
        crate::walk::write::write(value, &tree, &ctx, &mut out)?;
        out.into_bytes().map_err(WriteError::Charge)
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
