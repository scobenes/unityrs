//! The TPK, loaded from a path at runtime, behind a cache keyed by class and
//! Unity version.
//!
//! rabex 0.2.1's `TypeTreeCache` (both variants) keys its map by `ClassId`
//! alone: the first Unity version that asks for a class fixes that class's
//! tree for every later version (docs/spikes/rabex.md section 5). UnityPy
//! keys by class and version. [`VersionedCache`] is that fix, as the fork
//! would make it.

use std::borrow::Cow;
use std::path::Path;

use rabex::UnityVersion;
use rabex::objects::ClassId;
use rabex::tpk::{TpkFile, TpkTypeTreeBlob};
use rabex::typetree::{TypeTreeNode, TypeTreeProvider};

/// A [`TypeTreeProvider`] cache keyed by `(class, version)`. Thread-safe, as
/// rabex's `sync::TypeTreeCache` is.
pub struct VersionedCache<T> {
    pub inner: T,
    cache: elsa::sync::FrozenMap<(ClassId, String), Box<Option<TypeTreeNode>>>,
}

impl<T: TypeTreeProvider> VersionedCache<T> {
    pub fn new(inner: T) -> Self {
        Self {
            inner,
            cache: elsa::sync::FrozenMap::new(),
        }
    }
}

impl<T: TypeTreeProvider> TypeTreeProvider for VersionedCache<T> {
    fn get_typetree_node(
        &self,
        class_id: ClassId,
        target_version: &UnityVersion,
    ) -> Option<Cow<'_, TypeTreeNode>> {
        let key = (class_id, target_version.to_string());
        if let Some(hit) = self.cache.get(&key) {
            return hit.as_ref().map(Cow::Borrowed);
        }
        let tt = self
            .inner
            .get_typetree_node(class_id, target_version)
            .map(Cow::into_owned);
        self.cache
            .insert(key, Box::new(tt))
            .as_ref()
            .map(Cow::Borrowed)
    }
}

/// A TPK blob, or none: without `--tpk`, an object with no embedded typetree
/// has no tree (rabex's `NoTypetree`), as it would with an empty TPK.
pub struct MaybeTpk(pub Option<TpkTypeTreeBlob>);

impl TypeTreeProvider for MaybeTpk {
    fn get_typetree_node(
        &self,
        class_id: ClassId,
        target_version: &UnityVersion,
    ) -> Option<Cow<'_, TypeTreeNode>> {
        self.0
            .as_ref()?
            .get_typetree_node(class_id, target_version)
            .map(Cow::Owned)
    }
}

/// The provider every subcommand reads through.
pub type Tpk = VersionedCache<MaybeTpk>;

/// Loads a TPK file (rabex's `lz4.tpk` or UnityPy's `lzma.tpk`) from a path.
pub fn load_blob(path: &Path) -> Result<TpkTypeTreeBlob, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    TpkFile::from_reader(&mut bytes.as_slice())
        .and_then(|f| f.as_type_tree())
        .map_err(|e| format!("{}: {e}", path.display()))?
        .ok_or_else(|| format!("{}: not a type-tree TPK", path.display()))
}

/// The provider for an optional `--tpk` path.
pub fn load(path: Option<&Path>) -> Result<Tpk, String> {
    Ok(VersionedCache::new(MaybeTpk(
        path.map(load_blob).transpose()?,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A provider whose tree for a class names the version it was built for.
    struct ByVersion;

    impl TypeTreeProvider for ByVersion {
        fn get_typetree_node(
            &self,
            class_id: ClassId,
            target_version: &UnityVersion,
        ) -> Option<Cow<'_, TypeTreeNode>> {
            Some(Cow::Owned(TypeTreeNode {
                m_Type: format!("class {} at {target_version}", class_id.0),
                ..TypeTreeNode::default()
            }))
        }
    }

    fn versions() -> (UnityVersion, UnityVersion) {
        ("5.6.7f1".parse().unwrap(), "6000.3.17f1".parse().unwrap())
    }

    fn ask(p: &impl TypeTreeProvider, v: &UnityVersion) -> String {
        p.get_typetree_node(ClassId(28), v).unwrap().m_Type.clone()
    }

    /// The defect: rabex 0.2.1's cache answers a second version with the
    /// first version's tree. (If upstream fixes it, this test fails and the
    /// doc's finding should be revisited.)
    #[test]
    fn rabex_cache_is_keyed_by_class_only() {
        let (old, new) = versions();
        let rabex = rabex::typetree::typetree_cache::sync::TypeTreeCache::new(ByVersion);
        assert_eq!(ask(&rabex, &old), "class 28 at 5.6.7f1");
        assert_eq!(ask(&rabex, &new), "class 28 at 5.6.7f1");
        let unsync = rabex::typetree::typetree_cache::TypeTreeCache::new(ByVersion);
        assert_eq!(ask(&unsync, &old), "class 28 at 5.6.7f1");
        assert_eq!(ask(&unsync, &new), "class 28 at 5.6.7f1");
    }

    /// The fix: each version gets its own tree, in either order, and a
    /// repeated lookup is served from the cache.
    #[test]
    fn versioned_cache_keys_by_class_and_version() {
        let (old, new) = versions();
        let cache = VersionedCache::new(ByVersion);
        assert_eq!(ask(&cache, &old), "class 28 at 5.6.7f1");
        assert_eq!(ask(&cache, &new), "class 28 at 6000.3.17f1");
        assert_eq!(ask(&cache, &old), "class 28 at 5.6.7f1");
        let first = cache.get_typetree_node(ClassId(28), &new).unwrap();
        assert!(matches!(first, Cow::Borrowed(_)));
    }

    #[test]
    fn no_tpk_means_no_tree() {
        let (_, new) = versions();
        assert!(
            load(None)
                .unwrap()
                .get_typetree_node(ClassId(28), &new)
                .is_none()
        );
    }

    /// The TPK stays out of the binary: rabex resolves without its default
    /// `embed-tpk` feature anywhere in the workspace.
    #[test]
    fn rabex_is_built_without_the_embedded_tpk() {
        // Offline, `cargo metadata` needs every platform's packages unless it
        // is filtered to one; a fresh CI runner holds only the host's.
        let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
        let version = std::process::Command::new(rustc)
            .arg("-vV")
            .output()
            .expect("rustc -vV runs");
        let version = String::from_utf8(version.stdout).unwrap();
        let host = version
            .lines()
            .find_map(|l| l.strip_prefix("host: "))
            .expect("rustc -vV names the host");
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let out = std::process::Command::new(cargo)
            .args(["metadata", "--format-version", "1", "--offline"])
            .args(["--filter-platform", host])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .expect("cargo metadata runs");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let meta: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        let rabex = meta["resolve"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["id"].as_str().is_some_and(|id| id.contains("#rabex@")))
            .expect("rabex in the resolve graph");
        let features: Vec<&str> = rabex["features"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|f| f.as_str())
            .collect();
        assert!(!features.contains(&"embed-tpk"), "{features:?}");
        assert!(features.contains(&"compression-lzma"), "{features:?}");
    }
}
