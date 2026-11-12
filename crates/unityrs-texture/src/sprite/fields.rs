// Ported from UnityPy 1.25.2 (MIT): UnityPy/export/SpriteHelper.py's get_image_from_sprite (the render data it reads). See THIRD_PARTY_LICENSES.
//! What `get_image_from_sprite` reads of a Sprite and of its atlas
//! (`export/SpriteHelper.py:63-88`), from a [`Parsed`] object: the
//! Sprite's `m_PixelsToUnits`, `m_RD`, `m_SpriteAtlas`, `m_AtlasTags` and
//! `m_RenderDataKey`, and an atlas's `m_RenderDataMap` entry for the
//! sprite's key.
//!
//! [`select_render_data`] settles which render data a sprite renders
//! from, as `get_image_from_sprite` settles it (`:63-81`): a non-null
//! `m_SpriteAtlas` ([`Pptr::is_set`]) names the atlas; else, where
//! `m_AtlasTags` is not empty, the file's first `SpriteAtlas` whose name is
//! its first tag (a `SpriteAtlas` of another name before it is skipped, and
//! none found is no atlas); with an atlas, the entry of its
//! `m_RenderDataMap` whose key equals `m_RenderDataKey`
//! ([`atlas_render_data`]; none is `StopIteration`,
//! [`SpriteError::NoAtlasData`]); with none, the Sprite's own `m_RD`
//! ([`SpriteFields::rd`]). The caller resolves the pointer and lists the
//! file's atlases, since both are the file's. The mesh is always the
//! Sprite's own `m_RD`'s.
//!
//! A key's `GUID` is UnityPy's `GUID` class, whose equality compares
//! `data_0_` to `data_3_` in that order (`classes/generated.py`), so its
//! words are read by name (`data[0]` to `data[3]`), whatever order the
//! typetree gives them.
//!
//! Each step of every loop here counts decode work on the caller's meter
//! before it runs, so that a file of many atlases, an atlas of many entries
//! or fields, or names of many bytes, scanned for each of many sprites, is
//! bounded by the caller's allowance: each field a lookup walks (`field`,
//! the [`Parsed::field`] scan, metered), each atlas compared, each map
//! entry, and each tag checked, counts one unit, and each name or tag
//! comparison as many more as the bytes it may read (`compared`).

use std::sync::Arc;

use unityrs_core::limits::Meter;
use unityrs_core::{Map, Parsed, Value};

use super::{SpriteData, SpriteError};

/// A `PPtr`'s two ids.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Pptr {
    /// `m_FileID`.
    pub file_id: i64,
    /// `m_PathID`.
    pub path_id: i64,
}

impl Pptr {
    /// `bool(pptr)`: `m_PathID != 0` (`classes/PPtr.py`).
    #[must_use]
    pub fn is_set(&self) -> bool {
        self.path_id != 0
    }
}

/// A `GUID`'s four words, `data[0]` to `data[3]`, by name.
pub type Guid = [u64; 4];

/// A `pair<GUID, SInt64>`: `m_RenderDataKey`, and a key of an atlas's
/// `m_RenderDataMap`, equal as UnityPy's tuples of a `GUID` and an `int`
/// are, field by field.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AtlasKey {
    /// The `GUID`.
    pub guid: Guid,
    /// The `SInt64`.
    pub id: i64,
}

/// A render data's texture, alpha texture, rectangle and settings: a
/// `SpriteRenderData` (`m_RD`) or an atlas's `SpriteAtlasData`
/// (`:83-86`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderData {
    /// `texture`.
    pub texture: Pptr,
    /// `alphaTexture`, `None` where the version has none; merged in only
    /// where [`Pptr::is_set`] (`get_image`, `:47`).
    pub alpha_texture: Option<Pptr>,
    /// `textureRect`: `x`, `y`, `width`, `height`.
    pub texture_rect: [f64; 4],
    /// `settingsRaw`.
    pub settings_raw: u64,
}

const _: () = crate::send_sync::<RenderData>();
const _: () = crate::send_sync::<Pptr>();
const _: () = crate::send_sync::<Guid>();
const _: () = crate::send_sync::<AtlasKey>();
const _: () = crate::send_sync::<SpriteFields<'static>>();

/// What [`render()`](super::render()) and [`select_render_data`] read of
/// a Sprite, borrowing from its [`Parsed`] tree.
#[derive(Clone, Debug, PartialEq)]
pub struct SpriteFields<'p> {
    /// `m_PixelsToUnits`.
    pub pixels_to_units: f64,
    /// `m_RD`'s render data.
    pub rd: RenderData,
    /// `m_SpriteAtlas`, where the version has it.
    pub sprite_atlas: Option<Pptr>,
    /// `m_AtlasTags[0]` (raw bytes, as the file holds them), where the
    /// version has the field and it is not empty: the one tag UnityPy
    /// reads. Borrowed, so a Sprite of many tags costs nothing here.
    pub atlas_tag: Option<&'p [u8]>,
    /// `m_RenderDataKey`, where the version has it.
    pub render_data_key: Option<AtlasKey>,
}

impl<'p> SpriteFields<'p> {
    /// Reads them from a parsed Sprite, counting each lookup's and the tag
    /// check's work on `meter` (module docs); a field missing, or of
    /// another type than its typetree's (a tag that is not a string among
    /// them), is [`SpriteError::Malformed`].
    pub fn read(sprite: &'p Parsed, meter: &Arc<Meter>) -> Result<Self, SpriteError> {
        let top = |name: &[u8]| top(sprite, name, meter);
        let pixels_to_units = top(b"m_PixelsToUnits")?
            .and_then(Value::as_f64)
            .ok_or(SpriteError::Malformed("m_PixelsToUnits"))?;
        let rd = top(b"m_RD")?.ok_or(SpriteError::Malformed("m_RD"))?;
        let rd = render_data(sprite, rd, "m_RD", meter)?;
        let sprite_atlas = match top(b"m_SpriteAtlas")? {
            None => None,
            Some(v) => {
                Some(pptr(sprite, v, meter)?.ok_or(SpriteError::Malformed("m_SpriteAtlas"))?)
            }
        };
        let atlas_tag = match top(b"m_AtlasTags")? {
            None => None,
            Some(Value::List(items)) => {
                for v in items {
                    meter.count_decode(1)?;
                    if v.as_bytes().is_none() {
                        return Err(SpriteError::Malformed("m_AtlasTags"));
                    }
                }
                items.first().and_then(Value::as_bytes)
            }
            Some(_) => return Err(SpriteError::Malformed("m_AtlasTags")),
        };
        let render_data_key = match top(b"m_RenderDataKey")? {
            None => None,
            Some(v) => {
                Some(key(sprite, v, meter)?.ok_or(SpriteError::Malformed("m_RenderDataKey"))?)
            }
        };
        Ok(Self {
            pixels_to_units,
            rd,
            sprite_atlas,
            atlas_tag,
            render_data_key,
        })
    }

    /// The render data `rd` (the Sprite's own or its atlas's) with the
    /// Sprite's `m_PixelsToUnits`.
    #[must_use]
    pub fn data(&self, rd: &RenderData) -> SpriteData {
        SpriteData {
            texture_rect: rd.texture_rect,
            settings_raw: rd.settings_raw,
            pixels_to_units: self.pixels_to_units,
        }
    }
}

/// The render data `get_image_from_sprite` renders `sprite` from (module
/// docs, `:63-81`): `deref` resolves `m_SpriteAtlas` where it is set
/// (`None`, no object, is [`SpriteError::AtlasUnresolved`]); else, where
/// the Sprite has an atlas tag, `file_atlases` lists the file's
/// `SpriteAtlas` objects in its order, each as its name (`peek_name`) and a
/// parse of it, which is called only for the first whose name is the tag;
/// else, or with no atlas found, the Sprite's own `m_RD`. Each atlas
/// compared counts one unit of `meter`'s decode work, and as many more as
/// the bytes its name's comparison with the tag may read (`compared`),
/// before it is compared; [`atlas_render_data`] counts its own (module
/// docs).
pub fn select_render_data<'a, D, I, F>(
    sprite: &SpriteFields<'_>,
    deref: D,
    file_atlases: I,
    meter: &Arc<Meter>,
) -> Result<RenderData, SpriteError>
where
    D: FnOnce(Pptr) -> Option<&'a Parsed>,
    I: IntoIterator<Item = (&'a [u8], F)>,
    F: FnOnce() -> Option<&'a Parsed>,
{
    let key = sprite.render_data_key.as_ref();
    if let Some(p) = sprite.sprite_atlas.filter(Pptr::is_set) {
        let atlas = deref(p).ok_or(SpriteError::AtlasUnresolved)?;
        return atlas_render_data(atlas, key, meter);
    }
    if let Some(tag) = sprite.atlas_tag {
        for (name, parse) in file_atlases {
            meter.count_decode(compared(name, tag))?;
            if name == tag {
                let atlas = parse().ok_or(SpriteError::AtlasUnresolved)?;
                return atlas_render_data(atlas, key, meter);
            }
        }
    }
    Ok(sprite.rd)
}

/// The `SpriteAtlasData` of `atlas`'s `m_RenderDataMap` whose key is `key`
/// (`next(value for key, value in ...)`, `:78`): the first such entry, or
/// [`SpriteError::NoAtlasData`] where none is. A Sprite with no
/// `m_RenderDataKey` compares `None` with every key, which matches none.
/// Each entry counts one unit of `meter`'s decode work before it is read,
/// and each field lookup its own (module docs): the atlas's fields walked
/// to `m_RenderDataMap`, each key's to its words.
pub fn atlas_render_data(
    atlas: &Parsed,
    key: Option<&AtlasKey>,
    meter: &Arc<Meter>,
) -> Result<RenderData, SpriteError> {
    let map =
        top(atlas, b"m_RenderDataMap", meter)?.ok_or(SpriteError::Malformed("m_RenderDataMap"))?;
    let Value::List(entries) = map else {
        return Err(SpriteError::Malformed("m_RenderDataMap"));
    };
    for entry in entries {
        meter.count_decode(1)?;
        let Value::Tuple(pair) = entry else {
            return Err(SpriteError::Malformed("m_RenderDataMap"));
        };
        let (Some(k), Some(v)) = (pair.first(), pair.get(1)) else {
            return Err(SpriteError::Malformed("m_RenderDataMap"));
        };
        let k = self::key(atlas, k, meter)?.ok_or(SpriteError::Malformed("m_RenderDataMap"))?;
        if Some(&k) == key {
            return render_data(atlas, v, "m_RenderDataMap", meter);
        }
    }
    Err(SpriteError::NoAtlasData)
}

fn render_data(
    parsed: &Parsed,
    v: &Value,
    what: &'static str,
    meter: &Meter,
) -> Result<RenderData, SpriteError> {
    let Value::Dict(m) = v else {
        return Err(SpriteError::Malformed(what));
    };
    let field = |name: &[u8]| field(parsed, m, name, meter);
    let texture = match field(b"texture")? {
        Some(t) => pptr(parsed, t, meter)?,
        None => None,
    }
    .ok_or(SpriteError::Malformed(what))?;
    let alpha_texture = match field(b"alphaTexture")? {
        None => None,
        Some(t) => Some(pptr(parsed, t, meter)?.ok_or(SpriteError::Malformed(what))?),
    };
    let rect = field(b"textureRect")?
        .and_then(Value::as_dict)
        .ok_or(SpriteError::Malformed(what))?;
    let mut texture_rect = [0.0; 4];
    for (out, name) in texture_rect
        .iter_mut()
        .zip([&b"x"[..], b"y", b"width", b"height"])
    {
        *out = self::field(parsed, rect, name, meter)?
            .and_then(Value::as_f64)
            .ok_or(SpriteError::Malformed(what))?;
    }
    let settings_raw = field(b"settingsRaw")?
        .and_then(Value::as_u64)
        .ok_or(SpriteError::Malformed(what))?;
    Ok(RenderData {
        texture,
        alpha_texture,
        texture_rect,
        settings_raw,
    })
}

/// A `PPtr`'s ids, or `Ok(None)` where `v` is no `PPtr`.
fn pptr(parsed: &Parsed, v: &Value, meter: &Meter) -> Result<Option<Pptr>, SpriteError> {
    let Some(m) = v.as_dict() else {
        return Ok(None);
    };
    let id =
        |name: &[u8]| Ok::<_, SpriteError>(field(parsed, m, name, meter)?.and_then(Value::as_i64));
    let (Some(file_id), Some(path_id)) = (id(b"m_FileID")?, id(b"m_PathID")?) else {
        return Ok(None);
    };
    Ok(Some(Pptr { file_id, path_id }))
}

/// A `pair<GUID, SInt64>`, or `Ok(None)` where `v` is none.
fn key(parsed: &Parsed, v: &Value, meter: &Meter) -> Result<Option<AtlasKey>, SpriteError> {
    let Value::Tuple(pair) = v else {
        return Ok(None);
    };
    let (Some(guid), Some(id)) = (
        pair.first().and_then(Value::as_dict),
        pair.get(1).and_then(Value::as_i64),
    ) else {
        return Ok(None);
    };
    let mut words = [0_u64; 4];
    for (w, name) in words
        .iter_mut()
        .zip([&b"data[0]"[..], b"data[1]", b"data[2]", b"data[3]"])
    {
        let Some(word) = field(parsed, guid, name, meter)?.and_then(Value::as_u64) else {
            return Ok(None);
        };
        *w = word;
    }
    Ok(Some(AtlasKey { guid: words, id }))
}

/// The work of comparing two names: one unit, and as many more as the bytes
/// the comparison may read, which is every byte where their lengths are
/// equal (a slice comparison reads none where they differ).
fn compared(a: &[u8], b: &[u8]) -> u64 {
    1 + if a.len() == b.len() {
        a.len() as u64
    } else {
        0
    }
}

/// [`Parsed::field`], metered: the value under `name` in `map`, each field
/// walked to it (or past it, with none found) counted with its name's
/// comparison ([`compared`]) before it is compared.
fn field<'a>(
    parsed: &'a Parsed,
    map: &'a Map,
    name: &[u8],
    meter: &Meter,
) -> Result<Option<&'a Value>, SpriteError> {
    for (n, v) in parsed.entries(map) {
        meter.count_decode(compared(n, name))?;
        if n == name {
            return Ok(Some(v));
        }
    }
    Ok(None)
}

/// [`field`] of the root value of `parsed` (`parsed.get(&[name])`).
fn top<'a>(
    parsed: &'a Parsed,
    name: &[u8],
    meter: &Meter,
) -> Result<Option<&'a Value>, SpriteError> {
    match parsed.value() {
        Value::Dict(m) => field(parsed, m, name, meter),
        _ => Ok(None),
    }
}
