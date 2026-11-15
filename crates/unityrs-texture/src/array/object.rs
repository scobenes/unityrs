// Ported from UnityPy 1.25.2 (MIT): classes/legacy_patch/Texture2DArray.py's image extraction, with PARITY section 19.2's streamed-data correction. See THIRD_PARTY_LICENSES.
//! Object/resource bridge for `classes/legacy_patch/Texture2DArray.py:12-43`.
use super::{ArrayError, ArrayFields};
use crate::Image;
use std::sync::Arc;
use unityrs_core::{
    ObjectRef, Parsed, Value,
    env::Environment,
    limits::{Meter, MeteredVec},
};

fn field<'a>(
    p: &'a Parsed,
    value: &'a Value,
    name: &'static str,
    meter: &Meter,
) -> Result<&'a Value, ArrayError> {
    let Value::Dict(map) = value else {
        return Err(ArrayError::Field(name));
    };
    meter.count_decode((map.entries().len() as u64).saturating_mul(name.len() as u64 + 1))?;
    p.field(map, name.as_bytes()).ok_or(ArrayError::Field(name))
}

/// Read a `Texture2DArray`'s fields and resource, then decode all slices through
/// the shared array path. Empty inline bytes fall back to `m_StreamData`.
/// The Environment owns resource charges; returned images use `meter`.
pub fn images(
    object: &ObjectRef,
    env: &Environment,
    meter: &Arc<Meter>,
) -> Result<MeteredVec<Image>, ArrayError> {
    let parsed = object.parse_as_dict(false)?;
    let root = parsed.value();
    let get = |name| field(&parsed, root, name, meter);
    let int = |name| {
        get(name)?
            .as_i64()
            .and_then(|n| i32::try_from(n).ok())
            .ok_or(ArrayError::Field(name))
    };
    let fields = ArrayFields {
        graphics_format: int("m_Format")?,
        width: int("m_Width")?,
        height: int("m_Height")?,
        depth: int("m_Depth")?,
        data_size: get("m_DataSize")?
            .as_u64()
            .ok_or(ArrayError::Field("m_DataSize"))?,
        platform: object.file().target_platform().unwrap_or(0),
    };
    if matches!(fields.platform, crate::SWITCH | crate::XBOX360) {
        return Err(ArrayError::UnsupportedPlatform(fields.platform));
    }
    super::texture_format(fields.graphics_format)?;
    let Value::Bytes(inline) = get("image_data")? else {
        return Err(ArrayError::Field("image_data"));
    };
    let resource;
    let bytes = if inline.is_empty() {
        let stream = get("m_StreamData")?;
        let Value::Str(path) = field(&parsed, stream, "path", meter)? else {
            return Err(ArrayError::Field("path"));
        };
        let offset = field(&parsed, stream, "offset", meter)?
            .as_u64()
            .ok_or(ArrayError::Field("offset"))?;
        let size = field(&parsed, stream, "size", meter)?
            .as_u64()
            .ok_or(ArrayError::Field("size"))?;
        resource = env.resource(object.file(), path, offset, size)?;
        resource.as_ref()
    } else {
        inline.as_ref()
    };
    super::decode(&fields, bytes, None, meter)
}
