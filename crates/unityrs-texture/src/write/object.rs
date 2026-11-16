// Ported from UnityPy 1.25.2 (MIT): classes/legacy_patch/Texture2D.py. See LICENSE-UNITYPY.
//! Texture field edits (`classes/legacy_patch/Texture2D.py:14-60`) installed
//! through B1 only after the entire mip chain and object serialization succeed.
use super::{Options, Source, WriteError, image_to_texture2d};
use unityrs_core::walk::WriteRules;
use unityrs_core::{Bytes, ObjectRef, Parsed, RawStr, Value};

fn field<'a>(parsed: &'a Parsed, name: &[u8]) -> Option<&'a Value> {
    parsed.get(&[name])
}
fn integer(parsed: &Parsed, name: &[u8]) -> Result<i32, WriteError> {
    match field(parsed, name) {
        Some(Value::Int(n)) => {
            i32::try_from(*n).map_err(|_| WriteError::Invalid("integer field range"))
        }
        _ => Err(WriteError::Invalid("integer field")),
    }
}
fn set(parsed: &mut Parsed, path: &[&[u8]], value: Value) -> Result<(), WriteError> {
    if !parsed.replace_scalar(path, value)? {
        return Err(WriteError::Invalid("missing texture field"));
    }
    Ok(())
}

/// Update and re-save a `Texture2D`, propagating changed state to its parents.
/// `None` or target zero defaults to `m_TextureFormat`. Absent optional mip,
/// stream and platform fields stay absent, and platform blobs stay unchanged.
/// Failure preserves the object's previous installed bytes and changed state.
/// Like UnityPy's ObjectReader, subsequent reads still see the original data.
/// The returned bytes are the exact updated object serialization.
pub fn set_image(
    object: &ObjectRef,
    source: &Source<'_>,
    target: Option<i32>,
    mipmap_count: i32,
) -> Result<Bytes, WriteError> {
    if object.class_id() != 28 {
        return Err(WriteError::Invalid("object is not Texture2D"));
    }
    let meter = object.file().meter();
    let mut parsed = object.parse_as_dict(true)?;
    let format = match target {
        Some(n) if n != 0 => n,
        _ => integer(&parsed, b"m_TextureFormat")?,
    };
    let blob = match field(&parsed, b"m_PlatformBlob") {
        Some(Value::Bytes(b) | Value::Prims(unityrs_core::PrimArray::U8(b))) => Some(b.clone()),
        None => None,
        _ => return Err(WriteError::Invalid("platform blob")),
    };
    let encoded = image_to_texture2d(
        source,
        Options {
            format,
            platform: object.file().target_platform().unwrap_or(0),
            platform_blob: blob.as_deref(),
            flip: true,
            mipmap_count,
        },
        meter,
    )?;
    set(
        &mut parsed,
        &[b"m_Width"],
        Value::Int(i64::from(source.width())),
    )?;
    set(
        &mut parsed,
        &[b"m_Height"],
        Value::Int(i64::from(source.height())),
    )?;
    for (name, value) in [
        (
            b"m_MipMap".as_slice(),
            Value::Bool(encoded.mipmap_count > 1),
        ),
        (
            b"m_MipCount".as_slice(),
            Value::Int(i64::from(encoded.mipmap_count)),
        ),
    ] {
        if field(&parsed, name).is_some() {
            set(&mut parsed, &[name], value)?;
        }
    }
    set(
        &mut parsed,
        &[b"m_CompleteImageSize"],
        Value::Int(
            i64::try_from(encoded.data.len())
                .map_err(|_| WriteError::Invalid("complete image size"))?,
        ),
    )?;
    set(
        &mut parsed,
        &[b"m_TextureFormat"],
        Value::Int(i64::from(encoded.format)),
    )?;
    set(&mut parsed, &[b"image data"], Value::Bytes(encoded.data))?;
    if field(&parsed, b"m_StreamData").is_some() {
        set(
            &mut parsed,
            &[b"m_StreamData", b"path"],
            Value::Str(RawStr::from_static(b"")),
        )?;
        set(&mut parsed, &[b"m_StreamData", b"offset"], Value::Int(0))?;
        set(&mut parsed, &[b"m_StreamData", b"size"], Value::Int(0))?;
    }
    Ok(object.save_typetree(&parsed, None, WriteRules::UnityPy)?)
}
