// Ported from UnityPy 1.25.2 (MIT): UnityPy/export/MeshRendererExporter.py's texture PNG export and export/Texture2DConverter.py's inline/resource data selection. See THIRD_PARTY_LICENSES.
//! One texture at a time; decoded pixels and PNG output keep their charges.
use super::{ObjSink, view};
use crate::MeshError;
use std::sync::Arc;
use unityrs_core::{
    ObjectRef, RawStr, Value,
    env::Environment,
    limits::{Charge, Meter, heap_cost},
};
use unityrs_imageio::{Buffer, Charger};
struct PngMeter<'a>(&'a Arc<Meter>);
impl Charger for PngMeter<'_> {
    type Charge = Charge;
    type Refusal = unityrs_core::limits::ChargeError;
    fn charge(&mut self, _: Buffer, requests: &[usize]) -> Result<Charge, Self::Refusal> {
        self.0.charge(
            requests
                .iter()
                .fold(0u64, |n, &r| n.saturating_add(heap_cost(r as u64))),
        )
    }
}
pub(super) fn write(
    object: &ObjectRef,
    env: &Environment,
    meter: &Arc<Meter>,
    name: &RawStr,
    sink: &mut dyn ObjSink,
) -> Result<(), MeshError> {
    let parsed = object.parse_as_dict(false)?;
    let root = parsed.value();
    let get = |name| view::attr(&parsed, root, name, meter);
    let int = |name| {
        get(name)?
            .as_i64()
            .and_then(|n| i32::try_from(n).ok())
            .ok_or(MeshError::Field(name))
    };
    let width = int("m_Width")?;
    let height = int("m_Height")?;
    let texture_format = int("m_TextureFormat")?;
    let format = unityrs_texture::Format::from_texture_format(texture_format)
        .ok_or(MeshError::Field("unsupported texture format"))?;
    let Value::Bytes(inline) = get("image_data")? else {
        return Err(MeshError::Field("image_data"));
    };
    let resource;
    let data = if inline.is_empty() {
        let stream = get("m_StreamData")?;
        let path = view::string(view::attr(&parsed, stream, "path", meter)?)?;
        let offset = view::attr(&parsed, stream, "offset", meter)?
            .as_u64()
            .ok_or(MeshError::Field("offset"))?;
        let size = view::attr(&parsed, stream, "size", meter)?
            .as_u64()
            .ok_or(MeshError::Field("size"))?;
        resource = env.resource(object.file(), path, offset, size)?;
        resource.as_ref()
    } else {
        inline.as_ref()
    };
    let platform = object.file().target_platform().unwrap_or(0);
    if platform == unityrs_texture::SWITCH {
        let blob = match view::optional_attr(&parsed, root, "m_PlatformBlob", meter)? {
            Some(Value::Bytes(b) | Value::Prims(unityrs_core::PrimArray::U8(b))) => {
                Some(b.as_ref())
            }
            None => None,
            _ => return Err(MeshError::Field("m_PlatformBlob")),
        };
        if unityrs_texture::is_switch_swizzled(platform, blob) {
            return Err(MeshError::Field("unsupported swizzled texture"));
        }
    }
    let mut swapped = unityrs_core::limits::MeteredVec::new(meter);
    let data = if platform == unityrs_texture::XBOX360
        && unityrs_texture::XBOX_SWAP_FORMATS.contains(&texture_format)
    {
        swapped.reserve_exact(data.len())?;
        meter.count_decode(data.len() as u64)?;
        for pair in data.chunks(2) {
            for &byte in pair.iter().rev() {
                swapped.push(byte)?;
            }
        }
        swapped.as_slice()
    } else {
        data
    };
    let image = unityrs_texture::decode(format, data, width, height, meter)?;
    let mode = match image.mode() {
        unityrs_texture::Mode::L => unityrs_imageio::Mode::L,
        unityrs_texture::Mode::Rgb => unityrs_imageio::Mode::Rgb,
        unityrs_texture::Mode::Rgba => unityrs_imageio::Mode::Rgba,
    };
    let view = unityrs_imageio::Image::new(image.width(), image.height(), mode, image.pixels())
        .map_err(|_| MeshError::Field("texture image"))?;
    meter.count_decode(u64::from(image.width()).saturating_mul(u64::from(image.height())))?;
    // Pillow infers the format from the basename. A bare .png has no
    // extension, so the corrected renderer refuses it at image.save().
    if name.is_empty() {
        return Err(MeshError::Field("unknown file extension: "));
    }
    let png = unityrs_imageio::encode(&view, &mut PngMeter(meter)).map_err(|e| match e {
        unityrs_imageio::EncodeError::Refused { refusal, .. } => MeshError::Charge(refusal),
        unityrs_imageio::EncodeError::Alloc { request, .. } => MeshError::Charge(
            unityrs_core::limits::ChargeError::Allocator(meter.allocator_refused(request as u64)),
        ),
        unityrs_imageio::EncodeError::ZlibMemory => MeshError::Charge(
            unityrs_core::limits::ChargeError::Allocator(meter.allocator_refused(0)),
        ),
        _ => MeshError::Field("PNG encode"),
    })?;
    sink.file(name.as_bytes(), ".png", &mut |out| {
        out.write_bytes(png.bytes())
    })
}
