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

/// Every public type of the core is `Send + Sync` (plan section 3.16): a pool
/// or a binding may share any of them. This fails to compile if one stops
/// being so, and `tests::every_public_type_is_asserted` fails if a public
/// type is added to the crate without a line here.
const _: () = {
    const fn send_sync<T: Send + Sync + ?Sized>() {}
    // budget
    send_sync::<budget::Reservation>();
    send_sync::<budget::BudgetExceeded>();
    send_sync::<budget::Counting>();
    send_sync::<budget::DecodeBuf<'static>>();
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
    send_sync::<env::EnvOptions>();
    send_sync::<env::EnvStats>();
    send_sync::<env::Environment>();
    send_sync::<dyn env::CabResolver>();
    send_sync::<dyn env::Fetch>();
    send_sync::<env::DerefError>();
    send_sync::<env::pptr::ExternalIndexError>();
    send_sync::<env::PPtr>();
    send_sync::<env::Cab>();
    send_sync::<env::RawCab>();
    send_sync::<env::Registry>();
    // error
    send_sync::<Error>();
    send_sync::<LimitKind>();
    send_sync::<NotFound>();
    send_sync::<Unsupported>();
    send_sync::<LimitError>();
    // files
    send_sync::<files::LayoutKind>();
    send_sync::<files::BlockInfo>();
    send_sync::<BundleFile>();
    send_sync::<files::BundleHeader>();
    send_sync::<files::DirEntry>();
    send_sync::<files::FileSlot>();
    send_sync::<File>();
    send_sync::<files::ParentKind>();
    send_sync::<files::Parent>();
    send_sync::<RawFile>();
    send_sync::<files::External>();
    send_sync::<files::ObjectInfo>();
    send_sync::<files::ScriptRef>();
    send_sync::<SerializedFile>();
    send_sync::<files::SerializedHeader>();
    send_sync::<files::SerializedType>();
    send_sync::<files::WebPacker>();
    send_sync::<WebFile>();
    // limits, object, options, provider, raw, stream
    send_sync::<limits::Metered>();
    send_sync::<limits::Limits>();
    send_sync::<limits::Meter>();
    send_sync::<ObjectRef>();
    send_sync::<FileContext>();
    send_sync::<provider::TreeSource>();
    send_sync::<provider::ProviderChain>();
    send_sync::<provider::ProviderError>();
    send_sync::<provider::ScriptIdent>();
    send_sync::<provider::TreeRef>();
    send_sync::<provider::TreeRequest<'static>>();
    send_sync::<dyn provider::TypeTreeProvider>();
    send_sync::<RawStr>();
    send_sync::<Bytes>();
    send_sync::<stream::Endian>();
    send_sync::<stream::Reader<'static>>();
    // typetree
    send_sync::<typetree::TreeError>();
    send_sync::<typetree::NodeId>();
    send_sync::<typetree::NodeList>();
    send_sync::<typetree::NodeRow>();
    send_sync::<typetree::Kind>();
    send_sync::<typetree::Shape>();
    send_sync::<typetree::NodeInfo>();
    send_sync::<typetree::Tree>();
    // value, version
    send_sync::<PrimArray>();
    send_sync::<Value>();
    send_sync::<Map>();
    send_sync::<Parsed>();
    send_sync::<version::FlagsClass>();
    send_sync::<version::Packed>();
    send_sync::<version::VersionError>();
    send_sync::<version::VersionSource>();
    send_sync::<version::ResolvedVersion>();
    send_sync::<UnityVersion>();
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

    fn sources(dir: &Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                sources(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(std::fs::read_to_string(&path).unwrap());
            }
        }
    }

    /// The `const _` block above names every `pub struct`, `pub enum` and
    /// `pub trait` of the crate, so none can lose `Send + Sync` unnoticed.
    #[test]
    fn every_public_type_is_asserted() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let lib = std::fs::read_to_string(src.join("lib.rs")).unwrap();
        let mut files = Vec::new();
        sources(&src, &mut files);
        let mut missing = Vec::new();
        for text in &files {
            for line in text.lines() {
                for kw in ["pub struct ", "pub enum ", "pub trait "] {
                    let Some(rest) = line.strip_prefix(kw) else {
                        continue;
                    };
                    let name: String = rest
                        .chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '_')
                        .collect();
                    let asserted = [
                        format!("::{name}>();"),
                        format!("<{name}>();"),
                        format!("::{name}<'static>>();"),
                    ]
                    .iter()
                    .any(|p| lib.contains(p.as_str()));
                    if !asserted {
                        missing.push(name);
                    }
                }
            }
        }
        assert!(missing.is_empty(), "not asserted Send + Sync: {missing:?}");
    }
}
