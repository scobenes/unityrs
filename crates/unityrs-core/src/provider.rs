//! Typetree providers for stripped files, keyed by `(class, version)`.
//!
//! UnityPy's order (`files/ObjectReader.py:285-307`): an explicit `nodes=`
//! tree; else the type record's own tree; else the class tree for
//! `(class_id, version)` from the TPK (`helpers/Tpk.py:42-55`), and for a
//! `MonoBehaviour` the script generator's tree when it answers. Only a
//! `ValueError` is swallowed there; the script lookup dereferences `m_Script`
//! through the Environment, so it lives in
//! [`crate::env::Environment::object_tree`], and runs only when a provider
//! [`TypeTreeProvider::accepts_scripts`]. The cache key is the class and the packed version
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
//! when full it stops adding, which changes no answer. A cached key owns
//! copies of its strings, so it never keeps an input's buffer alive. A
//! provider is caller code: the chain calls it with no lock held, so a
//! provider may itself call [`ProviderChain::lookup`].
//!
//! Owner: package D. Construction and [`ProviderChain::lookup`] are
//! implemented by the scaffold.

use std::collections::HashMap;
use std::fmt;
use std::mem::size_of;
use std::sync::{Arc, RwLock};

use bytes::Bytes;

use crate::raw::RawStr;
use crate::typetree::Tree;
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
    /// What failed.
    pub detail: Box<str>,
}

/// A source of class (or script) trees.
pub trait TypeTreeProvider: Send + Sync + fmt::Debug {
    /// A short name; the dump's `l2.source` is `"tpk"` for the TPK provider.
    fn name(&self) -> &'static str;
    /// The tree for `req`, `Ok(None)` when the provider has none (where UnityPy
    /// raises `KeyError`/`ValueError`, which the caller turns into `no_typetree`).
    fn tree(&self, req: &TreeRequest<'_>) -> Result<Option<Arc<Tree>>, ProviderError>;
    /// Whether this provider answers script requests (`TreeRequest::script`):
    /// UnityPy's `env.typetree_generator`. Only then does the Environment
    /// dereference a `MonoBehaviour`'s `m_Script` (P3; the TPK provider does not).
    fn accepts_scripts(&self) -> bool {
        false
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

type CacheKey = (i32, Packed, Option<ScriptIdent>);
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

/// What one cached answer costs: its key and value in the table, at the
/// table's worst load (a doubling hash table is at least 7/16 full, so a
/// slot is charged twice over, with its control byte), plus the heap its
/// key owns.
fn entry_cost(key: &CacheKey) -> u64 {
    let slot = size_of::<CacheKey>() + size_of::<CacheValue>() + 1;
    let strings = key
        .2
        .as_ref()
        .map_or(0, |s| s.assembly.len() + s.namespace.len() + s.class.len());
    let limbs = key.1.heap_bytes();
    u64::try_from(3 * slot + strings + limbs).unwrap_or(u64::MAX)
}

/// The key with its strings copied out of the input's buffer.
fn owned_key(req: &TreeRequest<'_>) -> CacheKey {
    let own = |s: &RawStr| RawStr::from_shared(Bytes::copy_from_slice(s.as_bytes()));
    (
        req.class_id,
        req.version.packed().clone(),
        req.script.map(|s| ScriptIdent {
            assembly: own(&s.assembly),
            namespace: own(&s.namespace),
            class: own(&s.class),
        }),
    )
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
    /// and `bytes` bytes ([`entry_cost`]'s accounting).
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

    /// Answers cached now.
    #[must_use]
    pub fn cached(&self) -> usize {
        self.cache.read().map_or(0, |c| c.map.len())
    }

    /// The first provider's answer for `req`. An answer is cached while the
    /// cache has room; a miss is not cached. Providers are asked with no lock
    /// held.
    pub fn lookup(&self, req: &TreeRequest<'_>) -> Result<Option<TreeRef>, ProviderError> {
        let key = owned_key(req);
        let hit = self
            .cache
            .read()
            .ok()
            .and_then(|c| c.map.get(&key).cloned());
        if let Some((tree, name)) = hit {
            return Ok(Some(TreeRef {
                tree,
                source: TreeSource::Provider(name),
            }));
        }
        for provider in &self.providers {
            if let Some(tree) = provider.tree(req)? {
                let name = provider.name();
                self.publish(key, (tree.clone(), name));
                return Ok(Some(TreeRef {
                    tree,
                    source: TreeSource::Provider(name),
                }));
            }
        }
        Ok(None)
    }

    fn publish(&self, key: CacheKey, value: CacheValue) {
        let cost = entry_cost(&key);
        let Ok(mut cache) = self.cache.write() else {
            return;
        };
        if cache.map.contains_key(&key)
            || cache.map.len() >= self.max_entries
            || cache.bytes.saturating_add(cost) > self.max_bytes
        {
            return;
        }
        cache.bytes = cache.bytes.saturating_add(cost);
        cache.map.insert(key, value);
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
    use std::time::Duration;

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
        let got = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("deadlocked: the chain held a lock across the provider");
        assert_eq!(got, Ok(true));
    }
}
