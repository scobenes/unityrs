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
//! - [`raw`], [`limits`], [`budget`], [`once`], [`error`], [`options`]: shared plumbing;
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
pub mod once;
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

/// Asserts at compile time that `T` is `Send + Sync` (plan section 3.16):
/// `const _: () = crate::send_sync::<T>();`, here or next to the type.
pub(crate) const fn send_sync<T: Send + Sync + ?Sized>() {}

/// Every public type of the core is `Send + Sync` (plan section 3.16): a pool
/// or a binding may share any of them. This fails to compile if one stops
/// being so, and `tests::every_public_type_is_asserted` fails if a public
/// type is added to the crate with no assertion naming its full path, here
/// or in its own file (a package that adds a type adds the line next to it).
const _: () = {
    // budget
    send_sync::<budget::Reservation>();
    send_sync::<budget::BudgetExceeded>();
    send_sync::<budget::Counting>();
    send_sync::<budget::DecodeBuf>();
    send_sync::<budget::Permit>();
    send_sync::<budget::Unbounded>();
    send_sync::<dyn budget::MemoryBudget>();
    send_sync::<dyn budget::PermitHome>();
    // codec
    send_sync::<codec::Codec>();
    send_sync::<codec::Codecs>();
    send_sync::<codec::Lz4>();
    send_sync::<codec::Lzma>();
    send_sync::<codec::Stored>();
    send_sync::<dyn codec::BlockCodec>();
    send_sync::<dyn codec::BlockDecryptor>();
    // container, detect
    send_sync::<container::Container>();
    send_sync::<container::ContainerRow>();
    send_sync::<detect::DetectError>();
    send_sync::<detect::FileType>();
    send_sync::<detect::ParsedAs>();
    // env
    send_sync::<env::LoadMode>();
    send_sync::<env::Source>();
    send_sync::<env::ContainerItem>();
    send_sync::<env::ContainerItems<'static>>();
    send_sync::<env::EnvOptions>();
    send_sync::<env::EnvStats>();
    send_sync::<env::Environment>();
    send_sync::<env::Objects<'static>>();
    send_sync::<dyn env::CabResolver>();
    send_sync::<dyn env::Fetch>();
    send_sync::<env::pptr::DerefError>();
    send_sync::<env::pptr::ExternalIndexError>();
    send_sync::<env::pptr::PPtr>();
    send_sync::<env::registry::Cab>();
    send_sync::<env::registry::Keys<'static>>();
    send_sync::<env::registry::RawCab>();
    send_sync::<env::registry::Registry>();
    // error
    send_sync::<error::Error>();
    send_sync::<error::LimitKind>();
    send_sync::<error::NotFound>();
    send_sync::<error::Unsupported>();
    send_sync::<error::LimitError>();
    // files
    send_sync::<files::bundle::LayoutKind>();
    send_sync::<files::bundle::BlockInfo>();
    send_sync::<files::bundle::BundleFile>();
    send_sync::<files::bundle::BundleHeader>();
    send_sync::<files::bundle::DirEntry>();
    send_sync::<files::bundle::FileSlot>();
    send_sync::<files::File>();
    send_sync::<files::ParentKind>();
    send_sync::<files::Parent>();
    send_sync::<files::RawFile>();
    send_sync::<files::serialized::External>();
    send_sync::<files::serialized::ObjectInfo>();
    send_sync::<files::serialized::ScriptRef>();
    send_sync::<files::serialized::SerializedFile>();
    send_sync::<files::serialized::SerializedHeader>();
    send_sync::<files::serialized::SerializedType>();
    send_sync::<files::web::WebPacker>();
    send_sync::<files::web::WebFile>();
    // limits, object, options, provider, raw, stream
    send_sync::<limits::ChargeError>();
    send_sync::<limits::Charge>();
    send_sync::<limits::Limits>();
    send_sync::<limits::Meter>();
    send_sync::<limits::MeteredVec<u8>>();
    send_sync::<once::KeptError<Error>>();
    send_sync::<dyn once::CellError>();
    send_sync::<object::ObjectRef>();
    send_sync::<options::FileContext>();
    send_sync::<provider::TreeSource>();
    send_sync::<provider::ProviderChain>();
    send_sync::<provider::ProviderError>();
    send_sync::<provider::ScriptIdent>();
    send_sync::<provider::TreeRef>();
    send_sync::<provider::TreeRequest<'static>>();
    send_sync::<dyn provider::TypeTreeProvider>();
    send_sync::<raw::RawStr>();
    send_sync::<Bytes>();
    send_sync::<stream::Endian>();
    send_sync::<stream::Reader<'static>>();
    // typetree
    send_sync::<typetree::rows::TreeError>();
    send_sync::<typetree::rows::NodeId>();
    send_sync::<typetree::rows::NodeList>();
    send_sync::<typetree::rows::NodeRow>();
    send_sync::<typetree::tree::Kind>();
    send_sync::<typetree::tree::Shape>();
    send_sync::<typetree::tree::NodeInfo>();
    send_sync::<typetree::tree::Tree>();
    // value, version
    send_sync::<value::PrimArray>();
    send_sync::<value::Value>();
    send_sync::<value::Map>();
    send_sync::<value::Parsed>();
    send_sync::<version::FlagsClass>();
    send_sync::<version::Packed>();
    send_sync::<version::VersionError>();
    send_sync::<version::VersionSource>();
    send_sync::<version::ResolvedVersion>();
    send_sync::<version::UnityVersion>();
    send_sync::<version::VersionPolicy>();
    // walk
    send_sync::<walk::FloatMode>();
    send_sync::<walk::L2Kind>();
    send_sync::<walk::ParseError>();
    send_sync::<walk::ReadDetail>();
    send_sync::<walk::ReadErrorKind>();
    send_sync::<walk::RefTypes<'static>>();
    send_sync::<walk::WriteError>();
    send_sync::<walk::WriteRules>();
    send_sync::<walk::ReadCtx<'static>>();
    send_sync::<walk::ReadError>();
    send_sync::<walk::ReadOptions>();
    send_sync::<walk::WriteCtx<'static>>();
};

#[cfg(test)]
mod tests {
    use std::path::Path;

    /// Every `pub struct`, `pub enum` and `pub trait` of the crate is named,
    /// by its full module path, by a `send_sync` assertion
    /// (`unityrs_testkit::public_types`), so none can lose `Send + Sync`
    /// unnoticed, and a new type that shares a name with an asserted one
    /// does not pass on the other's line.
    #[test]
    fn every_public_type_is_asserted() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let missing = unityrs_testkit::public_types::unasserted(&src);
        assert!(missing.is_empty(), "not asserted Send + Sync: {missing:?}");
    }
}
