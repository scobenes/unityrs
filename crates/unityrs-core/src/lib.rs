//! Unity asset bundles, serialized files and typetrees, read with the semantics
//! of UnityPy 1.25.2 and UnityPyBoost.
//!
//! The plan is `docs/plans/core.md`. These modules are its shared interfaces:
//! every public type and signature here is the contract the P1 work packages
//! build against. A module whose first line is a `STUB` allow still has
//! `todo!()` bodies; the package that owns it (plan section 7) fills it and
//! deletes the allow.
//!
//! Layering, bottom up:
//! - [`raw`], [`limits`], [`budget`], [`error`], [`options`]: shared plumbing;
//! - [`tables`], [`classid`]: generated tables pinned to the oracle's UnityPy and Python;
//! - [`stream`], [`version`], [`detect`], [`codec`]: UnityPy's readers and rules;
//! - [`files`]: `BundleFile`, `WebFile`, `SerializedFile` and raw files;
//! - [`typetree`]: node rows, UnityPy's tree-building rule, the compiled [`typetree::Tree`];
//! - [`value`] and [`walk`]: the dict model, UnityPyBoost's reader, the
//!   pure-Python reader and UnityPy's writer;
//! - [`provider`], [`object`], [`container`], [`env`](mod@env): typetree sources,
//!   objects, container paths and the `Environment`.

#![forbid(unsafe_code)]
// No panics on untrusted input (plan section 3.15). Tests may opt out.
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::todo,
        clippy::unimplemented,
        clippy::unreachable,
        clippy::indexing_slicing
    )
)]

pub mod budget;
pub mod classid;
pub mod codec;
pub mod container;
pub mod detect;
pub mod env;
pub mod error;
pub mod files;
pub mod limits;
pub mod object;
pub mod options;
pub mod provider;
pub mod pystr;
pub mod raw;
pub mod stream;
pub mod tables;
pub mod typetree;
pub mod value;
pub mod version;
pub mod walk;

pub use bytes::Bytes;
pub use error::{Error, LimitError, LimitKind, NotFound, Unsupported};
pub use files::{BundleFile, File, RawFile, SerializedFile, WebFile};
pub use object::ObjectRef;
pub use options::FileContext;
pub use raw::RawStr;
pub use value::{Map, Parsed, PrimArray, Value};
pub use version::UnityVersion;

/// The crate version, as built.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Every public type the batch engine and the Python binding share across
/// threads is `Send + Sync` (plan section 3.16). This fails to compile if one
/// stops being so.
const _: () = {
    const fn send_sync<T: Send + Sync>() {}
    send_sync::<Error>();
    send_sync::<RawStr>();
    send_sync::<Bytes>();
    send_sync::<FileContext>();
    send_sync::<File>();
    send_sync::<BundleFile>();
    send_sync::<WebFile>();
    send_sync::<SerializedFile>();
    send_sync::<RawFile>();
    send_sync::<typetree::NodeList>();
    send_sync::<typetree::Tree>();
    send_sync::<Value>();
    send_sync::<Parsed>();
    send_sync::<ObjectRef>();
    send_sync::<provider::ProviderChain>();
    send_sync::<env::Environment>();
    send_sync::<walk::ParseError>();
    send_sync::<walk::WriteError>();
    send_sync::<container::Container>();
    send_sync::<codec::Codecs>();
    send_sync::<budget::Permit>();
};
