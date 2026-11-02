// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/Tpk.py. See THIRD_PARTY_LICENSES.
//! [`NoTree`]: which of UnityPy's exceptions a class with no tree raises.

/// Why no tree answers a class: the exception UnityPy raises there, the
/// TPK's (`helpers/Tpk.py:42-55`, `:58-64`, `:472-481`), which a binding
/// raises where the core reports no tree
/// ([`crate::walk::ParseError::NoTypeTree`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum NoTree {
    /// The TPK does not hold the class: `KeyError`.
    UnknownClass,
    /// No entry at or below the version, or the entry there marks the
    /// class removed: `ValueError` "Could not find exact version".
    NoEntry,
    /// The entry picked has no release root: `AssertionError`.
    NoReleaseRoot,
    /// The file has no version (below format 7: `ObjectReader.version` is
    /// `None`), which the TPK compares with its first entry:
    /// `NotImplementedError` "Unsupported comparison".
    NoVersion,
}

const _: () = crate::send_sync::<NoTree>();

impl NoTree {
    /// The name of the exception UnityPy raises.
    #[must_use]
    pub fn exception(self) -> &'static str {
        match self {
            Self::UnknownClass => "KeyError",
            Self::NoEntry => "ValueError",
            Self::NoReleaseRoot => "AssertionError",
            Self::NoVersion => "NotImplementedError",
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::NoTree;
    use crate::provider::{ProviderChain, ProviderError, TreeRequest, TypeTreeProvider};
    use crate::typetree::Tree;
    use crate::version::UnityVersion;

    /// A provider with no tree, that says `why` (or nothing) for every class.
    #[derive(Debug)]
    struct Says(Option<NoTree>);

    impl TypeTreeProvider for Says {
        fn name(&self) -> &'static str {
            "says"
        }

        fn tree(&self, _: &TreeRequest<'_>) -> Result<Option<Arc<Tree>>, ProviderError> {
            Ok(None)
        }

        fn why_none(&self, _: i32, _: Option<&UnityVersion>) -> Option<NoTree> {
            self.0
        }
    }

    /// A provider that keeps the trait's default.
    #[derive(Debug)]
    struct Silent;

    impl TypeTreeProvider for Silent {
        fn name(&self) -> &'static str {
            "silent"
        }

        fn tree(&self, _: &TreeRequest<'_>) -> Result<Option<Arc<Tree>>, ProviderError> {
            Ok(None)
        }
    }

    fn chain(providers: Vec<Arc<dyn TypeTreeProvider>>) -> ProviderChain {
        ProviderChain::new(providers)
    }

    /// The chain's reason is the last provider's that states one, the one a
    /// fallback that raised in turn would leave; a provider that keeps the
    /// default states none, and so does an empty chain.
    #[test]
    fn the_chains_reason_is_the_last_stated() {
        let v = UnityVersion::from_packed(2 << 48);
        let key = || -> Arc<dyn TypeTreeProvider> { Arc::new(Says(Some(NoTree::UnknownClass))) };
        let value = || -> Arc<dyn TypeTreeProvider> { Arc::new(Says(Some(NoTree::NoEntry))) };
        let why = |c: &ProviderChain| (c.why_none(7, Some(&v)), c.why_none(7, None));
        let both = |w: Option<NoTree>| (w, w);
        assert_eq!(why(&chain(vec![])), both(None));
        assert_eq!(why(&chain(vec![Arc::new(Silent)])), both(None));
        assert_eq!(
            why(&chain(vec![key(), value()])),
            both(Some(NoTree::NoEntry))
        );
        assert_eq!(
            why(&chain(vec![value(), key()])),
            both(Some(NoTree::UnknownClass))
        );
        let last_silent = chain(vec![key(), Arc::new(Silent)]);
        assert_eq!(why(&last_silent), both(Some(NoTree::UnknownClass)));
    }
}
