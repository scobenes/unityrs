// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/Tpk.py. See THIRD_PARTY_LICENSES.
//! [`TpkProvider`]: UnityPy's `get_typetree_node` (`helpers/Tpk.py:42-55`)
//! with `get_item_for_version` (`:472-481`) and its two caches, over a loaded
//! blob, as unityrs-core's [`TypeTreeProvider`] (crate docs).

use std::fmt;
use std::io::Read;
use std::mem::size_of;
use std::path::Path;
use std::sync::{Arc, OnceLock};

use unityrs_core::limits::{Charge, ChargeError, Limits, Meter, MeteredVec, heap_cost};
use unityrs_core::once::get_or_publish;
use unityrs_core::provider::{ProviderError, TreeRequest, TypeTreeProvider};
use unityrs_core::typetree::commonstring::CommonStrings;
use unityrs_core::typetree::{NodeList, Tree, TreeError};
use unityrs_core::{LimitError, LimitKind, UnityVersion};

use crate::blob::{ClassEntry, TypeTreeBlob, memory};
use crate::tree::{Build, WalkError, builds, generate_node};
use crate::{TpkError, file};

/// What a release root's walk resolved to.
type Outcome<T> = Result<Option<Arc<T>>, ProviderError>;

/// An outcome as a root keeps it: a kept error's heap is charged to the
/// provider's meter for as long as it is kept (crate docs, "Memory").
struct Kept<T> {
    outcome: Outcome<T>,
    _charge: Option<Charge>,
}

/// One release root that some class entry names: whether its tree is past
/// the provider's total ([`TpkProvider::MAX_TOTAL_ROWS`]), decided at load,
/// and its node list and tree, each computed once. UnityPy's `NODES_CACHE`
/// (`Tpk.py:17`, `:58-62`, `:86-87`) keys a tree by the entry's class
/// record, whose release root is all `generate_node` reads; so every entry
/// whose record UnityPy shares a tree for shares this root's too.
struct Root {
    id: u16,
    over: Option<LimitError>,
    rows: OnceLock<Kept<NodeList>>,
    tree: OnceLock<Kept<Tree>>,
}

/// A TPK's class trees, as UnityPy's `helpers/Tpk.py` builds them (crate
/// docs).
pub struct TpkProvider {
    blob: TypeTreeBlob,
    /// Every release root a class entry names, sorted by id, each once.
    roots: MeteredVec<Root>,
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

    /// The most rows the trees of every release root together may have,
    /// 2,097,152, which bounds what the provider builds and keeps however
    /// many classes are asked for. UnityPy 1.25.2's own TPK has 414,033
    /// over its 3,000 release roots (`tools/fixtures/tpk.py --survey`).
    /// Each root's rows are known at load, without walking it; in the order
    /// of their ids, a root whose tree would take the trees kept so far past
    /// this is refused with a [`LimitKind::Count`] limit (crate docs,
    /// "Limits"), whatever was asked before it.
    pub const MAX_TOTAL_ROWS: usize = 1 << 21;

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
        let roots = roots(&blob, limits, &meter).map_err(memory)?;
        Ok(Self {
            blob,
            roots,
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
    /// `ValueError` or `AssertionError` (crate docs). Every request whose
    /// picked entry names one release root gets the same `Arc`.
    pub fn node_list(
        &self,
        class_id: i32,
        version: &UnityVersion,
    ) -> Result<Option<Arc<NodeList>>, ProviderError> {
        match self.entry(class_id, version) {
            Some(root) => self.rows_of(root).clone(),
            None => Ok(None),
        }
    }

    /// `TPKTYPETREE.ClassInformation[class_id].getVersionedClass(version)`
    /// (`Tpk.py:49-51`), then `generate_node`'s assertion (`:64`): the
    /// picked entry's release root; `None` where UnityPy raises `KeyError`
    /// (no such class), `ValueError` (no entry at or below the version, or
    /// the class removed there) or `AssertionError` (no release root).
    fn entry(&self, class_id: i32, version: &UnityVersion) -> Option<&Root> {
        let entries = self.blob.classes.get(class_id)?;
        let at = get_item_for_version(version, entries)?;
        let id = entries.get(at)?.class?.release_root?;
        let roots = self.roots.as_slice();
        roots
            .binary_search_by_key(&id, |r| r.id)
            .ok()
            .and_then(|at| roots.get(at))
    }

    fn rows_of<'r>(&self, root: &'r Root) -> &'r Outcome<NodeList> {
        &get_or_publish(&root.rows, || self.kept(self.walk(root))).outcome
    }

    /// An outcome to keep, a failure's heap charged first. The meter is
    /// unlimited, so it refuses only where the allocator does, and the
    /// error is kept uncharged then.
    fn kept<T>(&self, outcome: Outcome<T>) -> Kept<T> {
        let charge = outcome.as_ref().err().and_then(|e| {
            let bytes = e.detail.len().saturating_add(2 * size_of::<usize>());
            let bytes = u64::try_from(bytes).unwrap_or(u64::MAX);
            self.meter.charge(heap_cost(bytes)).ok()
        });
        Kept {
            outcome,
            _charge: charge,
        }
    }

    /// `generate_node(class_info)` (`Tpk.py:58-88`) from one release root,
    /// unless its tree is past the provider's total.
    fn walk(&self, root: &Root) -> Outcome<NodeList> {
        if let Some(over) = root.over {
            return Err(limit_error(over));
        }
        let max_depth = usize::try_from(self.limits.max_tree_depth).unwrap_or(usize::MAX);
        let rows = generate_node(
            &self.blob,
            root.id,
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

/// Every release root a kept class entry names, sorted and each once, with
/// what [`builds`] finds its walk makes and the total's verdict: in the
/// order of their ids, a root whose tree would take the rows of the trees
/// before it past [`TpkProvider::MAX_TOTAL_ROWS`] is over, and the trees
/// after it are counted without it.
fn roots(
    blob: &TypeTreeBlob,
    limits: &Limits,
    meter: &Arc<Meter>,
) -> Result<MeteredVec<Root>, ChargeError> {
    let mut ids = MeteredVec::new(meter);
    for entry in blob.classes.kept_entries() {
        if let Some(id) = entry.class.and_then(|c| c.release_root) {
            ids.push(id)?;
        }
    }
    let ids = ids.as_mut_slice();
    ids.sort_unstable();
    let mut distinct = MeteredVec::new(meter);
    for (at, &id) in ids.iter().enumerate() {
        if at == 0 || ids.get(at - 1) != Some(&id) {
            distinct.push(id)?;
        }
    }
    let max_depth = usize::try_from(limits.max_tree_depth).unwrap_or(usize::MAX);
    let found = builds(
        blob,
        distinct.as_slice(),
        max_depth,
        TpkProvider::MAX_TREE_ROWS,
        meter,
    )?;
    let mut roots = MeteredVec::with_capacity(meter, distinct.len())?;
    let mut total = 0_usize;
    for (&id, build) in distinct.as_slice().iter().zip(found.as_slice()) {
        let mut over = None;
        if let Build::Rows(rows) = *build {
            let with = total.saturating_add(rows);
            if with > TpkProvider::MAX_TOTAL_ROWS {
                over = Some(LimitError {
                    kind: LimitKind::Count,
                    value: u64::try_from(with).unwrap_or(u64::MAX),
                    max: u64::try_from(TpkProvider::MAX_TOTAL_ROWS).unwrap_or(u64::MAX),
                });
            } else {
                total = with;
            }
        }
        roots.push(Root {
            id,
            over,
            rows: OnceLock::new(),
            tree: OnceLock::new(),
        })?;
    }
    Ok(roots)
}

fn error(e: &dyn fmt::Display) -> ProviderError {
    ProviderError {
        provider: TpkProvider::NAME,
        detail: e.to_string().into(),
        limit: None,
    }
}

/// A walk that crossed a bound, as a limit error.
fn limit(kind: LimitKind, value: usize, max: usize) -> ProviderError {
    limit_error(LimitError {
        kind,
        value: u64::try_from(value).unwrap_or(u64::MAX),
        max: u64::try_from(max).unwrap_or(u64::MAX),
    })
}

/// A bound, as the error that names it.
fn limit_error(limit: LimitError) -> ProviderError {
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

    /// [`TpkProvider::node_list`], compiled once per release root with
    /// [`Tree::compile`] on the provider's meter and kept: every request
    /// whose picked entry names one root gets the same `Arc` (UnityPy's
    /// `NODES_CACHE`, crate docs).
    fn tree(&self, req: &TreeRequest<'_>) -> Result<Option<Arc<Tree>>, ProviderError> {
        let Some(root) = self.entry(req.class_id, req.version) else {
            return Ok(None);
        };
        let compile = || {
            let Some(list) = self.rows_of(root).clone()? else {
                return Ok(None);
            };
            Tree::compile(list, &self.limits)
                .map(Some)
                .map_err(|e| match e {
                    TreeError::Limit(l) => limit_error(l),
                    e => error(&e),
                })
        };
        get_or_publish(&root.tree, || self.kept(compile()))
            .outcome
            .clone()
    }
}
