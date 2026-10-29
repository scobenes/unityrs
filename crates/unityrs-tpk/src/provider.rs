// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/Tpk.py. See THIRD_PARTY_LICENSES.
//! [`TpkProvider`]: UnityPy's `get_typetree_node` (`helpers/Tpk.py:42-55`)
//! with `get_item_for_version` (`:472-481`) and its two caches, over a loaded
//! blob, as unityrs-core's [`TypeTreeProvider`] (crate docs).

use std::fmt;
use std::io::Read;
use std::path::Path;
use std::sync::{Arc, OnceLock};

use unityrs_core::limits::{Limits, Meter, MeteredVec};
use unityrs_core::once::get_or_publish;
use unityrs_core::provider::{ProviderError, TreeRequest, TypeTreeProvider};
use unityrs_core::typetree::commonstring::CommonStrings;
use unityrs_core::typetree::{NodeList, Tree, TreeError};
use unityrs_core::{LimitError, LimitKind, UnityVersion};

use crate::blob::{ClassEntry, TypeTreeBlob, UnityClass, memory};
use crate::tree::{WalkError, generate_node};
use crate::{TpkError, file};

/// What a picked entry resolved to, kept once per entry.
type Outcome<T> = Result<Option<Arc<T>>, ProviderError>;

/// One class entry's node list and tree, each computed once (UnityPy's
/// `NODES_CACHE`, `Tpk.py:17`, `:58-62`, `:86-87`).
#[derive(Default)]
struct Cells {
    rows: OnceLock<Outcome<NodeList>>,
    tree: OnceLock<Outcome<Tree>>,
}

/// A TPK's class trees, as UnityPy's `helpers/Tpk.py` builds them (crate
/// docs).
pub struct TpkProvider {
    blob: TypeTreeBlob,
    /// One pair of cells per class entry, in the blob's entry order.
    cells: MeteredVec<Cells>,
    /// UnityPy's `get_common_strings()` over this TPK.
    common: CommonStrings,
    limits: Limits,
    /// The provider's own meter, unlimited (crate docs, "Memory").
    meter: Arc<Meter>,
}

impl fmt::Debug for TpkProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TpkProvider")
            .field("classes", &self.blob.classes.len())
            .field("versions", &self.blob.versions.len())
            .finish_non_exhaustive()
    }
}

/// UnityPy's `get_item_for_version(exactVersion, items)` (`Tpk.py:472-481`)
/// over a class's entries: the position of the last entry at or below
/// `version`, scanning forward in stored order and stopping at the first
/// above it. Its `if ret:` raises `ValueError` where nothing was kept and
/// where the kept entry is `None` (a `TpkUnityClass` is always true); the
/// caller takes the second as no entry.
fn get_item_for_version(version: &UnityVersion, entries: &[ClassEntry]) -> Option<usize> {
    let mut ret = None;
    for (at, entry) in entries.iter().enumerate() {
        if *version >= entry.version {
            ret = Some(at);
        } else {
            break;
        }
    }
    ret
}

impl TpkProvider {
    /// The most rows one class tree may have. UnityPy's own TPK's largest
    /// has 3,849 (`tools/fixtures/tpk.py --survey`); a tree past this is an
    /// error, which bounds the walk of a node that lists an ancestor.
    pub const MAX_TREE_ROWS: usize = 1 << 16;

    /// The largest TPK file the provider reads, 16 MiB. UnityPy 1.25.2's
    /// own is 187,318 bytes (`tools/fixtures/tpk.py --survey`).
    pub const MAX_FILE_BYTES: u64 = 16 << 20;

    /// The most a TPK's payload may decode to, 16 MiB. UnityPy 1.25.2's
    /// own decodes to 1,385,610 bytes (`tools/fixtures/tpk.py --survey`).
    pub const MAX_DECODED_BYTES: u64 = 16 << 20;

    /// The most the decoder may hold while it decodes, 64 MiB: its output
    /// and, for LZMA, the dictionary, which unityrs-core's codec keeps within
    /// twice the output, so the output's bound is the one a stream meets
    /// first.
    pub const MAX_DECODER_MEMORY: u64 = 64 << 20;

    /// The most records any one table of the blob may hold, 65,536: the
    /// most nodes and strings a 16-bit id can name. The entries of every
    /// class together are one table. UnityPy 1.25.2's own TPK's largest
    /// table is its 22,818 nodes (`tools/fixtures/tpk.py --survey`).
    pub const MAX_RECORDS: usize = 1 << 16;

    /// The provider's name, the dump's `l2.source`.
    pub const NAME: &'static str = "tpk";

    /// Loads the TPK at `path`, with the default [`Limits`].
    pub fn open(path: impl AsRef<Path>) -> Result<Self, TpkError> {
        Self::open_with(path, &Limits::default())
    }

    /// Loads the TPK at `path`; trees deeper than `limits.max_tree_depth`
    /// are errors. A file past [`Self::MAX_FILE_BYTES`] is refused unread,
    /// and one that is not a regular file (whose length is not known) is
    /// read only that far.
    pub fn open_with(path: impl AsRef<Path>, limits: &Limits) -> Result<Self, TpkError> {
        let file = std::fs::File::open(path)?;
        let len = file.metadata()?.len();
        let too_large = |value: u64| TpkError::TooLarge {
            what: "file",
            value,
            max: Self::MAX_FILE_BYTES,
        };
        if len > Self::MAX_FILE_BYTES {
            return Err(too_large(len));
        }
        let mut bytes = Vec::new();
        file.take(Self::MAX_FILE_BYTES + 1)
            .read_to_end(&mut bytes)?;
        let read = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        if read > Self::MAX_FILE_BYTES {
            return Err(too_large(read));
        }
        Self::from_bytes(&bytes, limits)
    }

    /// Loads a TPK from its bytes, as UnityPy's `TpkFile(stream).GetDataBlob()`
    /// reads them (`helpers/Tpk.py:36-39`; crate docs).
    pub fn from_bytes(bytes: &[u8], limits: &Limits) -> Result<Self, TpkError> {
        let len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        if len > Self::MAX_FILE_BYTES {
            return Err(TpkError::TooLarge {
                what: "file",
                value: len,
                max: Self::MAX_FILE_BYTES,
            });
        }
        let meter = Meter::unlimited();
        let payload = file::payload(bytes)?;
        let blob = TypeTreeBlob::read(&payload, &meter)?;
        drop(payload);
        let common = common_strings(&blob)?;
        let entries = blob.classes.entry_count();
        let mut cells = MeteredVec::with_capacity(&meter, entries).map_err(memory)?;
        for _ in 0..entries {
            cells.push(Cells::default()).map_err(memory)?;
        }
        Ok(Self {
            blob,
            cells,
            common,
            limits: *limits,
            meter,
        })
    }

    /// The number of classes the TPK holds (UnityPy's
    /// `len(ClassInformation)`; a class listed twice counts once, its last
    /// listing kept, as a Python dict keeps it).
    #[must_use]
    pub fn class_count(&self) -> usize {
        self.blob.classes.len()
    }

    /// The classes the TPK holds, sorted.
    #[must_use]
    pub fn class_ids(&self) -> Vec<i32> {
        self.blob.classes.ids().collect()
    }

    /// The TPK's version list, as stored (UnityPy's `Versions`).
    #[must_use]
    pub fn versions(&self) -> &[UnityVersion] {
        self.blob.versions.as_slice()
    }

    /// The node list UnityPy builds for `class_id` at `version`, before it
    /// is compiled: `Ok(None)` wherever UnityPy raises `KeyError`,
    /// `ValueError` or `AssertionError` (crate docs). Every version that
    /// picks one entry gets the same `Arc`.
    pub fn node_list(
        &self,
        class_id: i32,
        version: &UnityVersion,
    ) -> Result<Option<Arc<NodeList>>, ProviderError> {
        let Some((class, cells)) = self.entry(class_id, version) else {
            return Ok(None);
        };
        self.rows_of(class, cells).clone()
    }

    /// `TPKTYPETREE.ClassInformation[class_id].getVersionedClass(version)`
    /// (`Tpk.py:49-51`): the picked entry's class and its cells; `None`
    /// where UnityPy raises `KeyError` (no such class) or `ValueError` (no
    /// entry at or below the version, or the class removed there).
    fn entry(&self, class_id: i32, version: &UnityVersion) -> Option<(UnityClass, &Cells)> {
        let (first, entries) = self.blob.classes.get(class_id)?;
        let at = get_item_for_version(version, entries)?;
        let class = entries.get(at)?.class?;
        let cells = self.cells.as_slice().get(first.checked_add(at)?)?;
        Some((class, cells))
    }

    fn rows_of<'c>(&self, class: UnityClass, cells: &'c Cells) -> &'c Outcome<NodeList> {
        get_or_publish(&cells.rows, || self.walk(class))
    }

    /// `generate_node(class_info)` (`Tpk.py:58-88`) for one entry's class.
    fn walk(&self, class: UnityClass) -> Outcome<NodeList> {
        // `assert class_info.ReleaseRootNode is not None` (`Tpk.py:64`).
        let Some(root) = class.release_root else {
            return Ok(None);
        };
        let max_depth = usize::try_from(self.limits.max_tree_depth).unwrap_or(usize::MAX);
        let rows = generate_node(
            &self.blob,
            root,
            max_depth,
            Self::MAX_TREE_ROWS,
            &self.meter,
        )
        .map_err(|e| match e {
            WalkError::Depth { depth, max } => limit(LimitKind::TreeDepth, depth, max),
            WalkError::Rows { rows, max } => limit(LimitKind::Count, rows, max),
            WalkError::PastTable(what) => error(&what),
            WalkError::Memory(e) => error(&e),
        })?;
        // `TypeTreeNode.from_list(nodes)` (`Tpk.py:86`).
        NodeList::from_rows(rows)
            .map(|list| Some(Arc::new(list)))
            .map_err(|e| error(&e))
    }
}

fn error(e: &dyn fmt::Display) -> ProviderError {
    ProviderError {
        provider: TpkProvider::NAME,
        detail: e.to_string().into_boxed_str(),
        limit: None,
    }
}

/// A walk that crossed a bound, as a limit error.
fn limit(kind: LimitKind, value: usize, max: usize) -> ProviderError {
    let limit = LimitError {
        kind,
        value: u64::try_from(value).unwrap_or(u64::MAX),
        max: u64::try_from(max).unwrap_or(u64::MAX),
    };
    ProviderError {
        limit: Some(limit),
        ..error(&limit)
    }
}

/// UnityPy's `get_common_strings()` (`helpers/TypeTreeNode.py:308-328`)
/// over `blob`: `GetStrings`, the string of every index in order
/// (`Tpk.py:425-426`), numbered as [`CommonStrings::new`] numbers them; each
/// string shared with the rows that name it. An index past the string
/// buffer, an `IndexError` in every blob parse in UnityPy, is refused here.
fn common_strings(blob: &TypeTreeBlob) -> Result<CommonStrings, TpkError> {
    let strings = blob.strings.as_slice();
    let indices = blob.common_indices.as_slice();
    if let Some((at, &index)) = indices
        .iter()
        .enumerate()
        .find(|&(_, &i)| usize::from(i) >= strings.len())
    {
        return Err(TpkError::CommonStringPastBuffer {
            at,
            index,
            strings: strings.len(),
        });
    }
    Ok(CommonStrings::new(
        indices
            .iter()
            .filter_map(|&i| strings.get(usize::from(i)).cloned()),
    ))
}

impl TypeTreeProvider for TpkProvider {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    /// The TPK's common strings (UnityPy's `get_common_strings()`, crate
    /// docs).
    fn common_strings(&self) -> Option<&CommonStrings> {
        Some(&self.common)
    }

    /// [`TpkProvider::node_list`], compiled once per entry with
    /// [`Tree::compile`] on the provider's meter and kept: every request
    /// that picks one entry gets the same `Arc` (UnityPy's `NODES_CACHE`).
    fn tree(&self, req: &TreeRequest<'_>) -> Result<Option<Arc<Tree>>, ProviderError> {
        let Some((class, cells)) = self.entry(req.class_id, req.version) else {
            return Ok(None);
        };
        get_or_publish(&cells.tree, || {
            let Some(list) = self.rows_of(class, cells).clone()? else {
                return Ok(None);
            };
            Tree::compile(list, &self.limits)
                .map(Some)
                .map_err(|e| match e {
                    TreeError::Limit(l) => ProviderError {
                        limit: Some(l),
                        ..error(&l)
                    },
                    e => error(&e),
                })
        })
        .clone()
    }
}
