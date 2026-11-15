// Ported from UnityPy 1.25.2 (MIT): tools/extractor.py and export/SpriteHelper.py. See THIRD_PARTY_LICENSES.
//! tools/extractor.py:250-281, with W2's extension dispatcher and Tier C
//! zero-side/no-extension policy. The Sprite always returns its own `m_RD`
//! texture, even when an atlas supplied the rendered image.
use super::{Error, Identity, Report, Sink, Written, default_ext, paths};
use crate::simple;
use std::sync::Arc;
use unityrs_core::{
    ObjectRef, Parsed, PrimArray, Value,
    env::{Environment, PPtr},
    limits::{Charge, ChargeError, Meter, MeteredVec, heap_cost},
};
use unityrs_imageio::writers::Outcome;
use unityrs_mesh::obj::{ChargedBytes, MeteredWrite};
use unityrs_meshdecode::{MeshHandler, Source, Version};
use unityrs_texture::{
    Format, Formats, Image, Mode, Selection, Skip, TextureFields, fetch, select_in,
    sprite::{self, Sheet, SpriteFields},
};
fn field<'a>(p: &'a Parsed, v: &'a Value, name: &[u8]) -> Result<&'a Value, Error> {
    v.as_dict()
        .and_then(|m| p.attr(m, name))
        .ok_or(Error::Field("image field"))
}
fn int(p: &Parsed, name: &[u8]) -> Result<i32, Error> {
    field(p, p.value(), name)?
        .as_i64()
        .and_then(|n| i32::try_from(n).ok())
        .ok_or(Error::Field("image integer"))
}
fn texture(env: &Environment, obj: &ObjectRef, meter: &Arc<Meter>) -> Result<Image, Error> {
    let p = simple::read(obj, env)?;
    let (width, height) = (int(&p, b"m_Width")?, int(&p, b"m_Height")?);
    let id = int(&p, b"m_TextureFormat")?;
    let Value::Bytes(inline) = field(&p, p.value(), b"image_data")? else {
        return Err(Error::Field("image_data"));
    };
    let stream = p
        .value()
        .as_dict()
        .and_then(|m| p.attr(m, b"m_StreamData"))
        .map(|s| {
            let Value::Str(path) = field(&p, s, b"path")? else {
                return Err(Error::Field("path"));
            };
            Ok::<_, Error>(fetch::Stream {
                path: path.clone(),
                offset: field(&p, s, b"offset")?
                    .as_i64()
                    .map(i128::from)
                    .or_else(|| field(&p, s, b"offset").ok()?.as_u64().map(i128::from))
                    .ok_or(Error::Field("offset"))?,
                size: field(&p, s, b"size")?
                    .as_i64()
                    .map(i128::from)
                    .or_else(|| field(&p, s, b"size").ok()?.as_u64().map(i128::from))
                    .ok_or(Error::Field("size"))?,
            })
        })
        .transpose()?;
    let bytes = fetch::fetch(env, obj.file(), inline, stream.as_ref(), meter)?;
    let version = if width != 0
        && height != 0
        && !bytes.is_empty()
        && Format::from_texture_format(id).is_some_and(Format::is_crunched)
    {
        let (major, minor, ..) = obj
            .file()
            .version()
            .ok_or(Error::Field("version"))?
            .version
            .as_tuple()
            .map_err(|_| Error::Field("version"))?;
        (major, minor)
    } else {
        (u16::MAX, u16::MAX)
    };
    let format = match select_in(
        &TextureFields {
            texture_format: id,
            width,
            height,
            // Platform handling below also supports Xbox byte swapping.
            platform: 0,
            platform_blob: None,
        },
        Formats::DECODED,
        version,
    ) {
        Selection::Export(format) => format,
        Selection::Skip(Skip::EmptyImage) => Format::Rgba32,
        Selection::Skip(_) => return Err(Error::Field("unsupported texture format")),
    };
    let platform = obj.file().target_platform().unwrap_or(0);
    let blob = p
        .value()
        .as_dict()
        .and_then(|m| p.attr(m, b"m_PlatformBlob"))
        .and_then(|v| match v {
            Value::Bytes(b) | Value::Prims(PrimArray::U8(b)) => Some(b.as_ref()),
            _ => None,
        });
    if width != 0 && height != 0 && unityrs_texture::is_switch_swizzled(platform, blob) {
        return Err(Error::Field("unsupported swizzled texture"));
    }
    let mut swapped = MeteredVec::new(meter);
    let bytes = if platform == unityrs_texture::XBOX360
        && unityrs_texture::XBOX_SWAP_FORMATS.contains(&id)
    {
        swapped.reserve_exact(bytes.len())?;
        for pair in bytes.chunks(2) {
            for &byte in pair.iter().rev() {
                swapped.push(byte)?;
            }
        }
        swapped.as_slice()
    } else {
        bytes.as_ref()
    };
    Ok(unityrs_texture::decode(
        format, bytes, width, height, meter,
    )?)
}
fn deref(env: &Environment, obj: &ObjectRef, ptr: sprite::Pptr) -> Result<ObjectRef, Error> {
    Ok(env.deref(
        obj.file(),
        PPtr {
            file_id: i32::try_from(ptr.file_id).map_err(|_| Error::Field("m_FileID"))?,
            path_id: ptr.path_id,
        },
    )?)
}
pub(super) fn image(
    env: &Environment,
    obj: &ObjectRef,
    parsed: &Parsed,
    meter: &Arc<Meter>,
) -> Result<(Image, Option<Identity>), Error> {
    if obj.class_id() == 28 {
        // exportTexture2D tests width before fetching any image data.
        if int(parsed, b"m_Width")? == 0 {
            return Ok((
                unityrs_texture::decode(Format::Rgba32, &[], 0, 0, meter)?,
                None,
            ));
        }
        return Ok((texture(env, obj, meter)?, None));
    }
    let fields = SpriteFields::read(parsed, meter)?;
    let mut atlas = fields
        .sprite_atlas
        .filter(sprite::Pptr::is_set)
        .map(|p| deref(env, obj, p))
        .transpose()?;
    if atlas.is_none()
        && let Some(tag) = fields.atlas_tag
    {
        for candidate in obj.file().objects().filter(|o| o.class_id() == 687_078_895) {
            meter.count_decode(1)?;
            let parsed = match simple::read(&candidate, env) {
                Ok(parsed) => parsed,
                Err(error) => {
                    let error = Error::from(error);
                    if error.resource_refusal() {
                        return Err(error);
                    }
                    continue;
                }
            };
            if parsed
                .value()
                .as_dict()
                .and_then(|m| parsed.attr(m, b"m_Name"))
                .is_some_and(|v| matches!(v, Value::Str(n) if n.as_bytes() == tag))
            {
                atlas = Some(candidate);
                break;
            }
        }
    }
    let rd = if let Some(atlas) = atlas.as_ref() {
        sprite::atlas_render_data(
            &simple::read(atlas, env)?,
            fields.render_data_key.as_ref(),
            meter,
        )?
    } else {
        fields.rd
    };
    let holder = atlas.as_ref().unwrap_or(obj);
    let tex = deref(env, holder, rd.texture)?;
    let image = texture(env, &tex, meter)?;
    let alpha = rd
        .alpha_texture
        .filter(sprite::Pptr::is_set)
        .map(|p| {
            let obj = deref(env, holder, p)?;
            texture(env, &obj, meter)
        })
        .transpose()?;
    let sheet = Sheet::new(&image, alpha.as_ref(), meter)?;
    let version = obj.file().version().ok_or(Error::Field("version"))?;
    let rendered = sprite::render(
        &sheet,
        &fields.data(&rd),
        || {
            MeshHandler::new(Source::sprite(parsed), Version::of(&version.version))
                .process(None, meter)
        },
        meter,
    )?;
    // UnityPy returns the original render data's pointer's assetsfile, not
    // the dereferenced target's file (important for external textures).
    let returned = Some(Identity {
        file: obj.file().clone(),
        path_id: fields.rd.texture.path_id,
    });
    Ok((rendered, returned))
}
struct Charger<'a>(&'a Arc<Meter>);
impl unityrs_imageio::Charger for Charger<'_> {
    type Charge = Charge;
    type Refusal = ChargeError;
    fn charge(
        &mut self,
        _: unityrs_imageio::Buffer,
        requests: &[usize],
    ) -> Result<Charge, ChargeError> {
        self.0.charge(
            requests
                .iter()
                .fold(0_u64, |n, &r| n.saturating_add(heap_cost(r as u64))),
        )
    }
}
#[allow(clippy::too_many_arguments)]
pub(super) fn save(
    obj: &ObjectRef,
    parsed: &Parsed,
    image: &Image,
    stem: &[u8],
    extension: &[u8],
    sink: &mut dyn Sink,
    meter: &Arc<Meter>,
    report: &mut Report,
) -> Result<(), Error> {
    // UnityPy skips zero-width textures before fetching, but dispatches
    // zero-height textures and zero-side sprites to the selected writer.
    if obj.class_id() == 28 && int(parsed, b"m_Width")? == 0 {
        return Ok(());
    }
    let extension = default_ext(extension, b".png");
    let path = paths::suffix(stem, extension, meter)?;
    let mode = match image.mode() {
        Mode::L => unityrs_imageio::Mode::L,
        Mode::Rgb => unityrs_imageio::Mode::Rgb,
        Mode::Rgba => unityrs_imageio::Mode::Rgba,
    };
    let view = unityrs_imageio::Image::new(image.width(), image.height(), mode, image.pixels())
        .map_err(|_| Error::Field("image view"))?;
    let basename = path
        .as_bytes()
        .rsplit(|b| *b == b'/')
        .next()
        .unwrap_or_default();
    let mut decoded = ChargedBytes::new(meter);
    let filename = if let Ok(name) = std::str::from_utf8(basename) {
        name
    } else {
        // SGI ignores non-ASCII characters; IM counts code points before
        // ASCII encoding. One non-ASCII scalar per surrogateescaped byte
        // preserves both behaviors, including IM's truncation, without
        // trying to represent Python's lone surrogates in a Rust str.
        for chunk in basename.utf8_chunks() {
            decoded.write_bytes(chunk.valid().as_bytes())?;
            for _ in chunk.invalid() {
                decoded.write_bytes("\u{fffd}".as_bytes())?;
            }
        }
        std::str::from_utf8(decoded.as_bytes()).map_err(|_| Error::Field("image filename"))?
    };
    let outcome = unityrs_imageio::writers::save_by_extension(
        &view,
        std::str::from_utf8(extension).unwrap_or(""),
        filename,
        &mut Charger(meter),
    )
    .map_err(Error::Image)?;
    let (path, saved) = match outcome {
        Outcome::NoFile => return Ok(()),
        Outcome::Original(s) => (path, s),
        Outcome::PngFallback(s) => (paths::suffix(path.as_bytes(), b".png", meter)?, s),
    };
    report.written.reserve(1)?;
    meter.count_decode(saved.bytes().len() as u64)?;
    sink.file(path.as_bytes(), saved.bytes())?;
    report.written.push(Written {
        object: obj.clone(),
        path,
        size: saved.bytes().len() as u64,
    })?;
    Ok(())
}
