//! Read and validate the runtime `Avatar.m_Avatar` tables. The authoring
//! `m_HumanDescription` is import evidence only and is not read here.
//! [AssetStudio Avatar v0.16.47](https://github.com/Perfare/AssetStudio/blob/v0.16.47/AssetStudio/Classes/Avatar.cs)
use crate::{AnimError, read::Dict};
use std::sync::Arc;
use unityrs_core::{
    ObjectRef, RawStr,
    limits::{Meter, MeteredVec},
    value::{PrimArray, Value},
};

/// Local translation, XYZW rotation and scale, in Unity space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Xform {
    pub t: [f32; 3],
    pub q: [f32; 4],
    pub s: [f32; 3],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SkeletonNode {
    /// The parent node, -1 for the root.
    pub parent: i32,
    /// The `m_AxesArray` record, -1 for none.
    pub axes: i32,
}

/// One runtime joint-axis record.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Axes {
    pub pre: [f32; 4],
    pub post: [f32; 4],
    pub sign: [f32; 3],
    /// Limits in radians, relative to the neutral pose.
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub length: f32,
    pub kind: i32,
}

#[derive(Debug)]
pub struct Skeleton {
    pub nodes: MeteredVec<SkeletonNode>,
    /// The CRC32 of each node's path below the Animator node.
    pub ids: MeteredVec<u32>,
    pub axes: MeteredVec<Axes>,
}

/// The four twist settings in `m_Human`, as fractions.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Twist {
    pub arm: f32,
    pub fore_arm: f32,
    pub upper_leg: f32,
    pub leg: f32,
}

impl Twist {
    /// Each setting with its serialized field name.
    #[must_use]
    pub const fn fields(&self) -> [(&'static str, f32); 4] {
        [
            ("m_ArmTwist", self.arm),
            ("m_ForeArmTwist", self.fore_arm),
            ("m_UpperLegTwist", self.upper_leg),
            ("m_LegTwist", self.leg),
        ]
    }
}

#[derive(Debug)]
pub struct Human {
    /// `m_RootX`, the rest body pose in the Animator root frame.
    pub root: Xform,
    pub skeleton: Skeleton,
    pub pose: MeteredVec<Xform>,
    /// `m_HumanBoneIndex`: human skeleton node per internal body bone, -1 absent.
    pub bones: MeteredVec<i32>,
    pub left_hand: MeteredVec<i32>,
    pub right_hand: MeteredVec<i32>,
    pub scale: f32,
    pub twist: Twist,
    pub arm_stretch: f32,
    pub leg_stretch: f32,
    pub feet_spacing: f32,
    pub has_left_hand: bool,
    pub has_right_hand: bool,
    pub has_translation_dof: bool,
}

/// The runtime tables of one Avatar. Index arrays are validated on read.
#[derive(Debug)]
pub struct Avatar {
    pub object: ObjectRef,
    pub skeleton: Skeleton,
    pub skeleton_pose: MeteredVec<Xform>,
    pub default_pose: MeteredVec<Xform>,
    pub name_ids: MeteredVec<u32>,
    pub human: Human,
    /// Avatar skeleton node per human skeleton node, -1 absent.
    pub human_skeleton_index: MeteredVec<i32>,
    /// Human skeleton node per Avatar skeleton node, -1 absent.
    pub human_skeleton_reverse_index: MeteredVec<i32>,
    pub root_motion_bone: i32,
    /// `m_RootMotionBoneX`, the root-motion bone's rest pose.
    pub root_motion_x: Xform,
    pub root_motion_skeleton: Skeleton,
    pub root_motion_pose: MeteredVec<Xform>,
    /// Avatar skeleton node per root-motion skeleton node, -1 absent.
    pub root_motion_index: MeteredVec<i32>,
    /// `m_TOS` path hashes and the stored relative paths.
    pub tos: MeteredVec<(u32, RawStr)>,
}

impl Avatar {
    /// The stored path for a skeleton ID, if `m_TOS` has one.
    pub fn tos_path(&self, id: u32, meter: &Meter) -> Result<Option<&RawStr>, AnimError> {
        meter.count_decode(self.tos.len() as u64)?;
        Ok(self
            .tos
            .as_slice()
            .iter()
            .find(|(hash, _)| *hash == id)
            .map(|(_, path)| path))
    }
}

pub fn read_avatar(object: &ObjectRef, meter: &Arc<Meter>) -> Result<Avatar, AnimError> {
    if object.class_id() != 90 {
        return Err(AnimError::WrongClass(object.class_id()));
    }
    meter.count_decode(u64::from(object.info().byte_size))?;
    let tree = object
        .tree(None)?
        .ok_or_else(|| unityrs_core::walk::ParseError::NoTypeTree {
            class_id: object.class_id(),
            why: object.why_no_tree(),
        })?;
    let parsed = object.read_typetree_lossless(Some(&tree.tree), true, meter)?;
    let dict = Dict {
        parsed: &parsed,
        meter,
    };
    let runtime = dict
        .root("m_Avatar")?
        .ok_or(AnimError::Malformed("m_Avatar"))?;
    let avatar_skeleton = skeleton(&dict, dict.required(runtime, "m_AvatarSkeleton")?)?;
    let skeleton_pose = pose(&dict, dict.required(runtime, "m_AvatarSkeletonPose")?)?;
    let default_pose = pose(&dict, dict.required(runtime, "m_DefaultPose")?)?;
    let name_ids = u32s(&dict, runtime, "m_SkeletonNameIDArray")?;
    let human = human(&dict, data(&dict, dict.required(runtime, "m_Human")?)?)?;
    let human_skeleton_index = i32s(&dict, runtime, "m_HumanSkeletonIndexArray")?;
    let human_skeleton_reverse_index = i32s(&dict, runtime, "m_HumanSkeletonReverseIndexArray")?;
    let root_motion_bone = dict.int(runtime, "m_RootMotionBoneIndex")?;
    let root_motion_x = xform(&dict, dict.required(runtime, "m_RootMotionBoneX")?)?;
    let root_motion_skeleton = skeleton(&dict, dict.required(runtime, "m_RootMotionSkeleton")?)?;
    let root_motion_pose = pose(&dict, dict.required(runtime, "m_RootMotionSkeletonPose")?)?;
    let root_motion_index = i32s(&dict, runtime, "m_RootMotionSkeletonIndexArray")?;
    let mut tos = MeteredVec::new(meter);
    for row in dict.rows("m_TOS")? {
        let pair: &[Value] = match row {
            Value::Tuple(pair) => pair,
            Value::List(pair) => pair,
            _ => return Err(AnimError::Malformed("m_TOS")),
        };
        let [hash, Value::Str(path)] = pair else {
            return Err(AnimError::Malformed("m_TOS"));
        };
        let hash = hash
            .as_u64()
            .and_then(|hash| u32::try_from(hash).ok())
            .ok_or(AnimError::Malformed("m_TOS"))?;
        meter.count_decode(path.as_bytes().len() as u64)?;
        tos.push((hash, path.clone()))?;
    }
    let avatar = Avatar {
        object: object.clone(),
        skeleton: avatar_skeleton,
        skeleton_pose,
        default_pose,
        name_ids,
        human,
        human_skeleton_index,
        human_skeleton_reverse_index,
        root_motion_bone,
        root_motion_x,
        root_motion_skeleton,
        root_motion_pose,
        root_motion_index,
        tos,
    };
    validate(&avatar, meter)?;
    Ok(avatar)
}

#[allow(clippy::too_many_lines, reason = "one ordered list of index checks")]
fn validate(avatar: &Avatar, meter: &Arc<Meter>) -> Result<(), AnimError> {
    let nodes = avatar.skeleton.nodes.len();
    let human = &avatar.human;
    let human_nodes = human.skeleton.nodes.len();
    check_skeleton(&avatar.skeleton, "m_AvatarSkeleton", meter)?;
    check_skeleton(&human.skeleton, "m_Human.m_Skeleton", meter)?;
    let motion_nodes = avatar.root_motion_skeleton.nodes.len();
    if motion_nodes > 0 {
        check_skeleton(&avatar.root_motion_skeleton, "m_RootMotionSkeleton", meter)?;
    }
    let lengths = [
        (avatar.skeleton_pose.len(), nodes, "m_AvatarSkeletonPose"),
        (avatar.default_pose.len(), nodes, "m_DefaultPose"),
        (avatar.name_ids.len(), nodes, "m_SkeletonNameIDArray"),
        (human.pose.len(), human_nodes, "m_Human.m_SkeletonPose"),
        (
            avatar.human_skeleton_index.len(),
            human_nodes,
            "m_HumanSkeletonIndexArray",
        ),
        (
            avatar.human_skeleton_reverse_index.len(),
            nodes,
            "m_HumanSkeletonReverseIndexArray",
        ),
        (human.bones.len(), BODY_BONES, "m_HumanBoneIndex"),
        (
            avatar.root_motion_pose.len(),
            motion_nodes,
            "m_RootMotionSkeletonPose",
        ),
        (
            avatar.root_motion_index.len(),
            motion_nodes,
            "m_RootMotionSkeletonIndexArray",
        ),
    ];
    for (actual, expected, field) in lengths {
        if actual != expected {
            return Err(AnimError::InvalidAvatarIndex(field));
        }
    }
    for (hand, field) in [
        (&human.left_hand, "m_LeftHand"),
        (&human.right_hand, "m_RightHand"),
    ] {
        if !hand.is_empty() && hand.len() != HAND_BONES {
            return Err(AnimError::InvalidAvatarIndex(field));
        }
    }
    let ranges = [
        (human.bones.as_slice(), human_nodes, "m_HumanBoneIndex"),
        (human.left_hand.as_slice(), human_nodes, "m_LeftHand"),
        (human.right_hand.as_slice(), human_nodes, "m_RightHand"),
        (
            avatar.human_skeleton_index.as_slice(),
            nodes,
            "m_HumanSkeletonIndexArray",
        ),
        (
            avatar.human_skeleton_reverse_index.as_slice(),
            human_nodes,
            "m_HumanSkeletonReverseIndexArray",
        ),
        (
            avatar.root_motion_index.as_slice(),
            nodes,
            "m_RootMotionSkeletonIndexArray",
        ),
    ];
    for (values, bound, field) in ranges {
        meter.count_decode(values.len() as u64)?;
        if values.iter().any(|&value| !optional_index(value, bound)) {
            return Err(AnimError::InvalidAvatarIndex(field));
        }
    }
    // The forward and reverse skeleton maps must agree wherever mapped.
    for (human_node, &node) in avatar.human_skeleton_index.as_slice().iter().enumerate() {
        if let Ok(node) = usize::try_from(node)
            && avatar
                .human_skeleton_reverse_index
                .as_slice()
                .get(node)
                .and_then(|&back| usize::try_from(back).ok())
                != Some(human_node)
        {
            return Err(AnimError::InvalidAvatarIndex(
                "m_HumanSkeletonReverseIndexArray",
            ));
        }
    }
    // And the other way: a reverse entry names a human node mapped back.
    for (node, &human_node) in avatar
        .human_skeleton_reverse_index
        .as_slice()
        .iter()
        .enumerate()
    {
        if let Ok(human_node) = usize::try_from(human_node)
            && avatar
                .human_skeleton_index
                .as_slice()
                .get(human_node)
                .and_then(|&forward| usize::try_from(forward).ok())
                != Some(node)
        {
            return Err(AnimError::InvalidAvatarIndex("m_HumanSkeletonIndexArray"));
        }
    }
    // A body bone maps to one human skeleton node at most once.
    let mut seen = MeteredVec::with_capacity(meter, human_nodes)?;
    meter.count_decode(human_nodes as u64)?;
    for _ in 0..human_nodes {
        seen.push(false)?;
    }
    for &node in human
        .bones
        .as_slice()
        .iter()
        .chain(human.left_hand.as_slice())
        .chain(human.right_hand.as_slice())
    {
        if let Some(flag) = usize::try_from(node)
            .ok()
            .and_then(|node| seen.as_mut_slice().get_mut(node))
        {
            if *flag {
                return Err(AnimError::InvalidAvatarIndex("m_HumanBoneIndex"));
            }
            *flag = true;
        }
    }
    if !optional_index(avatar.root_motion_bone, nodes) {
        return Err(AnimError::InvalidAvatarIndex("m_RootMotionBoneIndex"));
    }
    Ok(())
}

/// Built body bones in Mecanim order: `HumanBodyBones` 0-8, `UpperChest`,
/// then 9-23. A hand table lists 15 finger bones in `HumanBodyBones` order.
pub const BODY_BONES: usize = 25;
pub const HAND_BONES: usize = 15;

fn optional_index(value: i32, bound: usize) -> bool {
    value == -1 || usize::try_from(value).is_ok_and(|value| value < bound)
}

/// Parents precede children and the root alone has no parent.
fn check_skeleton(
    skeleton: &Skeleton,
    field: &'static str,
    meter: &Meter,
) -> Result<(), AnimError> {
    let nodes = skeleton.nodes.as_slice();
    meter.count_decode(nodes.len() as u64)?;
    if skeleton.ids.len() != nodes.len() {
        return Err(AnimError::InvalidAvatarIndex(field));
    }
    for (i, node) in nodes.iter().enumerate() {
        let parent_ok = if i == 0 {
            node.parent == -1
        } else {
            usize::try_from(node.parent).is_ok_and(|parent| parent < i)
        };
        if !parent_ok || !optional_index(node.axes, skeleton.axes.len()) {
            return Err(AnimError::InvalidAvatarIndex(field));
        }
    }
    Ok(())
}

fn data<'a>(dict: &Dict<'a, '_>, value: &'a Value) -> Result<&'a Value, AnimError> {
    Ok(dict.field(value, "data")?.unwrap_or(value))
}

fn skeleton(dict: &Dict<'_, '_>, value: &Value) -> Result<Skeleton, AnimError> {
    let value = data(dict, value)?;
    let mut nodes = MeteredVec::new(dict.meter);
    for row in dict.list(dict.required(value, "m_Node")?, "m_Node")? {
        nodes.push(SkeletonNode {
            parent: dict.int(row, "m_ParentId")?,
            axes: dict.int(row, "m_AxesId")?,
        })?;
    }
    let mut axes = MeteredVec::new(dict.meter);
    for row in dict.list(dict.required(value, "m_AxesArray")?, "m_AxesArray")? {
        let limit = dict.required(row, "m_Limit")?;
        axes.push(Axes {
            pre: vector(dict, row, "m_PreQ")?,
            post: vector(dict, row, "m_PostQ")?,
            sign: vector(dict, row, "m_Sgn")?,
            min: vector(dict, limit, "m_Min")?,
            max: vector(dict, limit, "m_Max")?,
            length: float(dict, row, "m_Length")?,
            kind: dict.int(row, "m_Type")?,
        })?;
    }
    Ok(Skeleton {
        nodes,
        ids: u32s(dict, value, "m_ID")?,
        axes,
    })
}

fn pose(dict: &Dict<'_, '_>, value: &Value) -> Result<MeteredVec<Xform>, AnimError> {
    let value = data(dict, value)?;
    let mut rows = MeteredVec::new(dict.meter);
    for row in dict.list(dict.required(value, "m_X")?, "m_X")? {
        rows.push(xform(dict, row)?)?;
    }
    Ok(rows)
}

fn xform(dict: &Dict<'_, '_>, value: &Value) -> Result<Xform, AnimError> {
    Ok(Xform {
        t: vector(dict, value, "t")?,
        q: vector(dict, value, "q")?,
        s: vector(dict, value, "s")?,
    })
}

fn human(dict: &Dict<'_, '_>, value: &Value) -> Result<Human, AnimError> {
    let hand = |name| -> Result<MeteredVec<i32>, AnimError> {
        let hand = data(dict, dict.required(value, name)?)?;
        i32s(dict, hand, "m_HandBoneIndex")
    };
    let flag = |name| match dict.required(value, name)? {
        Value::Bool(value) => Ok(*value),
        _ => Err(AnimError::Malformed(name)),
    };
    Ok(Human {
        root: xform(dict, dict.required(value, "m_RootX")?)?,
        skeleton: skeleton(dict, dict.required(value, "m_Skeleton")?)?,
        pose: pose(dict, dict.required(value, "m_SkeletonPose")?)?,
        bones: i32s(dict, value, "m_HumanBoneIndex")?,
        left_hand: hand("m_LeftHand")?,
        right_hand: hand("m_RightHand")?,
        scale: float(dict, value, "m_Scale")?,
        twist: Twist {
            arm: float(dict, value, "m_ArmTwist")?,
            fore_arm: float(dict, value, "m_ForeArmTwist")?,
            upper_leg: float(dict, value, "m_UpperLegTwist")?,
            leg: float(dict, value, "m_LegTwist")?,
        },
        arm_stretch: float(dict, value, "m_ArmStretch")?,
        leg_stretch: float(dict, value, "m_LegStretch")?,
        feet_spacing: float(dict, value, "m_FeetSpacing")?,
        has_left_hand: flag("m_HasLeftHand")?,
        has_right_hand: flag("m_HasRightHand")?,
        has_translation_dof: flag("m_HasTDoF")?,
    })
}

fn float(dict: &Dict<'_, '_>, value: &Value, name: &'static str) -> Result<f32, AnimError> {
    match dict.required(value, name)? {
        Value::F32(bits) if f32::from_bits(*bits).is_finite() => Ok(f32::from_bits(*bits)),
        _ => Err(AnimError::Malformed(name)),
    }
}

/// A Vector3, Vector4 or quaternion struct, or a `float3`/`float4`.
fn vector<const N: usize>(
    dict: &Dict<'_, '_>,
    value: &Value,
    name: &'static str,
) -> Result<[f32; N], AnimError> {
    let value = dict.required(value, name)?;
    let mut out = [0.; N];
    for (slot, axis) in out.iter_mut().zip(["x", "y", "z", "w"]) {
        *slot = float(dict, value, axis).map_err(|_| AnimError::Malformed(name))?;
    }
    Ok(out)
}

fn i32s(
    dict: &Dict<'_, '_>,
    value: &Value,
    name: &'static str,
) -> Result<MeteredVec<i32>, AnimError> {
    let values: &[i32] = match dict.required(value, name)? {
        Value::Prims(PrimArray::I32(values)) => values,
        Value::List(values) if values.is_empty() => &[],
        _ => return Err(AnimError::Malformed(name)),
    };
    copy(dict.meter, values)
}

fn u32s(
    dict: &Dict<'_, '_>,
    value: &Value,
    name: &'static str,
) -> Result<MeteredVec<u32>, AnimError> {
    let values: &[u32] = match dict.required(value, name)? {
        Value::Prims(PrimArray::U32(values)) => values,
        Value::List(values) if values.is_empty() => &[],
        _ => return Err(AnimError::Malformed(name)),
    };
    copy(dict.meter, values)
}

fn copy<T: Copy>(meter: &Arc<Meter>, values: &[T]) -> Result<MeteredVec<T>, AnimError> {
    meter.count_decode(values.len() as u64)?;
    let mut out = MeteredVec::with_capacity(meter, values.len())?;
    for &value in values {
        out.push(value)?;
    }
    Ok(out)
}

const _: () = crate::send_sync::<Avatar>();
const _: () = crate::send_sync::<Human>();
const _: () = crate::send_sync::<Skeleton>();
const _: () = crate::send_sync::<SkeletonNode>();
const _: () = crate::send_sync::<Axes>();
const _: () = crate::send_sync::<Twist>();
const _: () = crate::send_sync::<Xform>();
