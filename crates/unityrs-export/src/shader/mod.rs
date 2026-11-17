// Ported from UnityPy 1.25.2 (MIT): UnityPy/export/ShaderConverter.py, UnityPy/classes/legacy_patch/Shader.py. See THIRD_PARTY_LICENSES.
//! Tier A Shader export and Tier C player/parameter record selection.
//! Malformed legacy blobs and unfillable replacements have named Tier U refusals.
//! Entry point: export/ShaderConverter.py:41-89;
//! classes/legacy_patch/Shader.py:4-7.
mod blob;
#[cfg(test)]
mod bytes_tests;
mod convert;
mod direct;
pub use direct::direct_stage;
mod layout;
mod legacy;
mod output;
mod sort;
mod view;
pub use blob::Program;
use output::Output;
use std::sync::Arc;
use unityrs_core::{
    Bytes, ObjectRef, Parsed, PrimArray, Value,
    budget::DecodeBuf,
    codec::{Codec, Lz4, decode_block},
    env::Environment,
    error::LimitKind,
    limits::{ChargeError, Meter, MeteredVec},
    walk::ParseError,
};
use view::View;

pub const HEADER: &str = "//////////////////////////////////////////\n//\n// NOTE: This is *not* a valid shader file\n//\n///////////////////////////////////////////\n";
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no Shader exporter for class {0}")]
    UnsupportedClass(i32),
    #[error("unsupported Shader layout: {0}")]
    Refusal(&'static str),
    #[error("unsupported Shader layout (Tier U): {0:?}")]
    Unsupported(Unsupported),
    #[error("invalid Shader code or parameter blob index")]
    LayoutIndex,
    #[error("missing or invalid Shader field {0}")]
    Field(&'static str),
    #[error("{0}")]
    PythonType(&'static str),
    #[error("unsupported operand type(s) for +: '{left}' and '{right}'")]
    AddType {
        left: &'static str,
        right: &'static str,
    },
    #[error("can only concatenate {container} (not \"{other}\") to {container}")]
    ConcatType {
        container: &'static str,
        other: &'static str,
    },
    #[error("'{0}' object cannot be interpreted as an integer")]
    IntegerType(&'static str),
    #[error("'UnknownObject' object has no attribute '{0}'")]
    Attribute(&'static str),
    #[error("{0}")]
    PropertyType(i64),
    #[error("{0} is not a valid ShaderGpuProgramType")]
    ProgramType(i32),
    #[error("")]
    NotImplemented,
    #[error("fixed-width field outside Shader blob")]
    Struct,
    #[error("Input source data size too small")]
    Lz4Prefix,
    #[error("Invalid size: 0x{0}")]
    Lz4HeaderSize(u32),
    #[error("Decompressor wrote {actual} bytes, but {expected} bytes expected from header")]
    Lz4HeaderLength { actual: usize, expected: u32 },
    #[error("signed integer is greater than maximum or less than minimum")]
    Lz4Size,
    #[error("list index out of range")]
    Index,
    #[error("String not terminated")]
    Unterminated,
    #[error("invalid UTF-8 in Shader code")]
    Utf8,
    #[error("invalid UTF-8 in Shader code")]
    Utf8Bytes(Bytes),
    #[error("string argument without an encoding")]
    StringBytes,
    #[error("cannot convert 'NoneType' object to bytes")]
    NoneBytes,
    #[error("bytes must be in range(0, 256)")]
    ByteRange,
    #[error("typetree read {actual} bytes, expected {expected}")]
    CheckRead { expected: u32, actual: i64 },
    #[error(transparent)]
    Parse(#[from] ParseError),
    #[error(transparent)]
    Core(#[from] unityrs_core::Error),
    #[error(transparent)]
    Charge(#[from] ChargeError),
}
/// Legacy layouts without an established sample/reference contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unsupported {
    BigEndian53Blob,
    BigEndian54Blob,
    GpuProgramIndex,
}
impl Error {
    /// The Python exception represented by a reference failure.
    pub fn exception(&self) -> Option<&'static str> {
        Some(match self {
            Self::PropertyType(_)
            | Self::ProgramType(_)
            | Self::Lz4Prefix
            | Self::Lz4HeaderSize(_)
            | Self::ByteRange => "ValueError",
            Self::Lz4HeaderLength { .. } => "LZ4BlockError",
            Self::Lz4Size => "OverflowError",
            Self::NotImplemented => "NotImplementedError",
            Self::Struct => "error",
            Self::Index => "IndexError",
            Self::Unterminated => "Exception",
            Self::Utf8 | Self::Utf8Bytes(_) => "UnicodeDecodeError",
            Self::StringBytes
            | Self::NoneBytes
            | Self::PythonType(_)
            | Self::AddType { .. }
            | Self::ConcatType { .. }
            | Self::IntegerType(_) => "TypeError",
            Self::Attribute(_) => "AttributeError",
            _ => return None,
        })
    }
}
/// Export one Shader into a fallibly grown buffer charged to `meter`.
/// The object's typetree reads retain the source meter, as E1's do.
pub fn export(
    obj: &ObjectRef,
    env: &Environment,
    meter: &Arc<Meter>,
) -> Result<MeteredVec<u8>, Error> {
    if obj.class_id() != 48 {
        return Err(Error::UnsupportedClass(obj.class_id()));
    }
    let tree = env.object_tree(obj, None)?.ok_or(ParseError::NoTypeTree {
        class_id: 48,
        why: obj.why_no_tree(),
    })?;
    let parsed = obj.read_typetree(Some(&tree.tree), false)?;
    if parsed.bytes_read != i64::from(obj.info().byte_size) {
        return Err(Error::CheckRead {
            expected: obj.info().byte_size,
            actual: parsed.bytes_read,
        });
    }
    let version = obj
        .file()
        .version()
        .ok_or(Error::Field("version"))?
        .version
        .as_tuple()
        .map_err(|_| Error::Field("version"))?;
    export_with_cap(
        &parsed,
        (u32::from(version.0), u32::from(version.1)),
        meter,
        obj.file().context().limits.max_block_output,
    )
}
/// Convert an already read tree. Useful for callers with a supplied typetree,
/// and for bounded fuzzing of arbitrary field shapes.
pub fn export_parsed(
    parsed: &Parsed,
    version: (u32, u32),
    meter: &Arc<Meter>,
) -> Result<MeteredVec<u8>, Error> {
    export_with_cap(
        parsed,
        version,
        meter,
        unityrs_core::limits::Limits::default().max_block_output,
    )
}
/// Convert current fields using the owning file's decompression bound.
pub fn export_with_cap(
    parsed: &Parsed,
    version: (u32, u32),
    meter: &Arc<Meter>,
    max_block_output: u64,
) -> Result<MeteredVec<u8>, Error> {
    let root = View {
        parsed,
        value: parsed.value(),
        meter,
        is_none: false,
    };
    if let Some(old) = root.optional("m_SubProgramBlob")?
        && !old.empty()
    {
        return legacy::export(root, version, meter, max_block_output);
    }
    let mut out = Output::new(meter);
    out.text(HEADER)?;
    if let Some(blob) = root.optional("compressedBlob")?
        && !blob.empty()
    {
        let platforms = root.field("platforms")?.seq()?;
        let bytes = bytes(blob.value, meter)?;
        let mut programs = MeteredVec::new(meter);
        let modern = root
            .optional("m_ParsedForm")?
            .map(layout::modern)
            .transpose()?
            .unwrap_or(false);
        // UnityPy's short-circuit guards precede get_entry and offsets:
        // export/ShaderConverter.py:70-87 (get_entry: export/ShaderConverter.py:64-68).
        for i in 0..platforms.len() {
            meter.count_decode(1)?;
            let compressed = root.field("compressedLengths")?.seq()?;
            if i >= compressed.len() {
                break;
            }
            let decompressed = root.field("decompressedLengths")?.seq()?;
            if i >= decompressed.len() {
                break;
            }
            let count = if modern {
                compressed.segment_count(i)?
            } else {
                1
            };
            let mut segments = MeteredVec::new(meter);
            let offsets = root.field("offsets")?.seq()?;
            if modern
                && (count == 0
                    || count != decompressed.segment_count(i)?
                    || count != offsets.segment_count(i)?)
            {
                return Err(Error::LayoutIndex);
            }
            for segment in 0..count {
                meter.count_decode(1)?;
                let length = if modern {
                    compressed.segment(i, segment)?
                } else {
                    compressed.entry(i)?
                };
                let size = if modern {
                    decompressed.segment(i, segment)?
                } else {
                    decompressed.entry(i)?
                };
                let offset = if modern {
                    offsets.segment(i, segment)?
                } else {
                    offsets.entry(i)?
                };
                let range = offset.slice(length, bytes.len())?;
                let buf = decode(bytes.slice(range), size, meter, max_block_output)?;
                if modern {
                    segments.push(buf)?;
                } else {
                    programs.push(Program::decoded(buf, version, meter)?)?;
                }
            }
            if modern {
                programs.push(Program::modern(
                    segments,
                    version,
                    root.field("m_ParsedForm")?,
                    platforms,
                    i,
                    meter,
                )?)?;
            }
        }
        convert::shader(
            root.field("m_ParsedForm")?,
            platforms,
            programs.as_slice(),
            &mut out,
            meter,
        )?;
    } else {
        let script = root.optional("m_Script")?.ok_or(Error::NoneBytes)?;
        let script = bytes(script.value, meter)?;
        meter.count_decode(script.len() as u64)?;
        std::str::from_utf8(&script).map_err(|_| utf8_error(&script, meter))?;
        out.text(&script)?;
    }
    Ok(out.bytes)
}

fn utf8_error(data: &[u8], meter: &Arc<Meter>) -> Error {
    let copy = DecodeBuf::new(meter, 0, u64::MAX, LimitKind::InputMemory).and_then(|mut bytes| {
        meter.count_decode(data.len() as u64)?;
        bytes.extend_from_slice(data)?;
        bytes.into_bytes()
    });
    match copy {
        Ok(bytes) => Error::Utf8Bytes(bytes),
        Err(error) => Error::Charge(error),
    }
}

fn decode(
    mut input: Bytes,
    size: view::Entry<'_>,
    meter: &Arc<Meter>,
    max_block_output: u64,
) -> Result<DecodeBuf, Error> {
    let size = size.integer(view::ArrayUse::Decompressed)?;
    // python-lz4's negative-size mode reads a stored LE size prefix.
    // The core codec takes the resulting nonnegative maximum.
    i32::try_from(size).map_err(|_| Error::Lz4Size)?;
    let prefixed = size < 0;
    let size = if prefixed {
        let prefix: [u8; 4] = input
            .get(..4)
            .ok_or(Error::Lz4Prefix)?
            .try_into()
            .map_err(|_| Error::Lz4Prefix)?;
        let size = u32::from_le_bytes(prefix);
        if size > i32::MAX.unsigned_abs() {
            return Err(Error::Lz4HeaderSize(size));
        }
        input = input.slice(4..);
        size
    } else {
        u32::try_from(size).map_err(|_| Error::Lz4Size)?
    };
    // Both LZ4 passes may scan the complete input, even on failure.
    meter.count_decode((input.len() as u64).saturating_mul(2))?;
    let mut buf = DecodeBuf::new(meter, 0, max_block_output, LimitKind::BlockOutput)?;
    decode_block(Codec::Lz4, &Lz4, &input, size, &mut buf, max_block_output)?;
    if prefixed && buf.len() as u64 != u64::from(size) {
        return Err(Error::Lz4HeaderLength {
            actual: buf.len(),
            expected: size,
        });
    }
    Ok(buf)
}
/// Validate a platform's table and every subprogram without producing text.
pub fn validate_blob(blob: &[u8], version: (u32, u32), meter: &Arc<Meter>) -> Result<(), Error> {
    Program::parse(blob, version, meter).map(|_| ())
}
fn bytes(v: &Value, meter: &Arc<Meter>) -> Result<Bytes, Error> {
    match v {
        Value::Bytes(b) | Value::Prims(PrimArray::U8(b)) => Ok(b.clone()),
        Value::Str(_) => Err(Error::StringBytes),
        Value::List(items) => byte_values(items, meter),
        Value::Tuple(items) => byte_values(items, meter),
        _ => Err(Error::Field("byte array")),
    }
}

// export/ShaderConverter.py:41-55 uses Python bytes(), including when a
// caller edits an already read class from a packed U8 array to a list.
fn byte_values(items: &[Value], meter: &Arc<Meter>) -> Result<Bytes, Error> {
    struct Owner {
        bytes: MeteredVec<u8>,
        _owner: unityrs_core::limits::Charge,
    }
    impl AsRef<[u8]> for Owner {
        fn as_ref(&self) -> &[u8] {
            self.bytes.as_slice()
        }
    }
    meter.count_decode(items.len() as u64)?;
    let mut bytes = MeteredVec::with_capacity(meter, items.len())?;
    for item in items {
        let byte = match item {
            Value::Int(n) => u8::try_from(*n).map_err(|_| Error::ByteRange)?,
            Value::UInt(n) => u8::try_from(*n).map_err(|_| Error::ByteRange)?,
            Value::Bool(n) => u8::from(*n),
            Value::F32(_) | Value::F64(_) => return Err(Error::IntegerType("float")),
            Value::Str(_) => return Err(Error::IntegerType("str")),
            Value::Bytes(_) => return Err(Error::IntegerType("bytes")),
            Value::List(_) | Value::Prims(_) => return Err(Error::IntegerType("list")),
            Value::Tuple(_) => return Err(Error::IntegerType("tuple")),
            Value::Dict(_) => return Err(Error::IntegerType("dict")),
        };
        bytes.push(byte)?;
    }
    let owner = meter.charge(128)?;
    Ok(Bytes::from_owner(Owner {
        bytes,
        _owner: owner,
    }))
}
fn compressed_range(len: usize, offset: i128, size: i128) -> std::ops::Range<usize> {
    let norm = |n: i128| {
        usize::try_from(if n < 0 {
            (len as i128 + n).max(0)
        } else {
            n.min(len as i128)
        })
        .unwrap_or(0)
    };
    let start = norm(offset);
    let end = norm(offset + size).max(start);
    start..end
}
