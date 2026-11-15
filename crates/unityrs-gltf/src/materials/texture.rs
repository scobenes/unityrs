//! One decode at a time; transfer encoded PNG ownership without copying it.
use super::pixels::{self, Usage};
use super::{GltfError, Result, input};
use crate::collect::raw_text;
use crate::{Image, Mime, Sampler, Scene, Texture};
use std::sync::Arc;
use unityrs_core::{
    Bytes, ObjectRef, Value,
    env::Environment,
    limits::{Charge, ChargeError, Meter, MeteredVec, heap_cost},
};
struct PngMeter<'a>(&'a Arc<Meter>);
impl unityrs_imageio::Charger for PngMeter<'_> {
    type Charge = Charge;
    type Refusal = ChargeError;
    fn charge(
        &mut self,
        _: unityrs_imageio::Buffer,
        requests: &[usize],
    ) -> std::result::Result<Charge, ChargeError> {
        self.0.charge(
            requests
                .iter()
                .fold(0u64, |n, &r| n.saturating_add(heap_cost(r as u64))),
        )
    }
}
#[allow(clippy::too_many_lines)]
fn decode(
    env: &Environment,
    object: &ObjectRef,
    meter: &Arc<Meter>,
) -> Result<unityrs_texture::Image> {
    if object.class_id() != 28 {
        return Err(GltfError::Invalid("texture_malformed"));
    }
    let p = object
        .parse_as_dict(false)
        .map_err(|e| GltfError::Mesh(e.into()))?;
    let root = p.value();
    let get = |name| input::attr(&p, root, name, meter);
    let int = |name| -> Result<i32> {
        get(name)?
            .as_i64()
            .and_then(|n| i32::try_from(n).ok())
            .ok_or(GltfError::Invalid("texture_malformed"))
    };
    let width = int(b"m_Width")?;
    let height = int(b"m_Height")?;
    let fmt = int(b"m_TextureFormat")?;
    let blob = match input::optional(&p, root, b"m_PlatformBlob", meter)? {
        Some(Value::Bytes(b) | Value::Prims(unityrs_core::PrimArray::U8(b))) => Some(b.as_ref()),
        None => None,
        _ => return Err(GltfError::Invalid("texture_malformed")),
    };
    // GlTF accepts every currently decoded format, independent of batch's V1
    // allowlist. Unsupported platforms are data omissions too.
    let selected = unityrs_texture::select_in(
        &unityrs_texture::TextureFields {
            texture_format: fmt,
            width,
            height,
            platform: object.file().target_platform().unwrap_or(0),
            platform_blob: blob,
        },
        unityrs_texture::Formats::DECODED,
        // Crunch's decoder follows the texture's own file's version; without
        // a usable one, never assume legacy crunch (as the NGUI reader does).
        object
            .file()
            .version()
            .and_then(|v| v.version.as_tuple().ok())
            .map_or((u16::MAX, u16::MAX), |(major, minor, ..)| (major, minor)),
    );
    let format = match selected {
        unityrs_texture::Selection::Export(f) => f,
        unityrs_texture::Selection::Skip(unityrs_texture::Skip::UnsupportedFormat(_)) => {
            return Err(GltfError::Invalid("undecodable_texture"));
        }
        unityrs_texture::Selection::Skip(unityrs_texture::Skip::EmptyImage) => {
            return Err(GltfError::Invalid("missing_texture"));
        }
        unityrs_texture::Selection::Skip(_) => {
            return Err(GltfError::Invalid("unsupported_texture_platform"));
        }
    };
    let empty = Bytes::new();
    let inline = match input::optional(&p, root, b"image_data", meter)? {
        Some(Value::Bytes(b)) => b,
        None => &empty,
        _ => return Err(GltfError::Invalid("texture_malformed")),
    };
    let stream = if inline.is_empty() {
        if let Some(s) = input::optional(&p, root, b"m_StreamData", meter)? {
            Some(unityrs_texture::fetch::Stream {
                path: input::string(input::attr(&p, s, b"path", meter)?)?.clone(),
                offset: i128::from(
                    input::attr(&p, s, b"offset", meter)?
                        .as_u64()
                        .ok_or(GltfError::Invalid("texture_malformed"))?,
                ),
                size: i128::from(
                    input::attr(&p, s, b"size", meter)?
                        .as_u64()
                        .ok_or(GltfError::Invalid("texture_malformed"))?,
                ),
            })
        } else {
            None
        }
    } else {
        None
    };
    let data = unityrs_texture::fetch::fetch(env, object.file(), inline, stream.as_ref(), meter)
        .map_err(|e| match e {
            unityrs_texture::fetch::Error::Charge(e) => GltfError::Charge(e),
            unityrs_texture::fetch::Error::Data(e)
            | unityrs_texture::fetch::Error::Resource { error: e, .. } => {
                if crate::collect::diagnostics::core_data(&e) {
                    GltfError::Invalid("missing_texture")
                } else {
                    GltfError::Mesh(e.into())
                }
            }
            _ => GltfError::Invalid("texture_malformed"),
        })?;
    // Source/header plausibility (including lengths) precedes any output charge.
    // Only resource/meter/allocator refusals escape; static decode bounds are data.
    let plan = unityrs_texture::plan(format, data.len(), width, height).map_err(decode_error)?;
    meter.count_decode(plan.output_bytes)?;
    let image =
        unityrs_texture::decode(format, &data, width, height, meter).map_err(decode_error)?;
    Ok(image)
}

pub(super) struct Source {
    pub image: unityrs_texture::Image,
    pub srgb: Option<bool>,
    pub sampler: Sampler,
    pub mirror_once: bool,
}
fn source(env: &Environment, object: &ObjectRef, meter: &Arc<Meter>) -> Result<Source> {
    let p = object
        .parse_as_dict(false)
        .map_err(|e| GltfError::Mesh(e.into()))?;
    let root = p.value();
    let srgb = input::optional(&p, root, b"m_ColorSpace", meter)?
        .map(|v| match v.as_i64() {
            Some(0) => Ok(false),
            Some(1) => Ok(true),
            _ => Err(GltfError::Invalid("texture_malformed")),
        })
        .transpose()?;
    let settings = input::optional(&p, root, b"m_TextureSettings", meter)?;
    let setting = |key: &[u8], default| -> Result<i64> {
        settings
            .map(|s| input::optional(&p, s, key, meter))
            .transpose()?
            .flatten()
            .map_or(Ok(default), |v| {
                v.as_i64().ok_or(GltfError::Invalid("texture_malformed"))
            })
    };
    let legacy = setting(b"m_WrapMode", 0)?;
    let (u, v, filter) = (
        setting(b"m_WrapU", legacy)?,
        setting(b"m_WrapV", legacy)?,
        setting(b"m_FilterMode", 1)?,
    );
    let wrap = |v| match v {
        0 => Ok(10497),
        1 => Ok(33071),
        2 | 3 => Ok(33648),
        _ => Err(GltfError::Invalid("texture_malformed")),
    };
    let mip = input::optional(&p, root, b"m_MipCount", meter)?
        .and_then(Value::as_i64)
        .unwrap_or(1)
        > 1;
    let (mag_filter, min_filter) = match filter {
        0 => (9728, if mip { 9984 } else { 9728 }),
        1 => (9729, if mip { 9985 } else { 9729 }),
        2 => (9729, if mip { 9987 } else { 9729 }),
        _ => return Err(GltfError::Invalid("texture_malformed")),
    };
    Ok(Source {
        image: decode(env, object, meter)?,
        srgb,
        sampler: Sampler {
            wrap_s: wrap(u)?,
            wrap_t: wrap(v)?,
            mag_filter,
            min_filter,
        },
        mirror_once: u == 3 || v == 3,
    })
}

pub(super) struct Embedded {
    pub texture: usize,
    pub mirror_once: bool,
    pub packed_sampler_conflict: bool,
    pub alpha_reason: Option<&'static str>,
}
pub(super) fn embed(
    env: &Environment,
    scene: &mut Scene,
    object: &ObjectRef,
    key: &[u8],
    usage: Usage,
    albedo: Option<&ObjectRef>,
    meter: &Arc<Meter>,
) -> Result<Embedded> {
    let src = source(env, object, meter)?;
    let (alpha, alpha_reason) = match albedo.map(|a| source(env, a, meter)).transpose() {
        Ok(alpha) => (alpha, None),
        Err(e) if super::recoverable(&e) => (None, Some(super::texture_reason(&e))),
        Err(e) => return Err(e),
    };
    let packed_sampler_conflict = alpha.as_ref().is_some_and(|a| a.sampler != src.sampler);
    let (width, height) = pixels::dimensions(&src, alpha.as_ref());
    let pixels = pixels::convert(&src, usage, alpha.as_ref(), meter)?;
    let view = unityrs_imageio::Image::new(
        width,
        height,
        unityrs_imageio::Mode::Rgba,
        pixels.as_slice(),
    )
    .map_err(|_| GltfError::Invalid("texture_malformed"))?;
    meter.count_decode(pixels.len() as u64)?;
    let png = unityrs_imageio::encode(&view, &mut PngMeter(meter)).map_err(|e| match e {
        unityrs_imageio::EncodeError::Refused { refusal, .. } => GltfError::Charge(refusal),
        unityrs_imageio::EncodeError::Alloc { request, .. } => GltfError::Charge(
            ChargeError::Allocator(meter.allocator_refused(request as u64)),
        ),
        unityrs_imageio::EncodeError::ZlibMemory => {
            GltfError::Charge(ChargeError::Allocator(meter.allocator_refused(0)))
        }
        _ => GltfError::Invalid("texture_encode_failed"),
    })?;
    let p = object
        .parse_as_dict(false)
        .map_err(|e| GltfError::Mesh(e.into()))?;
    let name = input::string(input::attr(&p, p.value(), b"m_Name", meter)?)?;
    let mut name = raw_text(
        meter,
        if name.is_empty() {
            key
        } else {
            name.as_bytes()
        },
    )?;
    name.0.extend_from_slice(b".png")?;
    drop(pixels);
    let (bytes, charge) = png.into_parts();
    let bytes = MeteredVec::from_charged_parts(bytes, charge)?;
    let image = scene.images.len();
    scene.images.push(Image {
        name: Some(name),
        mime: Mime::Png,
        bytes,
    })?;
    let texture = scene.textures.len();
    meter.count_decode(scene.samplers.len() as u64)?;
    let sampler = if let Some(i) = scene
        .samplers
        .as_slice()
        .iter()
        .position(|s| *s == src.sampler)
    {
        i
    } else {
        let i = scene.samplers.len();
        scene.samplers.push(src.sampler)?;
        i
    };
    scene.textures.push(Texture {
        image,
        sampler: Some(sampler),
    })?;
    Ok(Embedded {
        texture,
        mirror_once: src.mirror_once || alpha.as_ref().is_some_and(|a| a.mirror_once),
        packed_sampler_conflict,
        alpha_reason,
    })
}
fn decode_error(e: unityrs_texture::DecodeError) -> GltfError {
    use unityrs_texture::DecodeError as E;
    match e {
        E::Charge(e) => GltfError::Charge(e),
        E::TooLarge { .. } | E::PillowMemoryError { .. } | E::DecompressionBomb { .. } => {
            GltfError::Invalid("texture_too_large")
        }
        E::NoImageData => GltfError::Invalid("missing_texture"),
        E::Hdr(_) | E::NegativeSize { .. } | E::NotEnoughData { .. } | E::Panicked { .. } => {
            GltfError::Invalid("texture_malformed")
        }
        _ => GltfError::Mesh(unityrs_mesh::MeshError::Texture(e)),
    }
}
