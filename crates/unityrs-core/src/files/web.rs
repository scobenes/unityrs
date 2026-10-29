//! `WebFile` (`files/WebFile.py:15-56`): `UnityWebData`/`TuanjieWebData`,
//! plain, gzip or brotli. P3: until then [`WebFile::open`] returns
//! [`crate::Unsupported::WebFile`]. The type exists now so `File` and the
//! dump's `kind: "web"` records have their final shape.
//!
//! Owner: package C (P3 work; nothing to implement in P1 beyond the refusal).

// STUB(C): delete this allow when the module is implemented.
#![allow(
    clippy::todo,
    clippy::needless_pass_by_value,
    clippy::must_use_candidate,
    clippy::missing_panics_doc,
    unused_variables,
    dead_code
)]

use std::sync::Arc;

use bytes::Bytes;

use crate::error::Error;
use crate::files::{DirEntry, Parent};
use crate::options::FileContext;
use crate::raw::RawStr;

/// How a `WebFile` is packed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WebPacker {
    /// Plain.
    None,
    /// gzip.
    Gzip,
    /// brotli.
    Brotli,
}

/// A `WebFile`.
pub struct WebFile {
    name: RawStr,
    signature: RawStr,
    packer: WebPacker,
    directory: Box<[DirEntry]>,
    data: Bytes,
    parent: Option<Parent>,
    ctx: Arc<FileContext>,
}

impl std::fmt::Debug for WebFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebFile")
            .field("name", &self.name)
            .field("signature", &self.signature)
            .field("packer", &self.packer)
            .finish_non_exhaustive()
    }
}

impl WebFile {
    /// Opens a `WebFile`. P1: always `Unsupported::WebFile`.
    pub fn open(
        name: RawStr,
        data: Bytes,
        parent: Option<Parent>,
        ctx: &Arc<FileContext>,
    ) -> Result<Arc<Self>, Error> {
        todo!()
    }

    /// The signature.
    pub fn signature(&self) -> &RawStr {
        &self.signature
    }

    /// The packer.
    pub fn packer(&self) -> WebPacker {
        self.packer
    }

    /// The directory (`flags` always `None`).
    pub fn directory(&self) -> &[DirEntry] {
        &self.directory
    }
}
