//! The CAB registry: UnityPy's `env.cabs` dict (`environment.py:252-280`).
//!
//! Keys are [`crate::pystr::simplify_name`] of the registered name. As a
//! Python dict: a repeated key replaces the value and keeps the first
//! position, and iteration is in first-insertion order (`env.container`
//! walks it). Values are files already parsed, or, under
//! [`super::LoadMode::Lazy`], a bundle entry to parse on first use: the slot
//! is a `OnceLock` so concurrent askers parse it once. A lazy bundle registers
//! every directory entry's name at open, in directory order. An entry that
//! turns out to be a container is not a CAB: its slot answers `None` and its
//! own entries are registered then, at the end of the order (plan section
//! 3.13, difference (b)).
//!
//! Owner: package D.

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
use std::sync::{Arc, OnceLock, RwLock};

use crate::error::Error;
use crate::files::{BundleFile, RawFile, SerializedFile};
use crate::raw::RawStr;

/// A registered CAB: what `get_cab` returns.
#[derive(Clone, Debug)]
pub enum Cab {
    /// A `SerializedFile`.
    Serialized(Arc<SerializedFile>),
    /// A raw reader (`.resS`, `.resource`, name-rule files).
    Raw(RawCab),
}

/// A raw CAB's bytes.
#[derive(Clone, Debug)]
pub enum RawCab {
    /// A bundle entry, read through the bundle (no copy of the whole entry).
    InBundle {
        /// The bundle.
        bundle: Arc<BundleFile>,
        /// Its directory index.
        dir_index: usize,
    },
    /// A file loaded on its own.
    File(Arc<RawFile>),
}

/// One registry slot.
#[derive(Debug)]
pub(crate) enum Slot {
    /// Parsed at registration.
    Ready(Cab),
    /// A bundle entry parsed on first use.
    Lazy {
        /// The bundle.
        bundle: Arc<BundleFile>,
        /// Its directory index.
        dir_index: usize,
        /// The parse, once.
        cell: OnceLock<Result<Cab, Error>>,
    },
}

/// The registry.
#[derive(Debug, Default)]
pub struct Registry {
    inner: RwLock<RegistryInner>,
}

#[derive(Debug, Default)]
struct RegistryInner {
    order: Vec<RawStr>,
    slots: HashMap<RawStr, Arc<Slot>>,
}

impl Registry {
    /// `register_cab(name, cab)`.
    pub fn register(&self, name: &[u8], cab: Cab) {
        todo!()
    }

    /// Registers a bundle entry to parse on first use.
    pub fn register_lazy(&self, name: &[u8], bundle: Arc<BundleFile>, dir_index: usize) {
        todo!()
    }

    /// `get_cab(name)`, parsing a lazy slot now.
    pub fn get(&self, name: &[u8]) -> Result<Option<Cab>, Error> {
        todo!()
    }

    /// Keys in dict order.
    pub fn keys(&self) -> Vec<RawStr> {
        todo!()
    }

    /// Number of keys.
    pub fn len(&self) -> usize {
        todo!()
    }

    /// Whether it is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
