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

/// Path IDs: `GameObject` 1 and on, `Transform` 101 and on, then these.
pub const ANIMATOR: i64 = 200;
pub const AVATAR: i64 = 201;
pub const CONTROLLER: i64 = 202;
pub const CLIP: i64 = 203;

/// Rig and Avatar choices. The default is a valid neutral rig.
#[derive(Clone, Debug)]
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

fn path(bone: usize) -> String {
    match bone {
        0 => "Hips".to_owned(),
        _ => format!("Hips/{}", BONES[bone].1),
    }
}

fn ints(items: impl IntoIterator<Item = i32>) -> V {
    vector(V::Int(0), items.into_iter().map(V::Int).collect())
}

fn uints(items: impl IntoIterator<Item = u32>) -> V {
    vector(V::UInt(0), items.into_iter().map(V::UInt).collect())
}

fn xform() -> V {
    st(
        "xform",
        vec![
            ("t", xyz("float3", &[0., 0., 0.])),
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
/// `BONES[i]`. Every bone but those `no_axes` names has an axes record.
fn skeleton(rig: &Rig, axes_rows: bool) -> V {
    let nodes = 1 + BONES.len();
    let node = |parent: i32, axes: i32| {
        st(
            "Node",
            vec![("m_ParentId", V::Int(parent)), ("m_AxesId", V::Int(axes))],
        )
    };
    let mut rows = vec![node(-1, -1)];
    let mut records = Vec::new();
    for bone in 0..BONES.len() {
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
            crc32(path(i - 1).as_bytes())
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
fn human_bones(rig: &Rig) -> V {
    let mut index = [-1_i32; 25];
    for (node, &(bone, _)) in BONES.iter().enumerate() {
        if rig.unmapped_bone != Some(bone) {
            let slot = if bone <= 8 { bone } else { bone + 1 };
            index[slot] = i32::try_from(node + 1).unwrap_or_default();
        }
    }
    ints(index)
}

#[must_use]
pub fn avatar(rig: &Rig) -> Object {
    let nodes = 1 + BONES.len();
    let hand = || data(st("Hand", vec![("m_HandBoneIndex", ints([-1; 15]))]));
    let [arm, fore_arm, upper_leg, leg] = rig.twist;
    let [arm_stretch, leg_stretch, feet] = rig.stretch;
    let human = data(st(
        "Human",
        vec![
            ("m_RootX", xform()),
            ("m_Skeleton", skeleton(rig, true)),
            ("m_SkeletonPose", pose(nodes)),
            ("m_LeftHand", hand()),
            ("m_RightHand", hand()),
            ("m_HumanBoneIndex", human_bones(rig)),
            ("m_Scale", V::Float(1.)),
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
    for bone in 0..BONES.len() {
        if rig.no_tos != Some(bone) {
            let path = path(bone);
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
        ],
    );
    Object::new(
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
    )
}

/// The prefab: `GameObject` 1 holds the Animator; `BONES[i]` is `GameObject`
/// `i + 2`. Each `Transform` ID is its `GameObject` ID + 100.
#[must_use]
pub fn prefab(rig: &Rig) -> Vec<Object> {
    let mut objects = vec![
        Object::new(1, 1, game_object(b"rig", &[(0, 101), (0, ANIMATOR)])),
        Object::new(101, 4, transform((0, 1), (0, 0), &[(0, 102)], false)),
    ];
    let children: Vec<_> = (1..BONES.len())
        .map(|i| (0, 102 + i64::try_from(i).unwrap_or_default()))
        .collect();
    for (i, &(_, name)) in BONES.iter().enumerate() {
        let id = 2 + i64::try_from(i).unwrap_or_default();
        let name = if rig.renamed == Some(i) {
            b"Renamed".as_slice()
        } else {
            name.as_bytes()
        };
        objects.push(Object::new(id, 1, game_object(name, &[(0, id + 100)])));
        let (father, own) = if i == 0 {
            ((0, 101), children.as_slice())
        } else {
            ((0, 102), &[][..])
        };
        objects.push(Object::new(
            id + 100,
            4,
            transform((0, id), father, own, false),
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

/// A humanoid clip mapping each DOF to the next constant slot, with
/// neutral settings unless `settings` overrides one.
#[must_use]
pub fn clip(dofs: &[u32], settings: &[(&str, V)]) -> Object {
    let mut object = p::explicit(CLIP, 30., &[]);
    let bindings: Vec<_> = dofs
        .iter()
        .map(|&attribute| Binding {
            class: 95,
            custom: 8,
            attribute,
            ..Binding::default()
        })
        .collect();
    let constants = vec![0_u32; dofs.len()];
    p::add_compiled(
        &mut object,
        Layout::Unity2022,
        &bindings,
        &[],
        p::Storage {
            interval: [0., 1.],
            constants: &constants,
            ..p::Storage::default()
        },
    );
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
        "m_StartAtOrigin",
    ] {
        set(muscle, field, V::Bool(false));
    }
    for field in ["m_CycleOffset", "m_Level", "m_OrientationOffsetY"] {
        set(muscle, field, V::Float(0.));
    }
    for (field, value) in settings {
        set(muscle, field, value.clone());
    }
    object
}
