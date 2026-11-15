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
    let blob = if platform == unityrs_texture::SWITCH {
        match view::optional_attr(&parsed, root, "m_PlatformBlob", meter)? {
            Some(Value::Bytes(b) | Value::Prims(unityrs_core::PrimArray::U8(b))) => {
                Some(b.as_ref())
            }
            None => None,
            _ => return Err(MeshError::Field("m_PlatformBlob")),
        }
    } else {
        None
    };
    let fields = unityrs_texture::TextureFields {
        texture_format,
        width,
        height,
        platform,
        platform_blob: blob,
    };
    let format = selected_format(object, &fields, data)?;
    let image = unityrs_texture::decode_in_as(format, &fields, data, meter)?;

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

// Preserve the owning file's Crunch fork without indexing versions for plain
// formats or empty images, which return before that branch in the converter.
fn selected_format(
    object: &ObjectRef,
    fields: &unityrs_texture::TextureFields<'_>,
    data: &[u8],
) -> Result<unityrs_texture::Format, MeshError> {
    use unityrs_texture::{Format, Formats, Selection};
    if fields.width == 0 || fields.height == 0 {
        return Ok(Format::Rgba32);
    }
    let format = Format::from_texture_format(fields.texture_format)
        .or_else(|| (fields.texture_format == 6).then_some(Format::ArgbFloat))
        .ok_or_else(|| {
            if data.is_empty() {
                unityrs_texture::DecodeError::NoImageData.into()
            } else {
                MeshError::Field("unsupported texture")
            }
        })?;
    if !format.is_crunched() || data.is_empty() {
        return Ok(format);
    }
    let version = object
        .file()
        .version()
        .map(|v| {
            v.version
                .as_tuple()
                .map(|(major, minor, ..)| (major, minor))
        })
        .transpose()
        .map_err(unityrs_core::Error::from)?
        .unwrap_or((u16::MAX, u16::MAX));
    match unityrs_texture::select_in(fields, Formats::DECODED, version) {
        Selection::Export(format) => Ok(format),
        Selection::Skip(_) => Err(MeshError::Field("unsupported texture")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use unityrs_core::{Bytes, File, version::UnityVersion};

    #[test]
    fn obj_crunch_uses_the_file_version_after_empty_checks_only() {
        for version in ["2017.2.0f1", "2017.3.0f1", "5.0.0a1536"] {
            let mut options = unityrs_core::env::EnvOptions::unitypy();
            Arc::make_mut(&mut options.context).version.force_serialized =
                Some(UnityVersion::parse(&RawStr::from(version)).unwrap());
            let env = Environment::new(options);
            let data = unityrs_testkit::ngui::file(
                &[],
                &[unityrs_testkit::ngui::texture(
                    1,
                    b"synthetic",
                    4,
                    (1, 1),
                    vec![0; 4],
                )],
                true,
            )
            .unwrap();
            let File::Serialized(file) = env
                .load(unityrs_core::env::Source::Bytes {
                    name: RawStr::from("synthetic.assets"),
                    data: Bytes::from(data),
                })
                .unwrap()
            else {
                panic!("serialized file")
            };
            let object = file.object(1).unwrap();
            let mut fields = unityrs_texture::TextureFields {
                texture_format: 4,
                width: 1,
                height: 1,
                platform: 13,
                platform_blob: None,
            };
            assert_eq!(
                selected_format(&object, &fields, &[1]).unwrap(),
                unityrs_texture::Format::Rgba32
            );
            fields.texture_format = 28;
            let empty_format = selected_format(&object, &fields, &[]).unwrap();
            assert!(matches!(
                unityrs_texture::decode_in_as(empty_format, &fields, &[], &Meter::unlimited()),
                Err(unityrs_texture::DecodeError::NoImageData)
            ));
            let selected = selected_format(&object, &fields, &[1]);
            match version {
                "2017.2.0f1" => {
                    assert_eq!(selected.unwrap(), unityrs_texture::Format::Dxt1Crunched);
                }
                "2017.3.0f1" => assert_eq!(
                    selected.unwrap(),
                    unityrs_texture::Format::UnityDxt1Crunched
                ),
                _ => assert!(selected.is_err()),
            }
            for value in [6, 21] {
                fields.texture_format = value;
                let format = selected_format(&object, &fields, &[]).unwrap();
                assert!(matches!(
                    unityrs_texture::decode_in_as(format, &fields, &[], &Meter::unlimited()),
                    Err(unityrs_texture::DecodeError::Unsupported { .. })
                ));
            }
            fields.width = 0;
            assert_eq!(
                selected_format(&object, &fields, &[]).unwrap(),
                unityrs_texture::Format::Rgba32
            );
        }
    }
}
