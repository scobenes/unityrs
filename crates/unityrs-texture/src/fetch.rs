//! A texture's inline bytes, or its streamed resource. Own bundle rows are
//! searched before the Environment, candidates in order, containers from
//! inner to outer, and directory rows from last to first. Each core call
//! remains separately observable through [`Hooks`].

use std::sync::Arc;

use unityrs_core::env::{Environment, resource_candidates};
use unityrs_core::files::{BundleFile, File, ParentKind, SerializedFile};
use unityrs_core::limits::{ChargeError, Meter};
use unityrs_core::pystr::{lower_eq, ntpath_basename, simplify_name_vec};
use unityrs_core::{Bytes, RawStr};

/// `m_StreamData`, retaining signed and wide integers until read.
#[derive(Clone, Debug)]
pub struct Stream {
    /// The resource path as read.
    pub path: RawStr,
    /// The reader's offset.
    pub offset: i128,
    /// The read size; -1 reads to the end.
    pub size: i128,
}

/// The core range, or a Python slice of the whole resource.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Span {
    /// Offset and size, both representable by the core range API.
    Range(u64, u64),
    /// Signed or wider indices, clamped when the resource length is known.
    Slice(i128, i128),
}

impl Stream {
    /// The memory reader's `read(size)` at `offset`.
    #[must_use]
    pub fn span(&self) -> Span {
        if self.size == 0 {
            return Span::Range(0, 0);
        }
        if self.size == -1 {
            return Span::Slice(self.offset, i128::MAX);
        }
        let end = self.offset.checked_add(self.size);
        match (
            u64::try_from(self.offset),
            u64::try_from(self.size),
            end.and_then(|e| u64::try_from(e).ok()),
        ) {
            (Ok(o), Ok(n), Some(_)) => Span::Range(o, n),
            _ => Span::Slice(self.offset, end.unwrap_or(i128::MAX)),
        }
    }
}

/// Python's `data[start:stop]`, including negative indices and clamping.
#[must_use]
pub fn python_slice(len: usize, start: i128, stop: i128) -> std::ops::Range<usize> {
    let n = i128::try_from(len).unwrap_or(i128::MAX);
    let at = |i: i128| {
        let i = if i < 0 { i.saturating_add(n) } else { i };
        usize::try_from(i.clamp(0, n)).unwrap_or(len)
    };
    let (a, b) = (at(start), at(stop));
    a..b.max(a)
}

/// Fetch failures preserve whether the Environment or an own row failed,
/// so exporters can assign their own resource and data skip reasons.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A name, lookup work, or other metered helper was refused.
    #[error(transparent)]
    Charge(#[from] ChargeError),
    /// Opening or reading an own directory row failed.
    #[error("texture data: {0}")]
    Data(#[source] unityrs_core::Error),
    /// The Environment failed to resolve or read the resource.
    #[error("texture resource: {error}")]
    Resource {
        /// The original Environment failure.
        #[source]
        error: unityrs_core::Error,
        /// The original stream path, sharing its object's storage.
        path: RawStr,
    },
    /// An own row is a SerializedFile, which cannot be read as bytes.
    #[error("texture resource is a SerializedFile")]
    Serialized,
    /// An own row is a WebFile, unsupported as a resource reader.
    #[error("texture resource is a WebFile")]
    Web,
}

/// Per-core-call hooks. Map errors *inside* the caller's refusal probe;
/// Environment calls can additionally inspect evictions after the probe.
pub trait Hooks {
    /// The exporter's classified error.
    type Error;

    /// One core call, independently probed.
    fn probe<T>(&self, f: impl FnOnce() -> Result<T, Error>) -> Result<T, Self::Error>
    where
        Self: Sized;

    /// One Environment call, independently probed and observed.
    fn env_call<T>(&self, f: impl FnOnce() -> Result<T, Error>) -> Result<T, Self::Error>
    where
        Self: Sized;

    /// A failed own row whose type is known after its successful core call.
    fn row_error(&self, error: Error) -> Self::Error;
}

struct Direct;

impl Hooks for Direct {
    type Error = Error;

    fn probe<T>(&self, f: impl FnOnce() -> Result<T, Error>) -> Result<T, Error> {
        f()
    }

    fn env_call<T>(&self, f: impl FnOnce() -> Result<T, Error>) -> Result<T, Error> {
        f()
    }

    fn row_error(&self, error: Error) -> Error {
        error
    }
}

fn parent_bundle(kind: Option<&ParentKind>) -> Option<Arc<BundleFile>> {
    match kind? {
        ParentKind::Bundle { bundle, .. } => bundle.upgrade(),
        ParentKind::Web => None,
    }
}

fn read_span(s: &Stream) -> (u64, u64, Option<(i128, i128)>) {
    match s.span() {
        Span::Range(offset, size) => (offset, size, None),
        Span::Slice(start, stop) => (0, u64::MAX, Some((start, stop))),
    }
}

fn sliced(bytes: Bytes, slice: Option<(i128, i128)>) -> Bytes {
    match slice {
        None => bytes,
        Some((start, stop)) => bytes.slice(python_slice(bytes.len(), start, stop)),
    }
}

fn own_chain<H: Hooks>(
    from: &SerializedFile,
    s: &Stream,
    meter: &Arc<Meter>,
    hooks: &H,
) -> Result<Option<Bytes>, H::Error> {
    let work = |n: usize| {
        hooks.probe(|| {
            meter
                .count_decode(u64::try_from(n).unwrap_or(u64::MAX))
                .map_err(Error::Charge)
        })
    };
    work(s.path.len())?;
    for candidate in &resource_candidates(&s.path) {
        work(candidate.len())?;
        let name = hooks.probe(|| candidate.joined_vec(meter).map_err(Error::Charge))?;
        let key =
            hooks.probe(|| simplify_name_vec(name.as_slice(), meter).map_err(Error::Charge))?;
        let mut container = parent_bundle(from.parent().map(|p| &p.kind));
        while let Some(b) = container {
            work(b.directory().len().saturating_add(1))?;
            for (i, row) in b.directory().iter().enumerate().rev() {
                work(row.path.len())?;
                if !lower_eq(ntpath_basename(row.path.as_bytes()), key.as_slice()) {
                    continue;
                }
                match hooks.probe(|| b.entry(i).map_err(Error::Data))? {
                    File::Raw(_) => {
                        let (offset, size, slice) = read_span(s);
                        let bytes = hooks.probe(|| {
                            b.entry_range(i, offset..offset.saturating_add(size))
                                .map_err(Error::Data)
                        })?;
                        return Ok(Some(sliced(bytes, slice)));
                    }
                    File::Serialized(_) => return Err(hooks.row_error(Error::Serialized)),
                    File::Web(_) => return Err(hooks.row_error(Error::Web)),
                    File::Bundle(_) => {}
                }
            }
            container = parent_bundle(b.parent().map(|p| &p.kind));
        }
    }
    Ok(None)
}

/// Inline data wins. Otherwise search own rows, then the Environment.
/// Missing streams and empty paths give empty data. Returned bytes share
/// their existing allocation; all name and resource allocations are
/// charged by the core before allocation, and lookup work is metered.
pub fn fetch(
    env: &Environment,
    from: &Arc<SerializedFile>,
    inline: &Bytes,
    stream: Option<&Stream>,
    meter: &Arc<Meter>,
) -> Result<Bytes, Error> {
    fetch_with(env, from, inline, stream, meter, &Direct)
}

/// [`fetch`] with separately classified probes around every core call.
pub fn fetch_with<H: Hooks>(
    env: &Environment,
    from: &Arc<SerializedFile>,
    inline: &Bytes,
    stream: Option<&Stream>,
    meter: &Arc<Meter>,
    hooks: &H,
) -> Result<Bytes, H::Error> {
    if !inline.is_empty() {
        return Ok(inline.clone());
    }
    let Some(s) = stream.filter(|s| !s.path.is_empty()) else {
        return Ok(Bytes::new());
    };
    if let Some(bytes) = own_chain(from, s, meter, hooks)? {
        return Ok(bytes);
    }
    let (offset, size, slice) = read_span(s);
    hooks.env_call(|| {
        env.resource(from, &s.path, offset, size)
            .map(|bytes| sliced(bytes, slice))
            .map_err(|error| Error::Resource {
                error,
                path: s.path.clone(),
            })
    })
}

const _: () = crate::send_sync::<Stream>();
const _: () = crate::send_sync::<Span>();
const _: () = crate::send_sync::<Error>();
const _: () = crate::send_sync::<dyn Hooks<Error = Error> + Send + Sync>();
