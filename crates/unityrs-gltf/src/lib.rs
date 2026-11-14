//! Editable, charged glTF-space data and a two-pass GLB writer.
//!
//! Tables and attributes retain their own charges, including when moved out of
//! a scene. Use [`array`] and [`text`] to build them with work accounting.
//! The writer borrows the model, lays out buffer views, measures JSON, allocates
//! exactly that charged size, and streams BIN through fixed 64 KiB stack storage.
//! It never builds a BIN-sized buffer. Caller-owned output storage must itself
//! be metered or bounded (a file is suitable); partial output on I/O failure is
//! the caller's to discard. No allocation is hidden inside `contain`.
//!
//! Ordering is model order, with fixed schema key and attribute order. JSON
//! numbers use Rust's locale-independent shortest round-trip `Display` for f64
//! on the pinned toolchain; negative zero is canonicalised to 0. Bounds describe
//! the actual rounded f32 payload. Non-finite JSON numbers are never emitted.
//!
//! [`Options::normalise`] defaults to true. Normals and tangent xyz are made
//! unit length, tangent w is made ±1, negative/non-finite weights become zero,
//! and zero weight rows become [1,0,0,0]. Changed scalar components are counted
//! once per emitted shared accessor, in the report and asset extras. Non-finite f32
//! positions refuse their primitives; invalid normals drop NORMAL and TANGENT.
//! Raw mode preserves f32 bits in BIN and may fail validation. [`collect`]
//! converts Unity coordinates and UVs into this model using a typed [`Profile`].
//! Material mapping is separate work. Per-instance morph weights live on nodes.
#![forbid(unsafe_code)]
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::indexing_slicing
    )
)]

mod collect;
pub use collect::collect;
mod image;
mod json;
mod layout;
mod model;
mod normalise;
mod profile;
pub use profile::*;
mod schema;
mod validate;
mod writer;

pub use layout::{BufferView, Issue, Layout, Reason, Report, layout};
pub use model::*;
use std::{fmt, sync::Arc};
use unityrs_core::limits::{ChargeError, Meter, MeteredVec};
pub use writer::write_glb;

/// A bounded diagnostic; no input-sized error strings are allocated.
#[derive(Debug)]
pub enum GltfError {
    /// Memory, work or allocator refusal, preserving its provenance.
    Charge(ChargeError),
    /// Unity mesh decoding failed, with resource provenance intact.
    Mesh(unityrs_mesh::MeshError),
    /// Malformed model or a GLB beyond its 32-bit length limit.
    Invalid(&'static str),
    /// The output sink failed. It may contain a partial file.
    Io(std::io::Error),
}
const _: () = send_sync::<GltfError>();
impl fmt::Display for GltfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Charge(e) => e.fmt(f),
            Self::Mesh(e) => e.fmt(f),
            Self::Invalid(s) => f.write_str(s),
            Self::Io(e) => e.fmt(f),
        }
    }
}
impl std::error::Error for GltfError {}
impl From<ChargeError> for GltfError {
    fn from(e: ChargeError) -> Self {
        Self::Charge(e)
    }
}
impl From<std::io::Error> for GltfError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
const fn send_sync<T: Send + Sync + ?Sized>() {}

/// Copy a slice into storage charged before allocation, counting copy work.
pub fn array<T: Copy>(meter: &Arc<Meter>, values: &[T]) -> Result<MeteredVec<T>, GltfError> {
    meter.count_decode((values.len() as u64).saturating_mul(std::mem::size_of::<T>() as u64))?;
    let mut out = MeteredVec::with_capacity(meter, values.len())?;
    out.extend_from_slice(values)?;
    Ok(out)
}
/// A UTF-8 name or extras string in independently charged storage.
pub fn text(meter: &Arc<Meter>, value: &str) -> Result<Text, GltfError> {
    Ok(Text(array(meter, value.as_bytes())?))
}

#[cfg(test)]
mod tests {
    #[test]
    fn public_types() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let missing = unityrs_testkit::public_types::unasserted(&src);
        assert!(missing.is_empty(), "{missing:?}");
    }
    #[test]
    fn waits() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let found = unityrs_testkit::waits::waits(&src);
        assert!(found.is_empty(), "{found:?}");
    }
}
