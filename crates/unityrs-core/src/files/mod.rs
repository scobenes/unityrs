//! Files: what `ImportHelper.parse_file` builds (`helpers/ImportHelper.py:127-156`).
//!
//! A [`File`] is a handle (`Arc`) on an immutable, parsed file. Containers
//! decode lazily and parse each entry once ([`BundleFile::entry`]); the
//! UnityPy load order, where every entry is parsed and a failing entry fails
//! the bundle, is [`BundleFile::load_all`].
//!
//! Owner: package C.

// STUB(C): delete this allow when the module is implemented.
#![allow(
    clippy::todo,
    clippy::needless_pass_by_value,
    clippy::must_use_candidate,
    clippy::missing_panics_doc,
    unused_variables
)]

pub mod bundle;
pub mod serialized;
pub mod web;

use std::sync::{Arc, Weak};

use bytes::Bytes;

pub use bundle::{BlockInfo, BundleFile, BundleHeader, DirEntry, FileSlot, LayoutKind};
pub use serialized::{
    External, ObjectInfo, ScriptRef, SerializedFile, SerializedHeader, SerializedType,
};
pub use web::{WebFile, WebPacker};

use crate::detect::FileType;
use crate::error::Error;
use crate::limits::Meter;
use crate::options::FileContext;
use crate::raw::RawStr;

/// A parsed file.
#[derive(Clone, Debug)]
pub enum File {
    /// An `AssetsFile` that is not raw by the name rule.
    Serialized(Arc<SerializedFile>),
    /// A bundle.
    Bundle(Arc<BundleFile>),
    /// A `WebFile` (P3).
    Web(Arc<WebFile>),
    /// What UnityPy keeps as a reader: resources, name-rule files, zip entries.
    Raw(Arc<RawFile>),
}

/// A file UnityPy keeps as a reader.
#[derive(Clone, Debug)]
pub struct RawFile {
    /// The name it was loaded under.
    pub name: RawStr,
    /// Its bytes (a slice of the parent's buffer for an entry).
    pub data: Bytes,
}

/// What a child knows of its container. A file loaded at top level has none
/// (`parent: None`): UnityPy makes the `Environment` that loaded it its
/// parent (`environment.py:154`), which is the container `PPtr.deref` scans
/// (plan section 3.13, rule 4).
#[derive(Clone, Debug)]
pub struct Parent {
    /// The container's name.
    pub name: RawStr,
    /// The container's kind.
    pub kind: ParentKind,
    /// The input's meter, shared by every file inside one top-level load
    /// ([`crate::limits`]); a top-level file makes a new one.
    pub meter: Arc<Meter>,
}

/// A container kind, as the `SerializedFile` version chain and `PPtr.deref`
/// distinguish them.
#[derive(Clone, Debug)]
pub enum ParentKind {
    /// A `BundleFile`, whose raw engine string chain 2 may take.
    Bundle {
        /// `version_engine`, as read.
        version_engine: RawStr,
        /// The bundle, for `PPtr.deref`'s scan of its `files` dict (rule 5):
        /// weak, since the bundle holds its entries (`Arc::new_cyclic` at open).
        bundle: Weak<BundleFile>,
    },
    /// A `WebFile`, which gives no version.
    Web,
}

/// `parse_file`: builds the file `ty` says, under the name rule
/// ([`crate::detect::parsed_as`]). A file with a `parent` is an entry.
pub fn parse_file(
    name: RawStr,
    data: Bytes,
    ty: FileType,
    parent: Option<Parent>,
    ctx: &Arc<FileContext>,
) -> Result<File, Error> {
    todo!()
}
