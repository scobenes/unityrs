// Ported from UnityPy 1.25.2 (MIT): UnityPy/files/SerializedFile.py. See THIRD_PARTY_LICENSES.
//! The options every file loaded through one environment shares.
//!
//! A [`FileContext`] is built once (usually by [`crate::env::EnvOptions`]) and
//! `Arc`'d into every file, so a `SerializedFile` deep inside a bundle reads
//! with the same version policy, providers, codecs, budget and limits as its
//! parent.
//!
//! Owner: the scaffold (frozen fields; implemented constructors).

use std::fmt;
use std::sync::Arc;

use crate::budget::{MemoryBudget, Unbounded};
use crate::codec::{BlockDecryptor, Codecs};
use crate::limits::Limits;
use crate::provider::ProviderChain;
use crate::version::VersionPolicy;
use crate::walk::{FloatMode, ReadOptions, WriteRules};

/// Options shared by every file of one load.
#[derive(Clone)]
#[non_exhaustive]
pub struct FileContext {
    /// UnityPy's `config.FALLBACK_UNITY_VERSION`, plus opt-in, reported overrides.
    pub version: VersionPolicy,
    /// How a float32 signalling NaN reaches the value (plan section 3.10).
    pub floats: FloatMode,
    /// The writer's alignment rules (plan section 3.11).
    pub write: WriteRules,
    /// Typetree sources for stripped files, keyed by (class, version).
    pub providers: Arc<ProviderChain>,
    /// Block codecs by compression type.
    pub codecs: Arc<Codecs>,
    /// Block decryption (UnityCN: P3). `None` refuses encrypted bundles.
    pub decryptor: Option<Arc<dyn BlockDecryptor>>,
    /// Where decoded buffers are paid for.
    pub budget: Arc<dyn MemoryBudget>,
    /// Hard bounds.
    pub limits: Limits,
    /// Read the first AssetBundle object while a `SerializedFile` loads, and
    /// fail the file when it fails, as UnityPy does (`SerializedFile.py:316-324`).
    pub eager_asset_bundle: bool,
}

impl FileContext {
    /// UnityPy 1.25.2's behaviour: no fallback version, no providers, the
    /// UnityPy codecs, no decryptor, no budget, `FloatMode::UnityPy`,
    /// `WriteRules::UnityPy`, the AssetBundle read at load.
    #[must_use]
    pub fn unitypy() -> Self {
        Self {
            version: VersionPolicy::default(),
            floats: FloatMode::UnityPy,
            write: WriteRules::UnityPy,
            providers: Arc::new(ProviderChain::empty()),
            codecs: Arc::new(Codecs::unitypy()),
            decryptor: None,
            budget: Arc::new(Unbounded),
            limits: Limits::default(),
            eager_asset_bundle: true,
        }
    }

    /// The walker's options for this context.
    #[must_use]
    pub fn read_options(&self) -> ReadOptions {
        ReadOptions {
            floats: self.floats,
            limits: self.limits,
        }
    }
}

impl Default for FileContext {
    fn default() -> Self {
        Self::unitypy()
    }
}

impl fmt::Debug for FileContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileContext")
            .field("version", &self.version)
            .field("floats", &self.floats)
            .field("write", &self.write)
            .field("providers", &self.providers)
            .field("decryptor", &self.decryptor.is_some())
            .field("budget", &self.budget)
            .field("limits", &self.limits)
            .field("eager_asset_bundle", &self.eager_asset_bundle)
            .finish_non_exhaustive()
    }
}
