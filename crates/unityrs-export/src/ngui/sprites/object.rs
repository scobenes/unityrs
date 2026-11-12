// Ported from UnityPy 1.25.2 (MIT): UnityPyBoost/TypeTreeHelper.cpp's parse_class (which class a node's value is constructed as, else an UnknownObject), UnityPy/classes/generated.py's Sprite, SpriteRenderData, SpriteAtlasData and GUID, classes/PPtr.py's PPtr, files/ObjectReader.py's read (its set_object_reader), and export/SpriteHelper.py's get_image_from_sprite and get_image (how they walk, compare, test and follow what those give). See THIRD_PARTY_LICENSES.
//! The objects the sprites kind reads, as UnityPy constructs them (4.4
//! step 2; B3's review rounds 2 and 3, S3, S3a and S3b), from a [`Parsed`]
//! value: a Sprite, its render data and pointers, an atlas's
//! `m_RenderDataMap` and its entries, and a key's `GUID`.
//!
//! **Construction** (`parse_class`, `UnityPyBoost/TypeTreeHelper.cpp:458-583`,
//! the reader of every object of bytes): a value's attributes are its
//! node's children's cleaned names (`Parsed::attr`, so `m_PixelsToUnits?`
//! is `m_PixelsToUnits`); its class is `PPtr` where its node's type starts
//! with `PPtr<` (`UnityPyBoost/TypeTreeHelper.cpp:476-485`, which sets its
//! `assetsfile` over any child of that name), else the one its type names
//! ([`Layout`]). A class built with a field of no default missing is an
//! `UnknownObject`; so is a slotted class (one of no base: `PPtr`,
//! `SpriteRenderData`, `SpriteAtlasData`, `GUID`) with a field outside its
//! slots, and a class of instances with a `__dict__` (`Sprite`) with an
//! extra field named for an attribute its instances cannot set
//! ([`read::SETTERLESS`]). An `UnknownObject` keeps every field as an
//! attribute, and is true. Its object read then calls the value's
//! `set_object_reader` (`files/ObjectReader.py:260-261`): a root of a class
//! that has none (`PPtr`, `GUID`, `SpriteRenderData`, `SpriteAtlasData`),
//! or a child that cleans to that name and so shadows the method, fails the
//! read, `read` ([`attaches`]). Of the classes of no base, only these four
//! are modelled; a root of any other is a type no Unity build gives an
//! object (`PARITY.md`, Tier C).
//!
//! **What follows** (`get_image_from_sprite`, `export/SpriteHelper.py:63-86`,
//! and `get_image`, `export/SpriteHelper.py:44-59`):
//!
//! - a Sprite's object read must be a `Sprite` (its root's type, its six
//!   fields of no default, no setter-less extra): else it has no image,
//!   `read` ([`sprite`]); its rectangle, numbers and pointers' ids are of
//!   E17's shapes, else `read` (a stated rule, `PARITY.md`);
//! - a pointer is a constructed `PPtr` of integer ids, at the typetree's
//!   width ([`Ptr`]); any other value is followed as UnityPy follows it: its
//!   truth tested as Python's, and its `.path_id` or `deref` an
//!   `AttributeError`, `render`. A `PPtr`'s truth is `m_PathID != 0`
//!   (`classes/PPtr.py:98-99`), wherever UnityPy tests one ([`truthy`]);
//! - the atlas is used whatever class it is constructed as (an
//!   `UnknownObject` is true): its `m_RenderDataMap` attribute, none
//!   `render` (the `AttributeError`), walked as `next(...)` walks it
//!   ([`walk`]): each item unpacked into a key and a value (an item that
//!   does not unpack, or a map that is no sequence, is `render`), up to the
//!   first whose key equals the Sprite's;
//! - keys equal as Python's tuples do ([`Key`]): a `pair` of a constructed
//!   `GUID` (its words compared by `==`) and an id, numbers equal by value
//!   whatever their type ([`Num`]); a `GUID` that is an `UnknownObject`
//!   never equals one, nor does a key of a shape the engine does not
//!   compare (a stated divergence, `PARITY.md`);
//! - the matched value must be a constructed `SpriteAtlasData`
//!   (`export/SpriteHelper.py:79`'s `assert`, else `render`), then of E17's
//!   shapes ([`atlas_data`]);
//! - a constructed `SpriteRenderData` gives `alphaTexture` its default,
//!   none; an `UnknownObject`'s render data with no `alphaTexture` raises
//!   where it is read, `render` (`export/SpriteHelper.py:84`, [`Alpha`]);
//! - `m_AtlasTags[0]` is the tag only where it is a string; any other first
//!   item equals no atlas's name, so the Sprite's own `m_RD` ([`Tag`]).
//!
//! Each child or entry walked counts one unit of the meter's decode work
//! before it is looked at (a class check one per child and name checked).

use std::sync::Arc;

use unityrs_core::limits::Meter;
use unityrs_core::typetree::{NodeId, NodeList};
use unityrs_core::walk::write::cleans_to;
use unityrs_core::{Map, Parsed, Value};

use super::super::classify::{self, Class, Reason};
use super::super::read;

/// A class as `parse_class` builds it (module docs): the type its node
/// names (or the prefix, for `PPtr`), its fields of no default, its slots
/// where it has them, and the attributes its instances cannot set.
struct Layout {
    ty: &'static [u8],
    prefix: bool,
    required: &'static [&'static [u8]],
    slots: Option<&'static [&'static [u8]]>,
    setterless: &'static [&'static [u8]],
}

/// `Sprite`'s (`classes/generated.py:4867-4882`).
const SPRITE: Layout = Layout {
    ty: b"Sprite",
    prefix: false,
    required: &[
        b"m_Extrude",
        b"m_Name",
        b"m_Offset",
        b"m_PixelsToUnits",
        b"m_RD",
        b"m_Rect",
    ],
    slots: None,
    setterless: &read::SETTERLESS,
};

/// `PPtr`'s (`classes/PPtr.py:15-19`): its `assetsfile` is set by
/// `parse_class` itself, so a child of that name changes nothing.
const PPTR: Layout = Layout {
    ty: b"PPtr<",
    prefix: true,
    required: &[b"m_FileID", b"m_PathID"],
    slots: Some(&[b"m_FileID", b"m_PathID", b"assetsfile"]),
    setterless: &[],
};

/// `SpriteRenderData`'s (`classes/generated.py:10903-10919`): its four
/// fields of no default first.
const RENDER_DATA: Layout = Layout {
    ty: b"SpriteRenderData",
    prefix: false,
    required: &[
        b"settingsRaw",
        b"texture",
        b"textureRect",
        b"textureRectOffset",
    ],
    slots: Some(&[
        b"settingsRaw",
        b"texture",
        b"textureRect",
        b"textureRectOffset",
        b"alphaTexture",
        b"atlasRectOffset",
        b"downscaleMultiplier",
        b"indices",
        b"m_Bindpose",
        b"m_IndexBuffer",
        b"m_SourceSkin",
        b"m_SubMeshes",
        b"m_VertexData",
        b"secondaryTextures",
        b"uvTransform",
        b"vertices",
    ]),
    setterless: &[],
};

/// `SpriteAtlasData`'s (`classes/generated.py:10828-10837`).
const ATLAS_DATA: Layout = Layout {
    ty: b"SpriteAtlasData",
    prefix: false,
    required: &[
        b"alphaTexture",
        b"downscaleMultiplier",
        b"settingsRaw",
        b"texture",
        b"textureRect",
        b"textureRectOffset",
        b"uvTransform",
    ],
    slots: Some(&[
        b"alphaTexture",
        b"downscaleMultiplier",
        b"settingsRaw",
        b"texture",
        b"textureRect",
        b"textureRectOffset",
        b"uvTransform",
        b"atlasRectOffset",
        b"secondaryTextures",
    ]),
    setterless: &[],
};

/// `GUID`'s words, each of no default (`classes/generated.py:8023-8027`).
const GUID_WORDS: [&[u8]; 4] = [b"data_0_", b"data_1_", b"data_2_", b"data_3_"];

const GUID: Layout = Layout {
    ty: b"GUID",
    prefix: false,
    required: &GUID_WORDS,
    slots: Some(&GUID_WORDS),
    setterless: &[],
};

/// The classes of no `set_object_reader` the engine models (module docs).
const UNATTACHED: [&Layout; 4] = [&PPTR, &GUID, &RENDER_DATA, &ATLAS_DATA];

/// `decode_error` `render`: an exception `get_image_from_sprite` raises.
pub(super) fn render() -> Class {
    Class::Skip(Reason::decode_error("render"))
}

fn work(meter: &Meter, n: usize) -> Result<(), Class> {
    meter
        .count_decode(u64::try_from(n).unwrap_or(u64::MAX))
        .map_err(|e| classify::charge(&e))
}

/// A class's node, as `Parsed::attr` finds it: the node whose first child
/// is the map's first entry's key.
fn node<'a>(parsed: &'a Parsed, map: &Map) -> Option<(&'a NodeList, NodeId)> {
    let list = usize::try_from(map.list())
        .ok()
        .and_then(|i| parsed.lists().get(i))?;
    let first = map.entries().first()?.0;
    let n = NodeId(first.0.checked_sub(1)?);
    (list.children(n).first() == Some(&first)).then_some((&**list, n))
}

/// Whether UnityPy constructs `map`'s value as `layout`'s class (module
/// docs): its node's type is the class's, a child cleans to each of its
/// fields of no default, no child cleans to an attribute its instances
/// cannot set, and, for a slotted class, each child cleans to one of its
/// slots.
fn constructs(parsed: &Parsed, map: &Map, layout: &Layout, meter: &Meter) -> Result<bool, Class> {
    let Some((list, n)) = node(parsed, map) else {
        return Ok(false);
    };
    let ty = list.type_name(n);
    let typed = if layout.prefix {
        ty.starts_with(layout.ty)
    } else {
        ty == layout.ty
    };
    if !typed {
        return Ok(false);
    }
    let kids = list.children(n);
    let names =
        layout.required.len() + layout.slots.map_or(0, <[_]>::len) + layout.setterless.len();
    work(meter, kids.len().saturating_mul(names))?;
    let cleaned = |k: &NodeId, name: &[u8]| cleans_to(list.name(*k), name);
    let has = layout
        .required
        .iter()
        .all(|r| kids.iter().any(|k| cleaned(k, r)));
    let fits = layout
        .slots
        .is_none_or(|s| kids.iter().all(|k| s.iter().any(|x| cleaned(k, x))));
    let sets = !kids
        .iter()
        .any(|k| layout.setterless.iter().any(|x| cleaned(k, x)));
    Ok(has && fits && sets)
}

/// Whether UnityPy's object read of `parsed` attaches its reader
/// (`files/ObjectReader.py:260-261`, module docs): its root is of a class
/// that has `set_object_reader`, and no child of it shadows the method;
/// else the read fails, `read`.
pub(super) fn attaches(parsed: &Parsed, meter: &Meter) -> Result<bool, Class> {
    let Value::Dict(root) = parsed.value() else {
        return Ok(false);
    };
    work(meter, root.len())?;
    if read::shadowed(parsed, root) {
        return Ok(false);
    }
    for layout in UNATTACHED {
        if constructs(parsed, root, layout, meter)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// The attribute `name` of `map`'s object (`Parsed::attr`), its children
/// counted.
fn attr<'a>(
    parsed: &'a Parsed,
    map: &'a Map,
    name: &[u8],
    meter: &Meter,
) -> Result<Option<&'a Value>, Class> {
    work(meter, map.len().saturating_add(1))?;
    Ok(parsed.attr(map, name))
}

/// A number as Python's `==` compares it, whatever its type: an integer of
/// any width (a `bool` its 0 or 1, a float with no fraction its integer),
/// else a float's bits. A NaN equals nothing, so it is none.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Num {
    /// An integer's value.
    Int(i128),
    /// A float with a fraction, or infinite.
    Float(u64),
}

impl Num {
    /// `v` as a number, or none (not a number, or a NaN).
    #[must_use]
    pub fn of(v: &Value) -> Option<Self> {
        match v {
            Value::Int(i) => Some(Self::Int(i128::from(*i))),
            Value::UInt(u) => Some(Self::Int(i128::from(*u))),
            Value::Bool(b) => Some(Self::Int(i128::from(*b))),
            Value::F32(_) | Value::F64(_) => v.as_f64().and_then(Self::float),
            _ => None,
        }
    }

    fn float(x: f64) -> Option<Self> {
        if x.is_nan() {
            return None;
        }
        if x.fract() == 0.0 && x.abs() < 2f64.powi(100) {
            // Exact: an integral float of this size is an integer of it.
            #[allow(clippy::cast_possible_truncation)]
            return Some(Self::Int(x as i128));
        }
        Some(Self::Float(x.to_bits()))
    }
}

/// A pointer field's value, as UnityPy constructs it (module docs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Ptr {
    /// A constructed `PPtr` of integer ids.
    Ids(Ids),
    /// Any other value, of its truth: following it raises, `render`.
    Other(bool),
}

/// A `PPtr`'s ids, at the typetree's width (a `UInt64` past `2^63` kept).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Ids {
    /// `m_FileID`.
    pub file_id: i128,
    /// `m_PathID`.
    pub path_id: i128,
}

impl Ptr {
    /// The pointer's truth (`bool(...)`): a `PPtr`'s `m_PathID != 0`.
    #[must_use]
    pub fn is_set(self) -> bool {
        match self {
            Self::Ids(i) => i.path_id != 0,
            Self::Other(t) => t,
        }
    }

    /// The ids of a constructed `PPtr`, or none (following it raises).
    #[must_use]
    pub fn ids(self) -> Option<Ids> {
        match self {
            Self::Ids(i) => Some(i),
            Self::Other(_) => None,
        }
    }
}

/// `v` as a pointer ([`Ptr`]): a constructed `PPtr` whose ids are not
/// integers is `read` (E17's shapes, a stated rule).
fn pointer(parsed: &Parsed, v: &Value, meter: &Meter) -> Result<Ptr, Class> {
    if let Value::Dict(m) = v
        && constructs(parsed, m, &PPTR, meter)?
    {
        let id = |name: &[u8]| -> Result<i128, Class> {
            attr(parsed, m, name, meter)?
                .and_then(read::integer)
                .ok_or_else(read::shape)
        };
        let file_id = id(b"m_FileID")?;
        let path_id = id(b"m_PathID")?;
        return Ok(Ptr::Ids(Ids { file_id, path_id }));
    }
    Ok(Ptr::Other(truthy(parsed, v, meter)?))
}

/// `bool(v)` as Python takes it: a number not 0 (a NaN among them), a
/// sequence not empty, a constructed `PPtr` of `m_PathID != 0`
/// (`classes/PPtr.py:98-99`), and any other object true (an
/// `UnknownObject`, or a class of neither `__bool__` nor `__len__`).
fn truthy(parsed: &Parsed, v: &Value, meter: &Meter) -> Result<bool, Class> {
    Ok(match v {
        Value::Bool(b) => *b,
        Value::Int(i) => *i != 0,
        Value::UInt(u) => *u != 0,
        Value::F32(_) | Value::F64(_) => v.as_f64() != Some(0.0),
        Value::Str(s) => !s.as_bytes().is_empty(),
        Value::Bytes(b) => !b.is_empty(),
        Value::Prims(p) => !p.is_empty(),
        Value::List(l) => !l.is_empty(),
        Value::Tuple(t) => !t.is_empty(),
        Value::Dict(m) => {
            if constructs(parsed, m, &PPTR, meter)? {
                let path = attr(parsed, m, b"m_PathID", meter)?;
                !matches!(path.and_then(Num::of), Some(Num::Int(0)))
            } else {
                true
            }
        }
    })
}

/// A key UnityPy's `==` may find equal to another: a tuple of a
/// constructed `GUID`'s four words and an id, each a [`Num`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct Key {
    guid: [Num; 4],
    id: Num,
}

/// `v` as a [`Key`], or none where it equals no key (module docs).
pub(super) fn key(parsed: &Parsed, v: &Value, meter: &Meter) -> Result<Option<Key>, Class> {
    work(meter, 1)?;
    let Value::Tuple(pair) = v else {
        return Ok(None);
    };
    let [Value::Dict(guid), id] = &pair[..] else {
        return Ok(None);
    };
    let Some(id) = Num::of(id) else {
        return Ok(None);
    };
    if !constructs(parsed, guid, &GUID, meter)? {
        return Ok(None);
    }
    let mut words = [Num::Int(0); 4];
    for (w, name) in words.iter_mut().zip(GUID_WORDS) {
        let Some(n) = attr(parsed, guid, name, meter)?.and_then(Num::of) else {
            return Ok(None);
        };
        *w = n;
    }
    Ok(Some(Key { guid: words, id }))
}

/// What `m_AtlasTags` gives the lookup, read only where `m_SpriteAtlas` is
/// not set (`export/SpriteHelper.py:66-75`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tag<'p> {
    /// Absent or false (empty, 0, a `PPtr` of path id 0): no lookup.
    None,
    /// A first item that is a string: the tag.
    Name(&'p [u8]),
    /// A first item of another type, which equals no name: no atlas.
    NoMatch,
    /// True but not indexable (a number, an object, a `PPtr`):
    /// `m_AtlasTags[0]` raises, `render`.
    Raises,
}

fn first_char(s: &[u8]) -> &[u8] {
    // A `str` decoded with `surrogateescape`: a valid character, or one
    // byte that is none.
    let n = s.utf8_chunks().next().map_or(0, |c| {
        c.valid()
            .chars()
            .next()
            .map_or(usize::from(!c.invalid().is_empty()), char::len_utf8)
    });
    s.get(..n).unwrap_or_default()
}

fn tag<'p>(parsed: &Parsed, v: Option<&'p Value>, meter: &Meter) -> Result<Tag<'p>, Class> {
    let Some(v) = v else {
        return Ok(Tag::None);
    };
    if !truthy(parsed, v, meter)? {
        return Ok(Tag::None);
    }
    Ok(match v {
        Value::List(items) => first_of(items),
        Value::Tuple(items) => first_of(items),
        Value::Str(s) => Tag::Name(first_char(s.as_bytes())),
        // Indexable, and its first item a number or a byte's.
        Value::Prims(_) | Value::Bytes(_) => Tag::NoMatch,
        _ => Tag::Raises,
    })
}

fn first_of(items: &[Value]) -> Tag<'_> {
    match items.first() {
        Some(Value::Str(s)) => Tag::Name(s.as_bytes()),
        _ => Tag::NoMatch,
    }
}

/// A render data's `alphaTexture` (`export/SpriteHelper.py:84`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Alpha {
    /// A constructed class's default, none.
    None,
    /// An `UnknownObject`'s render data with none: its read raises,
    /// `render`.
    Missing,
    /// The pointer.
    Set(Ptr),
}

/// A `SpriteRenderData` or `SpriteAtlasData` of E17's shapes: a texture, an
/// alpha texture ([`Alpha`]), a rectangle of four floats and a
/// non-negative integer `settingsRaw`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct RenderData {
    /// `texture`.
    pub texture: Ptr,
    /// `alphaTexture`.
    pub alpha: Alpha,
    /// `textureRect`'s `x`, `y`, `width` and `height`.
    pub texture_rect: [f64; 4],
    /// `settingsRaw`.
    pub settings_raw: u64,
}

/// What the sprites kind reads of a Sprite, borrowing from its value.
pub(super) struct SpriteRead<'p> {
    /// `m_PixelsToUnits`.
    pub ppu: f64,
    /// `m_RD`'s render data.
    pub rd: RenderData,
    /// `m_SpriteAtlas`, where it has one.
    pub atlas: Option<Ptr>,
    /// `m_AtlasTags`'s.
    pub tag: Tag<'p>,
    /// `m_RenderDataKey`, where it is one a key may equal.
    pub key: Option<Key>,
}

/// A Sprite's object read as UnityPy's `Sprite` (module docs): its fields,
/// else `read`.
pub(super) fn sprite<'p>(parsed: &'p Parsed, meter: &Arc<Meter>) -> Result<SpriteRead<'p>, Class> {
    let Value::Dict(root) = parsed.value() else {
        return Err(read::shape());
    };
    work(meter, root.len())?;
    if read::shadowed(parsed, root) || !constructs(parsed, root, &SPRITE, meter)? {
        return Err(read::shape());
    }
    let ppu = attr(parsed, root, b"m_PixelsToUnits", meter)?
        .and_then(Value::as_f64)
        .ok_or_else(read::shape)?;
    let rd = match attr(parsed, root, b"m_RD", meter)? {
        Some(Value::Dict(m)) => {
            let constructed = constructs(parsed, m, &RENDER_DATA, meter)?;
            render_data(parsed, m, constructed, meter)?
        }
        _ => return Err(read::shape()),
    };
    let atlas = match attr(parsed, root, b"m_SpriteAtlas", meter)? {
        Some(v) => Some(pointer(parsed, v, meter)?),
        None => None,
    };
    let tag = tag(parsed, attr(parsed, root, b"m_AtlasTags", meter)?, meter)?;
    let key = match attr(parsed, root, b"m_RenderDataKey", meter)? {
        Some(v) => key(parsed, v, meter)?,
        None => None,
    };
    Ok(SpriteRead {
        ppu,
        rd,
        atlas,
        tag,
        key,
    })
}

/// An item of an atlas's `m_RenderDataMap` (`for key, value in ...`).
pub(super) enum Item<'p> {
    /// Unpacked: its key, where one may equal the Sprite's, and its value.
    Entry(Option<Key>, &'p Value),
    /// Unpacked, but its key is no value a key equals (a number, a
    /// character): never the match.
    Other,
}

/// How an item unpacks into two names.
#[derive(Debug, PartialEq)]
enum Unpacked<'v> {
    /// Into its two items, values.
    Pair(&'v Value, &'v Value),
    /// Into two numbers or characters.
    Opaque,
}

/// Whether `v` unpacks into two names, and how.
fn unpack(v: &Value) -> Option<Unpacked<'_>> {
    match v {
        Value::Tuple(t) => match &t[..] {
            [k, v] => Some(Unpacked::Pair(k, v)),
            _ => None,
        },
        Value::List(l) => match &l[..] {
            [k, v] => Some(Unpacked::Pair(k, v)),
            _ => None,
        },
        Value::Prims(p) => (p.len() == 2).then_some(Unpacked::Opaque),
        Value::Bytes(b) => (b.len() == 2).then_some(Unpacked::Opaque),
        Value::Str(s) => {
            let one = first_char(s.as_bytes()).len();
            let rest = s.as_bytes().get(one..).unwrap_or_default();
            (one > 0 && !rest.is_empty() && first_char(rest).len() == rest.len())
                .then_some(Unpacked::Opaque)
        }
        _ => None,
    }
}

/// `next(value for key, value in atlas.m_RenderDataMap if ...)`'s walk of
/// `atlas` (module docs): each item handed to `visit` in order, until
/// `visit` returns `false`; the walk's failure, `render`, in `Ok(Some(..))`
/// where an item, or the map, ends it (a key not yet met answers that). A
/// refused count, or `visit`'s error, is `Err`.
pub(super) fn walk<'p>(
    atlas: &'p Parsed,
    meter: &Meter,
    mut visit: impl FnMut(Item<'p>) -> Result<bool, Class>,
) -> Result<Option<Class>, Class> {
    let Value::Dict(root) = atlas.value() else {
        return Ok(Some(render()));
    };
    let Some(map) = attr(atlas, root, b"m_RenderDataMap", meter)? else {
        return Ok(Some(render()));
    };
    let items: &[Value] = match map {
        Value::List(items) => items,
        Value::Tuple(items) => items,
        Value::Str(s) if s.as_bytes().is_empty() => &[],
        Value::Bytes(b) if b.is_empty() => &[],
        Value::Prims(p) if p.is_empty() => &[],
        // A string, bytes or numbers: items that do not unpack; any other
        // value is no sequence.
        _ => return Ok(Some(render())),
    };
    for item in items {
        work(meter, 1)?;
        let item = match unpack(item) {
            None => return Ok(Some(render())),
            Some(Unpacked::Opaque) => Item::Other,
            Some(Unpacked::Pair(k, v)) => Item::Entry(key(atlas, k, meter)?, v),
        };
        if !visit(item)? {
            break;
        }
    }
    Ok(None)
}

/// The matched entry's value: a constructed `SpriteAtlasData`
/// (`export/SpriteHelper.py:79`'s `assert`, else `render`) of E17's shapes
/// (else `read`).
pub(super) fn atlas_data(atlas: &Parsed, v: &Value, meter: &Meter) -> Result<RenderData, Class> {
    let Value::Dict(m) = v else {
        return Err(render());
    };
    if !constructs(atlas, m, &ATLAS_DATA, meter)? {
        return Err(render());
    }
    render_data(atlas, m, true, meter)
}

/// `m`'s render data ([`RenderData`]), of a constructed class or not
/// (`constructed`, which decides a missing `alphaTexture`); a texture
/// missing, or a rectangle or `settingsRaw` of another shape, is `read`.
fn render_data(
    parsed: &Parsed,
    m: &Map,
    constructed: bool,
    meter: &Meter,
) -> Result<RenderData, Class> {
    let texture = match attr(parsed, m, b"texture", meter)? {
        Some(t) => pointer(parsed, t, meter)?,
        None => return Err(read::shape()),
    };
    let alpha = match attr(parsed, m, b"alphaTexture", meter)? {
        Some(t) => Alpha::Set(pointer(parsed, t, meter)?),
        None if constructed => Alpha::None,
        None => Alpha::Missing,
    };
    let Some(Value::Dict(rect)) = attr(parsed, m, b"textureRect", meter)? else {
        return Err(read::shape());
    };
    let mut texture_rect = [0.0; 4];
    for (out, name) in texture_rect
        .iter_mut()
        .zip([&b"x"[..], b"y", b"width", b"height"])
    {
        *out = attr(parsed, rect, name, meter)?
            .and_then(Value::as_f64)
            .ok_or_else(read::shape)?;
    }
    let settings_raw = attr(parsed, m, b"settingsRaw", meter)?
        .and_then(Value::as_u64)
        .ok_or_else(read::shape)?;
    Ok(RenderData {
        texture,
        alpha,
        texture_rect,
        settings_raw,
    })
}

#[cfg(test)]
mod tests;
