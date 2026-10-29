//! Multi-file scenes for the `Environment` tests: bundles whose objects
//! point into each other, resources in sibling entries and in other bundles.
//!
//! Owner: package D.

// STUB(D): delete this allow when the scenes are implemented.
#![allow(clippy::todo, clippy::must_use_candidate, unused_variables)]

/// Named files, as a caller would load them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Scene {
    /// `(name, bytes)` in load order.
    pub files: Vec<(Vec<u8>, Vec<u8>)>,
}

/// Two bundles: an object in the first points (`m_FileID` 1) at an object in
/// the second's CAB; the first also holds a `.resS` the object reads.
pub fn cross_bundle_pointer() -> Scene {
    todo!()
}

/// A loose `SerializedFile` (no container) with an external pointer: the
/// "no container is set" refusal.
pub fn loose_external_pointer() -> Scene {
    todo!()
}
