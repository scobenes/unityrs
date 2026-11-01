// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/ImportHelper.py, UnityPy/files/File.py. See THIRD_PARTY_LICENSES.
//! Files: what `ImportHelper.parse_file` builds (`helpers/ImportHelper.py:127-156`).
//!
//! A [`File`] is a handle (`Arc`) on an immutable, parsed file. Containers
//! decode lazily and parse each entry once ([`BundleFile::entry`]); the
//! UnityPy load order, where every entry is parsed and a failing entry fails
//! the bundle, is [`BundleFile::load_all`].
//!
//! **Memory.** Every file charges its input's [`Meter`] for itself and for
//! every table it keeps, before it allocates them (plan section 3.15): a
//! top-level load makes the meter and passes it to [`parse_file`], and a
//! container passes its own to each entry it parses, so one input's files
//! share one bound however many there are. Each file keeps its [`Charge`]
//! (the bytes return when the file drops) and answers [`File::meter`].
//!
//! Owner: package C.

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

use crate::detect::{FileType, ParsedAs, parsed_as};
use crate::error::{Error, LimitError, LimitKind, Unsupported};
use crate::limits::{Charge, ChargeError, Meter, heap_cost};
use crate::options::FileContext;
use crate::raw::RawStr;
use crate::stream::ReaderKind;

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

impl File {
    /// The meter of the input this file belongs to.
    #[must_use]
    pub fn meter(&self) -> &Arc<Meter> {
        match self {
            Self::Serialized(f) => f.meter(),
            Self::Bundle(f) => f.meter(),
            Self::Web(f) => f.meter(),
            Self::Raw(f) => f.meter(),
        }
    }

    /// What `check_file_type` called the bytes [`parse_file`] built this
    /// from, kept so a caller never detects them again (a gzip entry's
    /// detection inflates it): each kind but a reader has one type, and a
    /// reader keeps its own ([`RawFile::file_type`]).
    #[must_use]
    pub fn file_type(&self) -> FileType {
        match self {
            Self::Serialized(_) => FileType::AssetsFile,
            Self::Bundle(_) => FileType::BundleFile,
            Self::Web(_) => FileType::WebFile,
            Self::Raw(f) => f.file_type(),
        }
    }
}

/// A file UnityPy keeps as a reader.
#[derive(Debug)]
pub struct RawFile {
    /// The name it was loaded under.
    pub name: RawStr,
    /// Its bytes (a slice of the parent's buffer for an entry).
    pub data: Bytes,
    /// What `check_file_type` called `data`: `ResourceFile`, a zip entry's
    /// `Zip`, or an `AssetsFile` the name rule keeps as a reader.
    ty: FileType,
    /// The reader it is: a resource read from it follows its rules.
    reader: ReaderKind,
    /// The handle's own allocation, charged to the input.
    charge: Charge,
}

impl RawFile {
    /// A raw file of `data` under `name`, which `check_file_type` called
    /// `ty`, its allocation charged to `meter` first (`data` is a slice of a
    /// buffer already charged, or the caller's). A memory reader, as UnityPy
    /// keeps bytes and every entry of a container.
    pub fn new(
        name: RawStr,
        data: Bytes,
        ty: FileType,
        meter: &Arc<Meter>,
    ) -> Result<Arc<Self>, ChargeError> {
        Self::with_reader(name, data, ty, meter, ReaderKind::Memory)
    }

    /// [`Self::new`] as the reader `reader` ([`ReaderKind::Stream`] for a
    /// top-level file loaded from a path).
    pub fn with_reader(
        name: RawStr,
        data: Bytes,
        ty: FileType,
        meter: &Arc<Meter>,
        reader: ReaderKind,
    ) -> Result<Arc<Self>, ChargeError> {
        let charge = meter.charge(heap_cost(
            u64::try_from(std::mem::size_of::<Self>() + 16).unwrap_or(u64::MAX),
        ))?;
        Ok(Arc::new(Self {
            name,
            data,
            ty,
            reader,
            charge,
        }))
    }

    /// The reader UnityPy keeps it as, whose rules a resource read from it
    /// follows (`ResourceReader._get_resource_data`).
    #[must_use]
    pub fn reader_kind(&self) -> ReaderKind {
        self.reader
    }

    /// The meter of the input it belongs to.
    #[must_use]
    pub fn meter(&self) -> &Arc<Meter> {
        self.charge.meter()
    }

    /// What `check_file_type` called its bytes.
    #[must_use]
    pub fn file_type(&self) -> FileType {
        self.ty
    }
}

/// What a child knows of its container. A file loaded at top level has none
/// (`parent: None`): UnityPy makes the `Environment` that loaded it its
/// parent (`environment.py:154`), which is the container `PPtr.deref` scans
/// (plan section 3.13, rule 4). The input's meter travels separately, as
/// every file's [`File::meter`], top-level ones included.
#[derive(Clone, Debug)]
pub struct Parent {
    /// The container's name.
    pub name: RawStr,
    /// The container's kind.
    pub kind: ParentKind,
    /// How deep the child is: 1 for an entry of a top-level container. A
    /// container at [`crate::limits::Limits::max_container_depth`] refuses to
    /// parse an entry that is itself a container (`LimitKind::ContainerDepth`).
    pub depth: u32,
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
/// ([`crate::detect::parsed_as`]), from bytes (UnityPy's memory reader). A
/// file with a `parent` is an entry. Everything it keeps is charged to
/// `meter`, the input's.
pub fn parse_file(
    name: RawStr,
    data: Bytes,
    ty: FileType,
    parent: Option<Parent>,
    meter: &Arc<Meter>,
    ctx: &Arc<FileContext>,
) -> Result<File, Error> {
    parse_file_with_reader(name, data, ty, parent, meter, ctx, ReaderKind::Memory)
}

/// [`parse_file`] by the rules of the reader UnityPy reads the file with:
/// [`ReaderKind::Stream`] for a top-level file loaded from a path
/// (`environment.py:136-139`; [`crate::env::Source::Path`]), whose
/// `SerializedFile`, bundle header or kept reader follows them. A
/// container's entries are memory whatever its reader. A `WebFile` is P3.
pub fn parse_file_with_reader(
    name: RawStr,
    data: Bytes,
    ty: FileType,
    parent: Option<Parent>,
    meter: &Arc<Meter>,
    ctx: &Arc<FileContext>,
    reader: ReaderKind,
) -> Result<File, Error> {
    // UnityPy helpers/ImportHelper.py:127-156; a zip input is refused (P3).
    let kind = parsed_as(ty, name.as_bytes(), parent.is_some());
    if matches!(kind, ParsedAs::BundleFile | ParsedAs::WebFile)
        && let Some(p) = &parent
        && p.depth > ctx.limits.max_container_depth
    {
        return Err(Error::Limit(LimitError {
            kind: LimitKind::ContainerDepth,
            value: u64::from(p.depth),
            max: u64::from(ctx.limits.max_container_depth),
        }));
    }
    Ok(match kind {
        ParsedAs::SerializedFile => File::Serialized(SerializedFile::parse_with_reader(
            name, data, parent, meter, ctx, reader,
        )?),
        ParsedAs::BundleFile => File::Bundle(BundleFile::open_with_reader(
            name, data, parent, meter, ctx, reader,
        )?),
        ParsedAs::WebFile => File::Web(WebFile::open(name, data, parent, meter, ctx)?),
        ParsedAs::Zip => return Err(Error::Unsupported(Unsupported::Zip)),
        ParsedAs::Raw | ParsedAs::Error => {
            File::Raw(RawFile::with_reader(name, data, ty, meter, reader)?)
        }
    })
}
