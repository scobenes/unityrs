// Ported from UnityPy 1.25.2 (MIT): UnityPyBoost/TypeTreeHelper.cpp's object mode (clean_name, parse_class) and the attrs fields of UnityPy/classes/generated.py's Mesh, Sprite, VertexData, ChannelInfo, StreamInfo, SubMesh, StreamingInfo, CompressedMesh, PackedBitVector, BoneWeights4, BoneInfluence and SpriteRenderData, as MeshHandler reads them. See THIRD_PARTY_LICENSES.
//! The handler's input: a dict-mode value the core read, viewed as the
//! objects UnityPy's object mode builds from it.
//!
//! UnityPy hands `MeshHandler` an object (`obj.read()`), which UnityPyBoost
//! builds class by class (`parse_class`, `UnityPyBoost/TypeTreeHelper.cpp:458-583`;
//! `PARITY.md` section 9.2): each class node's fields are its children's
//! clean names (`clean_name`, `:28-65`), and the class named by the node's
//! type is called with them. A class whose required field is missing, or a
//! slotted class (every class here but `Mesh` and `Sprite`, `PARITY.md`
//! section 9.2.1) given a field it lacks, falls back to an `UnknownObject`
//! holding the fields as attributes. So a field the handler reads is the
//! value of the last child of that clean name, and a missing one is the
//! field's default (`None`) on a class that was built and an
//! `AttributeError` on an `UnknownObject` ([`Obj::get`]).
//!
//! The core's value is a dict keyed by each child's raw name (the first
//! child of a name holding its last value), so [`Obj`] finds the child by
//! clean name in the node list and its value under its raw name, never
//! copying.
//!
//! **What is refused** ([`MeshError::Malformed`], Tier U): a node whose
//! type is not the class its field's typetree names (object mode would look
//! up another class); a field whose value is not of a type some version's
//! typetree gives it (an `int` field holding a string, a class field
//! holding an `int`), or an integer outside the widest type any version
//! gives it (`ChannelInfo`'s `UInt8`s, `StreamInfo`'s `UInt32`s). UnityPy's
//! arithmetic on such values is uncharacterised, and the bounds keep every
//! loop the handler runs proportional to its input. A message names a node's
//! type by at most [`NAME_BYTES`] bytes of it ([`type_name`]), so a refusal
//! allocates no more for a long one.
//!
//! **Work.** Each scan of a node's children counts one unit of decode work
//! for every name the scan may compare, counted before it, on the meter
//! [`Obj`] carries: [`Obj::get`] the children and the dict's entries, and
//! the test of whether object mode builds a class ([`builds`]) each child
//! against each field it looks for. So reading a class of N fields is work
//! in N, wherever the handler reads it.

use unityrs_core::limits::Meter;
use unityrs_core::typetree::{NodeId, NodeList};
use unityrs_core::{Map, Parsed, PrimArray, RawStr, Value};

use crate::error::MeshError;

/// The most bytes of a node's type a [`MeshError::Malformed`] message holds
/// (module docs).
pub(crate) const NAME_BYTES: usize = 64;

/// `ty` for a message: its bytes as `escape_ascii` escapes them, at most
/// [`NAME_BYTES`] of output, cut between two escapes and then followed by
/// `…`.
pub(crate) fn type_name(ty: &[u8]) -> String {
    let mut out = String::new();
    for &b in ty {
        let escaped = b.escape_ascii();
        if out.len().saturating_add(escaped.len()) > NAME_BYTES {
            out.push('…');
            break;
        }
        out.extend(escaped.map(char::from));
    }
    out
}

/// Counts `n` units of work on `meter` (module docs).
fn count(meter: &Meter, n: usize) -> Result<(), MeshError> {
    meter.count_decode(u64::try_from(n).unwrap_or(u64::MAX))?;
    Ok(())
}

/// A class's attrs fields (`classes/generated.py`), and whether object
/// mode builds it as an `UnknownObject` when given a field it lacks
/// (`PARITY.md` section 9.2.1: a non-empty `__slots__`).
#[derive(Debug)]
pub(crate) struct Class {
    /// The node types that look this class up (a node's `m_Type`).
    pub(crate) types: &'static [&'static str],
    /// Fields without a default.
    pub(crate) required: &'static [&'static str],
    /// Fields whose default is `None`.
    pub(crate) optional: &'static [&'static str],
    /// A non-empty `__slots__`: an extra field gives an `UnknownObject`.
    pub(crate) slotted: bool,
}

/// `Mesh` (`classes/generated.py:4605-4642`), whose `__slots__` is `()`.
pub(crate) const MESH: Class = Class {
    types: &["Mesh"],
    required: &[
        "m_BindPose",
        "m_CompressedMesh",
        "m_IndexBuffer",
        "m_LocalAABB",
        "m_MeshCompression",
        "m_MeshUsageFlags",
        "m_Name",
        "m_SubMeshes",
    ],
    optional: &[
        "m_BakedConvexCollisionMesh",
        "m_BakedTriangleCollisionMesh",
        "m_BoneNameHashes",
        "m_BonesAABB",
        "m_CollisionTriangles",
        "m_CollisionVertexCount",
        "m_Colors",
        "m_CookingOptions",
        "m_IndexFormat",
        "m_IsReadable",
        "m_KeepIndices",
        "m_KeepVertices",
        "m_MeshLodInfo",
        "m_MeshMetrics_0_",
        "m_MeshMetrics_1_",
        "m_Normals",
        "m_RootBoneNameHash",
        "m_ShapeVertices",
        "m_Shapes",
        "m_Skin",
        "m_StreamCompression",
        "m_StreamData",
        "m_Tangents",
        "m_UV",
        "m_UV1",
        "m_Use16BitIndices",
        "m_VariableBoneCountWeights",
        "m_VertexData",
        "m_Vertices",
    ],
    slotted: false,
};

/// `Sprite` (`classes/generated.py:4867-4882`), whose `__slots__` is `()`.
pub(crate) const SPRITE: Class = Class {
    types: &["Sprite"],
    required: &[
        "m_Extrude",
        "m_Name",
        "m_Offset",
        "m_PixelsToUnits",
        "m_RD",
        "m_Rect",
    ],
    optional: &[
        "m_AtlasTags",
        "m_Bones",
        "m_Border",
        "m_IsPolygon",
        "m_PhysicsShape",
        "m_Pivot",
        "m_RenderDataKey",
        "m_ScriptableObjects",
        "m_SpriteAtlas",
    ],
    slotted: false,
};

/// `SpriteRenderData` (`classes/generated.py:10903-10919`).
pub(crate) const SPRITE_RENDER_DATA: Class = Class {
    types: &["SpriteRenderData"],
    required: &["settingsRaw", "texture", "textureRect", "textureRectOffset"],
    optional: &[
        "alphaTexture",
        "atlasRectOffset",
        "downscaleMultiplier",
        "indices",
        "m_Bindpose",
        "m_IndexBuffer",
        "m_SourceSkin",
        "m_SubMeshes",
        "m_VertexData",
        "secondaryTextures",
        "uvTransform",
        "vertices",
    ],
    slotted: true,
};

/// `VertexData` (`classes/generated.py:11800-11809`).
pub(crate) const VERTEX_DATA: Class = Class {
    types: &["VertexData"],
    required: &["m_DataSize", "m_VertexCount"],
    optional: &[
        "m_Channels",
        "m_CurrentChannels",
        "m_Streams",
        "m_Streams_0_",
        "m_Streams_1_",
        "m_Streams_2_",
        "m_Streams_3_",
    ],
    slotted: true,
};

/// `ChannelInfo` (`classes/generated.py:7213-7217`).
pub(crate) const CHANNEL_INFO: Class = Class {
    types: &["ChannelInfo"],
    required: &["dimension", "format", "offset", "stream"],
    optional: &[],
    slotted: true,
};

/// `StreamInfo` (`classes/generated.py:11043-11049`).
pub(crate) const STREAM_INFO: Class = Class {
    types: &["StreamInfo"],
    required: &["channelMask", "offset", "stride"],
    optional: &["align", "dividerOp", "frequency"],
    slotted: true,
};

/// `SubMesh` (`classes/generated.py:11098-11107`).
pub(crate) const SUB_MESH: Class = Class {
    types: &["SubMesh"],
    required: &[
        "firstByte",
        "firstVertex",
        "indexCount",
        "localAABB",
        "vertexCount",
    ],
    optional: &["baseVertex", "isTriStrip", "topology", "triangleCount"],
    slotted: true,
};

/// `StreamingInfo` (`classes/generated.py:11067-11070`).
pub(crate) const STREAMING_INFO: Class = Class {
    types: &["StreamingInfo"],
    required: &["offset", "path", "size"],
    optional: &[],
    slotted: true,
};

/// `CompressedMesh` (`classes/generated.py:7488-7501`).
pub(crate) const COMPRESSED_MESH: Class = Class {
    types: &["CompressedMesh"],
    required: &[
        "m_BoneIndices",
        "m_NormalSigns",
        "m_Normals",
        "m_TangentSigns",
        "m_Tangents",
        "m_Triangles",
        "m_UV",
        "m_Vertices",
        "m_Weights",
    ],
    optional: &["m_BindPoses", "m_Colors", "m_FloatColors", "m_UVInfo"],
    slotted: true,
};

/// `PackedBitVector` (`classes/generated.py:9160-9165`).
pub(crate) const PACKED_BIT_VECTOR: Class = Class {
    types: &["PackedBitVector"],
    required: &["m_Data", "m_NumItems"],
    optional: &["m_BitSize", "m_Range", "m_Start"],
    slotted: true,
};

/// A skin element: `BoneWeights4` or `BoneInfluence`, the same fields
/// (`classes/generated.py:6956-6976`); `m_Skin` is a list of either.
pub(crate) const BONE_WEIGHTS: Class = Class {
    types: &["BoneWeights4", "BoneInfluence"],
    required: &[
        "boneIndex_0_",
        "boneIndex_1_",
        "boneIndex_2_",
        "boneIndex_3_",
        "weight_0_",
        "weight_1_",
        "weight_2_",
        "weight_3_",
    ],
    optional: &[],
    slotted: true,
};

/// Whether UnityPyBoost's `clean_name(name)`
/// (`UnityPyBoost/TypeTreeHelper.cpp:28-65`) is `target`, a plain
/// identifier: a leading `(int&)` and one trailing `?` dropped, and each of
/// ` .:-[]` read as `_` (the `pass`/`from` and leading-digit rules cannot
/// make any target here).
fn clean_name_is(name: &[u8], target: &str) -> bool {
    let name = name.strip_prefix(b"(int&)").unwrap_or(name);
    let name = name.strip_suffix(b"?").unwrap_or(name);
    let target = target.as_bytes();
    name.len() == target.len()
        && name.iter().zip(target).all(|(&a, &b)| {
            let a = if b" .:-[]".contains(&a) { b'_' } else { a };
            a == b
        })
}

/// Whether object mode builds `node` as `class` rather than an
/// `UnknownObject` (`UnityPyBoost/TypeTreeHelper.cpp:501-574`): every
/// required field among its children's clean names, and, for a slotted
/// class, none that is not a field. The comparisons are counted on `meter`
/// first: each child against each required field, and, for a slotted
/// class, against every field.
fn builds(list: &NodeList, node: NodeId, class: &Class, meter: &Meter) -> Result<bool, MeshError> {
    let kids = list.children(node);
    let fields = class.required.len().saturating_add(class.optional.len());
    let per_child = class
        .required
        .len()
        .saturating_add(if class.slotted { fields } else { 0 });
    count(meter, kids.len().saturating_mul(per_child))?;
    let has = |f: &str| kids.iter().any(|&c| clean_name_is(list.name(c), f));
    let is_field = |c: NodeId| {
        class
            .required
            .iter()
            .chain(class.optional)
            .any(|f| clean_name_is(list.name(c), f))
    };
    Ok(class.required.iter().all(|f| has(f))
        && (!class.slotted || kids.iter().all(|&c| is_field(c))))
}

/// One object, as object mode builds it from a class node (module docs),
/// with the meter its reads count their work on.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Obj<'a> {
    list: &'a NodeList,
    node: NodeId,
    map: &'a Map,
    built: bool,
    meter: &'a Meter,
}

const _: () = crate::send_sync::<Obj<'static>>();

impl<'a> Obj<'a> {
    /// The root object of `parsed`, a node of `class`, its reads counted on
    /// `meter`.
    pub(crate) fn root(
        parsed: &'a Parsed,
        class: &'static Class,
        meter: &'a Meter,
    ) -> Result<Self, MeshError> {
        let list = parsed
            .lists()
            .first()
            .ok_or_else(|| MeshError::malformed("a value without a tree"))?;
        Self::new(
            list,
            list.root(),
            parsed.value(),
            class,
            "the object",
            meter,
        )
    }

    /// `node`'s object, `value` its dict.
    fn new(
        list: &'a NodeList,
        node: NodeId,
        value: &'a Value,
        class: &'static Class,
        field: &str,
        meter: &'a Meter,
    ) -> Result<Self, MeshError> {
        let ty = list.type_name(node);
        if !class.types.iter().any(|t| t.as_bytes() == ty) {
            return Err(MeshError::malformed(format!(
                "{field} has type {}, not {}",
                type_name(ty),
                class.types.join(" or ")
            )));
        }
        let Value::Dict(map) = value else {
            return Err(MeshError::malformed(format!("{field} is not a class")));
        };
        if map.list() != 0 {
            return Err(MeshError::malformed(format!(
                "{field} is read through a referenced type"
            )));
        }
        Ok(Self {
            list,
            node,
            map,
            built: builds(list, node, class, meter)?,
            meter,
        })
    }

    /// Whether object mode built the class, not an `UnknownObject`.
    pub(crate) fn built(&self) -> bool {
        self.built
    }

    /// The child whose clean name is `name`: the last, as object mode's
    /// dict keeps the last value of a name.
    fn child(&self, name: &str) -> Option<NodeId> {
        self.list
            .children(self.node)
            .iter()
            .rev()
            .find(|&&c| clean_name_is(self.list.name(c), name))
            .copied()
    }

    /// The attribute `name` (module docs): its value; `None`, the field's
    /// default, on a class that was built; `AttributeError` on an
    /// `UnknownObject`. Its scans, of the children and of the dict, are
    /// counted first, a unit a name.
    pub(crate) fn get(&self, name: &'static str) -> Result<Option<(NodeId, &'a Value)>, MeshError> {
        let scanned = self
            .list
            .children(self.node)
            .len()
            .saturating_add(self.map.entries().len());
        count(self.meter, scanned)?;
        let found = self.child(name).and_then(|c| {
            let raw = self.list.name(c);
            self.map
                .entries()
                .iter()
                .find(|(k, _)| self.list.name(*k) == raw)
                .map(|(_, v)| (c, v))
        });
        match found {
            Some(hit) => Ok(Some(hit)),
            None if self.built => Ok(None),
            None => Err(MeshError::NoAttribute(name)),
        }
    }

    /// The attribute `name`, an integer from 0 to `max` (a Python `bool`
    /// reads as its `int`, as Python's arithmetic takes it).
    pub(crate) fn uint(&self, name: &'static str, max: u64) -> Result<Option<u64>, MeshError> {
        let Some((_, v)) = self.get(name)? else {
            return Ok(None);
        };
        let n = match v {
            Value::Bool(b) => Some(u64::from(*b)),
            Value::Int(i) => u64::try_from(*i).ok(),
            Value::UInt(u) => Some(*u),
            _ => {
                return Err(MeshError::malformed(format!("{name} is not an integer")));
            }
        };
        match n {
            Some(n) if n <= max => Ok(Some(n)),
            _ => Err(MeshError::malformed(format!("{name} is out of its range"))),
        }
    }

    /// The attribute `name`, a 32-bit signed integer (a `bool` as its
    /// `int`).
    pub(crate) fn int32(&self, name: &'static str) -> Result<Option<i64>, MeshError> {
        let Some((_, v)) = self.get(name)? else {
            return Ok(None);
        };
        let n = match v {
            Value::Bool(b) => i64::from(*b),
            Value::Int(i) if i32::try_from(*i).is_ok() => *i,
            Value::Int(_) | Value::UInt(_) => {
                return Err(MeshError::malformed(format!("{name} is out of its range")));
            }
            _ => return Err(MeshError::malformed(format!("{name} is not an integer"))),
        };
        Ok(Some(n))
    }

    /// The attribute `name`, `bytes` (`TypelessData`).
    pub(crate) fn bytes(&self, name: &'static str) -> Result<Option<&'a [u8]>, MeshError> {
        match self.get(name)? {
            None => Ok(None),
            Some((_, Value::Bytes(b))) => Ok(Some(b)),
            Some(_) => Err(MeshError::malformed(format!("{name} is not bytes"))),
        }
    }

    /// The attribute `name`, a `str`.
    pub(crate) fn string(&self, name: &'static str) -> Result<Option<&'a RawStr>, MeshError> {
        match self.get(name)? {
            None => Ok(None),
            Some((_, Value::Str(s))) => Ok(Some(s)),
            Some(_) => Err(MeshError::malformed(format!("{name} is not a string"))),
        }
    }

    /// The attribute `name`, an object of `class`.
    pub(crate) fn class(
        &self,
        name: &'static str,
        class: &'static Class,
    ) -> Result<Option<Obj<'a>>, MeshError> {
        match self.get(name)? {
            None => Ok(None),
            Some((node, v)) => Self::new(self.list, node, v, class, name, self.meter).map(Some),
        }
    }

    /// The attribute `name`, a list of objects of `class`.
    pub(crate) fn list(
        &self,
        name: &'static str,
        class: &'static Class,
    ) -> Result<Option<List<'a>>, MeshError> {
        let Some((node, v)) = self.get(name)? else {
            return Ok(None);
        };
        let items: &'a [Value] = match v {
            Value::List(items) => items,
            Value::Tuple(items) => items,
            Value::Prims(p) if p.is_empty() => &[],
            _ => {
                return Err(MeshError::malformed(format!(
                    "{name} is not a list of classes"
                )));
            }
        };
        // UnityPy TypeTreeHelper.cpp:812-874: a vector's element is its
        // `Array` child's second child.
        let elem = match self.list.children(node) {
            [array, ..] => match self.list.children(*array) {
                [_, elem] => Some(*elem),
                _ => None,
            },
            [] => None,
        };
        let Some(elem) = elem else {
            return Err(MeshError::malformed(format!("{name} is not a vector")));
        };
        Ok(Some(List {
            nodes: self.list,
            elem,
            items,
            class,
            name,
            built: builds(self.list, elem, class, self.meter)?,
            meter: self.meter,
        }))
    }

    /// The attribute `name`, a list of integers.
    pub(crate) fn ints(&self, name: &'static str) -> Result<Option<Ints<'a>>, MeshError> {
        match self.get(name)? {
            None => Ok(None),
            Some((_, v)) => Ints::of(v, name).map(Some),
        }
    }
}

/// A list of objects of one class (one element node, so one answer to
/// whether object mode builds them).
#[derive(Clone, Copy, Debug)]
pub(crate) struct List<'a> {
    nodes: &'a NodeList,
    elem: NodeId,
    items: &'a [Value],
    class: &'static Class,
    name: &'static str,
    built: bool,
    meter: &'a Meter,
}

const _: () = crate::send_sync::<List<'static>>();

impl<'a> List<'a> {
    /// The number of elements.
    pub(crate) fn len(&self) -> usize {
        self.items.len()
    }

    /// Element `i`, if there is one.
    pub(crate) fn get(&self, i: usize) -> Option<Result<Obj<'a>, MeshError>> {
        let v = self.items.get(i)?;
        Some(self.element(v))
    }

    fn element(&self, v: &'a Value) -> Result<Obj<'a>, MeshError> {
        let ty = self.nodes.type_name(self.elem);
        if !self.class.types.iter().any(|t| t.as_bytes() == ty) {
            return Err(MeshError::malformed(format!(
                "{}'s elements have type {}",
                self.name,
                type_name(ty)
            )));
        }
        let Value::Dict(map) = v else {
            return Err(MeshError::malformed(format!(
                "{}'s elements are not classes",
                self.name
            )));
        };
        if map.list() != 0 {
            return Err(MeshError::malformed(format!(
                "{} is read through a referenced type",
                self.name
            )));
        }
        Ok(Obj {
            list: self.nodes,
            node: self.elem,
            map,
            built: self.built,
            meter: self.meter,
        })
    }

    /// The elements in order.
    pub(crate) fn iter(&self) -> impl Iterator<Item = Result<Obj<'a>, MeshError>> + '_ {
        self.items.iter().map(|v| self.element(v))
    }
}

/// A list of integers: a primitive run, or a list or tuple of `int`s.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Ints<'a> {
    /// A primitive run.
    Run(&'a PrimArray),
    /// A list or tuple of values, each checked as it is read.
    Items(&'a [Value], &'static str),
    /// `bytes` (`TypelessData`), which `bytes()` copies as it is.
    Raw(&'a [u8]),
}

const _: () = crate::send_sync::<Ints<'static>>();

impl<'a> Ints<'a> {
    fn of(v: &'a Value, name: &'static str) -> Result<Self, MeshError> {
        match v {
            Value::Prims(p) => match p {
                PrimArray::F32(_) | PrimArray::F64(_) if !p.is_empty() => Err(
                    MeshError::malformed(format!("{name} is not a list of integers")),
                ),
                _ => Ok(Self::Run(p)),
            },
            Value::List(items) => Ok(Self::Items(items, name)),
            Value::Tuple(items) => Ok(Self::Items(items, name)),
            Value::Bytes(b) => Ok(Self::Raw(b)),
            _ => Err(MeshError::malformed(format!("{name} is not a list"))),
        }
    }

    /// The number of integers.
    pub(crate) fn len(&self) -> usize {
        match self {
            Self::Run(p) => p.len(),
            Self::Items(items, _) => items.len(),
            Self::Raw(b) => b.len(),
        }
    }

    /// Element `i` of `bytes(self)`: the value, which must be a byte
    /// (`ValueError` "bytes must be in range(0, 256)" otherwise; a `bool`
    /// is its `int`). `i` past the end reads as 0.
    pub(crate) fn byte(&self, i: usize) -> Result<u8, MeshError> {
        fn byte<T: TryInto<u8>>(v: Option<T>) -> Result<u8, MeshError> {
            v.map_or(Ok(0), |v| v.try_into().map_err(|_| MeshError::ByteRange))
        }
        match self {
            Self::Run(p) => match p {
                PrimArray::U8(b) => Ok(b.get(i).copied().unwrap_or(0)),
                PrimArray::Bool(v) => Ok(v.get(i).copied().map_or(0, u8::from)),
                PrimArray::I8(v) => byte(v.get(i).copied()),
                PrimArray::U16(v) => byte(v.get(i).copied()),
                PrimArray::I16(v) => byte(v.get(i).copied()),
                PrimArray::U32(v) => byte(v.get(i).copied()),
                PrimArray::I32(v) => byte(v.get(i).copied()),
                PrimArray::U64(v) => byte(v.get(i).copied()),
                PrimArray::I64(v) => byte(v.get(i).copied()),
                // Refused by `of` unless empty.
                PrimArray::F32(_) | PrimArray::F64(_) => Ok(0),
            },
            Self::Items(items, name) => match items.get(i) {
                None => Ok(0),
                Some(Value::Bool(b)) => Ok(u8::from(*b)),
                Some(Value::Int(v)) => byte(Some(*v)),
                Some(Value::UInt(v)) => byte(Some(*v)),
                Some(_) => Err(MeshError::malformed(format!(
                    "{name} holds a value that is not an integer"
                ))),
            },
            Self::Raw(b) => Ok(b.get(i).copied().unwrap_or(0)),
        }
    }

    /// The bytes themselves, where they need no check (a `U8` run, or
    /// `bytes`).
    pub(crate) fn as_u8(&self) -> Option<&'a [u8]> {
        match self {
            Self::Run(PrimArray::U8(b)) => Some(b),
            Self::Raw(b) => Some(b),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{NAME_BYTES, type_name};

    #[test]
    fn a_type_name_is_cut_between_escapes() {
        assert_eq!(type_name(b"Mesh"), "Mesh");
        let exact = "x".repeat(NAME_BYTES);
        assert_eq!(type_name(exact.as_bytes()), exact);
        let long = "y".repeat(1 << 20);
        assert_eq!(
            type_name(long.as_bytes()),
            format!("{}…", &long[..NAME_BYTES])
        );
        // Each 0xFF escapes to four bytes, `\xff`: sixteen fit.
        let cut = type_name(&[0xFF; 100]);
        assert_eq!(cut, format!("{}…", "\\xff".repeat(NAME_BYTES / 4)));
        // An escape that would cross the bound is left out whole.
        let mut ty = vec![b'a'; NAME_BYTES - 2];
        ty.push(0xFF);
        assert_eq!(type_name(&ty), format!("{}…", "a".repeat(NAME_BYTES - 2)));
    }
}
