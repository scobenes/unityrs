use crate::{SceneError, builtin::BuiltinRef};
use unityrs_core::env::{DerefError, PPtr};
use unityrs_core::limits::MeteredVec;
use unityrs_core::{ObjectRef, RawStr};

/// Index into `Graph::nodes()`.
pub type NodeId = usize;
/// The optional caller bound can tighten the file's `max_tree_depth`.
#[derive(Clone, Copy, Debug, Default)]
pub struct GraphOptions {
    /// Maximum node depth, counting a root as one. Zero refuses a root.
    pub max_depth: Option<u32>,
}
/// The graph owns all array charges; retained file handles keep core charges.
#[derive(Debug)]
pub struct Graph {
    pub(crate) nodes: MeteredVec<Node>,
    pub(crate) roots: MeteredVec<NodeId>,
    pub(crate) issues: MeteredVec<Issue>,
}
impl Graph {
    /// Nodes in depth-first first-reach order.
    #[must_use]
    pub fn nodes(&self) -> &[Node] {
        self.nodes.as_slice()
    }
    /// Root ids in object-table order for a scene, or the one prefab root.
    #[must_use]
    pub fn roots(&self) -> &[NodeId] {
        self.roots.as_slice()
    }
    /// Recoverable broken links and malformed descendants, in walk order.
    #[must_use]
    pub fn issues(&self) -> &[Issue] {
        self.issues.as_slice()
    }
}
/// Transform or its `RectTransform` subclass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransformKind {
    Transform,
    RectTransform,
}
/// Local translation, quaternion (x,y,z,w), and scale, in Unity space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Trs {
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
}
/// One reached `GameObject` and Transform.
#[derive(Debug)]
pub struct Node {
    pub game_object: ObjectRef,
    pub transform: ObjectRef,
    pub kind: TransformKind,
    pub name: RawStr,
    pub active: bool,
    pub layer: u32,
    pub tag: u16,
    pub local: Trs,
    pub parent: Option<NodeId>,
    pub children: MeteredVec<NodeId>,
    pub components: MeteredVec<Component>,
}
/// A non-null reference. Unresolved references retain the core's reason.
#[derive(Debug)]
pub enum PPtrTarget {
    Object(ObjectRef),
    Builtin(BuiltinRef),
    Unresolved(DerefError),
}
/// Components in the `GameObject`'s stored order. Broken component pointers
/// are retained, too, so they do not shift the remaining components.
#[derive(Debug)]
#[allow(
    clippy::large_enum_variant,
    reason = "inline variants avoid infallible Box allocations"
)]
pub enum Component {
    MeshFilter {
        object: ObjectRef,
        mesh: Option<PPtrTarget>,
    },
    MeshRenderer(RendererInfo),
    SkinnedMeshRenderer(SkinInfo),
    LodGroup(LodInfo),
    Other(i32, ObjectRef),
    Unresolved {
        pointer: PPtr,
        reason: DerefError,
    },
}
/// Shared renderer facts, without geometry decoding.
#[derive(Debug)]
pub struct RendererInfo {
    pub object: ObjectRef,
    pub materials: MeteredVec<Option<PPtrTarget>>,
    pub static_batch: Option<(u16, u16)>,
    pub subset_indices: MeteredVec<u32>,
    pub enabled: bool,
    pub static_batch_root: Option<PPtrTarget>,
}
/// Skin references and weights as stored, without normalisation.
#[derive(Debug)]
pub struct SkinInfo {
    pub renderer: RendererInfo,
    pub mesh: Option<PPtrTarget>,
    pub bones: MeteredVec<Option<PPtrTarget>>,
    pub root_bone: Option<PPtrTarget>,
    pub blend_weights: MeteredVec<f32>,
}
/// `LODGroup` selection inputs in stored order.
#[derive(Debug)]
pub struct LodInfo {
    pub object: ObjectRef,
    pub local_reference_point: [f32; 3],
    pub size: f32,
    pub enabled: bool,
    pub lods: MeteredVec<Lod>,
}
/// One LOD's screen threshold, fade width and renderer references.
#[derive(Debug)]
pub struct Lod {
    pub screen_relative_height: f32,
    pub fade_transition_width: f32,
    pub renderers: MeteredVec<Option<PPtrTarget>>,
}
/// Recoverable graph failures. Resource refusals always fail the call.
#[derive(Debug)]
pub enum Issue {
    Unresolved {
        from: ObjectRef,
        pointer: PPtr,
        reason: DerefError,
    },
    Builtin {
        from: ObjectRef,
        builtin: BuiltinRef,
    },
    Malformed {
        object: ObjectRef,
        error: SceneError,
    },
    Cycle {
        transform: ObjectRef,
        parent: Option<NodeId>,
    },
    Depth {
        transform: ObjectRef,
        depth: u32,
        limit: u32,
    },
    ParentMismatch {
        transform: ObjectRef,
        expected: NodeId,
        actual: PPtr,
    },
}
const _: () = crate::send_sync::<NodeId>();
const _: () = crate::send_sync::<GraphOptions>();
const _: () = crate::send_sync::<Graph>();
const _: () = crate::send_sync::<TransformKind>();
const _: () = crate::send_sync::<Trs>();
const _: () = crate::send_sync::<Node>();
const _: () = crate::send_sync::<PPtrTarget>();
const _: () = crate::send_sync::<Component>();
const _: () = crate::send_sync::<RendererInfo>();
const _: () = crate::send_sync::<SkinInfo>();
const _: () = crate::send_sync::<LodInfo>();
const _: () = crate::send_sync::<Lod>();
const _: () = crate::send_sync::<Issue>();
