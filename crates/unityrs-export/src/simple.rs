// Ported from UnityPy 1.25.2 (MIT): UnityPy/tools/extractor.py. See THIRD_PARTY_LICENSES.
//! Single-object payloads, ready for a caller's file sink. `TextAsset` and Font
//! follow tools/extractor.py:165-187; MonoBehaviour follows
//! tools/extractor.py:210-224, including its semantic `.bin` fallback.
//! `VideoClip` is Tier X: UnityPy's dispatch has no `VideoClip` entry
//! (tools/extractor.py:307-319). Its `StreamedResource` fields are format facts,
//! read through the core's resource resolver, with no transcoding.
//!
//! The object/tree/resource reads charge the source file's meter. Every
//! payload and JSON traversal stack charges `meter` before fallible growth;
//! the returned payload owns its charge. Budget/allocator/limit failures
//! propagate, never becoming a successful raw fallback. No filesystem naming
//! or extraction loop lives here (the extractor is separate work).
use crate::pyjson::{self, Options, Sink};
use std::sync::Arc;
use unityrs_core::limits::{ChargeError, Meter, MeteredVec};
use unityrs_core::typetree::TreeError;
use unityrs_core::walk::{ParseError, ReadDetail};
use unityrs_core::{ObjectRef, Parsed, PrimArray, Value, env::Environment};

/// Why a MonoBehaviour is emitted as raw `.bin` instead of `.json`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fallback {
    NoTypeTree,
    ReadFailed,
    JsonUnwritable,
}
/// One file's bytes and suffix. An empty Font produces no file (`None`).
#[derive(Debug)]
pub struct Export<'a> {
    pub extension: &'a str,
    pub bytes: MeteredVec<u8>,
    pub fallback: Option<Fallback>,
}
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no simple exporter for class {0}")]
    UnsupportedClass(i32),
    #[error("missing or invalid export field {0}")]
    Field(&'static str),
    #[error("typetree read {actual} bytes, expected {expected}")]
    CheckRead { expected: u32, actual: i64 },
    #[error(transparent)]
    Parse(#[from] ParseError),
    #[error(transparent)]
    Core(#[from] unityrs_core::Error),
    #[error(transparent)]
    Charge(#[from] ChargeError),
    #[error(transparent)]
    Json(#[from] pyjson::WriteError),
}
/// The simple-export dispatch, keyed by Unity class id. This does not widen
/// the batch engine's output kinds or the CLI's commands.
#[must_use]
pub const fn supports(class_id: i32) -> bool {
    matches!(class_id, 49 | 128 | 114 | 329)
}
/// Export an object. `extension` is the suffix already split from the
/// destination, including its dot. `TextAsset` keeps it, defaulting an empty
/// suffix to `.txt`; Font uses `.otf`/`.ttf`, MonoBehaviour `.json`/`.bin`.
/// `VideoClip` keeps the supplied suffix, defaulting to `.video`.
///
/// MonoBehaviour encodes `json.dumps(tree, indent=4, ensure_ascii=False)`
/// using UTF-8 surrogateescape, exactly as tools/extractor.py:215-223 does.
/// A missing or unreadable tree, or a bytes value JSON cannot write, falls
/// back to the object's raw bytes; [`Export::fallback`] says why.
pub fn export<'a>(
    obj: &ObjectRef,
    env: &Environment,
    extension: &'a str,
    meter: &Arc<Meter>,
) -> Result<Option<Export<'a>>, Error> {
    if !supports(obj.class_id()) {
        return Err(Error::UnsupportedClass(obj.class_id()));
    }
    let parsed = read(obj, env);
    let parsed = match parsed {
        Ok(parsed) => parsed,
        Err(e) if obj.class_id() == 114 && read_failed(&e) => {
            let why = if matches!(e, Error::Parse(ParseError::NoTypeTree { .. })) {
                Fallback::NoTypeTree
            } else {
                Fallback::ReadFailed
            };
            return raw(obj, why, meter).map(Some);
        }
        Err(e) => return Err(e),
    };
    let (extension, bytes) = match obj.class_id() {
        49 => {
            let Value::Str(script) = attr(&parsed, b"m_Script")? else {
                return Err(Error::Field("m_Script"));
            };
            (
                if extension.is_empty() {
                    ".txt"
                } else {
                    extension
                },
                copy(script.as_bytes(), meter)?,
            )
        }
        128 => {
            let data = attr(&parsed, b"m_FontData")?;
            let bytes = font(data, meter)?;
            if bytes.is_empty() {
                return Ok(None);
            }
            // UnityPy tools/extractor.py:178-187 compares the slice with
            // b"OTTO": only TypelessData (bytes), never a list, can match.
            let extension = if matches!(data, Value::Bytes(b) if b.starts_with(b"OTTO")) {
                ".otf"
            } else {
                ".ttf"
            };
            (extension, bytes)
        }
        114 => match json(&parsed, meter) {
            Ok(bytes) => (".json", bytes),
            Err(Error::Json(pyjson::WriteError::Unwritable(_))) => {
                return raw(obj, Fallback::JsonUnwritable, meter).map(Some);
            }
            Err(e) => return Err(e),
        },
        329 => {
            let resource = attr(&parsed, b"m_ExternalResources")?;
            let map = resource
                .as_dict()
                .ok_or(Error::Field("m_ExternalResources"))?;
            let get = |name| {
                parsed
                    .attr(map, name)
                    .ok_or(Error::Field("StreamedResource"))
            };
            let Value::Str(path) = get(b"m_Source")? else {
                return Err(Error::Field("m_Source"));
            };
            let offset = get(b"m_Offset")?.as_u64().ok_or(Error::Field("m_Offset"))?;
            let size = get(b"m_Size")?.as_u64().ok_or(Error::Field("m_Size"))?;
            let data = env.resource(obj.file(), path, offset, size)?;
            (
                if extension.is_empty() {
                    ".video"
                } else {
                    extension
                },
                copy(&data, meter)?,
            )
        }
        id => return Err(Error::UnsupportedClass(id)),
    };
    Ok(Some(Export {
        extension,
        bytes,
        fallback: None,
    }))
}
pub(crate) fn read(obj: &ObjectRef, env: &Environment) -> Result<Parsed, Error> {
    let Some(tree) = env.object_tree(obj, None)? else {
        return Err(ParseError::NoTypeTree {
            class_id: obj.class_id(),
            why: obj.why_no_tree(),
        }
        .into());
    };
    let parsed = obj.read_typetree(Some(&tree.tree), false)?;
    // UnityPy helpers/TypeTreeHelper.py:155-156 raises on this mismatch.
    // Check locally to avoid core's infallible, uncharged Arc<Parsed> error.
    let expected = obj.info().byte_size;
    if parsed.bytes_read != i64::from(expected) {
        return Err(Error::CheckRead {
            expected,
            actual: parsed.bytes_read,
        });
    }
    Ok(parsed)
}
fn read_failed(error: &Error) -> bool {
    match error {
        Error::CheckRead { .. } => true,
        Error::Parse(p) => semantic(p),
        _ => false,
    }
}
// Traverse wrappers without recursion or allocating diagnostics. Semantic
// exceptions match Python's catch-all; resource refusals retain provenance.
fn semantic(error: &ParseError) -> bool {
    enum Fault<'a> {
        Parse(&'a ParseError),
        Tree(&'a TreeError),
        Deref(&'a unityrs_core::env::DerefError),
        Core(&'a unityrs_core::Error),
    }
    let mut fault = Fault::Parse(error);
    loop {
        fault = match fault {
            Fault::Parse(p) => match p {
                ParseError::NoTypeTree { .. } | ParseError::CheckRead { .. } => return true,
                ParseError::Read(r) => match &r.detail {
                    ReadDetail::Tree(t) => Fault::Tree(t),
                    ReadDetail::Limit(_) | ReadDetail::Charge(_) => return false,
                    _ => return true,
                },
                ParseError::Tree(t) => Fault::Tree(t),
                ParseError::Provider(p) => {
                    return p.limit.is_none() && p.memory.is_none() && p.allocator.is_none();
                }
                ParseError::Deref(d) => Fault::Deref(d),
                _ => return false,
            },
            Fault::Tree(t) => {
                return matches!(t, TreeError::Empty | TreeError::LevelUnderflow { .. });
            }
            Fault::Deref(d) => match d {
                unityrs_core::env::DerefError::Load(e) => Fault::Core(e),
                _ => return true,
            },
            Fault::Core(e) => match e {
                unityrs_core::Error::Tree(t) => Fault::Tree(t),
                unityrs_core::Error::AssetBundle(p) => Fault::Parse(p),
                unityrs_core::Error::Entry { source, .. } => Fault::Core(source),
                unityrs_core::Error::Deref(d) => Fault::Deref(d),
                unityrs_core::Error::Limit(_)
                | unityrs_core::Error::Budget(_)
                | unityrs_core::Error::Allocator(_) => return false,
                unityrs_core::Error::Detect(d) => {
                    return matches!(
                        d,
                        unityrs_core::detect::DetectError::Unterminated
                            | unityrs_core::detect::DetectError::Gzip(_)
                    );
                }
                unityrs_core::Error::Io(e) => return e.0.kind() != std::io::ErrorKind::OutOfMemory,
                _ => return true,
            },
        };
    }
}
pub(crate) fn attr<'a>(parsed: &'a Parsed, key: &[u8]) -> Result<&'a Value, Error> {
    parsed
        .value()
        .as_dict()
        .and_then(|m| parsed.attr(m, key))
        .ok_or(Error::Field("object attribute"))
}
fn copy(bytes: &[u8], meter: &Arc<Meter>) -> Result<MeteredVec<u8>, Error> {
    meter.count_decode(bytes.len() as u64)?;
    let mut out = MeteredVec::with_capacity(meter, bytes.len())?;
    out.extend_from_slice(bytes)?;
    Ok(out)
}
fn raw(obj: &ObjectRef, why: Fallback, meter: &Arc<Meter>) -> Result<Export<'static>, Error> {
    let bytes = copy(&obj.raw_data()?, meter)?;
    Ok(Export {
        extension: ".bin",
        bytes,
        fallback: Some(why),
    })
}
fn font(data: &Value, meter: &Arc<Meter>) -> Result<MeteredVec<u8>, Error> {
    macro_rules! bytes {
        ($items:expr) => {
            font_iter($items.iter().map(|&x| u8::try_from(x).ok()), meter)
        };
    }
    match data {
        Value::Bytes(bytes) | Value::Prims(PrimArray::U8(bytes)) => copy(bytes, meter),
        Value::Prims(p) => match p {
            PrimArray::Bool(v) => font_iter(v.iter().map(|&x| Some(u8::from(x))), meter),
            PrimArray::I8(v) => bytes!(v),
            PrimArray::U16(v) => bytes!(v),
            PrimArray::I16(v) => bytes!(v),
            PrimArray::U32(v) => bytes!(v),
            PrimArray::I32(v) => bytes!(v),
            PrimArray::U64(v) => bytes!(v),
            PrimArray::I64(v) => bytes!(v),
            _ => Err(Error::Field("m_FontData integer")),
        },
        Value::List(items) => font_values(items, meter),
        Value::Tuple(items) => font_values(items, meter),
        _ => Err(Error::Field("m_FontData")),
    }
}
fn font_values(items: &[Value], meter: &Arc<Meter>) -> Result<MeteredVec<u8>, Error> {
    font_iter(
        items.iter().map(|v| {
            v.as_u64()
                .and_then(|n| u8::try_from(n).ok())
                .or_else(|| v.as_bool().map(u8::from))
        }),
        meter,
    )
}
fn font_iter(
    items: impl ExactSizeIterator<Item = Option<u8>>,
    meter: &Arc<Meter>,
) -> Result<MeteredVec<u8>, Error> {
    let mut out = MeteredVec::with_capacity(meter, items.len())?;
    meter.count_decode(items.len() as u64)?;
    for item in items {
        out.push(item.ok_or(Error::Field("m_FontData byte"))?)?;
    }
    Ok(out)
}
// Charge every counting write, even when a later value is unwritable.
struct WorkCount<'a> {
    len: u64,
    meter: &'a Meter,
    error: Option<ChargeError>,
}
impl Sink for WorkCount<'_> {
    fn put(&mut self, bytes: &[u8]) {
        if self.error.is_none() {
            let n = bytes.len() as u64;
            match self.meter.count_decode(n) {
                Ok(()) => self.len += n,
                Err(e) => self.error = Some(e),
            }
        }
    }
    fn check(&self) -> Result<(), ChargeError> {
        self.error.clone().map_or(Ok(()), Err)
    }
}
struct Buffer {
    bytes: MeteredVec<u8>,
    error: Option<ChargeError>,
}
impl Sink for Buffer {
    fn put(&mut self, bytes: &[u8]) {
        if self.error.is_none() {
            self.error = self.bytes.extend_from_slice(bytes).err();
        }
    }
    fn check(&self) -> Result<(), ChargeError> {
        self.error.clone().map_or(Ok(()), Err)
    }
}
fn json(parsed: &Parsed, meter: &Arc<Meter>) -> Result<MeteredVec<u8>, Error> {
    let write = |sink: &mut Buffer| {
        pyjson::value_with(
            parsed,
            parsed.value(),
            0,
            Options::MONO_BEHAVIOUR,
            sink,
            meter,
        )
    };
    let mut count = WorkCount {
        len: 0,
        meter,
        error: None,
    };
    pyjson::value_with(
        parsed,
        parsed.value(),
        0,
        Options::MONO_BEHAVIOUR,
        &mut count,
        meter,
    )?;
    let len = usize::try_from(count.len).map_err(|_| Error::Field("JSON length"))?;
    let mut out = Buffer {
        bytes: MeteredVec::with_capacity(meter, len)?,
        error: None,
    };
    write(&mut out)?;
    if let Some(error) = out.error {
        return Err(error.into());
    }
    Ok(out.bytes)
}
