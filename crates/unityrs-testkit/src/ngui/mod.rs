// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/TypeTreeHelper.py, UnityPy/classes/generated.py. See THIRD_PARTY_LICENSES.
//! NGUI-shaped atlases for the cutter's reference (`tools/parity/ngui_cutter.py`,
//! `docs/BATCH_SPEC.md` section 5): the atlas MonoBehaviour with a typetree,
//! the MonoScript its `m_Script` names, its `Material` and the `Texture2D`
//! sheets, inline or streamed from a `.resS`, in bundles whose views join
//! them.
//!
//! Every object is a [`V`], a typed value that gives both its typetree nodes
//! and its little-endian bytes, in the layout UnityPy 1.25.2 reads them by
//! (`helpers/TypeTreeHelper.py:182-263`): a value whose node is aligned
//! (a `bool`, a string, `TypelessData`, every vector here) is followed by
//! zeros up to a multiple of 4, a `pair` is its `first` then its `second`,
//! and a node whose first child is `Array` is a vector, its count an `int`.
//! A Material's `m_TexEnvs` holds pairs whose `first` is a string or a
//! `FastPropertyName` struct with a `name`, as UnityPy types it either way
//! (`classes/generated.py:11477`).
//!
//! A typetree row's level is a `u8`, so [`nodes`], [`write`] and [`file`]
//! refuse a value deeper than it holds, and a length or count past an
//! `int`, with a [`BuildError`], before any level is added or any size
//! narrowed. Each file gives each object a type record of its own, so a
//! file mixes shapes freely ([`file`]). [`scenes`] lists the scenes
//! `tools/parity/ngui_scenes.py` asks cargo for.
//!
//! Owner: P1b, the NGUI cutter's reference (`docs/plans/extract.md` section 3,
//! item 3).

mod objects;
mod scenes;
mod sprite_atlases;
mod sprite_objects;
mod sprites;

pub use objects::{
    PropName, atlas, format, material, material_tpk, mono_behaviour, mono_script, set_field,
    sprite, texture, texture_streamed,
};
pub use scenes::{AtlasScene, CutOptions, SceneBundle, failure_scenes, scenes};
pub use sprite_atlases::{AtlasEntry, sprite_atlas};
pub use sprites::{Mesh, SpriteSpec, packed, sprite_scenes};

use crate::blob::NodeSpec;
use crate::serialized::{ExternalSpec, ObjectSpec, SerializedSpec, TypeSpec};
use crate::writer::ByteWriter;

/// A typed value: its typetree nodes and its bytes ([`nodes`], [`write`]).
#[derive(Clone, Debug, PartialEq)]
pub enum V {
    /// `int`.
    Int(i32),
    /// `unsigned int`.
    UInt(u32),
    /// `UInt8`, unaligned.
    UInt8(u8),
    /// `UInt64`.
    UInt64(u64),
    /// `SInt64`.
    Int64(i64),
    /// `bool`, aligned.
    Bool(bool),
    /// `float`: UnityPy widens it exactly to a double.
    Float(f32),
    /// `double`.
    Double(f64),
    /// `string`: an `int` length, the bytes, aligned.
    Str(Vec<u8>),
    /// `TypelessData`: an `int` length and the bytes, aligned; UnityPy reads
    /// it as `bytes`.
    Bytes(Vec<u8>),
    /// `PPtr<target>`: `m_FileID`, `m_PathID`.
    PPtr(String, i32, i64),
    /// A struct: its type and its fields, in order.
    Struct(String, Vec<(String, V)>),
    /// A vector: its node type (`vector`, `map`), the element's shape (for
    /// the nodes, so an empty vector has them too) and the elements.
    Vector(String, Box<V>, Vec<V>),
    /// `pair`: `first`, `second`.
    Pair(Box<V>, Box<V>),
}

/// Why a value has no typetree or no bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildError {
    /// A row would sit at `level`, past the `u8` a typetree row holds.
    TooDeep {
        /// The deepest level the value needs.
        level: usize,
    },
    /// A string's, bytes' or vector's length, past the `int` it is written as.
    TooLong {
        /// The length.
        len: usize,
    },
    /// More objects than a type index (`int`) counts.
    TooManyObjects(usize),
}

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooDeep { level } => write!(f, "a typetree row at level {level}, past 255"),
            Self::TooLong { len } => write!(f, "a length of {len}, past an int"),
            Self::TooManyObjects(n) => write!(f, "{n} objects, past an int"),
        }
    }
}

impl std::error::Error for BuildError {}

/// A string value.
#[must_use]
pub fn s(text: &str) -> V {
    V::Str(text.as_bytes().to_vec())
}

/// A pointer to a `target`.
#[must_use]
pub fn pptr(target: &str, at: (i32, i64)) -> V {
    V::PPtr(target.to_owned(), at.0, at.1)
}

/// A struct of `ty`.
#[must_use]
pub fn st(ty: &str, fields: Vec<(&str, V)>) -> V {
    V::Struct(
        ty.to_owned(),
        fields.into_iter().map(|(n, v)| (n.to_owned(), v)).collect(),
    )
}

/// A `vector` of `items`, each shaped as `shape`.
#[must_use]
pub fn vector(shape: V, items: Vec<V>) -> V {
    V::Vector("vector".to_owned(), Box::new(shape), items)
}

/// A `pair`.
#[must_use]
pub fn pair(first: V, second: V) -> V {
    V::Pair(Box::new(first), Box::new(second))
}

/// How many levels below its own row `v`'s rows reach, counted without
/// recursion.
#[must_use]
pub fn depth(v: &V) -> usize {
    let mut deepest = 0;
    let mut stack = vec![(v, 0usize)];
    while let Some((v, at)) = stack.pop() {
        let below = match v {
            V::Str(_) | V::Vector(..) => 2,
            V::Bytes(_) | V::PPtr(..) => 1,
            _ => 0,
        };
        deepest = deepest.max(at + below);
        match v {
            V::Struct(_, fields) => stack.extend(fields.iter().map(|(_, f)| (f, at + 1))),
            V::Vector(_, shape, _) => stack.push((shape, at + 2)),
            V::Pair(first, second) => stack.extend([(&**first, at + 1), (&**second, at + 1)]),
            _ => {}
        }
    }
    deepest
}

/// The typetree rows of the field `name` holding `v`, its own row at
/// `level`; [`BuildError::TooDeep`] when a row would pass level 255.
pub fn nodes(level: u8, name: &str, v: &V) -> Result<Vec<NodeSpec>, BuildError> {
    let deepest = usize::from(level) + depth(v);
    if deepest > usize::from(u8::MAX) {
        return Err(BuildError::TooDeep { level: deepest });
    }
    let mut out = Vec::new();
    rows(level, name, v, &mut out);
    Ok(out)
}

/// [`nodes`]' rows, once the depth is checked: no level below passes 255.
fn rows(level: u8, name: &str, v: &V, out: &mut Vec<NodeSpec>) {
    let leaf = |ty: &str, size: i32| NodeSpec::new(level, ty, name).size(size);
    match v {
        V::Int(_) => out.push(leaf("int", 4)),
        V::UInt(_) => out.push(leaf("unsigned int", 4)),
        V::UInt8(_) => out.push(leaf("UInt8", 1)),
        V::UInt64(_) => out.push(leaf("UInt64", 8)),
        V::Int64(_) => out.push(leaf("SInt64", 8)),
        V::Float(_) => out.push(leaf("float", 4)),
        V::Double(_) => out.push(leaf("double", 8)),
        V::Bool(_) => out.push(leaf("bool", 1).aligned()),
        V::Str(_) => out.extend([
            leaf("string", -1),
            NodeSpec::new(level + 1, "Array", "Array")
                .size(-1)
                .aligned(),
            NodeSpec::new(level + 2, "int", "size").size(4),
            NodeSpec::new(level + 2, "char", "data").size(1),
        ]),
        V::Bytes(_) => out.extend([
            leaf("TypelessData", -1).aligned(),
            NodeSpec::new(level + 1, "int", "size").size(4),
            NodeSpec::new(level + 1, "UInt8", "data").size(1),
        ]),
        V::PPtr(target, ..) => out.extend([
            leaf(&format!("PPtr<{target}>"), 12),
            NodeSpec::new(level + 1, "int", "m_FileID").size(4),
            NodeSpec::new(level + 1, "SInt64", "m_PathID").size(8),
        ]),
        V::Struct(ty, fields) => {
            out.push(leaf(ty, -1));
            for (n, f) in fields {
                rows(level + 1, n, f, out);
            }
        }
        V::Vector(ty, shape, _) => {
            out.extend([
                leaf(ty, -1),
                NodeSpec::new(level + 1, "Array", "Array")
                    .size(-1)
                    .aligned(),
                NodeSpec::new(level + 2, "int", "size").size(4),
            ]);
            rows(level + 2, "data", shape, out);
        }
        V::Pair(first, second) => {
            out.push(leaf("pair", -1));
            rows(level + 1, "first", first, out);
            rows(level + 1, "second", second, out);
        }
    }
}

fn len_i32(len: usize) -> Result<i32, BuildError> {
    i32::try_from(len).map_err(|_| BuildError::TooLong { len })
}

/// `v`'s bytes, in the order and alignment [`nodes`] describes; refuses a
/// value [`nodes`] refuses at level 0, and a length past an `int`.
pub fn write(w: &mut ByteWriter, v: &V) -> Result<(), BuildError> {
    let deepest = depth(v);
    if deepest > usize::from(u8::MAX) {
        return Err(BuildError::TooDeep { level: deepest });
    }
    bytes(w, v)
}

fn bytes(w: &mut ByteWriter, v: &V) -> Result<(), BuildError> {
    match v {
        V::Int(x) => {
            w.i32(*x);
        }
        V::UInt(x) => {
            w.u32(*x);
        }
        V::UInt8(x) => {
            w.u8(*x);
        }
        V::UInt64(x) => {
            w.u64(*x);
        }
        V::Int64(x) => {
            w.i64(*x);
        }
        V::Float(x) => {
            w.f32(*x);
        }
        V::Double(x) => {
            w.f64(*x);
        }
        V::Bool(x) => {
            w.bool(*x).align(4);
        }
        V::Str(b) | V::Bytes(b) => {
            w.i32(len_i32(b.len())?).bytes(b).align(4);
        }
        V::PPtr(_, file, path) => {
            w.i32(*file).i64(*path);
        }
        V::Struct(_, fields) => {
            for (_, f) in fields {
                bytes(w, f)?;
            }
        }
        V::Vector(_, _, items) => {
            w.i32(len_i32(items.len())?);
            for item in items {
                bytes(w, item)?;
            }
            w.align(4);
        }
        V::Pair(first, second) => {
            bytes(w, first)?;
            bytes(w, second)?;
        }
    }
    Ok(())
}

/// One object: its path id, class id and value (a [`V::Struct`] whose type
/// is the class's name, written as the root node `Base`), its bytes cut to
/// `cut` when that is set (a read that runs out), and `trailing` zero bytes
/// stored after them (a read that ends before the object does, which
/// UnityPy's `check_read` refuses).
#[derive(Clone, Debug, PartialEq)]
pub struct Object {
    /// `path_id`.
    pub path_id: i64,
    /// The class id.
    pub class_id: i32,
    /// The value.
    pub value: V,
    /// The object's stored bytes are the value's first `cut`.
    pub cut: Option<usize>,
    /// Zero bytes stored after the value's (or the cut's).
    pub trailing: usize,
}

impl Object {
    /// An object of `class_id` holding `value`.
    #[must_use]
    pub fn new(path_id: i64, class_id: i32, value: V) -> Self {
        Self {
            path_id,
            class_id,
            value,
            cut: None,
            trailing: 0,
        }
    }

    /// The same object, its stored bytes cut to their first `n`.
    #[must_use]
    pub fn cut_to(mut self, n: usize) -> Self {
        self.cut = Some(n);
        self
    }

    /// The same object with `n` zero bytes stored after its value.
    #[must_use]
    pub fn with_trailing(mut self, n: usize) -> Self {
        self.trailing = n;
        self
    }
}

/// A `SerializedFile` (format 22, little-endian, engine 2022.3.0f1) of
/// `objects`, each with a type record of its own, with the given externals
/// (`archive:/CAB-x/CAB-x`). With `type_trees` false it stores no typetree,
/// so UnityPy reads its objects by the TPK's.
pub fn file(
    externals: &[&str],
    objects: &[Object],
    type_trees: bool,
) -> Result<Vec<u8>, BuildError> {
    file_on(None, externals, objects, type_trees)
}

/// [`file`], its target platform `platform` (a `BuildTarget` value) when
/// that is set, else the default's.
pub fn file_on(
    platform: Option<i32>,
    externals: &[&str],
    objects: &[Object],
    type_trees: bool,
) -> Result<Vec<u8>, BuildError> {
    let base = SerializedSpec::default();
    let mut types = Vec::new();
    let mut objs = Vec::new();
    for (i, o) in objects.iter().enumerate() {
        let type_id = i32::try_from(i).map_err(|_| BuildError::TooManyObjects(objects.len()))?;
        // Format 22 keeps the class id in the type record (an `int`); an
        // object row holds one only below format 16.
        let class_id = u16::try_from(o.class_id).unwrap_or(0);
        types.push(TypeSpec {
            class_id: o.class_id,
            nodes: nodes(0, "Base", &o.value)?,
            ..TypeSpec::default()
        });
        let mut w = ByteWriter::le();
        write(&mut w, &o.value)?;
        let mut data = w.into_inner();
        if let Some(n) = o.cut {
            data.truncate(n);
        }
        data.resize(data.len() + o.trailing, 0);
        objs.push(ObjectSpec {
            path_id: o.path_id,
            type_id,
            class_id,
            data,
            ..ObjectSpec::default()
        });
    }
    Ok(crate::serialized::build(&SerializedSpec {
        platform: platform.unwrap_or(base.platform),
        type_trees,
        types,
        objects: objs,
        externals: externals
            .iter()
            .map(|p| ExternalSpec {
                path: p.as_bytes().to_vec(),
                ..ExternalSpec::default()
            })
            .collect(),
        ..SerializedSpec::default()
    }))
}

/// Pixel `(x, y)` of a sheet, `y` counted from the image's top (the row
/// order `Texture2D.image` gives, after UnityPy's flip):
/// `[base + x + 16 y, 0x40 + x, 0x80 + y, 0xFF - x - y]`, each byte
/// wrapping, the last left out for RGB24. Every channel of a sheet up to
/// 15 x 15 is distinct from the zeros a crop pads with.
#[must_use]
pub fn pixel(base: u8, x: u8, y: u8) -> [u8; 4] {
    [
        base.wrapping_add(x).wrapping_add(y.wrapping_mul(16)),
        0x40u8.wrapping_add(x),
        0x80u8.wrapping_add(y),
        0xFFu8.wrapping_sub(x).wrapping_sub(y),
    ]
}

/// A sheet's `image_data` as Unity stores it, bottom row first, of
/// [`pixel`]s: 4 bytes a pixel, or 3 (`rgb`).
#[must_use]
pub fn sheet(base: u8, width: u8, height: u8, rgb: bool) -> Vec<u8> {
    let mut out = Vec::new();
    for row in 0..height {
        let y = height - 1 - row;
        for x in 0..width {
            let p = pixel(base, x, y);
            out.extend_from_slice(if rgb { &p[..3] } else { &p });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{BuildError, Object, V, depth, file, nodes, pixel, s, sheet, st, vector, write};
    use crate::writer::ByteWriter;

    #[test]
    fn a_struct_writes_its_fields_aligned_where_unity_aligns_them() {
        let v = st(
            "T",
            vec![("b", V::Bool(true)), ("n", s("abcde")), ("i", V::Int(-2))],
        );
        let mut w = ByteWriter::le();
        write(&mut w, &v).unwrap();
        let mut want = vec![1, 0, 0, 0, 5, 0, 0, 0];
        want.extend_from_slice(b"abcde\0\0\0");
        want.extend_from_slice(&(-2i32).to_le_bytes());
        assert_eq!(w.into_inner(), want);
        let n = nodes(0, "Base", &v).unwrap();
        let names: Vec<_> = n.iter().map(|x| (x.level, x.name.clone())).collect();
        assert_eq!(names[0], (0, b"Base".to_vec()));
        assert_eq!(names[1], (1, b"b".to_vec()));
        assert_eq!(names[2], (1, b"n".to_vec()));
        assert_eq!(names[3], (2, b"Array".to_vec()));
    }

    #[test]
    fn an_empty_vector_still_has_its_element_nodes() {
        let v = vector(V::Int(0), Vec::new());
        assert_eq!(nodes(1, "v", &v).unwrap().len(), 4);
        let mut w = ByteWriter::le();
        write(&mut w, &v).unwrap();
        assert_eq!(w.into_inner(), [0, 0, 0, 0]);
    }

    /// A struct nested `n` deep, holding an `int`.
    fn nested(n: usize) -> V {
        (0..n).fold(V::Int(0), |inner, _| st("S", vec![("f", inner)]))
    }

    #[test]
    fn the_deepest_row_a_level_holds_is_255() {
        // A string's rows reach two below it: at 253 its last is 255.
        assert_eq!(
            nodes(253, "n", &s("x")).unwrap().last().map(|r| r.level),
            Some(255)
        );
        assert_eq!(
            nodes(254, "n", &s("x")),
            Err(BuildError::TooDeep { level: 256 })
        );
        assert_eq!(nodes(255, "n", &V::Int(0)).unwrap().len(), 1);
        assert_eq!(nodes(0, "Base", &nested(255)).unwrap().len(), 256);
        assert_eq!(
            nodes(0, "Base", &nested(256)),
            Err(BuildError::TooDeep { level: 256 })
        );
    }

    #[test]
    fn a_value_too_deep_for_a_typetree_is_refused_before_it_is_written() {
        let deep = nested(1_000);
        assert_eq!(depth(&deep), 1_000);
        let mut w = ByteWriter::le();
        assert_eq!(
            write(&mut w, &deep),
            Err(BuildError::TooDeep { level: 1_000 })
        );
        assert!(w.into_inner().is_empty(), "nothing is written first");
        let object = Object::new(1, 114, deep);
        assert_eq!(
            file(&[], &[object], true),
            Err(BuildError::TooDeep { level: 1_000 })
        );
        // A class id past a `u16` (`SpriteAtlas`'s) is the type record's.
        let object = Object::new(1, 687_078_895, V::Int(0));
        assert!(file(&[], &[object], true).is_ok());
    }

    #[test]
    fn a_cut_object_stores_its_first_bytes_and_trailing_ones_after() {
        let whole = file(
            &[],
            &[Object::new(1, 49, st("T", vec![("i", V::Int(7))]))],
            true,
        )
        .unwrap();
        let cut = file(
            &[],
            &[Object::new(1, 49, st("T", vec![("i", V::Int(7))])).cut_to(2)],
            true,
        )
        .unwrap();
        assert!(cut.len() < whole.len());
        let longer = file(
            &[],
            &[Object::new(1, 49, st("T", vec![("i", V::Int(7))])).with_trailing(16)],
            true,
        )
        .unwrap();
        assert_eq!(longer.len(), whole.len() + 16);
    }

    #[test]
    fn a_sheet_is_stored_bottom_row_first() {
        let data = sheet(1, 2, 2, false);
        assert_eq!(&data[..4], &pixel(1, 0, 1));
        assert_eq!(&data[12..], &pixel(1, 1, 0));
        assert_eq!(sheet(1, 2, 2, true).len(), 12);
    }
}
