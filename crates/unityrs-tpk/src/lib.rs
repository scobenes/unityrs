// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/Tpk.py. See THIRD_PARTY_LICENSES.
//! A TPK (a type-tree package, AssetRipper's format, which UnityPy ships as
//! `resources/lzma.tpk`) loaded from a path, as unityrs-core's
//! [`TypeTreeProvider`] for objects whose file stores no typetree
//! (`docs/plans/core.md` section 3.12).
//!
//! **What UnityPy does** (`helpers/Tpk.py`), and so what [`TpkProvider`] does:
//! - `get_typetree_node(class_id, version)` (`:42-55`) looks the class up
//!   (`ClassInformation[class_id]`: a `KeyError` for a class the TPK does not
//!   hold), picks its entry for the version with `get_item_for_version`
//!   (`:472-481`: a forward scan over the entries in their stored order that
//!   keeps the last entry whose version is at or below the request and stops
//!   at the first above it; a `ValueError` when it keeps nothing or keeps an
//!   entry that is `None`, the class removed at that version), and builds
//!   that entry's tree;
//! - `generate_node` (`:58-88`) asserts the entry has a release root
//!   (`AssertionError` otherwise; an editor root does not count) and walks
//!   the node buffer from it, pre-order, each node's level its parent's plus
//!   one, each row's `m_Index` its position, `m_TypeFlags` left unset; one
//!   tree per entry (`NODES_CACHE`), shared by every version that picks it.
//!
//! Each of the three exceptions is "no tree": [`TypeTreeProvider::tree`]
//! answers `Ok(None)`, which the dump records as `no_typetree`. Anything else
//! UnityPy's walk raises (a node or string id past its table: `IndexError`)
//! is a [`ProviderError`]. Versions compare as UnityPy's integers do
//! (`UnityVersion` is an `int`): a TPK's versions are its stored `u64`s
//! ([`UnityVersion::from_packed`]), and the request's is the file's.
//!
//! **Limits, where UnityPy has none.** The walk stops with an error at a
//! level of [`Limits::max_tree_depth`] or past [`TpkProvider::MAX_TREE_ROWS`]
//! rows (the rows still to walk counted), so a node that lists itself or an
//! ancestor, on which UnityPy's walk never ends, is an error; a tree deeper
//! than the limit, which UnityPy builds, is one too (a safety divergence,
//! plan section 2.4's F3).
//!
//! **Memory.** The TPK is the provider's, not an input's: its rows and trees
//! are built on the provider's own unlimited meter, once per entry, and kept
//! (plan section 3.15's constant). Each tree is computed with no lock held
//! and published through [`unityrs_core::once::get_or_publish`].
//!
//! The parser is rabex 0.2.1's, vendored as `unityrs-rabex` with the patches
//! in its `UPSTREAM.md`; this crate uses only its TPK reader and the F4
//! accessor (`class_entries`, `release_rows`), never rabex's own version,
//! class id or node types (`clippy.toml` here disallows them).

#![forbid(unsafe_code)]
// No panics on a TPK's contents (plan section 3.15). Tests may opt out.
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::todo,
        clippy::unimplemented,
        clippy::unreachable,
        clippy::indexing_slicing
    )
)]

use std::collections::HashMap;
use std::fmt;
use std::path::Path;
use std::sync::{Arc, OnceLock};

use rabex::tpk::{TpkFile, TpkNodeRow, TpkTypeTreeBlob, UnityClass};
use unityrs_core::limits::{Limits, Meter, MeteredVec};
use unityrs_core::once::get_or_publish;
use unityrs_core::provider::{ProviderError, TreeRequest, TypeTreeProvider};
use unityrs_core::typetree::{NodeList, NodeRow, Tree};
use unityrs_core::{RawStr, UnityVersion};

/// Asserts at compile time that `T` is `Send + Sync` (plan section 3.16).
const fn send_sync<T: Send + Sync + ?Sized>() {}

/// Why a TPK did not load.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TpkError {
    /// The file could not be read.
    #[error("reading the TPK: {0}")]
    Io(#[from] std::io::Error),
    /// rabex's reader refused it (a bad header, a codec failure, a count or
    /// a string past the end of the data, invalid UTF-8).
    #[error("parsing the TPK: {0}")]
    Parse(#[from] rabex::tpk::Error),
    /// It holds something other than type trees (UnityPy's `init` asserts a
    /// `TpkTypeTreeBlob`).
    #[error("the TPK holds no type trees")]
    NotTypeTrees,
}

const _: () = send_sync::<TpkError>();

/// What a picked entry resolved to, kept once per entry.
type Outcome<T> = Result<Option<Arc<T>>, ProviderError>;

/// One entry of a class: its version, and its class or `None` (the class
/// removed at that version), with the node list and the tree it gives,
/// each computed once.
struct Entry {
    version: UnityVersion,
    class: Option<UnityClass>,
    rows: OnceLock<Outcome<NodeList>>,
    tree: OnceLock<Outcome<Tree>>,
}

/// A TPK's class trees, as UnityPy's `helpers/Tpk.py` builds them (module
/// docs).
pub struct TpkProvider {
    blob: TpkTypeTreeBlob,
    /// Every class's entries, in their stored order.
    classes: HashMap<i32, Box<[Entry]>>,
    /// The blob's version list, as stored.
    versions: Box<[UnityVersion]>,
    /// The string buffer, each string once, shared by the rows that name it.
    strings: HashMap<Box<str>, RawStr>,
    limits: Limits,
    meter: Arc<Meter>,
}

const _: () = send_sync::<TpkProvider>();

impl fmt::Debug for TpkProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TpkProvider")
            .field("classes", &self.classes.len())
            .field("versions", &self.versions.len())
            .finish_non_exhaustive()
    }
}

/// UnityPy's `get_item_for_version` (`helpers/Tpk.py:472-481`): the last
/// entry at or below `version`, scanning forward and stopping at the first
/// above it. `None` entries count; the caller treats a pick of one as none.
fn pick<'e>(entries: &'e [Entry], version: &UnityVersion) -> Option<&'e Entry> {
    let mut picked = None;
    for entry in entries {
        if *version >= entry.version {
            picked = Some(entry);
        } else {
            break;
        }
    }
    picked
}

impl TpkProvider {
    /// The most rows one class tree may have. UnityPy's own TPK's largest
    /// has 3,849 (`tools/fixtures/tpk.py --survey`); a tree past this is an
    /// error, which bounds the walk of a node that lists an ancestor.
    pub const MAX_TREE_ROWS: usize = 1 << 16;

    /// The provider's name, the dump's `l2.source`.
    pub const NAME: &'static str = "tpk";

    /// Loads the TPK at `path`, with the default [`Limits`].
    pub fn open(path: impl AsRef<Path>) -> Result<Self, TpkError> {
        Self::open_with(path, &Limits::default())
    }

    /// Loads the TPK at `path`; trees deeper than `limits.max_tree_depth`
    /// are errors.
    pub fn open_with(path: impl AsRef<Path>, limits: &Limits) -> Result<Self, TpkError> {
        Self::from_bytes(&std::fs::read(path)?, limits)
    }

    /// Loads a TPK from its bytes (as UnityPy's `TpkFile(stream).GetDataBlob()`
    /// reads them, `helpers/Tpk.py:36-39`).
    pub fn from_bytes(bytes: &[u8], limits: &Limits) -> Result<Self, TpkError> {
        let file = TpkFile::from_reader(&mut &bytes[..])?;
        let blob = file.as_type_tree()?.ok_or(TpkError::NotTypeTrees)?;
        let mut classes = HashMap::new();
        for id in blob.class_ids() {
            let entries = blob.class_entries(id).into_iter().flatten();
            let entries = entries
                .map(|(version, class)| Entry {
                    version: UnityVersion::from_packed(version),
                    class: class.cloned(),
                    rows: OnceLock::new(),
                    tree: OnceLock::new(),
                })
                .collect();
            classes.insert(id, entries);
        }
        let versions = blob
            .versions
            .iter()
            .map(|v| UnityVersion::from_packed(v.0))
            .collect();
        let strings = blob
            .string_buffer
            .iter()
            .map(|s| (Box::from(s.as_str()), RawStr::from(s.as_str())))
            .collect();
        Ok(Self {
            blob,
            classes,
            versions,
            strings,
            limits: *limits,
            meter: Meter::unlimited(),
        })
    }

    /// The number of classes the TPK holds (UnityPy's
    /// `len(ClassInformation)`; a class listed twice counts once, its last
    /// listing kept, as a Python dict keeps it).
    #[must_use]
    pub fn class_count(&self) -> usize {
        self.classes.len()
    }

    /// The classes the TPK holds, sorted.
    #[must_use]
    pub fn class_ids(&self) -> Vec<i32> {
        let mut ids: Vec<i32> = self.classes.keys().copied().collect();
        ids.sort_unstable();
        ids
    }

    /// The TPK's version list, as stored (UnityPy's `Versions`).
    #[must_use]
    pub fn versions(&self) -> &[UnityVersion] {
        &self.versions
    }

    /// The node list UnityPy builds for `class_id` at `version`, before it
    /// is compiled: `Ok(None)` wherever UnityPy raises `KeyError`,
    /// `ValueError` or `AssertionError` (module docs). Every version that
    /// picks one entry gets the same `Arc`.
    pub fn node_list(
        &self,
        class_id: i32,
        version: &UnityVersion,
    ) -> Result<Option<Arc<NodeList>>, ProviderError> {
        let Some(entry) = self.entry(class_id, version) else {
            return Ok(None);
        };
        self.rows_of(entry).clone()
    }

    /// The picked entry when it has a class (`Tpk.py:49-51`).
    fn entry(&self, class_id: i32, version: &UnityVersion) -> Option<&Entry> {
        let entries = self.classes.get(&class_id)?;
        pick(entries, version).filter(|e| e.class.is_some())
    }

    fn rows_of<'e>(&self, entry: &'e Entry) -> &'e Outcome<NodeList> {
        get_or_publish(&entry.rows, || self.walk(entry.class.as_ref()))
    }

    /// `generate_node` (`Tpk.py:58-88`) for one entry's class.
    fn walk(&self, class: Option<&UnityClass>) -> Outcome<NodeList> {
        let Some(class) = class else {
            return Ok(None);
        };
        let max_depth = usize::try_from(self.limits.max_tree_depth).unwrap_or(usize::MAX);
        let rows = self
            .blob
            .release_rows(class, max_depth, Self::MAX_TREE_ROWS)
            .map_err(|e| error(&e))?;
        let Some(rows) = rows else {
            // `assert class_info.ReleaseRootNode is not None` (`Tpk.py:64`).
            return Ok(None);
        };
        let rows: Vec<NodeRow> = rows
            .iter()
            .enumerate()
            .map(|(index, row)| self.node_row(index, row))
            .collect();
        let rows = MeteredVec::from_vec(rows, &self.meter).map_err(|e| error(&e))?;
        NodeList::from_rows(rows)
            .map(|list| Some(Arc::new(list)))
            .map_err(|e| error(&e))
    }

    /// One row as UnityPy's `TypeTreeNode(...)` call makes it
    /// (`Tpk.py:73-83`): `m_Index` the row's position, `m_TypeFlags`,
    /// `m_VariableCount` and `m_RefTypeHash` unset.
    fn node_row(&self, index: usize, row: &TpkNodeRow<'_>) -> NodeRow {
        let string = |s: &str| {
            self.strings
                .get(s)
                .cloned()
                .unwrap_or_else(|| RawStr::from(s))
        };
        NodeRow {
            level: row.level,
            ty: string(row.type_name),
            name: string(row.name),
            byte_size: row.byte_size,
            version: i32::from(row.version),
            type_flags: None,
            variable_count: None,
            // Below `MAX_TREE_ROWS`.
            index: Some(i32::try_from(index).unwrap_or(i32::MAX)),
            // UnityPy keeps the stored `u32` (`Tpk.py:361`), zero-extended.
            meta_flag: Some(i64::from(row.meta_flag)),
            ref_type_hash: None,
        }
    }
}

fn error(e: &dyn fmt::Display) -> ProviderError {
    ProviderError {
        provider: TpkProvider::NAME,
        detail: e.to_string().into_boxed_str(),
    }
}

impl TypeTreeProvider for TpkProvider {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    /// [`TpkProvider::node_list`], compiled once per entry with
    /// [`Tree::compile`] on the provider's meter and kept: every request
    /// that picks one entry gets the same `Arc` (UnityPy's `NODES_CACHE`).
    fn tree(&self, req: &TreeRequest<'_>) -> Result<Option<Arc<Tree>>, ProviderError> {
        let Some(entry) = self.entry(req.class_id, req.version) else {
            return Ok(None);
        };
        get_or_publish(&entry.tree, || {
            let Some(list) = self.rows_of(entry).clone()? else {
                return Ok(None);
            };
            Tree::compile(list, &self.limits)
                .map(Some)
                .map_err(|e| error(&e))
        })
        .clone()
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    /// Every public type is asserted `Send + Sync` by its full path (plan
    /// 3.16), as in unityrs-core.
    #[test]
    fn every_public_type_is_asserted() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let missing = unityrs_testkit::public_types::unasserted(&src);
        assert!(missing.is_empty(), "not asserted Send + Sync: {missing:?}");
    }

    /// Plan 4.1: no thread, no wait; a provider is called from a caller's
    /// thread, maybe a pool's.
    #[test]
    fn the_provider_starts_no_thread_and_never_waits() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let found = unityrs_testkit::waits::waits(&src);
        assert!(found.is_empty(), "waits or starts a thread: {found:?}");
    }

    /// Plan 3.16: each entry's rows and tree are computed outside their
    /// cells and published through `once::get_or_publish`.
    #[test]
    fn every_cell_is_filled_through_the_once_helper() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let found = unityrs_testkit::waits::cell_fills(&src);
        assert!(
            found.is_empty(),
            "a cell filled outside the once helper: {found:?}"
        );
    }
}
