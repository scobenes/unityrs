//! Freeze v8's chain from the Animator node to the hips (plan 2.3, B4b):
//! the hips parent in `A` from the rig's Transforms, the chain's validated
//! scope (the human scale's range, unit rest scales below the hips, the
//! plain chain or one F5 feature at its fixture value, the placement),
//! and the admission of each against the root configurations.
use super::{
    body_model::{FREEZE, full, local, refused, verdict},
    body_solve::{extra, named_extra},
    file,
    root::variant_clip,
};
use serde_json::Value;
use std::collections::BTreeSet;
use std::sync::Arc;
use unityrs_anim::{
    AnimError, BodyModel, RigPaths,
    humanoid::{
        BodyAvatar, body_avatar,
        body_model::{self as m, Edges},
        body_solve::{self as s, AnimatorBodyPose, HipsParent},
        chain::{
            ANIMATOR_SCALE, AnimatorChain, BETWEEN, CHAIN_VARIANT, ChainFeature, HIPS_SCALE,
            HUMAN_SCALE_EVIDENCE, HUMAN_SCALE_RANGE, OFFSET_PLACEMENT, admit_chain,
        },
        rig_avatar,
        root::{self, ADMITTED},
    },
};
use unityrs_core::limits::Meter;
use unityrs_scene::{GraphOptions, graph_from_game_object, graph_from_scene_file};
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
#[allow(
    clippy::redundant_closure_for_method_calls,
    reason = "the method path is not general over the borrows' lifetimes"
)]
pub(super) fn chain(
    rig: &Rig,
    edit: impl FnOnce(&mut Vec<Object>),
) -> Result<AnimatorChain, AnimError> {
    read(
        rig,
        GraphOptions::default(),
        false,
        edit,
        |body, paths, meter| body.animator_chain(paths, meter),
    )
    .0
}

/// `run` on `rig`'s prefab after `edit`, its graph read with `options`
/// from the prefab root, or with `scene` from the whole file (the rig's
/// root first), and the decode work charged in all and by `run` alone.
pub(super) fn read<R>(
    rig: &Rig,
    options: GraphOptions,
    scene: bool,
    edit: impl FnOnce(&mut Vec<Object>),
    run: impl FnOnce(&BodyAvatar<'_>, &RigPaths<'_>, &Arc<Meter>) -> R,
) -> (R, [u64; 2]) {
    let meter = Meter::unlimited();
    let mut objects = h::prefab(rig);
    edit(&mut objects);
    let (env, file) = file(Layout::Unity2022, &objects);
    let graph = if scene {
        graph_from_scene_file(&env, &file, &options, &meter)
    } else {
        graph_from_game_object(&env, &file.object(1).unwrap(), &options, &meter)
    }
    .unwrap();
    let paths = RigPaths::new(&graph, 0, &meter).unwrap();
    let avatar = rig_avatar(&env, &file.object(ANIMATOR).unwrap(), &meter)
        .unwrap()
        .avatar
        .unwrap();
    let body = body_avatar(&avatar, &meter).unwrap();
    let before = meter.decoded();
    let out = run(&body, &paths, &meter);
    (out, [meter.decoded(), meter.decoded() - before])
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

fn offset(objects: &mut [Object]) {
    local_trs(objects, 101, "m_LocalPosition", &OFFSET_PLACEMENT.0);
    local_trs(objects, 101, "m_LocalRotation", &OFFSET_PLACEMENT.1);
}

pub(super) fn rig(armature: Vec<([f32; 3], [f32; 4], f32)>) -> Rig {
    Rig {
        hierarchy: true,
        armature,
        ..full()
    }
}

/// The testkit rig with F5's `feature` at its fixture value.
fn featured(
    feature: ChainFeature,
    edit: impl FnOnce(&mut Vec<Object>),
) -> Result<AnimatorChain, AnimError> {
    let between = if feature == ChainFeature::Intermediate {
        vec![BETWEEN]
    } else {
        Vec::new()
    };
    chain(&rig(between), |o| {
        match feature {
            ChainFeature::ScaledAnimator => scale(o, 101, [ANIMATOR_SCALE; 3]),
            ChainFeature::HipsScale => scale(o, 102, [HIPS_SCALE; 3]),
            _ => {}
        }
        edit(o);
    })
}

const FEATURES: [ChainFeature; 3] = [
    ChainFeature::ScaledAnimator,
    ChainFeature::Intermediate,
    ChainFeature::HipsScale,
];

/// The plain chain at identity and offset placement, and each F5 feature
/// alone at identity: one node between (its transform the hips parent's,
/// exactly, as the reference's `link` from the identity), a scaled
/// Animator node (which never enters `A`) and a hips scale.
#[test]
fn the_validated_chains_are_in_scope() {
    let plain = chain(&rig(Vec::new()), |_| {}).unwrap();
    assert_eq!(plain.validated(), Some((ChainFeature::Plain, false)));
    assert_eq!(plain.parent(), HipsParent::IDENTITY);
    let placed = chain(&rig(Vec::new()), |o| offset(o)).unwrap();
    assert_eq!(placed.validated(), Some((ChainFeature::Plain, true)));
    for feature in FEATURES {
        let found = featured(feature, |_| {}).unwrap();
        assert_eq!(found.validated(), Some((feature, false)), "{feature:?}");
    }
    let found = featured(ChainFeature::Intermediate, |_| {}).unwrap();
    let (t, q, s) = BETWEEN;
    assert_eq!(found.parent().position, t.map(f64::from));
    assert_eq!(found.parent().rotation, q.map(f64::from));
    assert_eq!(found.parent().scale, f64::from(s));
    let found = featured(ChainFeature::HipsScale, |_| {}).unwrap();
    assert_eq!(found.hips_scale(), f64::from(HIPS_SCALE));
}

/// The plain rig at human scale `h` (`m_Scale` and `m_RootX.t.y`).
fn at_scale(h: f32) -> Option<(ChainFeature, bool)> {
    let rig = Rig {
        scale: h,
        body_offset: [0., h, 0.],
        ..rig(Vec::new())
    };
    chain(&rig, |_| {}).unwrap().validated()
}

/// The human scale's range, its endpoints as literal bits apart from
/// `HUMAN_SCALE_RANGE`: the armature rig's and the large rig's `m_Scale`
/// as freeze v8's training recordings store them. Each endpoint and one
/// ulp inside it are in scope, one ulp outside is not.
#[test]
fn the_human_scale_range_is_the_recorded_endpoints() {
    let plain = Some((ChainFeature::Plain, false));
    let (low, high) = (0x3f47_f9bb_u32, 0x3fb7_a311_u32);
    assert_eq!(HUMAN_SCALE_RANGE.map(f32::to_bits), [low, high]);
    for (bits, inside) in [
        (low - 1, false),
        (low, true),
        (low + 1, true),
        (high - 1, true),
        (high, true),
        (high + 1, false),
    ] {
        let want = if inside { plain } else { None };
        assert_eq!(at_scale(f32::from_bits(bits)), want, "{bits:#x}");
    }
}

/// The rest-scale rule: every Transform below the hips at exactly one, on
/// every component. Each bone of the full rig, the optional ones among
/// them, an ulp off on any one axis, or mirrored, takes the chain out of
/// scope; the hips' own scale is the chain's (`HIPS_SCALE`), not this
/// rule's. The Transforms are `102 + i` for `bones` index `i`, the hips
/// first.
#[test]
fn every_rest_scale_below_the_hips_must_be_one() {
    let full = rig(Vec::new());
    let bones = i64::try_from(h::BONES.len() + full.optional.len()).unwrap();
    assert_eq!(full.optional.len(), 10);
    let plain = chain(&full, |_| {}).unwrap();
    assert_eq!(plain.validated(), Some((ChainFeature::Plain, false)));
    // The last bone's Transform is a Transform, and past it is none: the
    // sweep's range is the rig's.
    let ends = chain(&full, |o| {
        let class = |id| o.iter().find(|o| o.path_id == id).map(|o| o.class_id);
        assert_eq!((class(101 + bones), class(102 + bones)), (Some(4), None));
    });
    assert!(ends.is_ok());
    let mut values = vec![[-1.; 3]];
    for axis in 0..3 {
        for off in [1_f32.next_up(), 1_f32.next_down()] {
            let mut value = [1.; 3];
            value[axis] = off;
            values.push(value);
        }
    }
    for transform in 103..102 + bones {
        for &value in &values {
            let found = chain(&full, |o| scale(o, transform, value));
            assert_eq!(found.unwrap().validated(), None, "{transform} {value:?}");
        }
    }
}

/// scale outside the range (Codex's `m_Scale = m_RootX.t.y = 2`), a rest
/// scale below the hips other than one, an F5 value but the fixture's,
/// two features together, an F5 feature off the identity placement by
/// any amount, or more than one node between.
#[test]
fn chains_out_of_scope_have_none() {
    let none = |found: Result<AnimatorChain, AnimError>| {
        assert_eq!(found.unwrap().validated(), None);
    };
    assert_eq!(at_scale(2.), None);
    // The Spine's rest scale (the eighth bone, Transform 109) of two.
    none(chain(&rig(Vec::new()), |o| scale(o, 109, [2.; 3])));
    none(chain(&rig(Vec::new()), |o| {
        scale(o, 109, [1. + f32::EPSILON; 3]);
    }));
    // An extra Transform at scale 2 below the hips that the Avatar does
    // not name.
    assert_eq!(
        verdict(chain(&rig(Vec::new()), |o| extra(o, 109, 2.))),
        Ok(())
    );
    none(chain(&rig(Vec::new()), |o| extra(o, 109, 2.)));
    // The same, named by a skeleton row outside the hips' subtree.
    none(chain(&rig(Vec::new()), named_extra));
    // A hips scale of 10, an Animator scale of 1.5, another armature.
    none(chain(&rig(Vec::new()), |o| scale(o, 102, [10.; 3])));
    none(chain(&rig(Vec::new()), |o| scale(o, 101, [1.5; 3])));
    let (t, q, _) = BETWEEN;
    none(chain(&rig(vec![(t, q, 0.9)]), |_| {}));
    // Two features together.
    let animator = |o: &mut Vec<Object>| scale(o, 101, [ANIMATOR_SCALE; 3]);
    let hips = |o: &mut Vec<Object>| scale(o, 102, [HIPS_SCALE; 3]);
    none(featured(ChainFeature::Intermediate, animator));
    none(featured(ChainFeature::Intermediate, hips));
    none(featured(ChainFeature::ScaledAnimator, hips));
    // F5 at the offset placement, or 5e-7 off the identity.
    for feature in FEATURES {
        none(featured(feature, |o| offset(o)));
        none(featured(feature, |o| {
            local_trs(o, 101, "m_LocalPosition", &[5e-7, 0., 0.]);
        }));
    }
    none(chain(&rig(Vec::new()), |o| {
        local_trs(o, 101, "m_LocalPosition", &[5e-7, 0., 0.]);
    }));
    // Two nodes between: (0, 1, 0) plus 2 times (1, 0, 0) turned a
    // quarter about up, (0, 0, -1), as the reference's `link` composes.
    let deep = vec![
        ([0., 1., 0.], TURN, 2.),
        ([1., 0., 0.], [0., 0., 0., 1.], 1.),
    ];
    let found = chain(&rig(deep), |_| {}).unwrap();
    let want = [0., 1., -2.];
    let parent = found.parent();
    assert!(
        parent
            .position
            .iter()
            .zip(want)
            .all(|(p, w)| (p - w).abs() < 1e-6)
    );
    assert_eq!(parent.scale, 2.);
    assert_eq!(found.validated(), None);
}

/// A quarter turn about up, as f32 components.
const TURN: [f32; 4] = [0., 0.707_106_77, 0., 0.707_106_77];

/// Nonuniform, nonpositive or underflowing scales, malformed rotations and
/// overflowing links refuse as the chain composes.
#[test]
fn bad_links_refuse() {
    let root = Err(AnimError::UnsupportedAnimatorRoot.to_string());
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
    // Singular or materially non-unit rotations between.
    for q in [
        [0., 0., 0., 0.5],
        [0.; 4],
        [0., 0., 0., 1.001],
        [0.6, 0., 0., 0.8001],
    ] {
        let between = vec![([0., 0.05, 0.1], q, 0.8)];
        assert_eq!(verdict(chain(&rig(between), |_| {})), root, "{q:?}");
    }
    // The hips' own link: a zero or non-unit rotation, a translation
    // that is not finite.
    for q in [[0.; 4], [0., 0., 0., 0.5]] {
        let found = chain(&rig(Vec::new()), |o| {
            local_trs(o, 102, "m_LocalRotation", &q);
        });
        assert_eq!(verdict(found), root, "{q:?}");
    }
    for t in [[f32::NAN, 0., 0.], [0., f32::INFINITY, 0.]] {
        let found = chain(&rig(Vec::new()), |o| {
            local_trs(o, 102, "m_LocalPosition", &t);
        });
        assert_eq!(
            verdict(found),
            Err(refused(BodyModel::NonFiniteBody)),
            "{t:?}"
        );
    }
    // An f32 unit quaternion is a rotation.
    let between = vec![([0., 0.05, 0.1], [-0.707_106_77, 0., 0., 0.707_106_77], 0.8)];
    assert!(chain(&rig(between), |_| {}).is_ok());
    // A composed scale that underflows to zero (eight of 1e-42) or to a
    // subnormal (seven of 1e-44).
    for (depth, tiny) in [(8, 1e-42), (7, 1e-44)] {
        let small = vec![([0.; 3], [0., 0., 0., 1.], tiny); depth];
        assert_eq!(verdict(chain(&rig(small), |_| {})), root, "{depth}");
    }
    // A link that overflows: nine scales of 3e38.
    let huge = vec![([0.; 3], [0., 0., 0., 1.], 3e38); 9];
    assert_eq!(
        verdict(chain(&rig(huge), |_| {})),
        Err(refused(BodyModel::NonFiniteBody))
    );
}

/// The plain chain takes every admitted variant, at both placements; an
/// F5 feature only `CHAIN_VARIANT`, the one E4 passed on it; a chain out
/// of scope none.
#[test]
fn admission_follows_e4s_passing_set() {
    let meter = Meter::unlimited();
    let admitted = |found: &AnimatorChain, name: &str| {
        let settings = root::root_settings(&variant_clip(name), &meter).unwrap();
        verdict(admit_chain(found, &settings))
    };
    let refusal = Err(AnimError::UnsupportedAnimatorRoot.to_string());
    let plain = chain(&rig(Vec::new()), |_| {}).unwrap();
    let placed = chain(&rig(Vec::new()), |o| offset(o)).unwrap();
    let outside = chain(&rig(Vec::new()), |o| scale(o, 102, [10.; 3])).unwrap();
    for row in &ADMITTED {
        assert_eq!(admitted(&plain, row.variant), Ok(()), "{}", row.variant);
        assert_eq!(admitted(&placed, row.variant), Ok(()), "{}", row.variant);
        assert_eq!(admitted(&outside, row.variant), refusal, "{}", row.variant);
        for feature in FEATURES {
            let found = featured(feature, |_| {}).unwrap();
            let want = if row.variant == CHAIN_VARIANT {
                Ok(())
            } else {
                refusal.clone()
            };
            assert_eq!(
                admitted(&found, row.variant),
                want,
                "{feature:?} {}",
                row.variant
            );
        }
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
    let clip = variant_clip(CHAIN_VARIANT);
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

const BUILDER: &str =
    include_str!("../../../../tools/fixtures/unity/Editor/UnityrsBodyFixtures.cs");
const FIXTURE: &str = include_str!("../../../../tools/parity/anim_fixture_body.py");

/// A C# or Python number literal: `1.25f`, `.8`, `-1`.
fn number(text: &str) -> f64 {
    text.trim().trim_end_matches('f').parse().unwrap()
}

/// The number after `key` in `source`, up to `,`, `;`, `)` or `}`.
fn value_after(source: &str, key: &str) -> f64 {
    let at = source.find(key).unwrap_or_else(|| panic!("{key}")) + key.len();
    let end = at + source[at..].find([',', ';', ')', '}']).unwrap();
    number(&source[at..end])
}

/// The arguments of the call after `key`: `new Vector3(0, .05f, .1f)`.
fn arguments_after(source: &str, key: &str) -> Vec<f64> {
    let at = source.find(key).unwrap_or_else(|| panic!("{key}")) + key.len();
    let end = at + source[at..].find(')').unwrap();
    source[at..end].split(',').map(number).collect()
}

/// `Quaternion.Euler` of one nonzero axis, in f32: `(sin, cos)` of half.
fn euler(degrees: &[f64]) -> [f32; 4] {
    let mut q = [0., 0., 0., 1.];
    let axes: Vec<usize> = (0..3).filter(|&k| degrees[k] != 0.).collect();
    assert!(axes.len() <= 1, "{degrees:?}");
    if let Some(&k) = axes.first() {
        let (sin, cos) = (degrees[k].to_radians() / 2.).sin_cos();
        q[k] = sin;
        q[3] = cos;
    }
    #[allow(clippy::cast_possible_truncation, reason = "Unity's f32 quaternion")]
    q.map(|v| v as f32)
}

/// Unity's f32 `Quaternion.Euler` can round a component an ulp away from
/// the f64 rotation rounded; the constants hold the recorded bits.
fn within_an_ulp(computed: [f32; 4], recorded: [f32; 4]) -> bool {
    computed
        .iter()
        .zip(recorded)
        .all(|(&a, b)| a.to_bits().abs_diff(b.to_bits()) <= 1 && a.signum() == b.signum())
}

fn f32s(values: &[f64]) -> Vec<f32> {
    #[allow(clippy::cast_possible_truncation, reason = "f32 fixture values")]
    values.iter().map(|&v| v as f32).collect()
}

/// Each F5 rig's feature, from its `ChainRigs` entry in the builder: a
/// rig whose only change is its size (`large`) is the plain chain.
fn chain_rigs() -> Vec<(String, ChainFeature)> {
    let at = BUILDER
        .find("static readonly BodyRig[] ChainRigs = {")
        .unwrap();
    let block = &BUILDER[at..at + BUILDER[at..].find("};").unwrap()];
    block
        .lines()
        .filter_map(|line| line.trim().strip_prefix("new BodyRig { Name = \""))
        .map(|rest| {
            let (name, rest) = rest.split_once('"').unwrap();
            let feature = if rest.contains("ScaledAnimator = true") {
                ChainFeature::ScaledAnimator
            } else if rest.contains("Armature = true") {
                ChainFeature::Intermediate
            } else if rest.contains("HipsScale = ") {
                ChainFeature::HipsScale
            } else {
                assert!(rest.trim() == ", Size = 1.5f },", "{rest}");
                ChainFeature::Plain
            };
            (name.to_owned(), feature)
        })
        .collect()
}

/// Freeze v8's `e4.passing`, tuple by tuple (variant, rig, Editor version,
/// placement), is the compact rig with every admitted variant at both
/// placements and each F5 rig with `CHAIN_VARIANT` at identity, in each
/// Editor version; each lies in the scope `admit_chain` admits for its
/// rig's feature. Removing or adding one tuple fails here.
#[test]
fn every_passing_tuple_is_freeze_v8s() {
    let frozen: Value = serde_json::from_str(FREEZE).unwrap();
    let passing: BTreeSet<String> = frozen["e4"]["passing"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap().to_owned())
        .collect();
    let versions: Vec<&str> = frozen["scope"]["editor_versions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    let rigs = chain_rigs();
    let named: Vec<&str> = rigs.iter().map(|(name, _)| name.as_str()).collect();
    let listed = FIXTURE
        .split("CHAIN_RIGS = (")
        .nth(1)
        .unwrap()
        .split(')')
        .next()
        .unwrap();
    let listed: Vec<&str> = listed.split(", ").map(|n| n.trim_matches('"')).collect();
    assert_eq!(named, listed);
    let mut expected = BTreeSet::new();
    for version in &versions {
        for row in &ADMITTED {
            for placement in ["identity", "offset"] {
                expected.insert(format!("{}/compact/{version}/{placement}", row.variant));
            }
        }
        for name in &named {
            expected.insert(format!("{CHAIN_VARIANT}/{name}/{version}/identity"));
        }
    }
    assert_eq!(passing, expected);
    // Each tuple within the scope: an F5 feature only with the chain
    // variant, only at identity.
    for tuple in &passing {
        let parts: Vec<&str> = tuple.split('/').collect();
        let feature = rigs
            .iter()
            .find(|(name, _)| name == parts[1])
            .map_or(ChainFeature::Plain, |&(_, feature)| feature);
        if feature != ChainFeature::Plain {
            assert_eq!((parts[0], parts[3]), (CHAIN_VARIANT, "identity"), "{tuple}");
        }
    }
    // The scope's evidence: the training recordings the human scale's range
    // and the unit rest scales were measured on, with both rigs at its ends.
    assert_eq!(frozen["training"]["digest"], HUMAN_SCALE_EVIDENCE);
    let training = frozen["scope"]["training_rigs"].as_array().unwrap();
    for rig in ["armature", "large", "compact"] {
        assert!(training.iter().any(|r| r == rig), "{rig}");
    }
}

/// F5's values, read as values from the builder and the fixture checker.
#[test]
fn the_f5_values_are_the_fixtures() {
    let animator = value_after(BUILDER, "const float AnimatorScale = ");
    let armature = value_after(BUILDER, ", ArmatureScale = ");
    let hips = value_after(BUILDER, "Name = \"hips-scale\", HipsScale = ");
    assert_eq!(
        f32s(&[animator, armature, hips]),
        [ANIMATOR_SCALE, BETWEEN.2, HIPS_SCALE]
    );
    let checks = [
        value_after(FIXTURE, "\"scaled-animator\": near(scale(\"0\"), "),
        value_after(FIXTURE, "near(scale(\"0/0\"), "),
        value_after(FIXTURE, "\"hips-scale\": near(scale(hips), "),
    ];
    assert_eq!(f32s(&checks), [ANIMATOR_SCALE, BETWEEN.2, HIPS_SCALE]);
    let (t, q, _) = BETWEEN;
    let at = arguments_after(BUILDER, "top = Child(root, \"Armature\", new Vector3(");
    assert_eq!(f32s(&at), t);
    let turned = arguments_after(BUILDER, "top.localRotation = Quaternion.Euler(");
    assert!(within_an_ulp(euler(&turned), q), "{turned:?}");
    let placed = arguments_after(BUILDER, "rigObject.transform.localPosition = new Vector3(");
    assert_eq!(f32s(&placed), OFFSET_PLACEMENT.0);
    let turned = arguments_after(
        BUILDER,
        "rigObject.transform.localRotation = Quaternion.Euler(",
    );
    assert!(
        within_an_ulp(euler(&turned), OFFSET_PLACEMENT.1),
        "{turned:?}"
    );
    assert!(FIXTURE.contains(&format!("CHAIN_VARIANT = \"{CHAIN_VARIANT}\"")));
}
