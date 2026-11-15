// Ported from UnityPy 1.25.2 (MIT): export/AudioClipConverter.py's AudioClip data selection. See THIRD_PARTY_LICENSES.
//! A multi-file `AudioClip` exporter; no batch kind is added. Inline bytes win
//! over streamed data (export/AudioClipConverter.py:20-33). Old layouts hold
//! `m_AudioData` and `m_Type`; 5.x layouts hold `m_Resource` and
//! `m_CompressionFormat` (classes/generated.py:4180-4201). The pinned converter
//! does not consult either format field during sample dispatch.
//!
//! An empty resource returns no files before resolving its path (Tier B,
//! PARITY.md section 19.2); otherwise source errors retain their provenance.
use crate::simple;
use std::sync::Arc;
use unityrs_audio::{Sample, VorbisHeaders};
use unityrs_core::{
    ObjectRef, PrimArray, Value,
    env::Environment,
    limits::{Meter, MeteredVec},
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Read(#[from] simple::Error),
    #[error(transparent)]
    Audio(#[from] unityrs_audio::Error),
}
const _: () = send_sync::<Error>();
const fn send_sync<T: Send + Sync + ?Sized>() {}

/// Export class 83 through its typetree and the Environment's resource
/// resolver. Output names and WAVs retain their charges on `meter`.
pub fn export(
    obj: &ObjectRef,
    env: &Environment,
    convert_float: bool,
    meter: &Arc<Meter>,
) -> Result<MeteredVec<Sample>, Error> {
    export_with_vorbis(obj, env, convert_float, None, meter)
}

/// Export using runtime Vorbis setup headers supplied by the caller. Enable
/// the `vorbis` feature for the decoder; batch never enables or calls it.
pub fn export_with_vorbis(
    obj: &ObjectRef,
    env: &Environment,
    convert_float: bool,
    headers: Option<&VorbisHeaders>,
    meter: &Arc<Meter>,
) -> Result<MeteredVec<Sample>, Error> {
    if obj.class_id() != 83 {
        return Err(simple::Error::UnsupportedClass(obj.class_id()).into());
    }
    let parsed = simple::read(obj, env)?;
    let name = simple::attr(&parsed, b"m_Name")?;
    let Value::Str(name) = name else {
        return Err(simple::Error::Field("m_Name").into());
    };
    let dict = parsed
        .value()
        .as_dict()
        .ok_or(simple::Error::Field("AudioClip"))?;
    let inline = parsed
        .attr(dict, b"m_AudioData")
        .map(|v| inline(v, meter))
        .transpose()?;
    if let Some(data) = inline.as_ref().filter(|d| !d.bytes().is_empty()) {
        let plan = unityrs_audio::plan(data.bytes(), convert_float, meter)?;
        return Ok(unityrs_audio::decode_with_vorbis(
            &plan,
            name.as_bytes(),
            headers,
            meter,
        )?);
    }
    let resource = parsed
        .attr(dict, b"m_Resource")
        .and_then(Value::as_dict)
        .ok_or(simple::Error::Field("m_AudioData or m_Resource"))?;
    let get = |key| {
        parsed
            .attr(resource, key)
            .ok_or(simple::Error::Field("m_Resource"))
    };
    let size = get(b"m_Size")?
        .as_u64()
        .ok_or(simple::Error::Field("m_Size"))?;
    if size == 0 {
        return Ok(MeteredVec::new(meter));
    }
    let Value::Str(path) = get(b"m_Source")? else {
        return Err(simple::Error::Field("m_Source").into());
    };
    let offset = get(b"m_Offset")?
        .as_u64()
        .ok_or(simple::Error::Field("m_Offset"))?;
    let data = env
        .resource(obj.file(), path, offset, size)
        .map_err(simple::Error::from)?;
    let plan = unityrs_audio::plan(&data, convert_float, meter)?;
    Ok(unityrs_audio::decode_with_vorbis(
        &plan,
        name.as_bytes(),
        headers,
        meter,
    )?)
}
enum Inline<'a> {
    Borrowed(&'a [u8]),
    Copied(MeteredVec<u8>),
}
impl Inline<'_> {
    fn bytes(&self) -> &[u8] {
        match self {
            Self::Borrowed(b) => b,
            Self::Copied(b) => b.as_slice(),
        }
    }
}
fn inline<'a>(value: &'a Value, meter: &Arc<Meter>) -> Result<Inline<'a>, simple::Error> {
    macro_rules! ints {
        ($values:expr) => {
            copy($values.iter().map(|&n| u8::try_from(n).ok()), meter)
        };
    }
    match value {
        Value::Bytes(b) | Value::Prims(PrimArray::U8(b)) => Ok(Inline::Borrowed(b)),
        Value::List(v) => values(v, meter),
        Value::Tuple(v) => values(v, meter),
        Value::Prims(p) => match p {
            PrimArray::Bool(v) => copy(v.iter().map(|&n| Some(u8::from(n))), meter),
            PrimArray::I8(v) => ints!(v),
            PrimArray::U16(v) => ints!(v),
            PrimArray::I16(v) => ints!(v),
            PrimArray::U32(v) => ints!(v),
            PrimArray::I32(v) => ints!(v),
            PrimArray::U64(v) => ints!(v),
            PrimArray::I64(v) => ints!(v),
            _ => Err(simple::Error::Field("m_AudioData byte")),
        },
        _ => Err(simple::Error::Field("m_AudioData")),
    }
}
fn copy<'a>(
    bytes: impl ExactSizeIterator<Item = Option<u8>>,
    meter: &Arc<Meter>,
) -> Result<Inline<'a>, simple::Error> {
    meter.count_decode(bytes.len() as u64)?;
    let mut out = MeteredVec::with_capacity(meter, bytes.len())?;
    for b in bytes {
        out.push(b.ok_or(simple::Error::Field("m_AudioData byte"))?)?;
    }
    Ok(Inline::Copied(out))
}

fn values<'a>(v: &[Value], meter: &Arc<Meter>) -> Result<Inline<'a>, simple::Error> {
    copy(
        v.iter().map(|v| {
            v.as_u64()
                .and_then(|n| u8::try_from(n).ok())
                .or_else(|| v.as_bool().map(u8::from))
        }),
        meter,
    )
}
