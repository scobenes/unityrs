//! Errors shared by specification-defined HDR block decoders.

/// A reserved or structurally illegal HDR encoding. These are content
/// errors after a length-only plan succeeds; no partial image escapes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum HdrError {
    /// BC6H's four reserved mode codes.
    #[error("reserved HDR block mode")]
    ReservedMode,
    /// An invalid ASTC mode, extent, endpoint budget or plane layout.
    #[error("illegal HDR block encoding")]
    InvalidBlock,
}

const _: () = crate::send_sync::<HdrError>();
