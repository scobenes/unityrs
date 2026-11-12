//! The cutter's reads (`docs/BATCH_SPEC.md` 4.4 steps 1 and 2, section 5):
//! a MonoBehaviour's head, a peeked name, a texture's fields, an atlas's
//! material name, each as the shape the cutter reads or a [`Class`].
//!
//! **The head** (the second carried question). UnityPy reads a
//! MonoBehaviour's head with its TPK's MonoBehaviour tree for the file's
//! version, `check_read` off, whatever typetree the object stores
//! (`parse_monobehaviour_head`). Version 1 configures no typetree provider
//! (`docs/plans/batch.md` A8), so the cutter reads it with fixed rows of
//! its own instead, nothing in them taken from a TPK (`docs/plans/core.md`
//! Q2). UnityPy 1.25.2's MIT class definitions name the four fields
//! (`classes/generated.py`: `MonoBehaviour`'s `m_GameObject`, a
//! `PPtr[GameObject]` it has from `Component`, `m_Enabled`, `m_Script`, a
//! `PPtr[MonoScript]`, and `m_Name`, a `str`), but type `m_Enabled` only as
//! an `int`. Its width and alignment, the rows' type names and their order
//! are format facts from Perfare/AssetStudio's release v0.16.47 (MIT; no
//! AssetStudio code is ported or kept here): `Component` reads
//! `m_GameObject` (`AssetStudio/Classes/Component.cs`; `EditorExtension`
//! before it reads only in an editor file), `Behaviour` one byte,
//! `m_Enabled`, then aligns (`Classes/Behaviour.cs`), and `MonoBehaviour`
//! `m_Script`, then the aligned string `m_Name` (`Classes/MonoBehaviour.cs`);
//! `AssetStudioUtility/SerializedTypeHelper.cs`'s `AddMonoBehaviour` names
//! the rows `PPtr<GameObject> m_GameObject`, `UInt8 m_Enabled` (aligned),
//! `PPtr<MonoScript> m_Script`, `string m_Name` (its `Array` aligned). Each
//! pointer is an `int m_FileID` and an `m_PathID` that is an `SInt64` from
//! SerializedFile format 14 on and an `int` before it, as UnityPy reads an
//! identifier in a file (`files/SerializedFile.py`'s
//! `LocalSerializedObjectIdentifier`) and AssetStudio a pointer
//! (`Classes/PPtr.cs`). Of each row's meta flags only the align bit is set,
//! the one the reader acts on ([`head_rows`]). Two tests pin the rows: one
//! reads them back from a file whose stored typetree is written from those
//! facts, at formats 9 and 22, and the reference's reads of the testkit's
//! atlases (format 22 files), which UnityPy makes with its TPK's tree at
//! run time, are recorded (`tools/fixtures/ngui.py`) and replayed against
//! these rows (X5); no scene has a file before format 14. No UnityPy sample
//! holds a MonoBehaviour to measure them against.
//! A head read that fails is the `m_Script` read's failure.
//! UnityPy chooses the head's tree by the file's Unity version through its
//! bundled typetree package, so the two can differ for a file whose version
//! string disagrees with its format: a divergence (`PARITY.md` 19.3,
//! `docs/BATCH_SPEC.md` 4.4 step 1), which no scene records.
//!
//! **Fields.** UnityPy reads only the `Texture2D` as an object, whose
//! attributes are its children's cleaned names ([`attr`], the core's
//! [`Parsed::attr`]); the atlas, its elements, the Material, the MonoScript,
//! a pointer and a peeked name are typetree dicts, keyed by the children's
//! raw names, a repeated one keeping its last value ([`field`], the core's
//! [`Parsed::field`]; review round 2's C2). Integers are kept wide
//! ([`integer`]).

use std::sync::Arc;

use unityrs_core::env::{DerefError, PPtr};
use unityrs_core::files::{BundleFile, ParentKind, SerializedFile};
use unityrs_core::limits::{Limits, Meter, MeteredVec};
use unityrs_core::typetree::{NodeList, NodeRow, Tree};
use unityrs_core::{Bytes, Map, ObjectRef, Parsed, PrimArray, RawStr, Value};
use unityrs_texture::{Format, SWITCH, Selection, Skip, TextureFields, select};

use super::classify::{self, Class, Reason};
use super::seam::Seam;
use super::table;

/// The format from which a pointer's `m_PathID` is 64 bits.
const WIDE_PATH_ID: u32 = 14;

/// A value of another shape than the cutter reads: a `ParseError`'s `Read`
/// (4.4 step 2's rule).
#[must_use]
pub fn shape() -> Class {
    Class::Skip(Reason::decode_error("read"))
}

/// The bundle a SerializedFile or bundle's parent link names, while alive.
#[must_use]
pub fn parent_bundle(kind: Option<&ParentKind>) -> Option<Arc<BundleFile>> {
    match kind? {
        ParentKind::Bundle { bundle, .. } => bundle.upgrade(),
        ParentKind::Web => None,
    }
}

/// The top-level bundle that holds `file`, held while a read uses it
/// (`docs/plans/batch.md` 2.6 step 7).
#[must_use]
pub fn top_bundle(file: &SerializedFile) -> Option<Arc<BundleFile>> {
    let mut top = parent_bundle(file.parent().map(|p| &p.kind))?;
    while let Some(up) = parent_bundle(top.parent().map(|p| &p.kind)) {
        top = up;
    }
    Some(top)
}

/// A dict's field by its raw name, as `parse_as_dict` keys it: of children
/// of one name, the last one's value.
#[must_use]
pub fn field<'a>(parsed: &'a Parsed, value: &'a Value, name: &[u8]) -> Option<&'a Value> {
    parsed.field(value.as_dict()?, name)
}

/// An object's attribute, as UnityPy's object mode keys it: the last child
/// whose cleaned name is `name` ([`Parsed::attr`]). The `Texture2D`'s only.
#[must_use]
pub fn attr<'a>(parsed: &'a Parsed, value: &'a Value, name: &[u8]) -> Option<&'a Value> {
    parsed.attr(value.as_dict()?, name)
}

/// A pointer struct: an integer `m_FileID` and `m_PathID`.
#[must_use]
pub fn pptr(parsed: &Parsed, value: &Value) -> Option<PPtr> {
    let file_id = field(parsed, value, b"m_FileID")?.as_i64()?;
    let path_id = field(parsed, value, b"m_PathID")?.as_i64()?;
    Some(PPtr {
        file_id: i32::try_from(file_id).ok()?,
        path_id,
    })
}

fn string(value: &Value) -> Option<&RawStr> {
    match value {
        Value::Str(s) => Some(s),
        _ => None,
    }
}

/// A MonoBehaviour's head: `m_Script` and `m_Name`.
#[derive(Clone, Debug)]
pub struct Head {
    /// `m_Script`.
    pub script: PPtr,
    /// `m_Name`.
    pub name: RawStr,
}

/// `m_MetaFlag`'s align bit, the one bit of a row the core's reader acts on
/// (UnityPy's `m_MetaFlag & 0x4000`, the core's [`NodeRow::aligned`]).
const ALIGN: i64 = 0x4000;

/// The head's rows (the module docs' **The head**): each a level, a type
/// and a name, its `m_MetaFlag` the align bit where the value is aligned
/// (`m_Enabled`, and a string's `Array`) and 0 elsewhere, its `m_ByteSize`
/// left 0 as UnityPy's `from_list` leaves it (the reader reads neither the
/// other bits nor the size).
fn head_rows(wide: bool) -> Vec<NodeRow> {
    let id_ty = if wide { "SInt64" } else { "int" };
    let row = |level, ty: &str, name: &str| NodeRow::new(level, ty, name).with_meta_flag(0);
    let aligned = |level, ty: &str, name: &str| NodeRow::new(level, ty, name).with_meta_flag(ALIGN);
    vec![
        row(0, "MonoBehaviour", "Base"),
        row(1, "PPtr<GameObject>", "m_GameObject"),
        row(2, "int", "m_FileID"),
        row(2, id_ty, "m_PathID"),
        aligned(1, "UInt8", "m_Enabled"),
        row(1, "PPtr<MonoScript>", "m_Script"),
        row(2, "int", "m_FileID"),
        row(2, id_ty, "m_PathID"),
        row(1, "string", "m_Name"),
        aligned(2, "Array", "Array"),
        row(3, "int", "size"),
        row(3, "char", "data"),
    ]
}

/// A peek tree kept for one type of X's own files: the type's full tree's
/// address (the key), that tree (kept alive, so the address is not reused
/// while the entry lives) and its peek tree, or none.
type Peek = (usize, Arc<Tree>, Option<Arc<Tree>>);

/// The cutter's reader: the head trees, and the peek tree of each type of
/// X's own files, built once, in a table sorted by the type's tree, charged
/// to `M_X`. A dependency's tree is never kept: its peek tree is built,
/// read with and dropped in the read (`docs/plans/batch.md` 2.6 step 7;
/// review round 2's S3).
#[derive(Debug)]
pub struct Reader {
    wide: Arc<Tree>,
    narrow: Arc<Tree>,
    peeks: MeteredVec<Peek>,
}

impl Reader {
    /// The reader, its head trees compiled. They are fixed rows, so their
    /// failing is the cutter's own fault, `internal`, whatever the core says.
    pub fn new(meter: &Arc<Meter>) -> Result<Self, Class> {
        fn internal<E>(_: E) -> Class {
            Class::Run(Reason::plain("internal"))
        }
        let tree = |wide| {
            let list = NodeList::trusted(head_rows(wide)).map_err(internal)?;
            Tree::compile(Arc::new(list), &Limits::default()).map_err(internal)
        };
        Ok(Self {
            wide: tree(true)?,
            narrow: tree(false)?,
            peeks: MeteredVec::new(meter),
        })
    }

    /// `obj`'s head, read with the fixed rows ([`head_rows`]), `check_read` off.
    pub fn head(&self, obj: &ObjectRef) -> Result<Head, Class> {
        let tree = if obj.file().header().version >= WIDE_PATH_ID {
            &self.wide
        } else {
            &self.narrow
        };
        let parsed = obj
            .read_typetree(Some(tree), false)
            .map_err(|e| classify::parse(&e))?;
        let root = parsed.value();
        let script = field(&parsed, root, b"m_Script")
            .and_then(|v| pptr(&parsed, v))
            .ok_or_else(shape)?;
        let name = field(&parsed, root, b"m_Name")
            .and_then(string)
            .ok_or_else(shape)?
            .clone();
        Ok(Head { script, name })
    }

    /// `peek_name` (4.4 step 1): the object's root's children up to and
    /// including the first named `m_Name` or `name`, read `check_read` off
    /// over the object's own nodes, and that child's value under its raw
    /// name (`parse_as_dict(peek_node)[key]`; review round 2's C3); a type
    /// with no typetree fails as its object read does (`asset_bundle`), one
    /// with no name field, or a name that is no string, as a `read`. Each
    /// core call is under the refusal probe on its own (review round 2's
    /// S4).
    pub(super) fn peek_name(&mut self, seam: &Seam<'_>, obj: &ObjectRef) -> Result<RawStr, Class> {
        let found = seam.probe(|| obj.tree(None).map_err(|e| classify::parse(&e)))?;
        let Some(tree) = found else {
            return Err(Class::Skip(Reason::decode_error("asset_bundle")));
        };
        let own = top_bundle(obj.file()).is_some_and(|b| Arc::ptr_eq(&b, &seam.registered.own));
        let peek = if own {
            self.kept(seam, &tree.tree, obj.file())?
        } else {
            peek_tree(seam, &tree.tree, obj.file())?
        };
        drop(tree);
        let Some(peek) = peek else {
            return Err(shape());
        };
        let parsed = seam.probe(|| {
            obj.read_typetree(Some(&peek), false)
                .map_err(|e| classify::parse(&e))
        })?;
        // The peek tree ends with the selected child: the root's last.
        let list = peek.list();
        let key = list
            .children(list.root())
            .last()
            .map(|c| list.name(*c))
            .ok_or_else(shape)?;
        field(&parsed, parsed.value(), key)
            .and_then(string)
            .cloned()
            .ok_or_else(shape)
    }

    /// The peek tree of one of X's own types, built once and kept.
    fn kept(
        &mut self,
        seam: &Seam<'_>,
        tree: &Arc<Tree>,
        file: &SerializedFile,
    ) -> Result<Option<Arc<Tree>>, Class> {
        let at = Arc::as_ptr(tree) as usize;
        let i = match self.peeks.as_slice().binary_search_by_key(&at, |p| p.0) {
            Ok(i) => i,
            Err(i) => {
                let peek = peek_tree(seam, tree, file)?;
                let peeks = &mut self.peeks;
                seam.probe(|| table::push(peeks, (at, tree.clone(), peek)))?;
                // Into its place: a rotation, no allocation.
                if let Some(tail) = self.peeks.as_mut_slice().get_mut(i..) {
                    tail.rotate_right(1);
                }
                i
            }
        };
        Ok(self.peeks.as_slice().get(i).and_then(|p| p.2.clone()))
    }
}

/// The peek tree of `tree` (UnityPy's `get_name_peek_node`), or none when
/// its root has no `m_Name` or `name` child: the rows up to the end of the
/// first such child's subtree, so that child is the peek root's last. Its
/// four core calls (the rows' charge and copy, the list, its compile) are
/// each under the probe on its own (review round 4's S3).
fn peek_tree(
    seam: &Seam<'_>,
    tree: &Arc<Tree>,
    file: &SerializedFile,
) -> Result<Option<Arc<Tree>>, Class> {
    let list = tree.list();
    let root = list.root();
    let children = list.children(root);
    let Some(pos) = children
        .iter()
        .position(|c| matches!(list.name(*c), b"m_Name" | b"name"))
    else {
        return Ok(None);
    };
    let end = children
        .get(pos + 1)
        .map_or(list.rows().len(), |next| next.index());
    let rows = list.rows().get(..end).unwrap_or_default();
    let mut kept = seam.probe(|| {
        MeteredVec::with_capacity(file.meter(), rows.len()).map_err(|e| classify::charge(&e))
    })?;
    seam.probe(|| {
        kept.extend_from_slice(rows)
            .map_err(|e| classify::charge(&e))
    })?;
    let peek = seam.probe(|| NodeList::from_rows(kept).map_err(|e| classify::tree(&e)))?;
    let limits = &file.context().limits;
    let compiled =
        seam.probe(|| Tree::compile(Arc::new(peek), limits).map_err(|e| classify::tree(&e)))?;
    Ok(Some(compiled))
}

/// `m_StreamData`: a resource's path and span, the integers as read (any
/// integer node, signed or not, of any width: 4.4 step 2). They matter only
/// when the stream is read (section 3's answer 4, [`Stream::span`]).
#[derive(Clone, Debug)]
pub struct Stream {
    /// `path`, as read.
    pub path: RawStr,
    /// `offset`.
    pub offset: i128,
    /// `size`.
    pub size: i128,
}

/// Where a stream's bytes are in the resource it names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Span {
    /// `offset..offset + size`, each within `u64`: the core's read, short
    /// at the resource's end as a Python slice is.
    Range(u64, u64),
    /// Any other: the memoryview reader's `read(size)` at `offset`
    /// (UnityPy's `EndianBinaryReader` over a memoryview, the core's
    /// `stream::py_read`), Python's slice `data[start:stop]` of the whole
    /// resource ([`python_slice`]); a size of -1 reads to the end, `stop`
    /// [`i128::MAX`].
    Slice(i128, i128),
}

impl Stream {
    /// The span `read(size)` at `offset` gives (section 3's answer 4; review
    /// round 2's C1): 0 is empty, -1 runs to the end, else the slice.
    #[must_use]
    pub fn span(&self) -> Span {
        if self.size == 0 {
            return Span::Range(0, 0);
        }
        if self.size == -1 {
            return Span::Slice(self.offset, i128::MAX);
        }
        let end = self.offset.checked_add(self.size);
        match (
            u64::try_from(self.offset),
            u64::try_from(self.size),
            end.and_then(|e| u64::try_from(e).ok()),
        ) {
            (Ok(o), Ok(n), Some(_)) => Span::Range(o, n),
            _ => Span::Slice(self.offset, end.unwrap_or(i128::MAX)),
        }
    }
}

/// Python's `data[start:stop]` over `len` bytes, as a range: a negative
/// index counts from the end, and each is clamped to `0..=len`; a stop
/// before the start is empty.
#[must_use]
pub fn python_slice(len: usize, start: i128, stop: i128) -> std::ops::Range<usize> {
    let n = i128::try_from(len).unwrap_or(i128::MAX);
    let at = |i: i128| {
        let i = if i < 0 { i.saturating_add(n) } else { i };
        usize::try_from(i.clamp(0, n)).unwrap_or(len)
    };
    let (a, b) = (at(start), at(stop));
    a..b.max(a)
}

/// `m_PlatformBlob`, any list (4.4 step 2): what `is_switch_swizzled` reads
/// of it, its length and its elements 8 to 12 as bytes, and no copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Blob {
    /// The element count (`len(platform_blob)`).
    pub len: usize,
    /// Elements 8 to 12 as the bytes `int.from_bytes` takes them for, when
    /// the blob has 12: each an integer in `0..=255` (or a bool, which
    /// Python's `bytes()` reads as 0 or 1); `None` when one is not
    /// ([`BLOB_ELEMENT`]).
    pub gobs: Option<[u8; 4]>,
}

/// Section 3's rule for a Switch blob element among bytes 8 to 12 that is
/// not a byte: UnityPy's `int.from_bytes(platform_blob[8:12], "little")`
/// raises, so the texture's answer is `decode_error`, detail `read`.
pub const BLOB_ELEMENT: &str = "read";

/// Element `i` of a primitive run as a byte, when it is one (a float is
/// not: `bytes()` refuses it).
fn prim_byte(p: &PrimArray, i: usize) -> Option<u8> {
    let int = |v: Option<i128>| v.and_then(|v| u8::try_from(v).ok());
    match p {
        PrimArray::Bool(v) => v.get(i).map(|b| u8::from(*b)),
        PrimArray::U8(v) => v.get(i).copied(),
        PrimArray::I8(v) => int(v.get(i).map(|x| i128::from(*x))),
        PrimArray::U16(v) => int(v.get(i).map(|x| i128::from(*x))),
        PrimArray::I16(v) => int(v.get(i).map(|x| i128::from(*x))),
        PrimArray::U32(v) => int(v.get(i).map(|x| i128::from(*x))),
        PrimArray::I32(v) => int(v.get(i).map(|x| i128::from(*x))),
        PrimArray::U64(v) => int(v.get(i).map(|x| i128::from(*x))),
        PrimArray::I64(v) => int(v.get(i).map(|x| i128::from(*x))),
        PrimArray::F32(_) | PrimArray::F64(_) => None,
    }
}

impl Blob {
    fn of(value: &Value) -> Option<Self> {
        let byte = |v: &Value| match v {
            Value::Bool(b) => Some(u8::from(*b)),
            other => u8::try_from(other.as_i64()?).ok(),
        };
        let (len, gobs) = match value {
            Value::Prims(p) => {
                let gobs = (8..12)
                    .map(|i| prim_byte(p, i))
                    .collect::<Option<Vec<u8>>>();
                (p.len(), gobs)
            }
            Value::List(items) => {
                let gobs = items
                    .get(8..12)
                    .map(|four| four.iter().map(byte).collect::<Option<Vec<u8>>>())
                    .unwrap_or_default();
                (items.len(), gobs)
            }
            _ => return None,
        };
        let gobs = gobs.and_then(|g| <[u8; 4]>::try_from(g).ok());
        Some(Self { len, gobs })
    }
}

/// A `Texture2D`'s fields, as the cutter reads them: the integers wide, as
/// read, narrowed only where section 3's answers use them ([`answer`]).
#[derive(Clone, Debug)]
pub struct Texture {
    /// `m_Width`.
    pub width: i128,
    /// `m_Height`.
    pub height: i128,
    /// `m_TextureFormat`.
    pub format: i128,
    /// `image_data`.
    pub image_data: Bytes,
    /// `m_StreamData`, when present.
    pub stream: Option<Stream>,
    /// `m_PlatformBlob`, when present.
    pub platform_blob: Option<Blob>,
}

impl Texture {
    /// The sides as the decoder takes them: a side outside `i32` is clamped
    /// to it, which keeps section 3's order (a negative side is answer 6,
    /// a side past `i32::MAX` more pixels than answer 7's bound).
    #[must_use]
    pub fn sides(&self) -> (i32, i32) {
        let narrow = |v: i128| {
            i32::try_from(v.clamp(i128::from(i32::MIN), i128::from(i32::MAX))).unwrap_or(i32::MAX)
        };
        (narrow(self.width), narrow(self.height))
    }
}

/// Section 3's answers 1 to 3 for `t` on `platform`: its format, or the
/// skip. A format outside `i32` is `unsupported_format(<decimal>)`; a Switch
/// blob is read as bytes ([`Blob`], [`BLOB_ELEMENT`]).
pub fn answer(t: &Texture, platform: i32) -> Result<Format, Class> {
    if t.width == 0 || t.height == 0 {
        return Err(Class::Skip(Reason::plain(Skip::EmptyImage.to_string())));
    }
    let Ok(format) = i32::try_from(t.format) else {
        return Err(Class::Skip(Reason::plain(format!(
            "unsupported_format({})",
            t.format
        ))));
    };
    let mut gobs = [0u8; 12];
    let blob = match t.platform_blob {
        Some(b) if platform == SWITCH && b.len >= 12 => match b.gobs {
            Some(g) => {
                gobs[8..].copy_from_slice(&g);
                Some(&gobs[..])
            }
            None if Format::from_texture_format(format).is_some() => {
                return Err(Class::Skip(Reason::decode_error(BLOB_ELEMENT)));
            }
            None => None,
        },
        _ => None,
    };
    let (width, height) = t.sides();
    match select(&TextureFields {
        texture_format: format,
        width,
        height,
        platform,
        platform_blob: blob,
    }) {
        Selection::Export(f) => Ok(f),
        Selection::Skip(s) => Err(Class::Skip(Reason::plain(s.to_string()))),
    }
}

/// The fields UnityPy 1.25.2's `Texture2D` needs (`classes/generated.py`).
const TEXTURE_FIELDS: [&[u8]; 11] = [
    b"image_data",
    b"m_CompleteImageSize",
    b"m_Height",
    b"m_ImageCount",
    b"m_IsReadable",
    b"m_LightmapFormat",
    b"m_Name",
    b"m_TextureDimension",
    b"m_TextureFormat",
    b"m_TextureSettings",
    b"m_Width",
];

/// Any integer, signed or not (not a bool, which UnityPy's dict keeps
/// apart), as read.
#[must_use]
pub fn integer(value: &Value) -> Option<i128> {
    match value {
        Value::Int(i) => Some(i128::from(*i)),
        Value::UInt(u) => Some(i128::from(*u)),
        _ => None,
    }
}

/// A texture's object read (4.4 step 2): the checked read, then the shape.
pub fn texture(obj: &ObjectRef) -> Result<Texture, Class> {
    let parsed = obj
        .read_typetree(None, true)
        .map_err(|e| classify::parse(&e))?;
    let root = parsed.value();
    let map: &Map = root.as_dict().ok_or_else(shape)?;
    if TEXTURE_FIELDS.iter().any(|f| parsed.attr(map, f).is_none()) {
        return Err(shape());
    }
    let get = |name: &[u8]| parsed.attr(map, name);
    let width = get(b"m_Width").and_then(integer).ok_or_else(shape)?;
    let height = get(b"m_Height").and_then(integer).ok_or_else(shape)?;
    let format = get(b"m_TextureFormat")
        .and_then(integer)
        .ok_or_else(shape)?;
    let stream = match get(b"m_StreamData") {
        None => None,
        Some(s) => {
            let path = attr(&parsed, s, b"path")
                .and_then(string)
                .ok_or_else(shape)?;
            let int = |name: &[u8]| attr(&parsed, s, name).and_then(integer);
            Some(Stream {
                path: path.clone(),
                offset: int(b"offset").ok_or_else(shape)?,
                size: int(b"size").ok_or_else(shape)?,
            })
        }
    };
    let image_data = match get(b"image_data") {
        Some(Value::Bytes(b)) => b.clone(),
        _ => return Err(shape()),
    };
    let platform_blob = match get(b"m_PlatformBlob") {
        None => None,
        Some(v) => Some(Blob::of(v).ok_or_else(shape)?),
    };
    Ok(Texture {
        width,
        height,
        format,
        image_data,
        stream,
        platform_blob,
    })
}

/// Dereferences `pptr` from `from`: an Environment call, checked as
/// 2.6 step 7 says ([`Seam::env_call`]).
pub(super) fn deref(
    seam: &Seam<'_>,
    from: &Arc<SerializedFile>,
    pptr: PPtr,
) -> Result<ObjectRef, Class> {
    seam.env_call(|| {
        seam.env()
            .deref(from, pptr)
            .map_err(|e: DerefError| classify::deref(&e))
    })
}

/// Section 5 step 1: the name of the texture the atlas's `material` names
/// as its `_MainTex`, or none. A failure is returned for the caller, which
/// swallows an object's skip.
pub(super) fn material_name(
    seam: &Seam<'_>,
    reader: &mut Reader,
    atlas: &ObjectRef,
    parsed: &Parsed,
) -> Result<Option<RawStr>, Class> {
    let Some(material) = field(parsed, parsed.value(), b"material") else {
        return Ok(None);
    };
    let at = pptr(parsed, material).ok_or_else(shape)?;
    let target = deref(seam, atlas.file(), at)?;
    let _top = top_bundle(target.file());
    let mat = seam.probe(|| {
        target
            .read_typetree(None, true)
            .map_err(|e| classify::parse(&e))
    })?;
    let envs = field(&mat, mat.value(), b"m_SavedProperties")
        .and_then(|p| field(&mat, p, b"m_TexEnvs"))
        .ok_or_else(shape)?;
    let Value::List(pairs) = envs else {
        return match envs {
            Value::Prims(p) if p.is_empty() => Ok(None),
            _ => Err(shape()),
        };
    };
    for pair in pairs {
        // A pair, or a two-element list (section 5 step 1).
        let two: &[Value] = match pair {
            Value::Tuple(t) => t,
            Value::List(l) => l,
            _ => return Err(shape()),
        };
        let [first, second] = two else {
            return Err(shape());
        };
        let key = match first {
            Value::Str(s) => Some(s),
            Value::Dict(_) => field(&mat, first, b"name").and_then(string),
            _ => None,
        };
        if key.map(RawStr::as_bytes) != Some(b"_MainTex".as_slice()) {
            continue;
        }
        let tex = field(&mat, second, b"m_Texture")
            .and_then(|t| pptr(&mat, t))
            .ok_or_else(shape)?;
        let texture = deref(seam, target.file(), tex)?;
        let _held = top_bundle(texture.file());
        return reader.peek_name(seam, &texture).map(Some);
    }
    Ok(None)
}

#[cfg(test)]
mod tests;
