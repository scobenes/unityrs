//! A synthetic humanoid rig: a prefab hierarchy, an Animator with its
//! controller, a runtime Avatar and humanoid clips. Values are typed
//! fixture choices, not recorded data.
use super::{Binding, playback as p};
use crate::ngui::{Object, V, pair, pptr, st, vector};
use crate::scene3d::{Layout, game_object, set, transform, xyz};

/// `HumanBodyBones` index and name of each required bone, in rig order.
/// Every bone is a child of `Hips`, which is the Animator node's child.
pub const BONES: [(usize, &str); 15] = [
    (0, "Hips"),
    (1, "LeftUpperLeg"),
    (2, "RightUpperLeg"),
    (3, "LeftLowerLeg"),
    (4, "RightLowerLeg"),
    (5, "LeftFoot"),
    (6, "RightFoot"),
    (7, "Spine"),
    (10, "Head"),
    (13, "LeftUpperArm"),
    (14, "RightUpperArm"),
    (15, "LeftLowerArm"),
    (16, "RightLowerArm"),
    (17, "LeftHand"),
    (18, "RightHand"),
];

/// `HumanBodyBones` index and name of each optional body bone a rig may
/// add after `BONES`, each a child of `Hips` as well.
pub const OPTIONAL_BONES: [(usize, &str); 10] = [
    (8, "Chest"),
    (54, "UpperChest"),
    (9, "Neck"),
    (11, "LeftShoulder"),
    (12, "RightShoulder"),
    (19, "LeftToes"),
    (20, "RightToes"),
    (21, "LeftEye"),
    (22, "RightEye"),
    (23, "Jaw"),
];

/// Path IDs: `GameObject` 1 and on, `Transform` 101 and on, then these.
pub const ANIMATOR: i64 = 200;
pub const AVATAR: i64 = 201;
pub const CONTROLLER: i64 = 202;
pub const CLIP: i64 = 203;

/// Rig and Avatar choices. The default is a valid neutral rig.
#[derive(Clone, Debug)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent fixture switches"
)]
pub struct Rig {
    pub twist: [f32; 4],
    pub stretch: [f32; 3],
    pub translation_dof: bool,
    pub root_motion_bone: i32,
    /// A `HumanBodyBones` index left unmapped in `m_HumanBoneIndex`.
    pub unmapped_bone: Option<usize>,
    /// A rig bone (index into `BONES`) whose axes record is absent.
    pub no_axes: Option<usize>,
    /// A rig bone whose `m_TOS` entry is absent.
    pub no_tos: Option<usize>,
    /// A rig bone whose `GameObject` name differs from its TOS path.
    pub renamed: Option<usize>,
    pub transform_hierarchy: bool,
    pub avatar: bool,
    /// The Avatar's rest body offset `m_RootX` translation.
    pub body_offset: [f32; 3],
    /// Move the Animator node off the identity.
    pub root_offset: bool,
    /// `HumanBodyBones` indices from `OPTIONAL_BONES` the rig adds, in order.
    pub optional: Vec<usize>,
    /// `m_HumanBoneMass`; `None` stores an equal share per mapped slot.
    pub bone_mass: Option<Vec<f32>>,
    /// `m_Scale`.
    pub scale: f32,
    /// The Avatar's human skeleton in `HumanTrait`'s hierarchy over the
    /// present bones, at `t_pose`, its nodes ordered by depth (`arrange`);
    /// otherwise every bone hangs on `Hips` at the origin.
    pub hierarchy: bool,
}

impl Default for Rig {
    fn default() -> Self {
        Self {
            twist: [0.; 4],
            stretch: [0.; 3],
            translation_dof: false,
            root_motion_bone: -1,
            unmapped_bone: None,
            no_axes: None,
            no_tos: None,
            renamed: None,
            transform_hierarchy: true,
            avatar: true,
            body_offset: [0.; 3],
            root_offset: false,
            optional: Vec::new(),
            bone_mass: None,
            scale: 1.,
            hierarchy: false,
        }
    }
}

/// IEEE CRC32, as Unity hashes TOS paths.
#[must_use]
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0_u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// The rig's bones: `BONES`, then the optional bones it adds.
fn bones(rig: &Rig) -> Vec<(usize, &'static str)> {
    let mut out = BONES.to_vec();
    out.extend(
        rig.optional
            .iter()
            .filter_map(|bone| OPTIONAL_BONES.iter().find(|(b, _)| b == bone)),
    );
    out
}

/// The rig bone (an index into `bones`) a bone's Transform hangs on: with
/// `hierarchy`, its nearest present `HumanTrait` ancestor, as the Avatar's
/// skeleton has it (`arrange`); otherwise `Hips`. `None` for `Hips`.
fn rig_parent(rig: &Rig, bones: &[(usize, &str)], bone: usize) -> Option<usize> {
    if bone == 0 {
        return None;
    }
    if !rig.hierarchy {
        return Some(0);
    }
    let mut up = trait_parent(bones[bone].0);
    while let Some(b) = up {
        if let Some(k) = bones.iter().position(|&(other, _)| other == b) {
            return Some(k);
        }
        up = trait_parent(b);
    }
    Some(0)
}

/// A bone's path from the Animator node: its Transform's ancestors' names.
fn path(rig: &Rig, bones: &[(usize, &str)], bone: usize) -> String {
    let mut names = vec![bones[bone].1];
    let mut at = bone;
    while let Some(parent) = rig_parent(rig, bones, at) {
        names.push(bones[parent].1);
        at = parent;
    }
    names.reverse();
    names.join("/")
}

/// The 25-slot `m_HumanBoneIndex` position of a `HumanBodyBones` index.
const fn slot(bone: usize) -> usize {
    match bone {
        0..=8 => bone,
        54 => 9,
        _ => bone + 1,
    }
}

fn ints(items: impl IntoIterator<Item = i32>) -> V {
    vector(V::Int(0), items.into_iter().map(V::Int).collect())
}

fn uints(items: impl IntoIterator<Item = u32>) -> V {
    vector(V::UInt(0), items.into_iter().map(V::UInt).collect())
}

fn xform() -> V {
    xform_at([0.; 3])
}

fn xform_at(t: [f32; 3]) -> V {
    st(
        "xform",
        vec![
            ("t", xyz("float3", &t)),
            ("q", xyz("float4", &[0., 0., 0., 1.])),
            ("s", xyz("float3", &[1., 1., 1.])),
        ],
    )
}

fn data(value: V) -> V {
    st("OffsetPtr", vec![("data", value)])
}

fn axes() -> V {
    st(
        "Axes",
        vec![
            ("m_PreQ", xyz("float4", &[-0.5, -0.5, 0.5, 0.5])),
            ("m_PostQ", xyz("float4", &[-0.5, -0.5, 0.5, 0.5])),
            ("m_Sgn", xyz("float3", &[1., 1., 1.])),
            (
                "m_Limit",
                st(
                    "Limit",
                    vec![
                        ("m_Min", xyz("float3", &[-0.5, -0.5, -0.5])),
                        ("m_Max", xyz("float3", &[0.5, 0.5, 0.5])),
                    ],
                ),
            ),
            ("m_Length", V::Float(0.1)),
            ("m_Type", V::Int(1)),
        ],
    )
}

/// The human skeleton: node 0 is the Animator node, node `i + 1` is
/// the rig's bone `i`. Every bone but the one `no_axes` names has an axes
/// record.
fn skeleton(rig: &Rig, axes_rows: bool) -> V {
    let bones = bones(rig);
    let nodes = 1 + bones.len();
    let node = |parent: i32, axes: i32| {
        st(
            "Node",
            vec![("m_ParentId", V::Int(parent)), ("m_AxesId", V::Int(axes))],
        )
    };
    let mut rows = vec![node(-1, -1)];
    let mut records = Vec::new();
    for bone in 0..bones.len() {
        let axes_id = if axes_rows && rig.no_axes != Some(bone) {
            records.push(axes());
            i32::try_from(records.len() - 1).unwrap_or_default()
        } else {
            -1
        };
        rows.push(node(i32::from(bone != 0), axes_id));
    }
    let ids = (0..nodes).map(|i| {
        if i == 0 {
            0
        } else {
            crc32(path(rig, &bones, i - 1).as_bytes())
        }
    });
    data(st(
        "Skeleton",
        vec![
            ("m_Node", vector(node(-1, -1), rows)),
            ("m_ID", uints(ids)),
            ("m_AxesArray", vector(axes(), records)),
        ],
    ))
}

fn pose(count: usize) -> V {
    data(st(
        "SkeletonPose",
        vec![("m_X", vector(xform(), vec![xform(); count]))],
    ))
}

/// `m_HumanBoneIndex` in Mecanim body order: `HumanBodyBones` 0-8,
/// `UpperChest`, then 9-23.
fn human_bones(rig: &Rig) -> [i32; 25] {
    let mut index = [-1_i32; 25];
    for (node, &(bone, _)) in bones(rig).iter().enumerate() {
        if rig.unmapped_bone != Some(bone) {
            index[slot(bone)] = i32::try_from(node + 1).unwrap_or_default();
        }
    }
    index
}

/// `m_HumanBoneMass`: the rig's, or an equal share per mapped slot but the
/// zero-mass leaves (slots 22-24: the Eyes and the Jaw), as Unity gives
/// them none.
fn bone_mass(rig: &Rig, index: &[i32; 25]) -> V {
    let masses = rig.bone_mass.clone().unwrap_or_else(|| {
        let heavy = |(slot, &node): (usize, &i32)| node >= 0 && slot < 22;
        let mapped = index.iter().enumerate().filter(|&e| heavy(e)).count();
        #[allow(clippy::cast_precision_loss, reason = "at most 25 slots")]
        let share = 1. / mapped.max(1) as f32;
        index
            .iter()
            .enumerate()
            .map(|e| if heavy(e) { share } else { 0. })
            .collect()
    });
    vector(V::Float(0.), masses.into_iter().map(V::Float).collect())
}

#[must_use]
pub fn avatar(rig: &Rig) -> Object {
    let bones = bones(rig);
    let nodes = 1 + bones.len();
    let index = human_bones(rig);
    let hand = || data(st("Hand", vec![("m_HandBoneIndex", ints([-1; 15]))]));
    let [arm, fore_arm, upper_leg, leg] = rig.twist;
    let [arm_stretch, leg_stretch, feet] = rig.stretch;
    let human = data(st(
        "Human",
        vec![
            ("m_RootX", xform_at(rig.body_offset)),
            ("m_Skeleton", skeleton(rig, true)),
            ("m_SkeletonPose", pose(nodes)),
            ("m_LeftHand", hand()),
            ("m_RightHand", hand()),
            ("m_HumanBoneIndex", ints(index)),
            ("m_HumanBoneMass", bone_mass(rig, &index)),
            ("m_Scale", V::Float(rig.scale)),
            ("m_ArmTwist", V::Float(arm)),
            ("m_ForeArmTwist", V::Float(fore_arm)),
            ("m_UpperLegTwist", V::Float(upper_leg)),
            ("m_LegTwist", V::Float(leg)),
            ("m_ArmStretch", V::Float(arm_stretch)),
            ("m_LegStretch", V::Float(leg_stretch)),
            ("m_FeetSpacing", V::Float(feet)),
            ("m_HasLeftHand", V::Bool(false)),
            ("m_HasRightHand", V::Bool(false)),
            ("m_HasTDoF", V::Bool(rig.translation_dof)),
        ],
    ));
    let identity = || ints((0..nodes).map(|i| i32::try_from(i).unwrap_or_default()));
    let mut tos = vec![pair(V::UInt(0), V::Str(Vec::new()))];
    for bone in 0..bones.len() {
        if rig.no_tos != Some(bone) {
            let path = path(rig, &bones, bone);
            tos.push(pair(
                V::UInt(crc32(path.as_bytes())),
                V::Str(path.into_bytes()),
            ));
        }
    }
    let runtime = st(
        "AvatarConstant",
        vec![
            ("m_AvatarSkeleton", skeleton(rig, false)),
            ("m_AvatarSkeletonPose", pose(nodes)),
            ("m_DefaultPose", pose(nodes)),
            (
                "m_SkeletonNameIDArray",
                uints((0..nodes).map(|i| u32::try_from(i).unwrap_or_default())),
            ),
            ("m_Human", human),
            ("m_HumanSkeletonIndexArray", identity()),
            ("m_HumanSkeletonReverseIndexArray", identity()),
            ("m_RootMotionBoneIndex", V::Int(rig.root_motion_bone)),
            ("m_RootMotionBoneX", xform()),
            (
                "m_RootMotionSkeleton",
                data(st(
                    "Skeleton",
                    vec![
                        (
                            "m_Node",
                            vector(
                                st(
                                    "Node",
                                    vec![("m_ParentId", V::Int(-1)), ("m_AxesId", V::Int(-1))],
                                ),
                                vec![],
                            ),
                        ),
                        ("m_ID", uints([])),
                        ("m_AxesArray", vector(axes(), vec![])),
                    ],
                )),
            ),
            ("m_RootMotionSkeletonPose", pose(0)),
            ("m_RootMotionSkeletonIndexArray", ints([])),
        ],
    );
    let mut avatar = Object::new(
        AVATAR,
        90,
        st(
            "Avatar",
            vec![
                ("m_Name", V::Str(b"avatar".to_vec())),
                ("m_Avatar", runtime),
                ("m_TOS", vector(pair(V::UInt(0), V::Str(Vec::new())), tos)),
            ],
        ),
    );
    if rig.hierarchy {
        arrange(rig, &mut avatar.value);
    }
    avatar
}

/// `HumanTrait`'s parent of a `HumanBodyBones` bone the rig can hold.
#[must_use]
pub const fn trait_parent(bone: usize) -> Option<usize> {
    Some(match bone {
        1 | 2 | 7 => 0,
        3 => 1,
        4 => 2,
        5 => 3,
        6 => 4,
        8 => 7,
        54 => 8,
        9 | 11 | 12 => 54,
        10 => 9,
        13 => 11,
        14 => 12,
        15 => 13,
        16 => 14,
        17 => 15,
        18 => 16,
        19 => 5,
        20 => 6,
        21..=23 => 10,
        _ => return None,
    })
}

/// A T-pose position in the Animator frame per `HumanBodyBones` bone,
/// dyadic so every offset between two is exact in f32.
#[must_use]
pub const fn t_pose(bone: usize) -> [f32; 3] {
    match bone {
        0 => [0., 1., 0.],
        1 => [-0.125, 0.875, 0.],
        2 => [0.125, 0.875, 0.],
        3 => [-0.125, 0.5, 0.],
        4 => [0.125, 0.5, 0.],
        5 => [-0.125, 0.125, 0.],
        6 => [0.125, 0.125, 0.],
        7 => [0., 1.125, 0.],
        8 => [0., 1.25, 0.],
        54 => [0., 1.375, 0.],
        9 => [0., 1.5, 0.],
        10 => [0., 1.625, 0.],
        11 => [-0.0625, 1.4375, 0.],
        12 => [0.0625, 1.4375, 0.],
        13 => [-0.1875, 1.4375, 0.],
        14 => [0.1875, 1.4375, 0.],
        15 => [-0.4375, 1.4375, 0.],
        16 => [0.4375, 1.4375, 0.],
        17 => [-0.6875, 1.4375, 0.],
        18 => [0.6875, 1.4375, 0.],
        19 => [-0.125, 0., 0.125],
        20 => [0.125, 0., 0.125],
        21 => [-0.03125, 1.6875, 0.0625],
        22 => [0.03125, 1.6875, 0.0625],
        _ => [0., 1.5625, 0.0625],
    }
}

fn field<'v>(value: &'v mut V, path: &[&str]) -> Option<&'v mut V> {
    path.iter().try_fold(value, |value, name| match value {
        V::Struct(_, fields) => fields.iter_mut().find(|(f, _)| f == name).map(|f| &mut f.1),
        _ => None,
    })
}

fn rows<'v>(value: &'v mut V, path: &[&str]) -> &'v mut [V] {
    match field(value, path) {
        Some(V::Vector(_, _, items)) => items,
        _ => &mut [],
    }
}

/// Puts the Avatar's human skeleton in `HumanTrait`'s hierarchy at
/// `t_pose`: each bone under its nearest present `HumanTrait` ancestor,
/// with the offset between their T-pose positions, and the nodes reordered
/// by depth so each parent precedes its children, as the reader requires.
fn arrange(rig: &Rig, avatar: &mut V) {
    let bones: Vec<usize> = bones(rig).iter().map(|&(bone, _)| bone).collect();
    let node = |bone: usize| bones.iter().position(|&b| b == bone).map(|k| k + 1);
    let parent = |bone: usize| {
        let mut up = trait_parent(bone);
        while let Some(b) = up {
            if node(b).is_some() {
                return Some(b);
            }
            up = trait_parent(b);
        }
        None
    };
    let n = bones.len() + 1;
    let up: Vec<usize> = (0..n)
        .map(|k| match k.checked_sub(1).and_then(|b| bones.get(b)) {
            Some(&bone) => parent(bone).and_then(node).unwrap_or(0),
            None => 0,
        })
        .collect();
    let depth = |mut k: usize| {
        let mut d = 0;
        while k != 0 && d < n {
            k = up[k];
            d += 1;
        }
        d
    };
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by_key(|&k| (depth(k), k));
    let mut moved = vec![0; n];
    for (new, &old) in order.iter().enumerate() {
        moved[old] = new;
    }
    let human = ["m_Avatar", "m_Human", "data"];
    let at = |tail: &[&'static str]| [&human[..], tail].concat();
    let permute = |items: &mut [V]| {
        let old = items.to_vec();
        for (item, &k) in items.iter_mut().zip(&order) {
            *item = old[k].clone();
        }
    };
    let pose = rows(avatar, &at(&["m_SkeletonPose", "data", "m_X"]));
    for (k, &bone) in bones.iter().enumerate() {
        let from = parent(bone).map_or([0.; 3], t_pose);
        let to = t_pose(bone);
        if let Some(x) = pose.get_mut(k + 1) {
            set(x, "t", xyz("float3", &[0, 1, 2].map(|c| to[c] - from[c])));
        }
    }
    permute(pose);
    let nodes = rows(avatar, &at(&["m_Skeleton", "data", "m_Node"]));
    for (k, row) in nodes.iter_mut().enumerate().skip(1) {
        let parent = i32::try_from(moved[up[k]]).unwrap_or_default();
        set(row, "m_ParentId", V::Int(parent));
    }
    permute(nodes);
    permute(rows(avatar, &at(&["m_Skeleton", "data", "m_ID"])));
    for slot in rows(avatar, &at(&["m_HumanBoneIndex"])) {
        if let V::Int(index) = slot
            && let Ok(old) = usize::try_from(*index)
            && let Some(&new) = moved.get(old)
        {
            *index = i32::try_from(new).unwrap_or_default();
        }
    }
}

/// The prefab: `GameObject` 1 holds the Animator; the rig's bone `i` is
/// `GameObject` `i + 2`. Each `Transform` ID is its `GameObject` ID + 100.
#[must_use]
pub fn prefab(rig: &Rig) -> Vec<Object> {
    let bones = bones(rig);
    let mut objects = vec![
        Object::new(1, 1, game_object(b"rig", &[(0, 101), (0, ANIMATOR)])),
        Object::new(101, 4, {
            let mut root = transform((0, 1), (0, 0), &[(0, 102)], false);
            if rig.root_offset {
                set(
                    &mut root,
                    "m_LocalPosition",
                    xyz("Vector3f", &[2., 0.25, -1.]),
                );
            }
            root
        }),
    ];
    let children = |of: usize| -> Vec<_> {
        (1..bones.len())
            .filter(|&i| rig_parent(rig, &bones, i) == Some(of))
            .map(|i| (0, 102 + i64::try_from(i).unwrap_or_default()))
            .collect()
    };
    for (i, &(_, name)) in bones.iter().enumerate() {
        let id = 2 + i64::try_from(i).unwrap_or_default();
        let name = if rig.renamed == Some(i) {
            b"Renamed".as_slice()
        } else {
            name.as_bytes()
        };
        objects.push(Object::new(id, 1, game_object(name, &[(0, id + 100)])));
        let father = rig_parent(rig, &bones, i).map_or(101, |parent| {
            102 + i64::try_from(parent).unwrap_or_default()
        });
        objects.push(Object::new(
            id + 100,
            4,
            transform((0, id), (0, father), &children(i), false),
        ));
    }
    let mut animator = super::animator(ANIMATOR, (0, CONTROLLER));
    set(
        &mut animator.value,
        "m_GameObject",
        pptr("GameObject", (0, 1)),
    );
    set(
        &mut animator.value,
        "m_Avatar",
        pptr("Avatar", (0, if rig.avatar { AVATAR } else { 0 })),
    );
    set(
        &mut animator.value,
        "m_HasTransformHierarchy",
        V::Bool(rig.transform_hierarchy),
    );
    objects.push(animator);
    objects.push(super::controller(CONTROLLER, &[(0, CLIP)], &[0]));
    if rig.avatar {
        objects.push(avatar(rig));
    }
    objects
}

/// A humanoid clip mapping each DOF to the next constant slot, with the
/// neutral settings and root basis unless `settings` overrides one.
#[must_use]
pub fn clip(dofs: &[u32], settings: &[(&str, V)]) -> Object {
    clip_with(dofs, &[], settings, 0, &[])
}

/// A humanoid clip whose DOF bindings are followed by local position
/// bindings on `transforms` (path hashes). The first `streamed` slots are
/// streamed with the given frame words; the rest are constants.
#[must_use]
pub fn clip_with(
    dofs: &[u32],
    transforms: &[u32],
    settings: &[(&str, V)],
    streamed: u32,
    words: &[u32],
) -> Object {
    let slots = dofs.len() + 3 * transforms.len();
    let constants = vec![0_u32; slots.saturating_sub(streamed as usize)];
    clip_stored(
        dofs,
        transforms,
        settings,
        p::Storage {
            interval: [0., 1.],
            streamed_count: streamed,
            words,
            constants: &constants,
            ..p::Storage::default()
        },
    )
}

/// A humanoid clip as `clip_with`, with its slots stored as `storage` says
/// (streamed, then dense, then constant, in binding order).
#[must_use]
pub fn clip_stored(
    dofs: &[u32],
    transforms: &[u32],
    settings: &[(&str, V)],
    storage: p::Storage<'_>,
) -> Object {
    let mut object = p::explicit(CLIP, 30., &[]);
    let mut bindings: Vec<_> = dofs
        .iter()
        .map(|&attribute| Binding {
            class: 95,
            custom: 8,
            attribute,
            ..Binding::default()
        })
        .collect();
    bindings.extend(transforms.iter().map(|&path| Binding {
        path,
        class: 4,
        attribute: 1,
        ..Binding::default()
    }));
    p::add_compiled(&mut object, Layout::Unity2022, &bindings, &[], storage);
    let mut index = vec![-1_i32; 200];
    for (slot, &dof) in dofs.iter().enumerate() {
        if let Some(entry) = usize::try_from(dof).ok().and_then(|dof| index.get_mut(dof)) {
            *entry = i32::try_from(slot).unwrap_or_default();
        }
    }
    let V::Struct(_, root) = &mut object.value else {
        return object;
    };
    let Some((_, muscle)) = root.iter_mut().find(|(name, _)| name == "m_MuscleClip") else {
        return object;
    };
    set(muscle, "m_IndexArray", ints(index));
    set(
        muscle,
        "m_ValueArrayReferencePose",
        vector(V::Float(0.), vec![]),
    );
    for field in [
        "m_Mirror",
        "m_LoopTime",
        "m_LoopBlend",
        "m_LoopBlendOrientation",
        "m_LoopBlendPositionY",
        "m_LoopBlendPositionXZ",
        "m_KeepOriginalOrientation",
        "m_KeepOriginalPositionY",
        "m_KeepOriginalPositionXZ",
        "m_HeightFromFeet",
    ] {
        set(muscle, field, V::Bool(false));
    }
    // Built clips store start-at-origin on, whatever their XZ basis flag.
    set(muscle, "m_StartAtOrigin", V::Bool(true));
    for field in ["m_CycleOffset", "m_Level", "m_OrientationOffsetY"] {
        set(muscle, field, V::Float(0.));
    }
    for (field, value) in settings {
        set(muscle, field, value.clone());
    }
    object
}
