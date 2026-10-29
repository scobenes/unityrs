// Ported from UnityPy 1.25.2 (MIT): UnityPy/files/WebFile.py. See THIRD_PARTY_LICENSES.
//! `WebFile` (`files/WebFile.py:15-56`): `UnityWebData`/`TuanjieWebData`,
//! plain, gzip or brotli. P3: until then [`WebFile::open`] returns
//! [`crate::Unsupported::WebFile`]. The type exists now so `File` and the
//! dump's `kind: "web"` records have their final shape.
//!
//! Owner: package C (P3 work; nothing to implement in P1 beyond the refusal).

use std::sync::Arc;

use bytes::Bytes;

use crate::error::{Error, Unsupported};
use crate::files::{DirEntry, Parent};
use crate::limits::{Charge, Meter};
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
    /// The handle, its directory and its inflated data, charged to the input.
    charge: Charge,
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
        meter: &Arc<Meter>,
        ctx: &Arc<FileContext>,
    ) -> Result<Arc<Self>, Error> {
        let _ = (name, data, parent, meter, ctx);
        Err(Error::Unsupported(Unsupported::WebFile))
    }

    /// The meter of the input it belongs to.
    #[must_use]
    pub fn meter(&self) -> &Arc<Meter> {
        self.charge.meter()
    }

    /// The name it was opened under.
    #[must_use]
    pub fn name(&self) -> &RawStr {
        &self.name
    }

    /// Its decompressed bytes.
    #[must_use]
    pub fn data(&self) -> &Bytes {
        &self.data
    }

    /// The container it is an entry of.
    #[must_use]
    pub fn parent(&self) -> Option<&Parent> {
        self.parent.as_ref()
    }

    /// The context it was opened with.
    #[must_use]
    pub fn context(&self) -> &Arc<FileContext> {
        &self.ctx
    }

    /// The signature.
    #[must_use]
    pub fn signature(&self) -> &RawStr {
        &self.signature
    }

    /// The packer.
    #[must_use]
    pub fn packer(&self) -> WebPacker {
        self.packer
    }

    /// The directory (`flags` always `None`).
    #[must_use]
    pub fn directory(&self) -> &[DirEntry] {
        &self.directory
    }
}
