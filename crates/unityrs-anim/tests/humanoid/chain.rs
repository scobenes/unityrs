//! Freeze v8's chain from the Animator node to the hips (plan 2.3, B4b):
//! the hips parent in `A` from the rig's Transforms, the recorded rigs,
//! and the admission of each against the root configurations.
use super::{
    body_model::{FREEZE, full, local, refused, verdict},
    file,
    root::{full_clip, variant_settings},
};
use serde_json::Value;
use std::collections::BTreeSet;
use unityrs_anim::{
    AnimError, BodyModel, RigPaths,
    humanoid::{
        BODY_SLOTS, body_avatar,
        body_model::{self as m, Edges},
        body_solve::{self as s, AnimatorBodyPose, HipsParent},
        chain::{
            AnimatorChain, CHAIN_VARIANT, COMPACT_LAYOUT, OFFSET_PLACEMENT, RECORDED_RIGS,
            RecordedRig, admit_chain,
        },
        rig_avatar,
        root::{self, ADMITTED},
        slot_bone,
    },
};
use unityrs_core::limits::Meter;
use unityrs_scene::{GraphOptions, graph_from_game_object};
use unityrs_testkit::{
    ngui::Object,
    scene3d::{
        Layout,
        anim::humanoid::{self as h, ANIMATOR, Rig},
        set, xyz,
    },
};

/// The chain of `rig`'s prefab after `edit`, which takes the objects: the
/// Animator node's Transform is 101, the hips' 102.
fn chain(rig: &Rig, edit: impl FnOnce(&mut [Object])) -> Result<AnimatorChain, AnimError> {
    let meter = Meter::unlimited();
    let mut objects = h::prefab(rig);
    edit(&mut objects);
    let (env, file) = file(Layout::Unity2022, &objects);
    let graph = graph_from_game_object(
        &env,
        &file.object(1).unwrap(),
        &GraphOptions::default(),
        &meter,
    )
    .unwrap();
    let paths = RigPaths::new(&graph, 0, &meter).unwrap();
    let avatar = rig_avatar(&env, &file.object(ANIMATOR).unwrap(), &meter)
        .unwrap()
        .avatar
        .unwrap();
    let body = body_avatar(&avatar, &meter).unwrap();
    body.animator_chain(&paths, &meter)
}

fn local_trs(objects: &mut [Object], transform: i64, field: &str, value: &[f32]) {
    let node = objects.iter_mut().find(|o| o.path_id == transform).unwrap();
    let kind = if value.len() == 4 {
        "Quaternionf"
    } else {
        "Vector3f"
    };
    set(&mut node.value, field, xyz(kind, value));
}

fn scale(objects: &mut [Object], transform: i64, value: [f32; 3]) {
    local_trs(objects, transform, "m_LocalScale", &value);
}

fn rig(armature: Vec<([f32; 3], [f32; 4], f32)>) -> Rig {
    Rig {
        hierarchy: true,
        armature,
        ..full()
    }
}

/// A `COMPACT_LAYOUT` bone's offset from the hips.
fn below_hips(name: &str) -> [f64; 3] {
    let mut at = COMPACT_LAYOUT.iter().find(|row| row.0 == name).unwrap();
    let mut sum = [0.; 3];
    while !at.1.is_empty() {
        sum = [0, 1, 2].map(|k| sum[k] + at.2[k]);
        at = COMPACT_LAYOUT.iter().find(|row| row.0 == at.1).unwrap();
    }
    sum
}

/// `recorded` as its fixture builds it: the compact layout's bones at
/// `hips` in `A`, `scale` times their offsets, the hips' Transform and the
/// chain's nodes the recorded ones; `edit` then changes the objects.
#[allow(clippy::cast_possible_truncation, reason = "f32 fixture positions")]
fn built(
    recorded: &RecordedRig,
    hips: [f64; 3],
    scale_by: f64,
    stretch: impl Fn(&str) -> f64,
    edit: impl FnOnce(&mut [Object]),
) -> Result<AnimatorChain, AnimError> {
    let positions = COMPACT_LAYOUT
        .iter()
        .map(|&(name, _, _)| {
            let slot = BODY_SLOTS.iter().position(|s| *s == name).unwrap();
            let offset = below_hips(name).map(|v| v * stretch(name));
            let at = [0, 1, 2].map(|k| (hips[k] + scale_by * offset[k]) as f32);
            (slot_bone(slot), at)
        })
        .collect();
    let between = recorded
        .between
        .map(|n| vec![(n.position, n.rotation, n.scale)])
        .unwrap_or_default();
    let rig = Rig {
        positions,
        optional: vec![8, 54, 9, 11, 12, 19, 20],
        ..rig(between)
    };
    chain(&rig, |o| {
        let hips = recorded.hips;
        local_trs(o, 102, "m_LocalPosition", &hips.position);
        local_trs(o, 102, "m_LocalRotation", &hips.rotation);
        scale(o, 102, [hips.scale; 3]);
        scale(o, 101, [recorded.animator_scale; 3]);
        edit(o);
    })
}

/// Each recorded rig built as the fixture does: the hips in `A` and the
/// compact bones' scale below it.
fn as_recorded(name: &str, edit: impl FnOnce(&mut [Object])) -> Result<AnimatorChain, AnimError> {
    let recorded = RECORDED_RIGS.iter().find(|r| r.rig == name).unwrap();
    let (hips, scale_by) = match name {
        "large" => ([0., 1.32, 0.], 1.5),
        "hips-scale" => ([0., 1.056, 0.], 1.2),
        "armature" => ([0., 0.72, 0.], 0.8),
        _ => ([0., 0.88, 0.], 1.),
    };
    built(recorded, hips, scale_by, |_| 1., edit)
}

fn rig_of(found: &AnimatorChain) -> Option<(&'static str, bool)> {
    found.recorded().map(|(rig, offset)| (rig.rig, offset))
}

/// A quarter turn about up, as f32 components.
const TURN: [f32; 4] = [0., 0.707_106_77, 0., 0.707_106_77];
const ROOT: Result<(), String> = Ok(());

#[test]
fn each_recorded_rig_is_named() {
    for recorded in &RECORDED_RIGS {
        let found = as_recorded(recorded.rig, |_| {}).unwrap();
        assert_eq!(rig_of(&found), Some((recorded.rig, false)));
        assert_eq!(found.hips_scale(), f64::from(recorded.hips.scale));
        match recorded.between {
            None => assert_eq!(found.parent(), HipsParent::IDENTITY),
            Some(node) => {
                // One node between: its transform is the hips parent's,
                // exactly, as the reference's `link` from the identity.
                assert_eq!(found.parent().position, node.position.map(f64::from));
                assert_eq!(found.parent().rotation, node.rotation.map(f64::from));
                assert_eq!(found.parent().scale, f64::from(node.scale));
            }
        }
    }
    let found = as_recorded("compact", |o| {
        local_trs(o, 101, "m_LocalPosition", &OFFSET_PLACEMENT.0);
        local_trs(o, 101, "m_LocalRotation", &OFFSET_PLACEMENT.1);
    })
    .unwrap();
    assert_eq!(rig_of(&found), Some(("compact", true)));
}

/// Chains and rigs E4 never passed compose but name no recorded rig: the
/// parameters, the placement and the rest skeleton must all be the
/// fixture's.
#[test]
fn unrecorded_rigs_are_not_named() {
    let unnamed = |found: Result<AnimatorChain, AnimError>| {
        assert_eq!(rig_of(&found.unwrap()), None);
    };
    // The testkit's own geometry, and one bone off the layout.
    unnamed(chain(&rig(Vec::new()), |_| {}));
    let arms = |name: &str| if name.ends_with("Hand") { 1.01 } else { 1. };
    unnamed(built(&RECORDED_RIGS[0], [0., 0.88, 0.], 1., arms, |_| {}));
    // A hips scale of 10, or an Animator scale but the recorded one.
    let ten = RecordedRig {
        hips: unityrs_anim::humanoid::chain::RecordedNode {
            position: [0., 8.8, 0.],
            rotation: [0., 0., 0., 1.],
            scale: 10.,
        },
        ..RECORDED_RIGS[4]
    };
    unnamed(built(&ten, [0., 8.8, 0.], 10., |_| 1., |_| {}));
    unnamed(as_recorded("scaled-animator", |o| scale(o, 101, [1.5; 3])));
    // F5 rigs at the offset placement, which E4 passed only at identity.
    for name in ["scaled-animator", "large", "armature", "hips-scale"] {
        unnamed(as_recorded(name, |o| {
            local_trs(o, 101, "m_LocalPosition", &OFFSET_PLACEMENT.0);
            local_trs(o, 101, "m_LocalRotation", &OFFSET_PLACEMENT.1);
        }));
    }
    // Two features together: a scaled Animator with the armature, a hips
    // scale with it, a scaled Animator with a hips scale.
    unnamed(as_recorded("armature", |o| scale(o, 101, [1.25; 3])));
    unnamed(as_recorded("armature", |o| scale(o, 102, [1.2; 3])));
    unnamed(as_recorded("hips-scale", |o| scale(o, 101, [1.25; 3])));
    // The large rig's hips over the compact skeleton: an unrecorded size.
    unnamed(built(&RECORDED_RIGS[2], [0., 1.32, 0.], 1., |_| 1., |_| {}));
    // The compact rig's Transforms over the large rig's skeleton.
    unnamed(built(
        &RECORDED_RIGS[0],
        [0., 1.32, 0.],
        1.5,
        |_| 1.,
        |_| {},
    ));
}

/// Two nodes compose as the reference's `link` does: the second's
/// translation turned by the first's rotation and scaled by its scale.
#[test]
fn deep_chains_compose_and_bad_links_refuse() {
    let deep = vec![
        ([0., 1., 0.], TURN, 2.),
        ([1., 0., 0.], [0., 0., 0., 1.], 1.),
    ];
    let found = chain(&rig(deep), |_| {}).unwrap();
    let parent = found.parent();
    // (0, 1, 0) plus 2 times (1, 0, 0) turned a quarter about up: (0, 0, -1).
    let want = [0., 1., -2.];
    assert!(
        parent
            .position
            .iter()
            .zip(want)
            .all(|(p, w)| (p - w).abs() < 1e-6)
    );
    assert_eq!(parent.scale, 2.);
    assert_eq!(rig_of(&found), None);
    let root = Err(AnimError::UnsupportedAnimatorRoot.to_string());
    // Nonuniform or nonpositive scales anywhere on the chain.
    let squashed = vec![([0.; 3], [0., 0., 0., 1.], 1.)];
    assert_eq!(
        verdict(chain(&rig(squashed), |o| scale(o, 190, [1., 2., 1.]))),
        root
    );
    assert_eq!(
        verdict(chain(&rig(Vec::new()), |o| scale(o, 101, [1., 1., 2.]))),
        root
    );
    assert_eq!(
        verdict(chain(&rig(Vec::new()), |o| scale(o, 102, [-1.; 3]))),
        root
    );
    // Singular or materially non-unit rotations, between or on the hips.
    for q in [
        [0., 0., 0., 0.5],
        [0.; 4],
        [0., 0., 0., 1.001],
        [0.6, 0., 0., 0.8001],
    ] {
        let between = vec![([0., 0.05, 0.1], q, 0.8)];
        assert_eq!(verdict(chain(&rig(between), |_| {})), root, "{q:?}");
        let hips = |o: &mut [Object]| local_trs(o, 102, "m_LocalRotation", &q);
        assert_eq!(verdict(chain(&rig(Vec::new()), hips)), root, "{q:?}");
    }
    // An f32 unit quaternion is a rotation.
    let between = vec![([0., 0.05, 0.1], [-0.707_106_77, 0., 0., 0.707_106_77], 0.8)];
    assert!(chain(&rig(between), |_| {}).is_ok());
    // A link that overflows refuses as it composes: nine scales of 3e38.
    let huge = vec![([0.; 3], [0., 0., 0., 1.], 3e38); 9];
    assert_eq!(
        verdict(chain(&rig(huge), |_| {})),
        Err(refused(BodyModel::NonFiniteBody))
    );
}

/// The compact rig takes every admitted variant, at both placements;
/// F5's rigs only `CHAIN_VARIANT`, the one E4 passed on them, so `large`
/// with any other clip refuses; an unrecorded rig takes none. A clip with
/// one cell off Original never reaches the chain: `root_settings` refuses
/// it.
#[test]
fn admission_follows_e4s_passing_set() {
    let meter = Meter::unlimited();
    let admitted = |found: &AnimatorChain, name: &str| {
        let clip = full_clip(&variant_settings(name));
        let settings = root::root_settings(&clip, &meter).unwrap();
        verdict(admit_chain(found, &settings))
    };
    let refusal = Err(AnimError::UnsupportedAnimatorRoot.to_string());
    let compact = as_recorded("compact", |_| {}).unwrap();
    let offset = as_recorded("compact", |o| {
        local_trs(o, 101, "m_LocalPosition", &OFFSET_PLACEMENT.0);
        local_trs(o, 101, "m_LocalRotation", &OFFSET_PLACEMENT.1);
    })
    .unwrap();
    let unrecorded = chain(&rig(Vec::new()), |_| {}).unwrap();
    for row in &ADMITTED {
        assert_eq!(admitted(&compact, row.variant), ROOT, "{}", row.variant);
        assert_eq!(admitted(&offset, row.variant), ROOT, "{}", row.variant);
        assert_eq!(
            admitted(&unrecorded, row.variant),
            refusal,
            "{}",
            row.variant
        );
        for name in ["scaled-animator", "large", "armature", "hips-scale"] {
            let found = as_recorded(name, |_| {}).unwrap();
            let want = if row.variant == CHAIN_VARIANT {
                ROOT
            } else {
                refusal.clone()
            };
            assert_eq!(
                admitted(&found, row.variant),
                want,
                "{name} {}",
                row.variant
            );
        }
    }
    for off in [
        "m_KeepOriginalOrientation",
        "m_KeepOriginalPositionY",
        "m_KeepOriginalPositionXZ",
    ] {
        let mut settings = variant_settings(CHAIN_VARIANT);
        settings.retain(|(field, _)| *field != off);
        let clip = full_clip(&settings);
        assert!(root::root_settings(&clip, &meter).is_err(), "{off}");
    }
}

/// Projection and solve end to end at `h = 1.25`: `target_body_pose` owns
/// the human scale and `solve_hips` takes its metres, so `h` applies once.
/// Against the reference's `animator_body` (at `h = 1`) and `solve_hips`
/// (at `h`) under Python 3.15.2, on the reference's five-bone rig
/// (`SolveTests`), a parent at `(1, 2, 3)`, turned `(0, .6, 0, .8)`,
/// scaled 1.5, and a hips scale of 1.2.
#[test]
fn projection_then_solve_applies_the_human_scale_once() {
    let clip = full_clip(&variant_settings(CHAIN_VARIANT));
    let pose: AnimatorBodyPose =
        root::target_body_pose(&clip, 0.5, 1.25, &Meter::unlimited()).unwrap();
    let rest = local();
    let mut weights = [0.; 25];
    let mut edges: Edges = [None; 25];
    let mut table = Vec::new();
    for (slot, point) in rest.iter().enumerate() {
        if point.is_some() {
            weights[slot] = 0.2;
            if slot != m::HIPS {
                edges[slot] = Some(m::HIPS);
                table.push((m::HIPS, slot, 0.5));
            }
        }
    }
    let parent = HipsParent {
        position: [1., 2., 3.],
        rotation: [0., 0.6, 0., 0.8],
        scale: 1.5,
    };
    let (t, q) = s::solve_hips_with(
        &table,
        pose,
        &rest,
        &weights,
        &edges,
        &m::body_frame(&rest).unwrap(),
        &parent,
        1.2,
    )
    .unwrap();
    let want: [u64; 7] = [
        0x400b_90cd_247c_f5e7,
        0xbff3_25fe_dbc9_1e1f,
        0x3fce_18e3_afbc_92eb,
        0x3fb4_5e12_a700_a223,
        0xbfea_211b_4020_2f52,
        0x3fb5_f9b5_d7f3_26b2,
        0x3fe2_16eb_8051_531b,
    ];
    let got: Vec<f64> = t.iter().chain(&q).copied().collect();
    for (g, w) in got.iter().zip(want) {
        assert!(
            (g - f64::from_bits(w)).abs() <= 4e-16,
            "{g} {}",
            f64::from_bits(w)
        );
    }
}

fn euler_x(degrees: f64) -> [f64; 4] {
    let (s, c) = (degrees.to_radians() / 2.).sin_cos();
    [s, 0., 0., c]
}

/// `RECORDED_RIGS` is freeze v8's `e4.passing` (each configuration's rig,
/// variants and placements) with the parameters its fixtures built each
/// with; `COMPACT_LAYOUT` is `rig_layout`'s compact rig. Drift fails here.
#[test]
fn the_recorded_rigs_are_freeze_v8s() {
    let frozen: Value = serde_json::from_str(FREEZE).unwrap();
    let passing: Vec<Vec<&str>> = frozen["e4"]["passing"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap().split('/').collect())
        .collect();
    let rigs: BTreeSet<&str> = passing.iter().map(|c| c[1]).collect();
    let ours: BTreeSet<&str> = RECORDED_RIGS.iter().map(|r| r.rig).collect();
    assert_eq!(rigs, ours);
    let all: BTreeSet<&str> = ADMITTED.iter().map(|r| r.variant).collect();
    for recorded in &RECORDED_RIGS {
        let of = |part: usize| -> BTreeSet<&str> {
            passing
                .iter()
                .filter(|c| c[1] == recorded.rig)
                .map(|c| c[part])
                .collect()
        };
        let variants = of(0);
        if recorded.every_variant {
            assert_eq!(variants, all);
        } else {
            assert_eq!(variants, BTreeSet::from([CHAIN_VARIANT]));
        }
        let places = of(3);
        let want = if recorded.offset {
            BTreeSet::from(["identity", "offset"])
        } else {
            BTreeSet::from(["identity"])
        };
        assert_eq!(places, want, "{}", recorded.rig);
    }
    let fixture = include_str!("../../../../tools/parity/anim_fixture_body.py");
    for line in [
        "CHAIN_VARIANT = \"keep-o1-y1-xz1-motion\"",
        "CHAIN_RIGS = (\"scaled-animator\", \"large\", \"armature\", \"hips-scale\")",
        "\"scaled-animator\": near(scale(\"0\"), 1.25),",
        "\"armature\": nodes.get(\"0/0\", (0, \"\"))[1] == \"Armature\" and near(scale(\"0/0\"), .8) and hips == \"0/0/0\",",
        "\"hips-scale\": near(scale(hips), 1.2),",
        "\"large\": abs(_f32s(rest[nodes[hips][0]][\"translation\"], replay, 3)[1] - 1.32) <= 1e-5,",
        "return all(abs(v - want) <= 1e-6 for v in values)",
    ] {
        assert!(fixture.contains(line), "{line}");
    }
    let builder = include_str!("../../../../tools/fixtures/unity/Editor/UnityrsBodyFixtures.cs");
    for line in [
        "new BodyRig { Name = \"scaled-animator\", ScaledAnimator = true },",
        "new BodyRig { Name = \"large\", Size = 1.5f },",
        "new BodyRig { Name = \"armature\", Armature = true },",
        "new BodyRig { Name = \"hips-scale\", HipsScale = 1.2f },",
        "const float AnimatorScale = 1.25f, ArmatureScale = .8f;",
        "top = Child(root, \"Armature\", new Vector3(0, .05f, .1f));",
        "top.localRotation = Quaternion.Euler(-90, 0, 0);",
        "hips.localPosition = new Vector3(0, .125f, .8375f);",
        "hips.localRotation = Quaternion.Euler(90, 0, 0);",
        "hips.localScale = Vector3.one * rig.HipsScale;",
        "hips.localPosition *= rig.HipsScale;",
        "if (rig.ScaledAnimator) rigObject.transform.localScale = Vector3.one * AnimatorScale;",
        "rigObject.transform.localPosition = new Vector3(2, .25f, -1);",
        "rigObject.transform.localRotation = Quaternion.Euler(0, 35, 0);",
        "nodes.Add(new BodyNode { Name = name, Parent = parent, Offset = offset * rig.Size });",
    ] {
        assert!(builder.contains(line), "{line}");
    }
    let near = |a: f64, b: f64| (a - b).abs() <= 1e-7;
    let f = f64::from;
    let [compact, animator, large, armature, hips_scale] = RECORDED_RIGS;
    let hips_y = COMPACT_LAYOUT[0].2[1];
    assert_eq!(
        (compact.size, animator.animator_scale, large.size),
        (1., 1.25, 1.5)
    );
    assert!(near(f(compact.hips.position[1]), hips_y));
    assert!(near(f(large.hips.position[1]), 1.5 * hips_y));
    assert!(near(f(hips_scale.hips.position[1]), 1.2 * hips_y));
    assert_eq!(hips_scale.hips.scale, 1.2);
    let between = armature.between.unwrap();
    assert_eq!((between.position, between.scale), ([0., 0.05, 0.1], 0.8));
    for (got, want) in [
        (between.rotation, euler_x(-90.)),
        (armature.hips.rotation, euler_x(90.)),
    ] {
        assert!(got.iter().zip(want).all(|(g, w)| near(f(*g), w)));
    }
    assert_eq!(armature.hips.position, [0., 0.125, 0.8375]);
    let (s, c) = (35_f64.to_radians() / 2.).sin_cos();
    let turn = OFFSET_PLACEMENT.1;
    assert!(near(f(turn[1]), s) && near(f(turn[3]), c) && turn[0] == 0. && turn[2] == 0.);
    assert_eq!(OFFSET_PLACEMENT.0, [2., 0.25, -1.]);
}

/// `COMPACT_LAYOUT` is `rig_layout`'s compact rig, and the large rig's
/// size `RIG_SIZES`'.
#[test]
fn the_compact_layout_is_rig_layouts() {
    let layout = include_str!("../../../../tools/parity/anim_fixture_rigs.py");
    for line in [
        "rows = [(\"Hips\", \"\", (0, (1.18 if tall else .88) - (1 - legs) * (1.06 if tall else .73), 0)),",
        "(\"Spine\", \"Hips\", (0, .21 if tall else .16, 0)), (\"Chest\", \"Spine\", (0, .27 if tall else .13, 0))]",
        "rows.append((\"UpperChest\", \"Chest\", (0, .08, 0)))",
        "rows += [(\"Neck\", torso, (0, .14, 0)), (\"Head\", \"Neck\", (0, .12, 0))]",
        "rows += [(side + \"UpperLeg\", \"Hips\", (sign * (.10 if tall else .13), -.05, hip_z)),",
        "(side + \"LowerLeg\", side + \"UpperLeg\", (0, (-.58 if tall else -.36) * legs, .04)),",
        "(side + \"Foot\", side + \"LowerLeg\", (0, (-.48 if tall else -.37) * legs, -.04)),",
        "(side + \"Toes\", side + \"Foot\", (0, -.04, .14)),",
        "(side + \"Shoulder\", torso, (sign * .08, .08, shoulder_z)),",
        "(side + \"UpperArm\", side + \"Shoulder\", (sign * (.10 if tall else .14), 0, 0)),",
        "(side + \"LowerArm\", side + \"UpperArm\", (sign * (.34 if tall else .26) * arms, 0, 0)),",
        "(side + \"Hand\", side + \"LowerArm\", (sign * (.26 if tall else .23) * arms, 0, 0))]",
        "RIG_SIZES = {\"large\": 1.5}",
    ] {
        assert!(layout.contains(line), "{line}");
    }
    let mut want = vec![
        ("Hips", "", [0., 0.88, 0.]),
        ("Spine", "Hips", [0., 0.16, 0.]),
        ("Chest", "Spine", [0., 0.13, 0.]),
        ("UpperChest", "Chest", [0., 0.08, 0.]),
        ("Neck", "UpperChest", [0., 0.14, 0.]),
        ("Head", "Neck", [0., 0.12, 0.]),
    ];
    for (side, sign) in [("Left", -1.), ("Right", 1.)] {
        let name = |bone: &str| -> &'static str { format!("{side}{bone}").leak() };
        want.extend([
            (name("UpperLeg"), "Hips", [sign * 0.13, -0.05, 0.]),
            (name("LowerLeg"), name("UpperLeg"), [0., -0.36, 0.04]),
            (name("Foot"), name("LowerLeg"), [0., -0.37, -0.04]),
            (name("Toes"), name("Foot"), [0., -0.04, 0.14]),
            (name("Shoulder"), "UpperChest", [sign * 0.08, 0.08, 0.]),
            (name("UpperArm"), name("Shoulder"), [sign * 0.14, 0., 0.]),
            (name("LowerArm"), name("UpperArm"), [sign * 0.26, 0., 0.]),
            (name("Hand"), name("LowerArm"), [sign * 0.23, 0., 0.]),
        ]);
    }
    assert_eq!(COMPACT_LAYOUT.to_vec(), want);
}
