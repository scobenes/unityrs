//! The ordered typed refusals, one cause at a time, and the causes that
//! must not fire: twist and stretch on limbs the clip leaves still, a
//! muscle of an absent optional bone, a generic writer off the skeleton.
use super::{SPINE, file, refusal, refusal_on};
use unityrs_anim::{
    humanoid::{dropped_muscles, rig_avatar},
    read_clip,
};
use unityrs_core::limits::Meter;
use unityrs_testkit::{
    ngui::{Object, V, vector},
    scene3d::{
        Layout,
        anim::humanoid::{self as h, ANIMATOR, CLIP, Rig},
    },
};

/// Left Forearm Stretch (`LeftLowerArm`) and Left Hand Down-Up (`LeftHand`).
const FOREARM: u32 = 42 + 42;
const HAND: u32 = 42 + 44;
/// An `UpperChest` muscle, absent from the fixture rig.
const UPPER_CHEST: u32 = 42 + 6;

fn muscles() -> Object {
    h::clip(&SPINE, &[])
}

fn rig(edit: impl FnOnce(&mut Rig)) -> Rig {
    let mut rig = Rig::default();
    edit(&mut rig);
    rig
}

fn flag(name: &'static str, value: bool) -> (&'static str, V) {
    (name, V::Bool(value))
}

#[test]
fn rig_and_slot_refusals_follow_the_module_order() {
    let neutral = Rig::default();
    let cases: Vec<(Rig, Object, &str)> = vec![
        (neutral.clone(), muscles(), "unsupported_humanoid_playback"),
        (
            rig(|r| r.body_offset = [0., 0.9, 0.]),
            muscles(),
            "unsupported_body_motion(hips_correction)",
        ),
        (rig(|r| r.avatar = false), muscles(), "missing_avatar"),
        (
            rig(|r| r.transform_hierarchy = false),
            muscles(),
            "unsupported_humanoid_hierarchy(0)",
        ),
        (
            rig(|r| r.no_tos = Some(3)),
            muscles(),
            "unsupported_humanoid_hierarchy(4)",
        ),
        (
            rig(|r| r.renamed = Some(2)),
            muscles(),
            "unsupported_humanoid_hierarchy(3)",
        ),
        (
            rig(|r| r.unmapped_bone = Some(10)),
            muscles(),
            "missing_human_bone(10)",
        ),
        (
            rig(|r| r.root_offset = true),
            muscles(),
            "unsupported_animator_root",
        ),
        (
            neutral.clone(),
            h::clip_with(&SPINE, &[h::crc32(b"Hips/Spine")], &[], 0, &[]),
            "conflicting_humanoid_writer(3)",
        ),
        (
            neutral.clone(),
            h::clip(&[0, 42], &[]),
            "unsupported_locomotion",
        ),
        (
            rig(|r| r.root_motion_bone = 1),
            muscles(),
            "unsupported_locomotion",
        ),
        (
            neutral.clone(),
            h::clip(&[14, 42], &[]),
            "unsupported_humanoid_ik",
        ),
        (
            neutral.clone(),
            h::clip(&[137], &[]),
            "unsupported_translation_dof",
        ),
        (
            rig(|r| r.translation_dof = true),
            muscles(),
            "unsupported_translation_dof",
        ),
        (
            neutral.clone(),
            h::clip(
                &SPINE,
                &[(
                    "m_ValueArrayReferencePose",
                    vector(V::Float(0.), vec![V::Float(1.)]),
                )],
            ),
            "unsupported_additive_pose",
        ),
        (
            neutral.clone(),
            h::clip(&[42, 13], &[]),
            "unsupported_body_motion(body_slots)",
        ),
        (
            rig(|r| r.no_axes = Some(7)),
            muscles(),
            "unsupported_humanoid_axes(7)",
        ),
    ];
    for (rig, clip, expected) in cases {
        assert_eq!(refusal(&rig, clip), expected);
    }
    assert_eq!(
        refusal_on(Layout::Unity6000, &Rig::default(), muscles()),
        "unsupported_humanoid_playback"
    );
}

#[test]
fn avatar_and_clip_settings_refuse_only_where_they_act() {
    let forearm = rig(|r| r.twist = [0., 0.25, 0., 0.]);
    let arm = rig(|r| r.twist = [0.5, 0., 0., 0.]);
    let stretch = rig(|r| r.stretch = [0.05, 0., 0.]);
    let level = ("m_Level", V::Float(0.125));
    let cases: Vec<(Rig, Object, &str)> = vec![
        // Twist on a limb the clip leaves still does not act.
        (forearm.clone(), muscles(), "unsupported_humanoid_playback"),
        (
            forearm,
            h::clip(&[FOREARM], &[]),
            "unsupported_humanoid_twist(m_ForeArmTwist)",
        ),
        (
            arm.clone(),
            h::clip(&[FOREARM], &[]),
            "unsupported_humanoid_twist(m_ArmTwist)",
        ),
        (arm, h::clip(&[HAND], &[]), "unsupported_humanoid_playback"),
        (stretch.clone(), muscles(), "unsupported_humanoid_playback"),
        (
            stretch,
            h::clip(&[HAND], &[]),
            "unsupported_humanoid_avatar_setting(m_ArmStretch)",
        ),
        // A non-neutral setting refuses before the root basis.
        (
            Rig::default(),
            h::clip(&SPINE, &[flag("m_KeepOriginalPositionY", true), level]),
            "unsupported_humanoid_clip_settings(m_Level)",
        ),
        (
            Rig::default(),
            h::clip(&SPINE, &[flag("m_KeepOriginalPositionY", true)]),
            "unsupported_humanoid_root_basis(m_KeepOriginalPositionY)",
        ),
        (
            Rig::default(),
            h::clip(&SPINE, &[flag("m_StartAtOrigin", false)]),
            "unsupported_humanoid_root_basis(m_StartAtOrigin)",
        ),
        (
            Rig::default(),
            h::clip(
                &SPINE,
                &[
                    flag("m_KeepOriginalPositionXZ", true),
                    flag("m_StartAtOrigin", false),
                ],
            ),
            "unsupported_humanoid_root_basis(m_KeepOriginalPositionXZ)",
        ),
        (
            Rig::default(),
            h::clip(&SPINE, &[flag("m_HeightFromFeet", true)]),
            "unsupported_humanoid_root_basis(m_HeightFromFeet)",
        ),
    ];
    for (rig, clip, expected) in cases {
        assert_eq!(refusal(&rig, clip), expected);
    }
}

#[test]
fn a_generic_writer_off_the_skeleton_is_not_a_conflict() {
    assert_eq!(
        refusal(
            &Rig::default(),
            h::clip_with(&SPINE, &[h::crc32(b"Hips/Prop")], &[], 0, &[])
        ),
        "unsupported_humanoid_playback"
    );
}

#[test]
fn a_muscle_of_an_absent_optional_bone_is_dropped_and_counted() {
    let clip = h::clip(&[42, UPPER_CHEST], &[]);
    let mut objects = h::prefab(&Rig::default());
    objects.push(clip);
    let (env, file) = file(Layout::Unity2022, &objects);
    let meter = Meter::unlimited();
    let clip = read_clip(&file.object(CLIP).unwrap(), &meter).unwrap();
    let avatar = rig_avatar(&env, &file.object(ANIMATOR).unwrap(), &meter).unwrap();
    let human = &avatar.avatar.as_ref().unwrap().human;
    assert_eq!(dropped_muscles(clip.humanoid().unwrap(), human), 1);
    // Dropped, it neither refuses as a missing bone nor needs axes.
    assert_eq!(
        refusal(&Rig::default(), h::clip(&[UPPER_CHEST], &[])),
        "unsupported_humanoid_playback"
    );
    // A required bone left out still refuses.
    assert_eq!(
        refusal(&rig(|r| r.unmapped_bone = Some(7)), muscles()),
        "missing_human_bone(7)"
    );
}

#[test]
fn a_missing_setting_has_no_confirmed_default() {
    for (field, expected) in [
        ("m_Mirror", "unsupported_humanoid_clip_settings(m_Mirror)"),
        (
            "m_StartAtOrigin",
            "unsupported_humanoid_root_basis(m_StartAtOrigin)",
        ),
    ] {
        let mut clip = muscles();
        let V::Struct(_, root) = &mut clip.value else {
            panic!("clip")
        };
        let (_, V::Struct(_, muscle)) = root
            .iter_mut()
            .find(|(name, _)| name == "m_MuscleClip")
            .unwrap()
        else {
            panic!("muscle clip")
        };
        muscle.retain(|(name, _)| name != field);
        assert_eq!(refusal(&Rig::default(), clip), expected);
    }
}
