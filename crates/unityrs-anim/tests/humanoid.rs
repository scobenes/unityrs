//! Runtime Avatar tables, the humanoid slot table and the ordered typed
//! refusals of plan section 2.4, on synthetic rigs.
use std::sync::Arc;
use unityrs_anim::{
    AnimError, RigPaths, StorageKind,
    humanoid::{
        ABSENT_SLOT, DOF_COUNT, DofKind, SettingValue, humanoid_refusal, muscle_bone, read_avatar,
        rig_avatar,
    },
    read_clip,
};
use unityrs_core::{
    Bytes, RawStr,
    env::{EnvOptions, Environment, Source},
    files::{File, SerializedFile},
    limits::Meter,
};
use unityrs_scene::{GraphOptions, graph_from_game_object};
use unityrs_testkit::{
    ngui::{Object, V, vector},
    scene3d::{
        self as s, Layout,
        anim::humanoid::{self as h, ANIMATOR, AVATAR, CLIP, Rig},
        set,
    },
};

fn file(layout: Layout, objects: &[Object]) -> (Environment, Arc<SerializedFile>) {
    let env = Environment::new(EnvOptions::unitypy());
    let File::Serialized(file) = env
        .load(Source::Bytes {
            name: RawStr::from_static(b"humanoid-fixture"),
            data: Bytes::from(s::file(layout, &[], objects).unwrap()),
        })
        .unwrap()
    else {
        panic!("serialized humanoid fixture")
    };
    (env, file)
}

/// The refusal for a clip on a rig, as the collector computes it.
fn refusal_on(layout: Layout, rig: &Rig, clip: Object) -> String {
    let mut objects = h::prefab(rig);
    objects.push(clip);
    let (env, file) = file(layout, &objects);
    let meter = Meter::unlimited();
    let graph = graph_from_game_object(
        &env,
        &file.object(1).unwrap(),
        &GraphOptions::default(),
        &meter,
    )
    .unwrap();
    let clip = read_clip(&file.object(CLIP).unwrap(), &meter).unwrap();
    assert!(clip.humanoid().is_some(), "humanoid slot table");
    let avatar = rig_avatar(&env, &file.object(ANIMATOR).unwrap(), &meter).unwrap();
    let paths = RigPaths::new(&graph, 0, &meter).unwrap();
    humanoid_refusal(&clip, &avatar, &paths, &meter).to_string()
}

fn refusal(rig: &Rig, clip: Object) -> String {
    refusal_on(Layout::Unity2022, rig, clip)
}

/// Spine Front-Back, Left-Right and Twist Left-Right.
const SPINE: [u32; 3] = [42, 43, 44];

#[test]
fn reads_the_runtime_avatar_tables_on_both_layouts() {
    for layout in [Layout::Unity2022, Layout::Unity6000] {
        let rig = Rig {
            twist: [0.5, 0.25, 0.75, 0.375],
            ..Rig::default()
        };
        let (_, file) = file(layout, &h::prefab(&rig));
        let avatar = read_avatar(&file.object(AVATAR).unwrap(), &Meter::unlimited()).unwrap();
        let human = &avatar.human;
        assert_eq!(human.skeleton.nodes.len(), 16);
        assert_eq!(human.skeleton.axes.len(), 15);
        assert_eq!(avatar.tos.len(), 16);
        assert_eq!(
            human.twist.fields().map(|(_, v)| v),
            [0.5, 0.25, 0.75, 0.375]
        );
        // Hips, Spine and Head, through the Mecanim body order.
        assert_eq!(human.bone_node(0), Some(1));
        assert_eq!(human.bone_node(7), Some(8));
        assert_eq!(human.bone_node(10), Some(9));
        assert_eq!(human.bone_node(54), None);
        assert_eq!(human.skeleton.axes.as_slice()[0].sign, [1.; 3]);
    }
}

#[test]
fn muscles_rotate_their_human_trait_bones() {
    // Spine, UpperChest, Left Arm Twist In-Out, Left Thumb 1 Stretched,
    // Left Thumb Spread and Right Little 3 Stretched.
    for (muscle, bone) in [(0, 7), (6, 54), (41, 13), (55, 24), (56, 24), (94, 53)] {
        assert_eq!(muscle_bone(muscle), Some(bone));
    }
    assert_eq!(muscle_bone(95), None);
    assert_eq!(DofKind::of(13), Some((DofKind::Body, 6)));
    assert_eq!(DofKind::of(42), Some((DofKind::Muscle, 0)));
    assert_eq!(DofKind::of(199), Some((DofKind::Translation, 62)));
    assert_eq!(DofKind::of(DOF_COUNT), None);
}

#[test]
fn the_slot_table_classifies_every_dof_and_refuses_generic_playback() {
    let mut objects = h::prefab(&Rig::default());
    objects.push(h::clip(&[7, 42, 44], &[]));
    let (_, file) = file(Layout::Unity2022, &objects);
    let meter = Meter::unlimited();
    let clip = read_clip(&file.object(CLIP).unwrap(), &meter).unwrap();
    assert!(matches!(
        clip.issue(),
        Some(AnimError::UnsupportedHumanoidPlayback)
    ));
    let human = clip.humanoid().unwrap();
    let slots: Vec<_> = human.slots().collect();
    assert_eq!(slots.len(), DOF_COUNT);
    let mapped: Vec<_> = slots
        .iter()
        .filter(|slot| slot.storage.is_some())
        .map(|slot| (slot.dof, slot.kind, slot.index_value, slot.storage))
        .collect();
    assert_eq!(
        mapped,
        [
            (7, DofKind::Body, 0, Some((StorageKind::Constant, 0))),
            (42, DofKind::Muscle, 1, Some((StorageKind::Constant, 1))),
            (44, DofKind::Muscle, 2, Some((StorageKind::Constant, 2))),
        ]
    );
    assert!(
        slots
            .iter()
            .filter(|slot| slot.storage.is_none())
            .all(|slot| slot.index_value == ABSENT_SLOT)
    );
    assert_eq!(human.counts().constant, 3);
    let settings = human.settings();
    assert_eq!(settings[2].field, "m_Level");
    assert_eq!(settings[2].value, Some(SettingValue::Float(0.)));
}

#[path = "humanoid/refusals.rs"]
mod refusals;

#[path = "humanoid/body.rs"]
mod body;
#[path = "humanoid/body_model.rs"]
mod body_model;
#[path = "humanoid/body_solve.rs"]
mod body_solve;
#[path = "humanoid/chain.rs"]
mod chain;
#[path = "humanoid/chain_graph.rs"]
mod chain_graph;
#[path = "humanoid/gate.rs"]
mod gate;
#[path = "humanoid/muscle_pose.rs"]
mod muscle_pose;
#[path = "humanoid/root.rs"]
mod root;
#[path = "humanoid/root_drift.rs"]
mod root_drift;

fn muscle_field(clip: &mut Object, name: &str, value: V) {
    let V::Struct(_, root) = &mut clip.value else {
        panic!("clip")
    };
    let muscle = &mut root
        .iter_mut()
        .find(|(field, _)| field == "m_MuscleClip")
        .unwrap()
        .1;
    set(muscle, name, value);
}

fn issue(clip: Object, layout: Layout) -> String {
    let (_, file) = file(layout, &[clip]);
    read_clip(&file.object(CLIP).unwrap(), &Meter::unlimited())
        .unwrap()
        .issue()
        .unwrap()
        .to_string()
}

fn index_array(entries: &[(usize, i32)], len: usize) -> V {
    let mut index = vec![-1; len];
    for &(dof, slot) in entries {
        index[dof] = slot;
    }
    vector(V::Int(0), index.into_iter().map(V::Int).collect())
}

#[test]
fn malformed_slot_tables_refuse_with_their_causes() {
    let mut short = h::clip(&[42], &[]);
    muscle_field(&mut short, "m_IndexArray", index_array(&[(42, 0)], 199));
    assert_eq!(
        issue(short, Layout::Unity2022),
        "unsupported_humanoid_layout"
    );

    let mut shared = h::clip(&[42, 43], &[]);
    muscle_field(
        &mut shared,
        "m_IndexArray",
        index_array(&[(42, 0), (43, 0)], 200),
    );
    assert_eq!(
        issue(shared, Layout::Unity2022),
        "conflicting_human_mapping(43)"
    );

    let mut outside = h::clip(&[42], &[]);
    muscle_field(&mut outside, "m_IndexArray", index_array(&[(42, 1)], 200));
    assert_eq!(
        issue(outside, Layout::Unity2022),
        "conflicting_human_mapping(42)"
    );

    // The binding says DOF 43, the index array maps DOF 42 to its slot.
    let mut swapped = h::clip(&[43], &[]);
    muscle_field(&mut swapped, "m_IndexArray", index_array(&[(42, 0)], 200));
    assert_eq!(
        issue(swapped, Layout::Unity2022),
        "conflicting_human_mapping(43)"
    );

    let mut unknown = h::clip(&[250], &[]);
    muscle_field(&mut unknown, "m_IndexArray", index_array(&[(42, 0)], 200));
    assert_eq!(issue(unknown, Layout::Unity2022), "unknown_human_dof(250)");

    assert_eq!(
        issue(h::clip(&[42], &[]), Layout::Unity2019),
        "unsupported_humanoid_layout"
    );

    // Widths are counted before any slot table is sized: a storage count
    // past the bindings refuses as a slot count, however large.
    assert_eq!(
        issue(h::clip_with(&[42], &[], &[], 2, &[]), Layout::Unity2022),
        "binding_slot_count(1, 2)"
    );
    assert_eq!(
        issue(
            h::clip_with(&[42], &[], &[], u32::MAX, &[]),
            Layout::Unity2022
        ),
        "binding_slot_count(1, 4294967295)"
    );
    // A counted streamed slot needs a key in the serialized frames.
    assert_eq!(
        issue(h::clip_with(&[42], &[], &[], 1, &[]), Layout::Unity2022),
        "missing_streamed_key(0)"
    );
    // A local position row is three slots wide; a DOF mapped onto one of
    // its slots conflicts.
    let mut generic = h::clip_with(&[42], &[h::crc32(b"Hips/Prop")], &[], 0, &[]);
    muscle_field(
        &mut generic,
        "m_IndexArray",
        index_array(&[(42, 0), (43, 2)], 200),
    );
    assert_eq!(
        issue(generic, Layout::Unity2022),
        "conflicting_human_mapping(43)"
    );
}

fn avatar_with(name: &str, value: V) -> Result<(), AnimError> {
    let mut avatar = h::avatar(&Rig::default());
    let V::Struct(_, root) = &mut avatar.value else {
        panic!("avatar")
    };
    let runtime = &mut root
        .iter_mut()
        .find(|(name, _)| name == "m_Avatar")
        .unwrap()
        .1;
    set(runtime, name, value);
    let (_, file) = file(Layout::Unity2022, &[avatar]);
    read_avatar(&file.object(AVATAR).unwrap(), &Meter::unlimited()).map(drop)
}

fn ints(values: impl IntoIterator<Item = i32>) -> V {
    vector(V::Int(0), values.into_iter().map(V::Int).collect())
}

#[test]
fn invalid_avatar_indices_refuse_on_read() {
    assert!(matches!(
        avatar_with("m_RootMotionBoneIndex", V::Int(16)),
        Err(AnimError::InvalidAvatarIndex("m_RootMotionBoneIndex"))
    ));
    // The root-motion tables: an index per root-motion skeleton node.
    assert!(matches!(
        avatar_with("m_RootMotionSkeletonIndexArray", ints([0])),
        Err(AnimError::InvalidAvatarIndex(
            "m_RootMotionSkeletonIndexArray"
        ))
    ));
    // Human node 5 has no Avatar node, yet Avatar node 5 names it.
    assert!(matches!(
        avatar_with(
            "m_HumanSkeletonIndexArray",
            ints((0..16).map(|i| if i == 5 { -1 } else { i }))
        ),
        Err(AnimError::InvalidAvatarIndex("m_HumanSkeletonIndexArray"))
    ));
    // Human node 5 names Avatar node 6, which names human node 6.
    assert!(matches!(
        avatar_with(
            "m_HumanSkeletonIndexArray",
            ints((0..16).map(|i| if i == 5 { 6 } else { i }))
        ),
        Err(AnimError::InvalidAvatarIndex(
            "m_HumanSkeletonReverseIndexArray"
        ))
    ));
}
