// Ported from UnityPy 1.25.2 (MIT): UnityPy/files/WebFile.py. See THIRD_PARTY_LICENSES.
//! `WebFile` (`files/WebFile.py:15-56`): `UnityWebData`/`TuanjieWebData`,
//! plain, gzip or brotli. Inflation is eager, bounded by `max_inflate` and
//! charged as decode work; directory members are parsed once on demand.
//! [`WebFile::load_all`] provides UnityPy's eager directory-order load.

use std::sync::Arc;

use bytes::Bytes;

use crate::detect::{DetectError, check_entry_type};
use crate::error::{Error, Unsupported};
use crate::files::{File, FileSlot, ParentKind, parse_file};
use crate::limits::{GrantScope, MeteredVec, heap_cost};
use crate::once::{CellError, get_or_publish_result};
use crate::stream::{Endian, Reader, ReaderKind};
use crate::typetree::parse::string_to_null;
mod inflate;
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
    pub(super) changed: std::sync::atomic::AtomicBool,
    name: RawStr,
    signature: RawStr,
    packer: WebPacker,
    directory: Box<[DirEntry]>,
    data: Bytes,
    files: Box<[FileSlot]>,
    pub(super) entries: Box<[super::teardown::FileCell]>,
    reader: ReaderKind,
    source_depth: u32,
    this: std::sync::Weak<Self>,
    parent: Option<Parent>,
    ctx: Arc<FileContext>,
    /// The handle, its directory and its inflated data, charged to the input.
    /// Last so all the storage it covers is freed before its refund.
    pub(super) charge: Charge,
}

impl Drop for WebFile {
    fn drop(&mut self) {
        super::teardown::drain(&mut self.entries, &self.charge);
    }
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
    /// Opens a WebFile from bytes, inflating gzip or brotli under the input meter.
    pub fn open(
        name: RawStr,
        data: Bytes,
        parent: Option<Parent>,
        meter: &Arc<Meter>,
        ctx: &Arc<FileContext>,
    ) -> Result<Arc<Self>, Error> {
        Self::open_at_depth(name, data, parent, meter, ctx, ReaderKind::Memory, 0)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn open_at_depth(
        name: RawStr,
        data: Bytes,
        parent: Option<Parent>,
        meter: &Arc<Meter>,
        ctx: &Arc<FileContext>,
        reader: ReaderKind,
        source_depth: u32,
    ) -> Result<Arc<Self>, Error> {
        let mut charge = meter.charge(heap_cost((size_of::<Self>() + 16) as u64))?;
        let (data, packer, reader) = if data.starts_with(&[31, 139]) {
            (
                inflate::gzip(&data, meter, ctx)?,
                WebPacker::Gzip,
                ReaderKind::Memory,
            )
        } else if data.get(32..38) == Some(b"brotli") {
            (
                inflate::brotli(&data, meter, ctx)?,
                WebPacker::Brotli,
                ReaderKind::Memory,
            )
        } else {
            (data, WebPacker::None, reader)
        };
        let mut r = Reader::with_kind(&data, Endian::Little, reader);
        // files/WebFile.py:39-56: prefix signature, signed table and strict UTF-8.
        let signature = string_to_null(&mut r, &data, meter)?;
        if !signature.as_bytes().starts_with(b"UnityWebData")
            && !signature.as_bytes().starts_with(b"TuanjieWebData")
        {
            return Err(inflate::bad("invalid signature"));
        }
        let end = i128::from(r.read_i32()?);
        let mut directory = MeteredVec::new(meter);
        while r.position() < end {
            meter.count_decode(12)?;
            let offset = i64::from(r.read_i32()?);
            let size = i64::from(r.read_i32()?);
            let n = r.read_i32()?;
            // Tier U: below -1 can rewind into this row forever. -1 is
            // Tier A and reads the remaining bytes through the reader.
            if n < -1 {
                return Err(Error::Unsupported(Unsupported::WebFileNameLength));
            }
            let range = r.read_range(i64::from(n))?;
            let name = data.slice(range);
            meter.count_decode(name.len() as u64)?;
            std::str::from_utf8(&name).map_err(|_| inflate::bad("invalid UTF-8 member name"))?;
            directory.push(DirEntry {
                path: RawStr::from_shared(name),
                offset,
                size,
                flags: None,
            })?;
        }
        let files = super::bundle::open::files_dict(&directory, meter)?;
        let mut entries = MeteredVec::new(meter);
        for _ in 0..directory.len() {
            entries.push(super::teardown::FileCell::new())?;
        }
        let directory = super::bundle::open::keep(directory, &mut charge)?;
        let files = super::bundle::open::keep(files, &mut charge)?;
        let entries = super::bundle::open::keep(entries, &mut charge)?;
        Ok(Arc::new_cyclic(|this| Self {
            changed: std::sync::atomic::AtomicBool::new(false),
            name,
            signature,
            packer,
            directory,
            data,
            files,
            entries,
            reader,
            source_depth,
            parent,
            charge,
            ctx: ctx.clone(),
            this: this.clone(),
        }))
    }

    /// UnityPy's files dict: first key position, last row value.
    #[must_use]
    pub fn files(&self) -> &[FileSlot] {
        &self.files
    }

    /// The bytes read by the file's own reader, with signed offsets and sizes.
    pub fn entry_bytes(&self, i: usize) -> Result<Bytes, Error> {
        let row = self
            .directory
            .get(i)
            .ok_or_else(|| inflate::bad("no directory row"))?;
        let mut r = Reader::with_kind(&self.data, Endian::Little, self.reader);
        r.set_position(i128::from(row.offset))?;
        Ok(self.data.slice(r.read_range(row.size)?))
    }

    /// Detect and parse an entry once; nested files share this input's meter.
    pub fn entry(&self, i: usize) -> Result<File, Error> {
        let cell = self
            .entries
            .get(i)
            .ok_or_else(|| inflate::bad("no directory row"))?;
        get_or_publish_result(cell, self.meter(), || {
            let scope = GrantScope::enter(self.meter());
            let got = self.parse_entry(i);
            let granted = scope.finish(self.meter());
            if got.as_ref().is_err_and(|e| !e.is_transient()) {
                self.meter().add_work(granted);
            }
            got
        })
        .cloned()
    }
    fn parse_entry(&self, i: usize) -> Result<File, Error> {
        let row = self
            .directory
            .get(i)
            .ok_or_else(|| inflate::bad("no directory row"))?;
        let got = (|| {
            let data = self.entry_bytes(i)?;
            let ty =
                check_entry_type(&data, &self.ctx.limits, self.meter()).map_err(|e| match e {
                    DetectError::Work(l) => Error::Limit(l),
                    e => Error::Detect(e),
                })?;
            let parent = Parent {
                name: self.name.clone(),
                kind: ParentKind::Web {
                    web: self.this.clone(),
                },
                depth: self
                    .parent
                    .as_ref()
                    .map_or(self.source_depth, |p| p.depth)
                    .saturating_add(1),
                pinned: if self.packer == WebPacker::None {
                    self.parent.as_ref().map_or(self.data.len() as u64, |p| {
                        p.pinned.max(self.data.len() as u64)
                    })
                } else {
                    self.data.len() as u64
                },
            };
            parse_file(
                row.path.clone(),
                data,
                ty,
                Some(parent),
                self.meter(),
                &self.ctx,
            )
        })();
        got.map_err(|e| match e {
            Error::Entry { .. } | Error::Limit(_) | Error::Budget(_) | Error::Allocator(_) => e,
            e => Error::Entry {
                name: row.path.clone(),
                source: Arc::new(e),
            },
        })
    }
    /// Eagerly load every row in directory order, including overwritten keys.
    pub fn load_all(&self) -> Result<(), Error> {
        for i in 0..self.directory.len() {
            match self.entry(i)? {
                File::Bundle(b) => b.load_all()?,
                File::Web(w) => w.load_all()?,
                _ => {}
            }
        }
        Ok(())
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
