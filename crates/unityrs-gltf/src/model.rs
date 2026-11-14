//! All indices refer to the corresponding scene table, never to layout IDs.
use std::sync::Arc;
use unityrs_core::limits::{Meter, MeteredVec};

/// UTF-8 text, constructed through [`crate::text`].
#[derive(Debug)]
pub struct Text(pub(crate) MeteredVec<u8>);
const _: () = crate::send_sync::<Text>();
impl Text {
    /// The original UTF-8 bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_slice()
    }
}
/// Ordered string metadata. Keys must be unique within an object.
pub type Extras = MeteredVec<(Text, Text)>;
const _: () = crate::send_sync::<Extras>();

/// A renderer repair or degradation, referring to its original model node.
#[derive(Debug, Clone, Copy)]
pub struct CollectionIssue {
    pub node: usize,
    pub reason: &'static str,
    /// Number of repaired entries, or one for a degraded renderer.
    pub count: u64,
}
const _: () = crate::send_sync::<CollectionIssue>();

/// Editable scene, already in glTF's right-handed coordinate and UV space.
#[derive(Debug)]
pub struct Scene {
    pub nodes: MeteredVec<Node>,
    pub roots: MeteredVec<usize>,
    pub meshes: MeteredVec<Mesh>,
    pub accessors: MeteredVec<Accessor>,
    pub skins: MeteredVec<Skin>,
    pub materials: MeteredVec<Material>,
    pub images: MeteredVec<Image>,
    pub textures: MeteredVec<Texture>,
    pub extras: Extras,
    pub collection_issues: MeteredVec<CollectionIssue>,
    /// (Collected weight accessor, original weight accessor) pairs. Analysis
    /// compares against the source so repairs made while merging palettes are
    /// counted once per emitted accessor, including clamping and zero rows.
    pub collected_weight_sources: MeteredVec<(usize, usize)>,
}
const _: () = crate::send_sync::<Scene>();
impl Scene {
    /// Empty scene. No allocation occurs until a table grows.
    #[must_use]
    pub fn new(meter: &Arc<Meter>) -> Self {
        Self {
            nodes: MeteredVec::new(meter),
            roots: MeteredVec::new(meter),
            meshes: MeteredVec::new(meter),
            accessors: MeteredVec::new(meter),
            skins: MeteredVec::new(meter),
            materials: MeteredVec::new(meter),
            images: MeteredVec::new(meter),
            textures: MeteredVec::new(meter),
            extras: MeteredVec::new(meter),
            collection_issues: MeteredVec::new(meter),
            collected_weight_sources: MeteredVec::new(meter),
        }
    }
}
/// Scene node. Weights belong to this instance, not its shared geometry.
#[derive(Debug)]
pub struct Node {
    pub name: Option<Text>,
    pub translation: [f64; 3],
    pub rotation: [f64; 4],
    pub scale: [f64; 3],
    pub mesh: Option<usize>,
    pub skin: Option<usize>,
    pub children: MeteredVec<usize>,
    pub weights: MeteredVec<f64>,
    pub extras: Extras,
}
const _: () = crate::send_sync::<Node>();
impl Node {
    /// Identity transform with empty children and metadata.
    #[must_use]
    pub fn new(meter: &Arc<Meter>) -> Self {
        Self {
            name: None,
            translation: [0.; 3],
            rotation: [0., 0., 0., 1.],
            scale: [1.; 3],
            mesh: None,
            skin: None,
            children: MeteredVec::new(meter),
            weights: MeteredVec::new(meter),
            extras: MeteredVec::new(meter),
        }
    }
}
/// Material bindings distinguish mesh instances; accessor indices share geometry.
#[derive(Debug)]
pub struct Mesh {
    pub name: Option<Text>,
    pub primitives: MeteredVec<Primitive>,
    pub target_names: MeteredVec<Text>,
    pub extras: Extras,
}
const _: () = crate::send_sync::<Mesh>();
/// glTF vertex attribute semantics, in a fixed serialization order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Attribute {
    Position,
    Normal,
    Tangent,
    TexCoord(u8),
    Color,
    Joints,
    Weights,
}
const _: () = crate::send_sync::<Attribute>();
impl Attribute {
    pub(crate) fn kind(self) -> Kind {
        match self {
            Self::Position => Kind::Position,
            Self::Normal => Kind::Normal,
            Self::Tangent => Kind::Tangent,
            Self::TexCoord(_) => Kind::TexCoord,
            Self::Color => Kind::Color,
            Self::Joints => Kind::Joints,
            Self::Weights => Kind::Weights,
        }
    }
}
/// Triangle-list primitive. Attributes must be strictly ordered by semantic.
#[derive(Debug)]
pub struct Primitive {
    pub attributes: MeteredVec<(Attribute, usize)>,
    pub indices: Option<usize>,
    pub material: Option<usize>,
    /// Each target has POSITION, NORMAL and TANGENT delta accessor slots.
    pub targets: MeteredVec<[Option<usize>; 3]>,
    pub extras: Extras,
}
const _: () = crate::send_sync::<Primitive>();
/// Accessor usage determines width, component rules and normalisation policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Position,
    Normal,
    Tangent,
    TexCoord,
    Color,
    Joints,
    Weights,
    Indices,
    InverseBind,
    MorphPosition,
    MorphNormal,
    MorphTangent,
}
const _: () = crate::send_sync::<Kind>();
impl Kind {
    pub(crate) fn width(self) -> usize {
        match self {
            Self::Indices => 1,
            Self::TexCoord => 2,
            Self::Tangent | Self::Color | Self::Joints | Self::Weights => 4,
            Self::InverseBind => 16,
            _ => 3,
        }
    }
    pub(crate) fn shape(self) -> &'static str {
        match self.width() {
            1 => "SCALAR",
            2 => "VEC2",
            3 => "VEC3",
            4 => "VEC4",
            _ => "MAT4",
        }
    }
    pub(crate) fn morph(self) -> bool {
        matches!(
            self,
            Self::MorphPosition | Self::MorphNormal | Self::MorphTangent
        )
    }
}
/// Dense scalar data. f64 is rounded to IEEE f32, nearest ties-to-even, on write.
#[derive(Debug)]
pub enum Data {
    Float(MeteredVec<f64>),
    U8(MeteredVec<u8>),
    U16(MeteredVec<u16>),
    U32(MeteredVec<u32>),
}
const _: () = crate::send_sync::<Data>();
impl Data {
    pub(crate) fn len(&self) -> usize {
        match self {
            Self::Float(v) => v.len(),
            Self::U8(v) => v.len(),
            Self::U16(v) => v.len(),
            Self::U32(v) => v.len(),
        }
    }
    pub(crate) fn component(&self) -> u32 {
        match self {
            Self::Float(_) => 5126,
            Self::U8(_) => 5121,
            Self::U16(_) => 5123,
            Self::U32(_) => 5125,
        }
    }
    pub(crate) fn bytes(&self) -> usize {
        match self {
            Self::U8(_) => 1,
            Self::U16(_) => 2,
            _ => 4,
        }
    }
    pub(crate) fn integer(&self, index: usize) -> Option<u32> {
        match self {
            Self::U8(v) => v.as_slice().get(index).map(|&v| u32::from(v)),
            Self::U16(v) => v.as_slice().get(index).map(|&v| u32::from(v)),
            Self::U32(v) => v.as_slice().get(index).copied(),
            Self::Float(_) => None,
        }
    }
    pub(crate) fn value(&self, index: usize) -> Option<f64> {
        match self {
            Self::Float(v) => v.as_slice().get(index).copied(),
            Self::U8(v) => v.as_slice().get(index).map(|&x| f64::from(x)),
            Self::U16(v) => v.as_slice().get(index).map(|&x| f64::from(x)),
            Self::U32(v) => v.as_slice().get(index).map(|&x| f64::from(x)),
        }
    }
}
/// Dense accessor, or zero-based sparse morph deltas. No dense sparse staging.
#[derive(Debug)]
pub struct Accessor {
    pub kind: Kind,
    /// Logical element count, including implicit zero rows of sparse targets.
    pub count: usize,
    pub data: Data,
    /// If present, data has one row per strictly increasing u32 index.
    /// Only morph targets may use this form; an empty set means all zeros.
    pub sparse_indices: Option<MeteredVec<u32>>,
}
const _: () = crate::send_sync::<Accessor>();
/// Joints in model node order; inverse bind accessor contains column-major MAT4s.
#[derive(Debug)]
pub struct Skin {
    pub name: Option<Text>,
    pub joints: MeteredVec<usize>,
    pub inverse_bind_matrices: Option<usize>,
    pub skeleton: Option<usize>,
}
const _: () = crate::send_sync::<Skin>();
/// Opaque material slot for the later material mapper; writes name and extras.
#[derive(Debug)]
pub struct Material {
    pub name: Option<Text>,
    pub extras: Extras,
}
const _: () = crate::send_sync::<Material>();
/// Already-encoded image. Its charged bytes stay alive through layout and write.
#[derive(Debug)]
pub struct Image {
    pub name: Option<Text>,
    pub mime: Mime,
    pub bytes: MeteredVec<u8>,
}
const _: () = crate::send_sync::<Image>();
/// Core glTF embedded image MIME types.
#[derive(Debug, Clone, Copy)]
pub enum Mime {
    Png,
    Jpeg,
}
const _: () = crate::send_sync::<Mime>();
/// Texture slot using glTF's default sampler.
#[derive(Debug)]
pub struct Texture {
    pub image: usize,
}
const _: () = crate::send_sync::<Texture>();
/// Writer policy. Raw mode can produce validator errors, while keeping JSON valid.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub normalise: bool,
}
const _: () = crate::send_sync::<Options>();
impl Default for Options {
    fn default() -> Self {
        Self { normalise: true }
    }
}
