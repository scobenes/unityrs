// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/Tpk.py. See THIRD_PARTY_LICENSES.
//! [`TpkProvider`]: UnityPy's `get_typetree_node` (`helpers/Tpk.py:42-55`)
//! with `get_item_for_version` (`:472-481`) and its two caches, over a loaded
//! blob, as unityrs-core's [`TypeTreeProvider`] (crate docs).

use std::fmt;
use std::io::Read;
use std::mem::size_of;
use std::path::Path;
use std::sync::{Arc, OnceLock};

use unityrs_core::budget::{Reservation, Unbounded};
use unityrs_core::codec::Lzma;
use unityrs_core::limits::{Charge, ChargeError, Limits, Meter, MeteredVec, heap_cost};
use unityrs_core::once::get_or_publish;
use unityrs_core::provider::{NoTree, ProviderError, TreeRequest, TypeTreeProvider};
use unityrs_core::typetree::commonstring::CommonStrings;
use unityrs_core::typetree::tree::NameTable;
use unityrs_core::typetree::{NodeList, Tree, TreeError};
use unityrs_core::{LimitError, LimitKind, UnityVersion};

use crate::blob::{ClassEntry, TypeTreeBlob, memory};
use crate::tree::{WalkError, builds, generate_node};
use crate::{TpkError, file};

/// What a release root's walk resolved to.
type Outcome<T> = Result<Option<Arc<T>>, ProviderError>;

/// An outcome as a root keeps it: a kept error's heap is charged to the
/// provider's meter for as long as it is kept (crate docs, "Memory").
struct Kept<T> {
    outcome: Outcome<T>,
    _charge: Option<Charge>,
}

/// One release root that some class entry names: whether its walk is past
/// the provider's total ([`TpkProvider::MAX_TOTAL_ROWS`]), decided at load,
/// and its node list and tree, each computed once. UnityPy's `NODES_CACHE`
/// (`Tpk.py:17`, `:58-62`, `:86-87`) keys a tree by the entry's class
/// record, whose release root is all `generate_node` reads; so every entry
/// whose record UnityPy shares a tree for shares this root's too.
struct Root {
    id: u16,
    /// The rows the walks up to this one's would take, when that is past
    /// the total: the figure its walk's `Count` refusal names, made when
    /// the walk is asked for (round 4 of CR8: made at load, it was counted
    /// for every root past the total, asked for or not).
    over: Option<u64>,
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
    /// The names of the blob's strings, numbered once at load, so a tree's
    /// compile reads none of them (crate docs, "Limits").
    names: NameTable,
    limits: Limits,
    /// The provider's own meter, unlimited, its refusals labelled
    /// [`Reservation::Provider`] (crate docs, "Memory").
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

/// [`TpkProvider::MAX_DECODER_MEMORY`] holds the most an LZMA decode of a
/// payload within [`TpkProvider::MAX_DECODED_BYTES`] charges: its output,
/// allocated once at that bound, and a dictionary of at most twice it with
/// liblzma's state (the bound's docs).
#[allow(
    clippy::cast_possible_truncation,
    reason = "twice MAX_DECODED_BYTES, 32 MiB, fits a u32; From is not const"
)]
const _: () = assert!(
    heap_cost(TpkProvider::MAX_DECODED_BYTES)
        + Lzma::decoder_charge((2 * TpkProvider::MAX_DECODED_BYTES) as u32)
        <= TpkProvider::MAX_DECODER_MEMORY
);

impl TpkProvider {
    /// The most rows one class tree may have. UnityPy's own TPK's largest
    /// has 3,849 (`tools/fixtures/tpk.py --survey`); a tree past this is an
    /// error, which bounds the walk of a node that lists an ancestor.
    pub const MAX_TREE_ROWS: usize = 1 << 16;

    /// The most rows the walks of every release root together may build,
    /// 2,097,152, which bounds what the provider builds on any one thread,
    /// kept or not, and so what it keeps, however many classes are asked
    /// for (threads that ask for a root at once each walk it, crate docs,
    /// "Threads"); a walk the allocator refused is not kept, so each later
    /// request for its root walks it again, each walk at most one row past
    /// [`Self::MAX_TREE_ROWS`] (crate docs, "Memory"). UnityPy
    /// 1.25.2's own TPK has 414,033 over its 3,000 release roots
    /// (`tools/fixtures/tpk.py --survey`). What each root's walk builds is
    /// known at load, without walking it: its tree's rows, or, for a walk
    /// that fails, the most it builds before it fails (its tree's rows, at
    /// most one past [`Self::MAX_TREE_ROWS`]). In the order of their ids, a
    /// root whose walk would take the rows of the walks before it past this
    /// is refused, unwalked, with a [`LimitKind::Count`] limit (crate docs,
    /// "Limits"), whatever was asked before it.
    pub const MAX_TOTAL_ROWS: usize = 1 << 21;

    /// The largest TPK file the provider reads, 16 MiB. UnityPy 1.25.2's
    /// own is 187,318 bytes (`tools/fixtures/tpk.py --survey`).
    pub const MAX_FILE_BYTES: u64 = 16 << 20;

    /// The most a TPK's payload may decode to, 16 MiB. UnityPy 1.25.2's
    /// own decodes to 1,385,610 bytes (`tools/fixtures/tpk.py --survey`).
    pub const MAX_DECODED_BYTES: u64 = 16 << 20;

    /// The most the decoder may hold while it decodes, 64 MiB: its output
    /// and, for LZMA, a dictionary and liblzma's state. An LZMA payload's
    /// output is allocated once, at [`Self::MAX_DECODED_BYTES`], before the
    /// decoder starts, so it never grows or moves, whatever the header
    /// declares; unityrs-core's codec retries with a dictionary at most
    /// twice the output so far ([`unityrs_core::codec::Lzma`]), so at most
    /// twice [`Self::MAX_DECODED_BYTES`], charged with liblzma's state
    /// ([`unityrs_core::codec::Lzma::decoder_charge`]). Together that is
    /// 50,372,880 bytes, which a constant assertion beside this one checks
    /// against this bound, so a payload within the decoded bound never meets
    /// this one (review round 13: the output's room grew by doubling, to
    /// twice the payload for some declared sizes, and a valid TPK was
    /// refused; `tests/bounds.rs`
    /// `an_lzma_payload_at_the_decoded_bound_loads_whatever_its_header_declares`).
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
    /// read only that far. The file's bytes are read into a buffer charged
    /// to the provider's meter before it grows, and held only while the
    /// TPK loads.
    pub fn open_with(path: impl AsRef<Path>, limits: &Limits) -> Result<Self, TpkError> {
        let file = std::fs::File::open(path)?;
        let len = file.metadata()?.len();
        if len > Self::MAX_FILE_BYTES {
            return Err(too_large_file(len));
        }
        let meter = provider_meter();
        let bytes = read_charged(file, len, &meter)?;
        Self::load(bytes.as_slice(), limits, meter)
    }

    /// Loads a TPK from its bytes, as UnityPy's `TpkFile(stream).GetDataBlob()`
    /// reads them (`helpers/Tpk.py:36-39`; crate docs).
    pub fn from_bytes(bytes: &[u8], limits: &Limits) -> Result<Self, TpkError> {
        Self::load(bytes, limits, provider_meter())
    }

    /// [`Self::from_bytes`] on the provider's `meter`, which the bytes a
    /// path held are already charged to.
    fn load(bytes: &[u8], limits: &Limits, meter: Arc<Meter>) -> Result<Self, TpkError> {
        let len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        if len > Self::MAX_FILE_BYTES {
            return Err(too_large_file(len));
        }
        let payload = file::payload(bytes, &meter)?;
        let blob = TypeTreeBlob::read(&payload, &meter)?;
        drop(payload);
        let common = common_strings(&blob, &meter)?;
        let roots = roots(&blob, limits, &meter).map_err(memory)?;
        let names = NameTable::new(blob.strings.as_slice(), &meter).map_err(|e| match e {
            TreeError::Charge(c) => memory(c),
            e => TpkError::Malformed(e.to_string().into()),
        })?;
        Ok(Self {
            blob,
            roots,
            common,
            names,
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

    /// The classes the TPK holds, sorted, read from the provider's own table:
    /// nothing is allocated for them (review round 6: a `Vec` of up to
    /// [`Self::MAX_RECORDS`] ids was, outside the provider's meter).
    pub fn class_ids(&self) -> impl DoubleEndedIterator<Item = i32> + ExactSizeIterator + '_ {
        self.blob.classes.ids()
    }

    /// The TPK's version list, as stored (UnityPy's `Versions`).
    #[must_use]
    pub fn versions(&self) -> &[UnityVersion] {
        self.blob.versions.as_slice()
    }

    /// The node list UnityPy builds for `class_id` at `version`, before it
    /// is compiled: `Ok(None)` wherever UnityPy raises `KeyError`,
    /// `ValueError` or `AssertionError` (crate docs). Every request whose
    /// picked entry names one release root gets the same `Arc`; a walk the
    /// allocator refuses is not kept ([`TypeTreeProvider::tree`]).
    pub fn node_list(
        &self,
        class_id: i32,
        version: &UnityVersion,
    ) -> Result<Option<Arc<NodeList>>, ProviderError> {
        match self.entry(class_id, version) {
            Ok(root) => self.rows_of(root),
            Err(_) => Ok(None),
        }
    }

    /// `TPKTYPETREE.ClassInformation[class_id].getVersionedClass(version)`
    /// (`Tpk.py:49-51`), then `generate_node`'s assertion (`:64`): the
    /// picked entry's release root, or the exception UnityPy raises
    /// ([`NoTree`]): `KeyError` (no such class), `ValueError` (no entry at
    /// or below the version, or the class removed there) or
    /// `AssertionError` (no release root). Every root a kept entry names is
    /// in `roots` (built from them at load).
    fn entry(&self, class_id: i32, version: &UnityVersion) -> Result<&Root, NoTree> {
        let entries = self
            .blob
            .classes
            .get(class_id)
            .ok_or(NoTree::UnknownClass)?;
        let at = get_item_for_version(version, entries).ok_or(NoTree::NoEntry)?;
        let class = entries.get(at).and_then(|e| e.class);
        let id = class.ok_or(NoTree::NoEntry)?.release_root;
        let roots = self.roots.as_slice();
        id.and_then(|id| roots.binary_search_by_key(&id, |r| r.id).ok())
            .and_then(|at| roots.get(at))
            .ok_or(NoTree::NoReleaseRoot)
    }

    fn rows_of(&self, root: &Root) -> Outcome<NodeList> {
        match root.rows.get() {
            Some(kept) => kept.outcome.clone(),
            None => self.publish(&root.rows, self.walk(root)),
        }
    }

    /// `outcome`, published into `cell` unless it is a refusal of memory
    /// ([`ProviderError::memory`]): that depends on what else was held at
    /// that moment, not on the request, so it is returned unkept and the
    /// next ask computes again, as `unityrs_core::once` treats a transient
    /// error. Otherwise the first outcome published wins.
    fn publish<T>(&self, cell: &OnceLock<Kept<T>>, outcome: Outcome<T>) -> Outcome<T> {
        if matches!(&outcome, Err(e) if e.memory.is_some()) {
            return outcome;
        }
        get_or_publish(cell, || self.kept(outcome)).outcome.clone()
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
    /// unless its walk is past the provider's total.
    fn walk(&self, root: &Root) -> Outcome<NodeList> {
        if let Some(rows) = root.over {
            return Err(limit_error(LimitError::refused(
                LimitKind::Count,
                rows,
                u64::try_from(Self::MAX_TOTAL_ROWS).unwrap_or(u64::MAX),
            )));
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
            WalkError::Memory(e) => charge_error(&e),
        })?;
        // `TypeTreeNode.from_list(nodes)` (`Tpk.py:86`).
        NodeList::from_rows(rows)
            .map(|list| Some(Arc::new(list)))
            .map_err(|e| match &e {
                TreeError::Charge(c) => charge_error(c),
                e => error(e),
            })
    }
}

/// The provider's own meter: unlimited, so it refuses only where the
/// allocator does, its refusals labelled [`Reservation::Provider`] (crate
/// docs, "Memory").
fn provider_meter() -> Arc<Meter> {
    Meter::new(u64::MAX, Arc::new(Unbounded), Reservation::Provider)
}

/// A file past [`TpkProvider::MAX_FILE_BYTES`].
fn too_large_file(value: u64) -> TpkError {
    TpkError::TooLarge {
        what: "file",
        value,
        max: TpkProvider::MAX_FILE_BYTES,
    }
}

/// The bytes of `file`, whose metadata says `len` (at most
/// [`TpkProvider::MAX_FILE_BYTES`]), read into a buffer charged to `meter`
/// before each growth: room for `len` first, then, for a file that grows
/// while it is read or reports no length, growths up to one byte past the
/// bound, which is refused (round 4: the buffer grew uncharged, before the
/// meter existed).
fn read_charged(
    file: std::fs::File,
    len: u64,
    meter: &Arc<Meter>,
) -> Result<MeteredVec<u8>, TpkError> {
    let mut bytes =
        MeteredVec::with_capacity(meter, usize::try_from(len).unwrap_or(0)).map_err(memory)?;
    let mut file = file.take(TpkProvider::MAX_FILE_BYTES + 1);
    let mut chunk = [0_u8; 16 << 10];
    loop {
        let n = match file.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        };
        bytes
            .extend_from_slice(chunk.get(..n).unwrap_or_default())
            .map_err(memory)?;
    }
    let read = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    if read > TpkProvider::MAX_FILE_BYTES {
        return Err(too_large_file(read));
    }
    Ok(bytes)
}

/// Every release root a kept class entry names, sorted and each once, with
/// what [`builds`] finds its walk makes and the total's verdict: in the
/// order of their ids, a root whose walk would take the rows the walks
/// before it build past [`TpkProvider::MAX_TOTAL_ROWS`] is over, and the
/// walks after it are counted without it. A walk that fails counts the most
/// rows it builds before it fails ([`crate::tree::Build::work`]), so the
/// work of every walk, kept or not, is within the total.
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
        let with = total.saturating_add(build.work());
        if with > TpkProvider::MAX_TOTAL_ROWS {
            over = Some(u64::try_from(with).unwrap_or(u64::MAX));
        } else {
            total = with;
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
        memory: None,
    }
}

/// A charge refused: by the allocator (the meter is unlimited), a refusal
/// of memory ([`ProviderError::memory`]), which the provider never keeps.
fn charge_error(e: &ChargeError) -> ProviderError {
    match e {
        ChargeError::Budget(b) => ProviderError {
            memory: Some(b.clone()),
            ..error(e)
        },
        ChargeError::Limit(_) => error(e),
    }
}

/// A walk that crossed a bound, as a limit error.
fn limit(kind: LimitKind, value: usize, max: usize) -> ProviderError {
    limit_error(LimitError::refused(
        kind,
        u64::try_from(value).unwrap_or(u64::MAX),
        u64::try_from(max).unwrap_or(u64::MAX),
    ))
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
/// string shared with the rows that name it. The table, one entry per index
/// (as many as [`TpkProvider::MAX_RECORDS`]), is charged to the provider's
/// `meter`, a growth the allocator refuses an error. An index past the
/// string buffer, an `IndexError` in every blob parse in UnityPy, is
/// refused here.
fn common_strings(blob: &TypeTreeBlob, meter: &Arc<Meter>) -> Result<CommonStrings, TpkError> {
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
    CommonStrings::new(
        indices
            .iter()
            .filter_map(|&i| strings.get(usize::from(i)).cloned()),
        meter,
    )
    .map_err(memory)
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
    /// [`Tree::compile_with_names`] (the blob's strings' names, numbered at
    /// load: the compile reads none, and its tables are the root's rows'
    /// size) on the provider's meter and kept: every request
    /// whose picked entry names one root gets the same `Arc` (UnityPy's
    /// `NODES_CACHE`, crate docs). A walk or compile the allocator refuses is
    /// that request's error, its [`ProviderError::memory`] set, and is not
    /// kept: after a refused walk the next request walks and compiles again,
    /// after a refused compile it compiles the rows the walk kept (review
    /// round 6: this said it walked again either way).
    fn tree(&self, req: &TreeRequest<'_>) -> Result<Option<Arc<Tree>>, ProviderError> {
        let Ok(root) = self.entry(req.class_id, req.version) else {
            return Ok(None);
        };
        if let Some(kept) = root.tree.get() {
            return kept.outcome.clone();
        }
        let compile = || {
            let Some(list) = self.rows_of(root)? else {
                return Ok(None);
            };
            Tree::compile_with_names(list, &self.limits, &self.names)
                .map(Some)
                .map_err(|e| match e {
                    TreeError::Limit(l) => limit_error(l),
                    TreeError::Charge(c) => charge_error(&c),
                    e => error(&e),
                })
        };
        self.publish(&root.tree, compile())
    }

    /// The exception UnityPy raises where [`Self::tree`] answers `None`
    /// ([`TpkProvider::node_list`]'s cases). With no version (a file below
    /// format 7), `ClassInformation[class_id]` raises `KeyError` first,
    /// then `get_item_for_version` compares `None` with the class's first
    /// entry, `NotImplementedError`, or with no entry raises `ValueError`.
    fn why_none(&self, class_id: i32, version: Option<&UnityVersion>) -> Option<NoTree> {
        match version {
            Some(v) => self.entry(class_id, v).err(),
            None => Some(match self.blob.classes.get(class_id) {
                None => NoTree::UnknownClass,
                Some([]) => NoTree::NoEntry,
                Some(_) => NoTree::NoVersion,
            }),
        }
    }
}

#[cfg(test)]
mod memory_tests;
#[cfg(test)]
mod tests;
