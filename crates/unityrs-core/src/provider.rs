//! Typetree providers for stripped files, keyed by `(class, version)`.
//!
//! UnityPy's order (`files/ObjectReader.py:285-307`): an explicit `nodes=`
//! tree; else the type record's own tree; else the class tree for
//! `(class_id, version)` from the TPK (`helpers/Tpk.py:42-55`), and, only
//! for that TPK tree (`generate_monobehaviour_node` is called inside
//! `if not node:`, `ObjectReader.py:296-304`), the script generator's tree
//! when its root is `MonoBehaviour` and the generator answers. An embedded
//! or explicit `MonoBehaviour` tree is never replaced. Only a `ValueError` is
//! swallowed there; the script lookup dereferences `m_Script` through the
//! Environment, so it lives in [`crate::env::Environment::object_tree`], and
//! runs only when a provider [`TypeTreeProvider::accepts_scripts`]. The cache key is the class and the packed version
//! integer, as UnityPy's `CLASSES_CACHE` is (`Tpk.py:16`, `:44`): rabex keys
//! by class alone, so the first version to ask fixes a class's tree for every
//! later version (`docs/spikes/rabex.md` section 5).
//!
//! The TPK itself is not here: `unityrs-tpk` (package A) loads one from a
//! path and implements [`TypeTreeProvider`]. With no provider, a stripped
//! object is `no_typetree`.
//!
//! **The cache.** [`ProviderChain`] caches *answers only*, as UnityPy does
//! (`Tpk.py:42-54` raises before `CLASSES_CACHE[key] = node` on a miss): a
//! miss is asked again next time, so input-chosen keys that find nothing
//! leave nothing behind. The key's class and version are still the input's
//! choice, and one context's chain outlives every input it reads, so the
//! cache is bounded in entries and bytes ([`ProviderChain::with_cache_bound`]);
//! when full it stops adding, which changes no answer. A lookup reads the
//! cache with a key *borrowed* from the request, so a hit (or a miss) copies
//! nothing, however long the request's script names are. An answer is kept
//! only when the cache has room for it, and that is decided from the
//! borrowed request (its key's size, whether it is already cached, whether
//! the cache is full) before anything is copied: only an answer that is
//! kept copies its key's strings, so a key over the byte bound, or any key
//! once the cache is full, costs no copy, a cached key never keeps an
//! input's buffer alive, and the copies count against the cache's byte
//! bound. Each entry is counted at four buckets of the table, which covers
//! the table at rest and while it doubles (`limits::HASH_TABLE_SHARES`).
//! A provider is caller code: the chain calls it with no lock held, so a
//! provider may itself call [`ProviderChain::lookup`].
//!
//! Owner: the scaffold (implemented). Package D adds tests and may add
//! providers; `unityrs-tpk` (package A) implements the TPK one.

use std::borrow::Borrow;
use std::collections::HashMap;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::mem::size_of;
use std::sync::{Arc, RwLock};

use bytes::Bytes;

use crate::limits::{HASH_TABLE_SHARES, heap_cost};
use crate::raw::RawStr;
use crate::typetree::Tree;
use crate::typetree::commonstring::CommonStrings;
use crate::version::{Packed, UnityVersion};

/// A `MonoBehaviour`'s script, for script-tree providers.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ScriptIdent {
    /// `m_AssemblyName`.
    pub assembly: RawStr,
    /// `m_Namespace`.
    pub namespace: RawStr,
    /// `m_ClassName`.
    pub class: RawStr,
}

/// What a provider is asked for.
#[derive(Clone, Copy, Debug)]
pub struct TreeRequest<'a> {
    /// The object's class.
    pub class_id: i32,
    /// The file's version (chain 2).
    pub version: &'a UnityVersion,
    /// For a `MonoBehaviour` whose `m_Script` resolved: its script.
    pub script: Option<&'a ScriptIdent>,
}

/// A provider failure.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{provider}: {detail}")]
pub struct ProviderError {
    /// The provider's name.
    pub provider: &'static str,
    /// What failed. Shared, so a clone allocates nothing: a provider keeps
    /// each entry's failure and hands every later request a clone of it.
    pub detail: Arc<str>,
    /// The bound the failure crossed, when it was one (a TPK tree deeper
    /// than [`crate::limits::Limits::max_tree_depth`] is
    /// [`crate::LimitKind::TreeDepth`]); `detail` is then its message.
    pub limit: Option<crate::LimitError>,
    /// The refusal, when the failure was memory the provider was refused (by
    /// the allocator, or a budget); `detail` is then its message. It depends
    /// on what else was held at that moment, not on the request, so a
    /// provider never keeps it as the request's answer (the next ask tries
    /// again), and a reader treats it as the caller's out-of-memory, never
    /// as a missing tree (the canonical dump's `crash` record, the oracle's
    /// `MemoryError`).
    pub memory: Option<crate::budget::BudgetExceeded>,
}

/// A source of class (or script) trees.
pub trait TypeTreeProvider: Send + Sync + fmt::Debug {
    /// A short name; the dump's `l2.source` is `"tpk"` for the TPK provider.
    fn name(&self) -> &'static str;
    /// The tree for `req`, or `Ok(None)` wherever UnityPy builds no node
    /// list, which the caller turns into `no_typetree`. For a TPK that is
    /// four cases: a class the TPK does not hold (`KeyError`); a version
    /// below the class's first entry, and a picked entry that is `None` (the
    /// class removed at that version), both `ValueError`
    /// (`helpers/Tpk.py:472-481`); and a picked entry with no release root
    /// (`AssertionError`, `Tpk.py:64`). The pick is UnityPy's floor over
    /// **every** entry in its stored order, `None` and release-less entries
    /// included: a provider never steps back past one of them to an earlier
    /// entry that has a tree.
    /// Every request that maps to one tree of the provider's data gets the
    /// same `Arc` (UnityPy's `NODES_CACHE`, `Tpk.py:58-88`, which shares one
    /// tree among the TPK class entries of one class record; the TPK
    /// provider shares one among the entries of one release root), so the
    /// trees a chain caches are bounded by the provider's data, not by the
    /// versions an input names.
    fn tree(&self, req: &TreeRequest<'_>) -> Result<Option<Arc<Tree>>, ProviderError>;
    /// Whether this provider answers script requests (`TreeRequest::script`):
    /// UnityPy's `env.typetree_generator`. Only then does the Environment
    /// dereference a `MonoBehaviour`'s `m_Script` (P3; the TPK provider does not).
    fn accepts_scripts(&self) -> bool {
        false
    }
    /// The common strings blob typetrees index (UnityPy's
    /// `get_common_strings()`, its TPK's table), when this provider has them:
    /// a TPK provider does.
    fn common_strings(&self) -> Option<&CommonStrings> {
        None
    }
}

/// Where an object's tree came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TreeSource {
    /// The type record's own tree (`"embedded"`).
    Embedded,
    /// A provider, by name (`"tpk"` for the TPK).
    Provider(&'static str),
    /// A `nodes=` override.
    Explicit,
}

/// A resolved tree.
#[derive(Clone, Debug)]
pub struct TreeRef {
    /// The tree.
    pub tree: Arc<Tree>,
    /// Its source.
    pub source: TreeSource,
}

/// A cache key, owned or borrowed from a request: UnityPy's
/// `(class, packed version)` plus the script.
trait KeyView {
    fn class_id(&self) -> i32;
    fn packed(&self) -> &Packed;
    fn script(&self) -> Option<[&[u8]; 3]>;
}

impl Hash for dyn KeyView + '_ {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.class_id().hash(h);
        self.packed().hash(h);
        self.script().hash(h);
    }
}

impl PartialEq for dyn KeyView + '_ {
    fn eq(&self, other: &Self) -> bool {
        self.class_id() == other.class_id()
            && self.packed() == other.packed()
            && self.script() == other.script()
    }
}

impl Eq for dyn KeyView + '_ {}

/// The key an answer is kept under: its strings copied out of the input's
/// buffer.
struct CacheKey {
    class_id: i32,
    packed: Packed,
    script: Option<[Bytes; 3]>,
}

#[cfg(test)]
thread_local! {
    /// Keys copied on this thread (the tests count them).
    static KEY_COPIES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

impl CacheKey {
    fn copy_of(req: &TreeRequest<'_>) -> Self {
        #[cfg(test)]
        KEY_COPIES.with(|n| n.set(n.get() + 1));
        let own = |s: &RawStr| Bytes::copy_from_slice(s.as_bytes());
        Self {
            class_id: req.class_id,
            packed: req.version.packed().clone(),
            script: req
                .script
                .map(|s| [own(&s.assembly), own(&s.namespace), own(&s.class)]),
        }
    }
}

impl KeyView for CacheKey {
    fn class_id(&self) -> i32 {
        self.class_id
    }
    fn packed(&self) -> &Packed {
        &self.packed
    }
    fn script(&self) -> Option<[&[u8]; 3]> {
        self.script
            .as_ref()
            .map(|[a, n, c]| [&a[..], &n[..], &c[..]])
    }
}

impl Hash for CacheKey {
    fn hash<H: Hasher>(&self, h: &mut H) {
        (self as &dyn KeyView).hash(h);
    }
}

impl PartialEq for CacheKey {
    fn eq(&self, other: &Self) -> bool {
        (self as &dyn KeyView) == (other as &dyn KeyView)
    }
}

impl Eq for CacheKey {}

impl<'a> Borrow<dyn KeyView + 'a> for CacheKey {
    fn borrow(&self) -> &(dyn KeyView + 'a) {
        self
    }
}

/// A request's key, borrowed: looking it up copies nothing.
struct RequestKey<'r, 'a>(&'r TreeRequest<'a>);

impl KeyView for RequestKey<'_, '_> {
    fn class_id(&self) -> i32 {
        self.0.class_id
    }
    fn packed(&self) -> &Packed {
        self.0.version.packed()
    }
    fn script(&self) -> Option<[&[u8]; 3]> {
        self.0.script.map(|s| {
            [
                s.assembly.as_bytes(),
                s.namespace.as_bytes(),
                s.class.as_bytes(),
            ]
        })
    }
}

type CacheValue = (Arc<Tree>, &'static str);

#[derive(Default)]
struct Cache {
    map: HashMap<CacheKey, CacheValue>,
    bytes: u64,
}

/// Providers in order, with a bounded cache of their answers keyed by
/// `(class, packed version, script)` (module docs).
pub struct ProviderChain {
    providers: Vec<Arc<dyn TypeTreeProvider>>,
    cache: RwLock<Cache>,
    max_entries: usize,
    max_bytes: u64,
}

/// One bucket of the cache's table: a key, a value and its control byte.
const BUCKET: u64 = (size_of::<CacheKey>() + size_of::<CacheValue>() + 1) as u64;

/// What keeping the answer to `req` costs, computed from the borrowed
/// request before anything is copied: [`HASH_TABLE_SHARES`] buckets of the
/// table (at rest and while it doubles), plus the heap its key would own
/// (each copied string and a big version's limbs, at [`heap_cost`]).
fn entry_cost(req: &TreeRequest<'_>) -> u64 {
    let len = |n: usize| heap_cost(u64::try_from(n).unwrap_or(u64::MAX));
    let strings = req.script.map_or(0, |s| {
        len(s.assembly.len())
            .saturating_add(len(s.namespace.len()))
            .saturating_add(len(s.class.len()))
    });
    (HASH_TABLE_SHARES * BUCKET)
        .saturating_add(strings)
        .saturating_add(len(req.version.packed().heap_bytes()))
}

impl ProviderChain {
    /// The cache's default bound in entries: far more than a TPK has
    /// classes at the versions one run meets.
    pub const DEFAULT_CACHE_ENTRIES: usize = 1 << 16;
    /// The cache's default bound in bytes.
    pub const DEFAULT_CACHE_BYTES: u64 = 64 << 20;

    /// No providers: stripped objects are `no_typetree`.
    #[must_use]
    pub fn empty() -> Self {
        Self::new(Vec::new())
    }

    /// Providers asked in order; the first answer wins. The cache has the
    /// default bounds.
    #[must_use]
    pub fn new(providers: Vec<Arc<dyn TypeTreeProvider>>) -> Self {
        Self::with_cache_bound(
            providers,
            Self::DEFAULT_CACHE_ENTRIES,
            Self::DEFAULT_CACHE_BYTES,
        )
    }

    /// Providers asked in order, with a cache of at most `entries` answers
    /// and `bytes` bytes (each entry costs its slot, at the table's worst load,
    /// plus the heap its key owns).
    #[must_use]
    pub fn with_cache_bound(
        providers: Vec<Arc<dyn TypeTreeProvider>>,
        entries: usize,
        bytes: u64,
    ) -> Self {
        Self {
            providers,
            cache: RwLock::new(Cache::default()),
            max_entries: entries,
            max_bytes: bytes,
        }
    }

    /// Whether the chain has no provider.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }

    /// Whether a provider answers script requests (UnityPy has an
    /// `env.typetree_generator`): only then does
    /// [`crate::env::Environment::object_tree`] dereference `m_Script`.
    #[must_use]
    pub fn accepts_scripts(&self) -> bool {
        self.providers.iter().any(|p| p.accepts_scripts())
    }

    /// The common strings of the first provider that has them, which blob
    /// typetrees read in place of [`crate::typetree::commonstring::COMMON_STRINGS`]
    /// (UnityPy reads its TPK's).
    #[must_use]
    pub fn common_strings(&self) -> Option<&CommonStrings> {
        self.providers.iter().find_map(|p| p.common_strings())
    }

    /// Answers cached now.
    #[must_use]
    pub fn cached(&self) -> usize {
        self.cache.read().map_or(0, |c| c.map.len())
    }

    /// The first provider's answer for `req`. An answer is cached while the
    /// cache has room; a miss is not cached. Providers are asked with no lock
    /// held. A script request asks only the providers that
    /// [`TypeTreeProvider::accepts_scripts`]: UnityPy asks its
    /// `typetree_generator` alone for a script's tree
    /// (`files/ObjectReader.py:346-350`), never the TPK, whose class tree is
    /// the one the request would replace.
    pub fn lookup(&self, req: &TreeRequest<'_>) -> Result<Option<TreeRef>, ProviderError> {
        let hit = self
            .cache
            .read()
            .ok()
            .and_then(|c| c.map.get(&RequestKey(req) as &dyn KeyView).cloned());
        if let Some((tree, name)) = hit {
            return Ok(Some(TreeRef {
                tree,
                source: TreeSource::Provider(name),
            }));
        }
        for provider in &self.providers {
            if req.script.is_some() && !provider.accepts_scripts() {
                continue;
            }
            if let Some(tree) = provider.tree(req)? {
                let name = provider.name();
                self.publish(req, (tree.clone(), name));
                return Ok(Some(TreeRef {
                    tree,
                    source: TreeSource::Provider(name),
                }));
            }
        }
        Ok(None)
    }

    fn publish(&self, req: &TreeRequest<'_>, value: CacheValue) {
        // Whether the answer is kept is decided from the borrowed request;
        // only then is the key copied (a copy, not caller code, so it may
        // run under the lock), so a key the cache will not keep is never
        // copied.
        let cost = entry_cost(req);
        let Ok(mut cache) = self.cache.write() else {
            return;
        };
        if cache.map.contains_key(&RequestKey(req) as &dyn KeyView)
            || cache.map.len() >= self.max_entries
            || cache.bytes.saturating_add(cost) > self.max_bytes
        {
            return;
        }
        cache.bytes = cache.bytes.saturating_add(cost);
        cache.map.insert(CacheKey::copy_of(req), value);
    }
}

impl Default for ProviderChain {
    fn default() -> Self {
        Self::empty()
    }
}

impl fmt::Debug for ProviderChain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names: Vec<&str> = self.providers.iter().map(|p| p.name()).collect();
        f.debug_struct("ProviderChain")
            .field("providers", &names)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, OnceLock, Weak, mpsc};

    use super::{ProviderChain, ProviderError, ScriptIdent, TreeRequest, TypeTreeProvider};
    use crate::raw::RawStr;
    use crate::typetree::{NodeList, NodeRow, Tree};
    use crate::version::UnityVersion;

    /// Answers class 1 with a tree and misses everything else, counting
    /// calls; with `chain` set it looks up class 2 through the chain from
    /// inside its own call (a re-entrant provider).
    #[derive(Debug, Default)]
    struct Counting {
        calls: AtomicUsize,
        chain: OnceLock<Weak<ProviderChain>>,
    }

    impl TypeTreeProvider for Counting {
        fn name(&self) -> &'static str {
            "counting"
        }

        fn accepts_scripts(&self) -> bool {
            true
        }

        fn tree(&self, req: &TreeRequest<'_>) -> Result<Option<Arc<Tree>>, ProviderError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            if req.class_id == 1 {
                if let Some(chain) = self.chain.get().and_then(Weak::upgrade) {
                    let inner = TreeRequest {
                        class_id: 2,
                        ..*req
                    };
                    chain.lookup(&inner)?;
                }
                let list = NodeList::trusted(vec![NodeRow::new(0, "Base", "Base")]).unwrap();
                return Ok(Some(Tree::for_tests(Arc::new(list))));
            }
            Ok(None)
        }
    }

    fn request(class_id: i32, version: &UnityVersion) -> TreeRequest<'_> {
        TreeRequest {
            class_id,
            version,
            script: None,
        }
    }

    #[test]
    fn answers_are_cached_and_misses_are_not() {
        let p = Arc::new(Counting::default());
        let chain = ProviderChain::new(vec![p.clone()]);
        let v = UnityVersion::from_packed(1);
        assert!(chain.lookup(&request(1, &v)).unwrap().is_some());
        assert!(chain.lookup(&request(1, &v)).unwrap().is_some());
        assert_eq!(p.calls.load(Ordering::Relaxed), 1, "the answer was cached");
        assert!(chain.lookup(&request(7, &v)).unwrap().is_none());
        assert!(chain.lookup(&request(7, &v)).unwrap().is_none());
        assert_eq!(p.calls.load(Ordering::Relaxed), 3, "a miss is asked again");
        assert_eq!(chain.cached(), 1, "misses leave nothing behind");
    }

    #[test]
    fn the_cache_stops_at_its_bound_and_answers_stay_the_same() {
        let p = Arc::new(Counting::default());
        let chain = ProviderChain::with_cache_bound(vec![p.clone()], 2, u64::MAX);
        for packed in 0..10 {
            let v = UnityVersion::from_packed(packed);
            assert!(chain.lookup(&request(1, &v)).unwrap().is_some());
        }
        assert_eq!(chain.cached(), 2);
        let bytes = ProviderChain::with_cache_bound(vec![p], usize::MAX, 1);
        let v = UnityVersion::from_packed(1);
        assert!(bytes.lookup(&request(1, &v)).unwrap().is_some());
        assert_eq!(
            bytes.cached(),
            0,
            "an entry over the byte bound is not kept"
        );
    }

    #[test]
    fn a_cached_key_does_not_keep_the_inputs_buffer() {
        let p = Arc::new(Counting::default());
        let chain = ProviderChain::new(vec![p]);
        let v = UnityVersion::from_packed(1);
        let input = bytes::Bytes::from(vec![b'x'; 1 << 20]);
        let script = ScriptIdent {
            assembly: RawStr::from_shared(input.slice(0..3)),
            namespace: RawStr::from_shared(input.slice(3..6)),
            class: RawStr::from_shared(input.slice(6..9)),
        };
        let req = TreeRequest {
            class_id: 1,
            version: &v,
            script: Some(&script),
        };
        assert!(chain.lookup(&req).unwrap().is_some());
        drop(script);
        // The only handle left on the buffer is ours: the cache copied.
        assert!(input.is_unique());
    }

    #[test]
    fn a_lookup_copies_nothing_but_the_key_it_keeps() {
        // Round 3: every lookup copied the request's script strings (and a
        // big version's limbs) before it looked in the cache, hits included.
        let copies = || super::KEY_COPIES.with(std::cell::Cell::get);
        let p = Arc::new(Counting::default());
        let chain = ProviderChain::new(vec![p]);
        let v = UnityVersion::from_packed(1);
        let long = RawStr::from_shared(bytes::Bytes::from(vec![b'n'; 1 << 20]));
        let script = ScriptIdent {
            assembly: long.clone(),
            namespace: long.clone(),
            class: long,
        };
        let req = |class_id| TreeRequest {
            class_id,
            version: &v,
            script: Some(&script),
        };
        let before = copies();
        assert!(chain.lookup(&req(1)).unwrap().is_some());
        assert_eq!(copies() - before, 1, "the answer kept copies its key once");
        for _ in 0..10 {
            assert!(chain.lookup(&req(1)).unwrap().is_some());
            assert!(chain.lookup(&req(7)).unwrap().is_none());
        }
        assert_eq!(copies() - before, 1, "hits and misses copy nothing");
        assert_eq!(chain.cached(), 1);
    }

    #[test]
    fn a_key_the_cache_will_not_keep_is_never_copied() {
        // Round 4: `publish` copied the request's strings before it found
        // the entry too big or the cache full, so such a key was copied on
        // every lookup.
        let copies = || super::KEY_COPIES.with(std::cell::Cell::get);
        let v = UnityVersion::from_packed(1);
        let long = RawStr::from_shared(bytes::Bytes::from(vec![b'n'; 1 << 20]));
        let script = ScriptIdent {
            assembly: long.clone(),
            namespace: long.clone(),
            class: long,
        };
        let req = TreeRequest {
            class_id: 1,
            version: &v,
            script: Some(&script),
        };
        // Too big: three 1 MiB names against a 2 MiB bound.
        let too_big = ProviderChain::with_cache_bound(
            vec![Arc::new(Counting::default())],
            usize::MAX,
            2 << 20,
        );
        let before = copies();
        for _ in 0..10 {
            assert!(too_big.lookup(&req).unwrap().is_some());
        }
        assert_eq!(copies() - before, 0, "an entry over the byte bound");
        assert_eq!(too_big.cached(), 0);
        // Full: a one-entry cache that already holds another answer.
        let full =
            ProviderChain::with_cache_bound(vec![Arc::new(Counting::default())], 1, u64::MAX);
        assert!(full.lookup(&request(1, &v)).unwrap().is_some());
        let before = copies();
        for _ in 0..10 {
            assert!(full.lookup(&req).unwrap().is_some());
        }
        assert_eq!(copies() - before, 0, "a full cache");
        assert_eq!(full.cached(), 1);
    }

    #[test]
    fn the_byte_bound_covers_the_table_at_rest_and_while_it_doubles() {
        // Round 4: an entry was counted at three buckets. A doubling table
        // holds up to 16/7 buckets per key at rest and 24/7 while the old
        // and the doubled table are both live, so three fell short at every
        // growth. Bucket counts are read from the map's capacity (hashbrown:
        // 7/8 of the buckets from 8 buckets up, one less below).
        let buckets = |cap: usize| match cap {
            0 => 0,
            c if c < 7 => c + 1,
            c => c / 7 * 8,
        };
        let p = Arc::new(Counting::default());
        let chain = ProviderChain::new(vec![p]);
        let mut held = 0;
        let mut grew = 0;
        for packed in 0..20_000 {
            let v = UnityVersion::from_packed(packed);
            assert!(chain.lookup(&request(1, &v)).unwrap().is_some());
            let cache = chain.cache.read().unwrap();
            let now = buckets(cache.map.capacity());
            // While it grew, the old table and the new one were both live.
            let peak = if now > held {
                grew += 1;
                held + now
            } else {
                now
            };
            assert!(
                u64::try_from(peak).unwrap() * super::BUCKET <= cache.bytes,
                "{} keys: {peak} buckets of {} bytes above the {} counted",
                cache.map.len(),
                super::BUCKET,
                cache.bytes
            );
            held = now;
        }
        assert!(grew >= 10, "the table doubled {grew} times");
    }

    #[test]
    fn a_thread_holding_a_lock_a_provider_needs_does_not_wait_on_the_chain() {
        // Plan 4.1: a provider is caller code (a Python one takes the GIL).
        // Thread A's lookup runs a provider that needs `gil`; thread B holds
        // `gil` and looks up another class through the same chain. Had the
        // chain held its cache lock across A's provider, B would wait on it
        // while holding what A's provider waits for.
        #[derive(Debug)]
        struct NeedsLock {
            gil: Arc<std::sync::Mutex<()>>,
            started: Arc<std::sync::Barrier>,
        }
        impl TypeTreeProvider for NeedsLock {
            fn name(&self) -> &'static str {
                "needs-lock"
            }
            fn tree(&self, req: &TreeRequest<'_>) -> Result<Option<Arc<Tree>>, ProviderError> {
                if req.class_id == 1 {
                    self.started.wait();
                    let _held = self.gil.lock().unwrap();
                }
                let list = NodeList::trusted(vec![NodeRow::new(0, "Base", "Base")]).unwrap();
                Ok(Some(Tree::for_tests(Arc::new(list))))
            }
        }
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let gil = Arc::new(std::sync::Mutex::new(()));
            let started = Arc::new(std::sync::Barrier::new(2));
            let chain = Arc::new(ProviderChain::new(vec![Arc::new(NeedsLock {
                gil: gil.clone(),
                started: started.clone(),
            })]));
            let a = {
                let chain = chain.clone();
                std::thread::spawn(move || {
                    let v = UnityVersion::from_packed(1);
                    chain.lookup(&request(1, &v)).map(|t| t.is_some())
                })
            };
            let held = gil.lock().unwrap();
            started.wait();
            let v = UnityVersion::from_packed(1);
            let b = chain.lookup(&request(2, &v)).map(|t| t.is_some());
            drop(held);
            let _ = tx.send((a.join().unwrap(), b));
        });
        let got = unityrs_testkit::guard::recv(
            &rx,
            "two lookups, one needing a lock the other holds (a chain lock held across a \
             provider deadlocks them)",
        );
        assert_eq!(got, (Ok(true), Ok(true)));
    }

    #[test]
    fn a_provider_may_call_the_chain_from_inside_its_answer() {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let p = Arc::new(Counting::default());
            let chain = Arc::new(ProviderChain::new(vec![p.clone()]));
            let _ = p.chain.set(Arc::downgrade(&chain));
            let v = UnityVersion::from_packed(1);
            let got = chain.lookup(&request(1, &v)).map(|t| t.is_some());
            let _ = tx.send(got);
        });
        let got = unityrs_testkit::guard::recv(
            &rx,
            "a lookup whose provider calls the chain (a chain lock held across the provider \
             deadlocks it)",
        );
        assert_eq!(got, Ok(true));
    }
}
