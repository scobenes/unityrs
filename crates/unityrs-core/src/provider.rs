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
//! Owner: package D. Construction is implemented by the scaffold.

// STUB(D): delete this allow when the module is implemented.
#![allow(
    clippy::todo,
    clippy::needless_pass_by_value,
    clippy::must_use_candidate,
    clippy::missing_panics_doc,
    unused_variables,
    dead_code
)]

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, RwLock};

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
type CacheValue = Option<(Arc<Tree>, &'static str)>;

/// Providers in order, with a cache keyed by `(class, packed version, script)`.
pub struct ProviderChain {
    providers: Vec<Arc<dyn TypeTreeProvider>>,
    cache: RwLock<HashMap<CacheKey, CacheValue>>,
}

impl ProviderChain {
    /// No providers: stripped objects are `no_typetree`.
    #[must_use]
    pub fn empty() -> Self {
        Self::new(Vec::new())
    }

    /// Providers asked in order; the first answer wins.
    #[must_use]
    pub fn new(providers: Vec<Arc<dyn TypeTreeProvider>>) -> Self {
        Self {
            providers,
            cache: RwLock::new(HashMap::new()),
        }
    }

    /// Whether the chain has no provider.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }

    /// The first provider's answer for `req`, cached (negative answers too).
    pub fn lookup(&self, req: &TreeRequest<'_>) -> Result<Option<TreeRef>, ProviderError> {
        todo!()
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
