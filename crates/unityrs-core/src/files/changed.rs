// Ported from UnityPy 1.25.2 (MIT): UnityPy/files/File.py. See THIRD_PARTY_LICENSES.
//! Parent propagation (`files/File.py:145-148`). Weak parents never keep an
//! input alive merely because a child was edited.
use super::{BundleFile, File, Parent, ParentKind, WebFile};
use std::sync::atomic::Ordering;

impl Parent {
    pub(crate) fn mark_changed(&self) {
        match &self.kind {
            ParentKind::Bundle { bundle, .. } => {
                if let Some(file) = bundle.upgrade() {
                    file.mark_changed();
                }
            }
            ParentKind::Web { web } => {
                if let Some(file) = web.upgrade() {
                    file.mark_changed();
                }
            }
        }
    }
}

macro_rules! changed {
    ($ty:ty) => {
        impl $ty {
            /// Mark this container and all its live parent containers changed.
            pub fn mark_changed(&self) {
                if let Some(parent) = self.parent() {
                    parent.mark_changed();
                }
                self.changed.store(true, Ordering::Release);
            }
            /// Whether this container or one of its children was marked changed.
            #[must_use]
            pub fn is_changed(&self) -> bool {
                self.changed.load(Ordering::Acquire)
            }
        }
    };
}
changed!(BundleFile);
changed!(WebFile);

impl File {
    /// Mark a parsed file and its parents changed. Raw readers have no flag.
    pub fn mark_changed(&self) {
        match self {
            Self::Serialized(f) => f.mark_changed(),
            Self::Bundle(f) => f.mark_changed(),
            Self::Web(f) => f.mark_changed(),
            Self::Raw(_) => {}
        }
    }
    /// Whether this parsed file was marked changed.
    #[must_use]
    pub fn is_changed(&self) -> bool {
        match self {
            Self::Serialized(f) => f.is_changed(),
            Self::Bundle(f) => f.is_changed(),
            Self::Web(f) => f.is_changed(),
            Self::Raw(_) => false,
        }
    }
}
