//! Freeze v8's body model (`humanoid::body_model`): every constant bound to
//! the freeze's JSON and the reference's source, and the f64 reference's
//! analytic cases (`tools/parity/tests/test_anim_body_model.py`) on
//! synthetic points and on a synthetic Avatar with a humanoid hierarchy.
use super::file;
use serde_json::Value;
use unityrs_anim::{
    AnimError, BodyModel,
    humanoid::{
        BODY_SLOTS, body_avatar,
        body_model::{
            self as m, COEFFICIENTS, FRAME_BONES, LATERAL_LIMIT, LATERAL_MUSCLES, Matrix, Point,
            Points, Quaternion, SUM_ORDER, TWIST_LIMIT, U64,
        },
        muscle_bone, read_avatar, slot_bone,
    },
};
use unityrs_core::limits::Meter;
use unityrs_testkit::{
    ngui::V,
    scene3d::{
        Layout,
        anim::humanoid::{self as h, AVATAR, BONES, OPTIONAL_BONES, Rig},
        set, xyz,
    },
};

pub(super) const FREEZE: &str = include_str!("../../../../tools/parity/anim_body_freeze_v8.json");

pub(super) fn refused(cause: BodyModel) -> String {
    AnimError::UnsupportedHumanoidBodyModel(cause).to_string()
}

/// A result's verdict: `Ok(())` or its error's name.
pub(super) fn verdict<T>(result: Result<T, AnimError>) -> Result<(), String> {
    result.map(drop).map_err(|e| e.to_string())
}

pub(super) fn slot_named(name: &str) -> usize {
    BODY_SLOTS.iter().position(|slot| *slot == name).unwrap()
}

/// The number after `"key": ` at or past `from` in the freeze's text,
/// parsed exactly (`serde_json`'s default parse may round).
pub(super) fn token(key: &str, from: usize) -> f64 {
    let needle = format!("\"{key}\": ");
    let start = from + FREEZE[from..].find(&needle).unwrap() + needle.len();
    let end = start + FREEZE[start..].find([',', '\n', '}']).unwrap();
    FREEZE[start..end].trim().parse().unwrap()
}

/// A Python tuple of names assigned to `name` in a reference source.
fn names(source: &str, name: &str) -> Vec<String> {
    let start = source.find(&format!("\n{name} = (")).unwrap() + name.len() + 5;
    let body = &source[start..start + source[start..].find(')').unwrap()];
    body.split(',')
        .map(|n| n.trim().trim_matches('"').to_owned())
        .filter(|n| !n.is_empty())
        .collect()
}

#[test]
fn every_constant_is_freeze_v8s() {
    let frozen: Value = serde_json::from_str(FREEZE).unwrap();
    let table = frozen["coefficients"].as_object().unwrap();
    let at = FREEZE.find("\"coefficients\": {").unwrap();
    assert_eq!(table.len(), COEFFICIENTS.len());
    for key in table.keys() {
        let (i, j) = key.split_once('>').unwrap();
        let want = token(key, at);
        let got = m::coefficient(&COEFFICIENTS, slot_named(i), slot_named(j));
        assert_eq!(got.map(f64::to_bits), Some(want.to_bits()), "{key}");
    }
    assert_eq!(frozen["bounds"]["rank"], Value::from(COEFFICIENTS.len()));
    assert_eq!(frozen["scope"]["free"], Value::Array(vec![]));
    let bounds = FREEZE.find("\"bounds\": {").unwrap();
    assert_eq!(
        token("twist_limit", bounds).to_bits(),
        TWIST_LIMIT.to_bits()
    );
    assert_eq!(TWIST_LIMIT, 4.25_f64.to_radians());
    assert_eq!(token("unit_roundoff_f64", bounds).to_bits(), U64.to_bits());
    let scope = FREEZE.find("\"scope\": {").unwrap();
    assert_eq!(
        token("lateral_limit", scope).to_bits(),
        LATERAL_LIMIT.to_bits()
    );
    // The sum order and frame bones are the reference's NAMES and
    // FRAME_BONES; the lateral muscles each bone's second (Left-Right).
    let tolerance = include_str!("../../../../tools/parity/anim_body_tolerance.py");
    let order: Vec<_> = SUM_ORDER.iter().map(|&s| BODY_SLOTS[s]).collect();
    assert_eq!(order, names(tolerance, "NAMES"));
    let frame: Vec<_> = FRAME_BONES.iter().map(|&s| BODY_SLOTS[s]).collect();
    assert_eq!(frame, names(tolerance, "FRAME_BONES"));
    let model = include_str!("../../../../tools/parity/anim_body_model.py");
    // The reference's own thresholds, from its source.
    let along = format!("> {} * U64 * norm(v)", m::ALONG_UP);
    assert!(model.contains(&along), "{along}");
    let uniform = format!("> {:e} * max(map(abs, scale))", m::UNIFORM_TOLERANCE);
    assert!(tolerance.contains(&uniform), "{uniform}");
    assert_eq!(
        names(model, "LATERAL_MUSCLES"),
        [
            "Spine Left-Right",
            "Chest Left-Right",
            "UpperChest Left-Right"
        ]
    );
    let bones: Vec<_> = LATERAL_MUSCLES
        .iter()
        .map(|&k| (muscle_bone(k), k % 3))
        .collect();
    let spine = ["Spine", "Chest", "UpperChest"].map(|n| Some(slot_bone(slot_named(n))));
    assert_eq!(bones, spine.map(|bone| (bone, 1)));
}

/// Points at the named slots.
pub(super) fn points(named: &[(&str, Point)]) -> Points {
    let mut out = [None; 25];
    for &(name, p) in named {
        out[slot_named(name)] = Some(p);
    }
    out
}

pub(super) fn close(got: &[f64], want: &[f64], bound: f64) {
    assert_eq!(got.len(), want.len());
    for (a, b) in got.iter().zip(want) {
        assert!((a - b).abs() <= bound, "{got:?} against {want:?}");
    }
}

#[test]
fn the_centre_of_a_single_segment() {
    let p = points(&[("Hips", [0., 1., 0.]), ("Spine", [0., 1.5, 0.])]);
    let mut weights = [0.; 25];
    weights[m::HIPS] = 0.6;
    weights[m::SPINE] = 0.4;
    let mut edges = [None; 25];
    edges[m::SPINE] = Some(m::HIPS);
    // .6 (0, 1, 0) + .4 (0, 1.5, 0) + .25 .6 (0, .5, 0).
    let table = [(m::HIPS, m::SPINE, 0.25)];
    let c = m::centre(&p, &weights, &edges, &table).unwrap();
    close(&c, &[0., 1.275, 0.], 1e-14);
    // Without a coefficient the mass sits at the bone's origin.
    close(
        &m::centre(&p, &weights, &edges, &[]).unwrap(),
        &[0., 1.2, 0.],
        1e-14,
    );
    // An edge whose end has no point has no centre.
    edges[m::NECK] = Some(m::HIPS);
    assert_eq!(
        verdict(m::centre(&p, &weights, &edges, &table)),
        Err(AnimError::InvalidAvatarIndex("m_HumanBoneIndex").to_string())
    );
}

/// The five-bone rig of the reference's frame cases, in its hips frame:
/// `F(T)` is the identity.
pub(super) fn local() -> Points {
    points(&[
        ("Hips", [0., 0., 0.]),
        ("LeftUpperLeg", [-0.1, -0.1, 0.]),
        ("RightUpperLeg", [0.1, -0.1, 0.]),
        ("LeftUpperArm", [-0.2, 0.5, 0.]),
        ("RightUpperArm", [0.2, 0.5, 0.]),
    ])
}

pub(super) fn about(axis: Point, degrees: f64) -> Quaternion {
    let half = degrees.to_radians() / 2.;
    let n = m::norm(axis);
    let (s, c) = half.sin_cos();
    [axis[0] * s / n, axis[1] * s / n, axis[2] * s / n, c]
}

pub(super) fn turned(p: &Points, q: Quaternion, only: impl Fn(usize) -> bool) -> Points {
    let mut out = *p;
    for (slot, point) in out.iter_mut().enumerate() {
        if only(slot) {
            *point = point.map(|v| m::unity_rotate(q, v));
        }
    }
    out
}

pub(super) fn same_rotation(got: Quaternion, want: Quaternion, bound: f64) {
    let sign = if got.iter().zip(&want).map(|(a, b)| a * b).sum::<f64>() < 0. {
        -1.
    } else {
        1.
    };
    close(&got.map(|v| sign * v), &want, bound);
}

#[allow(clippy::many_single_char_names, reason = "the reference's names")]
pub(super) fn matrix(q: Quaternion) -> Matrix {
    let [x, y, z, w] = q;
    [
        [
            1. - 2. * (y * y + z * z),
            2. * (x * y - z * w),
            2. * (x * z + y * w),
        ],
        [
            2. * (x * y + z * w),
            1. - 2. * (x * x + z * z),
            2. * (y * z - x * w),
        ],
        [
            2. * (x * z - y * w),
            2. * (y * z + x * w),
            1. - 2. * (x * x + y * y),
        ],
    ]
}

#[test]
fn the_quaternion_of_a_matrix_on_every_branch() {
    // Near the identity, near half turns about each axis, and a general
    // turn: each of Shepperd's four branches.
    let cases = [
        about([1., -2., 3.], 1e-2),
        about([1., 1e-5, 0.], 179.99),
        about([1e-5, 1., 0.], 179.99),
        about([0., 1e-5, 1.], 179.99),
        about([0.3, -0.5, 0.8], 75.),
    ];
    for q in cases {
        same_rotation(m::quaternion(&matrix(q)).unwrap(), q, 2e-15);
        assert!(m::quaternion(&matrix(q)).unwrap()[3] >= 0.);
    }
    // No direction: a length that overflows, or a non-finite entry.
    let mut huge = matrix([0., 0., 0., 1.]);
    (huge[2][1], huge[1][2]) = (1e300, -1e300);
    assert_eq!(m::quaternion(&huge), None);
    let mut nan = matrix([0., 0., 0., 1.]);
    nan[0][1] = f64::NAN;
    assert_eq!(m::quaternion(&nan), None);
}

#[test]
fn the_frame_of_the_rest_pose_and_a_turned_one() {
    let rest = m::body_frame(&local()).unwrap();
    close(
        rest.as_flattened(),
        &[1., 0., 0., 0., 1., 0., 0., 0., 1.],
        1e-15,
    );
    let turn = about([0.4, -0.7, 0.2], 37.);
    let pose = turned(&local(), turn, |_| true);
    same_rotation(m::body_rotation(&pose, &rest).unwrap(), turn, 1e-14);
}

#[test]
fn the_twist_scope() {
    let rest = m::body_frame(&local()).unwrap();
    let arms = |s| s == m::LEFT_UPPER_ARM || s == m::RIGHT_UPPER_ARM;
    let twisted = |degrees| turned(&local(), about([0., 1., 0.], degrees), arms);
    let four = m::shoulder_hip_twist(&twisted(4.)).unwrap();
    assert!((four.to_degrees() - 4.).abs() < 1e-12, "{four}");
    assert!(m::body_rotation(&twisted(4.), &rest).is_ok());
    for degrees in [5., -5.] {
        assert_eq!(
            verdict(m::body_rotation(&twisted(degrees), &rest)),
            Err(refused(BodyModel::ShoulderHipTwist))
        );
    }
    // A lateral along up has no twist about it: undefined, not zero.
    let mut along = local();
    along[m::LEFT_UPPER_ARM] = Some([0., 0.35, 0.]);
    along[m::RIGHT_UPPER_ARM] = Some([0., 0.65, 0.]);
    assert_eq!(
        verdict(m::shoulder_hip_twist(&along)),
        Err(refused(BodyModel::UndefinedTwist))
    );
    // No up: the arms' and legs' midpoints coincide.
    let mut flat = local();
    flat[m::LEFT_UPPER_ARM] = Some([-0.2, -0.1, 0.]);
    flat[m::RIGHT_UPPER_ARM] = Some([0.2, -0.1, 0.]);
    assert_eq!(
        verdict(m::frame_scope(&flat)),
        Err(refused(BodyModel::UndefinedTwist))
    );
    assert_eq!(
        verdict(m::body_frame(&flat)),
        Err(refused(BodyModel::UndefinedTwist))
    );
    // A frame bone without a point is undefined too.
    let mut missing = local();
    missing[m::RIGHT_UPPER_LEG] = None;
    assert_eq!(
        verdict(m::frame_scope(&missing)),
        Err(refused(BodyModel::UndefinedTwist))
    );
}

#[test]
fn the_spine_lateral_scope() {
    let mut muscles = vec![0.; 95];
    assert_eq!(verdict(m::lateral_scope(&muscles)), Ok(()));
    // Spine Front-Back and Twist are not Left-Right muscles.
    muscles[0] = 0.9;
    muscles[2] = -0.9;
    assert_eq!(verdict(m::lateral_scope(&muscles)), Ok(()));
    for muscle in LATERAL_MUSCLES {
        for (value, ok) in [
            (LATERAL_LIMIT, true),
            (-LATERAL_LIMIT, true),
            (f64::from_bits(LATERAL_LIMIT.to_bits() + 1), false),
            (-0.25, false),
            (f64::NAN, false),
        ] {
            let mut pose = muscles.clone();
            pose[muscle] = value;
            let want = if ok {
                Ok(())
            } else {
                Err(refused(BodyModel::SpineLateralRange))
            };
            assert_eq!(verdict(m::lateral_scope(&pose)), want, "{muscle} {value}");
        }
    }
    // A muscle past the slice's end counts 0.
    assert_eq!(verdict(m::lateral_scope(&[0.5])), Ok(()));
    assert_eq!(m::spine_lateral(&[0., -0.125, 0., 0., 0.0625]), 0.125);
    assert_eq!(
        refused(BodyModel::SpineLateralRange),
        "unsupported_humanoid_body_model(spine_lateral_range)"
    );
}

/// `HumanTrait`'s parent of each `HumanBodyBones` bone the testkit builds.
const fn trait_parent(bone: usize) -> Option<usize> {
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

/// A T-pose position in the Animator frame per `HumanBodyBones` bone.
fn layout(bone: usize) -> Point {
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

/// The rig's bones in its skeleton's node order (node `k + 1` is bone `k`).
fn rig_bones(rig: &Rig) -> Vec<usize> {
    let mut out: Vec<_> = BONES.iter().map(|&(bone, _)| bone).collect();
    out.extend(
        OPTIONAL_BONES
            .iter()
            .map(|&(b, _)| b)
            .filter(|b| rig.optional.contains(b)),
    );
    out
}

pub(super) fn field<'v>(value: &'v mut V, path: &[&str]) -> &'v mut V {
    path.iter().fold(value, |value, name| match value {
        V::Struct(_, fields) => &mut fields.iter_mut().find(|(f, _)| f == name).unwrap().1,
        _ => panic!("{name}"),
    })
}

pub(super) fn rows(value: &mut V) -> &mut Vec<V> {
    match value {
        V::Vector(_, _, items) => items,
        _ => panic!("not a vector"),
    }
}

/// `rig`'s Avatar with `HumanTrait`'s hierarchy over its present bones and
/// `layout`'s T-pose, `scale` on node `scaled` if given.
fn humanoid(rig: &Rig, scaled: Option<(usize, [f32; 3])>) -> unityrs_anim::humanoid::Avatar {
    humanoid_with(rig, scaled, None)
}

/// `humanoid` with bone `moved.0` hung on bone `moved.1` at the same
/// T-pose position.
fn humanoid_with(
    rig: &Rig,
    scaled: Option<(usize, [f32; 3])>,
    moved: Option<(usize, usize)>,
) -> unityrs_anim::humanoid::Avatar {
    let bones = rig_bones(rig);
    let node = |bone: usize| bones.iter().position(|&b| b == bone).map(|k| k + 1);
    let parent = |bone: usize| {
        if let Some((_, to)) = moved.filter(|&(from, _)| from == bone) {
            return Some(to);
        }
        let mut up = trait_parent(bone);
        while let Some(b) = up {
            if node(b).is_some() {
                return Some(b);
            }
            up = trait_parent(b);
        }
        None
    };
    // The testkit's node k + 1 is bone k; the reader wants each parent
    // before its children, so the nodes are reordered by depth.
    let n = bones.len() + 1;
    let up: Vec<usize> = (0..n)
        .map(|k| {
            if k == 0 {
                0
            } else {
                parent(bones[k - 1]).and_then(node).unwrap_or(0)
            }
        })
        .collect();
    let depth = |mut k: usize| {
        let mut d = 0;
        while k != 0 {
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
    let mut avatar = h::avatar(rig);
    let human = ["m_Avatar", "m_Human", "data"];
    let at = |tail: &[&'static str]| [&human[..], tail].concat();
    let permute = |items: &mut Vec<V>| {
        let old = items.clone();
        *items = order.iter().map(|&k| old[k].clone()).collect();
    };
    let pose = rows(field(
        &mut avatar.value,
        &at(&["m_SkeletonPose", "data", "m_X"]),
    ));
    for (k, &bone) in bones.iter().enumerate() {
        let from = parent(bone).map_or([0.; 3], layout);
        #[allow(
            clippy::cast_possible_truncation,
            reason = "dyadic offsets, exact in f32"
        )]
        let t = [0, 1, 2].map(|c| (layout(bone)[c] - from[c]) as f32);
        set(&mut pose[k + 1], "t", xyz("float3", &t));
        if let Some((_, s)) = scaled.filter(|&(at, _)| at == k + 1) {
            set(&mut pose[k + 1], "s", xyz("float3", &s));
        }
    }
    permute(pose);
    let nodes = rows(field(
        &mut avatar.value,
        &at(&["m_Skeleton", "data", "m_Node"]),
    ));
    for k in 1..n {
        let parent = i32::try_from(moved[up[k]]).unwrap();
        set(&mut nodes[k], "m_ParentId", V::Int(parent));
    }
    permute(nodes);
    permute(rows(field(
        &mut avatar.value,
        &at(&["m_Skeleton", "data", "m_ID"]),
    )));
    for slot in rows(field(&mut avatar.value, &at(&["m_HumanBoneIndex"]))) {
        if let V::Int(index) = slot
            && let Ok(old) = usize::try_from(*index)
        {
            *index = i32::try_from(moved[old]).unwrap();
        }
    }
    let (_, file) = file(Layout::Unity2022, &[avatar]);
    read_avatar(&file.object(AVATAR).unwrap(), &Meter::unlimited()).unwrap()
}

pub(super) fn full() -> Rig {
    Rig {
        optional: OPTIONAL_BONES.iter().map(|&(bone, _)| bone).collect(),
        body_offset: [0., 1., 0.],
        ..Rig::default()
    }
}

pub(super) fn without(names: &[&str]) -> Rig {
    let mut rig = full();
    rig.optional
        .retain(|&bone| !names.iter().any(|n| slot_bone(slot_named(n)) == bone));
    rig
}

#[test]
fn the_edges_follow_the_hierarchy_and_link_past_absent_slots() {
    let meter = Meter::unlimited();
    let avatar = humanoid(&full(), None);
    let body = body_avatar(&avatar, &meter).unwrap();
    let edges = body.edges(&meter).unwrap();
    for slot in 0..25 {
        let want = trait_parent(slot_bone(slot))
            .map(|bone| (0..25).find(|&s| slot_bone(s) == bone).unwrap());
        assert_eq!(edges[slot], want, "{}", BODY_SLOTS[slot]);
    }
    // Without UpperChest the Neck and Shoulders link past it to Chest; without
    // the Shoulders the upper arms link to UpperChest.
    let avatar = humanoid(&without(&["UpperChest"]), None);
    let edges = body_avatar(&avatar, &meter).unwrap().edges(&meter).unwrap();
    for slot in [m::NECK, m::LEFT_SHOULDER, m::RIGHT_SHOULDER] {
        assert_eq!(edges[slot], Some(m::CHEST));
    }
    assert_eq!(edges[m::UPPER_CHEST], None);
    let avatar = humanoid(&without(&["LeftShoulder", "RightShoulder"]), None);
    let edges = body_avatar(&avatar, &meter).unwrap().edges(&meter).unwrap();
    assert_eq!(edges[m::LEFT_UPPER_ARM], Some(m::UPPER_CHEST));
    assert_eq!(edges[m::RIGHT_UPPER_ARM], Some(m::UPPER_CHEST));
    // The testkit's flat rig hangs every bone on Hips: Hips>LeftLowerLeg has
    // no frozen coefficient.
    let (_, file) = file(Layout::Unity2022, &[h::avatar(&full())]);
    let flat = read_avatar(&file.object(AVATAR).unwrap(), &meter).unwrap();
    assert_eq!(
        verdict(body_avatar(&flat, &meter).unwrap().edges(&meter)),
        Err(refused(BodyModel::UnfittedEdge))
    );
}

#[test]
fn the_rest_points_weights_and_centre() {
    let meter = Meter::unlimited();
    let rig = full();
    let avatar = humanoid(&rig, None);
    let body = body_avatar(&avatar, &meter).unwrap();
    let rest = body.rest_points(&meter).unwrap();
    for (slot, point) in rest.iter().enumerate() {
        close(&point.unwrap(), &layout(slot_bone(slot)), 1e-6);
    }
    let weights = body.weights();
    close(&[weights.iter().sum::<f64>()], &[1.], 1e-15);
    for slot in 0..25 {
        let leaf = slot >= m::LEFT_EYE;
        assert_eq!(weights[slot] == 0., leaf, "{}", BODY_SLOTS[slot]);
    }
    // The centre against its definition, summed edge by edge from each parent.
    let edges = body.edges(&meter).unwrap();
    let c = m::centre(&rest, &weights, &edges, &COEFFICIENTS).unwrap();
    let mut want = [0.; 3];
    for i in 0..25 {
        let p = rest[i].unwrap();
        let mut point = p;
        for j in (0..25).filter(|&j| edges[j] == Some(i)) {
            let a = m::coefficient(&COEFFICIENTS, i, j).unwrap();
            let q = rest[j].unwrap();
            point = [0, 1, 2].map(|k| point[k] + a * (q[k] - p[k]));
        }
        want = [0, 1, 2].map(|k| want[k] + weights[i] * point[k]);
    }
    close(&c, &want, 1e-14);
    // The T-pose frame: right along +x, up along +y.
    let frame = m::body_frame(&rest).unwrap();
    close(
        frame.as_flattened(),
        &[1., 0., 0., 0., 1., 0., 0., 0., 1.],
        1e-7,
    );
    assert_eq!(verdict(m::frame_scope(&rest)), Ok(()));
    // A nonuniform or nonpositive scale anywhere in the skeleton refuses.
    for s in [[1., 1., 1.001], [-1., -1., -1.]] {
        let avatar = humanoid(&rig, Some((3, s)));
        let body = body_avatar(&avatar, &meter).unwrap();
        assert_eq!(
            verdict(body.rest_points(&meter)),
            Err(AnimError::UnsupportedAnimatorRoot.to_string())
        );
    }
    // A uniform one scales the bones below it.
    let avatar = humanoid(&rig, Some((1, [2., 2., 2.])));
    let scaled = body_avatar(&avatar, &meter)
        .unwrap()
        .rest_points(&meter)
        .unwrap();
    let hips = layout(0);
    let foot = layout(5);
    close(
        &scaled[m::LEFT_FOOT].unwrap(),
        &[0, 1, 2].map(|k| hips[k] + 2. * (foot[k] - hips[k])),
        1e-6,
    );
}

/// A chain of nine `1e38` scales overflows the accumulated f64 scale; a
/// zero translation below it would be `inf * 0`.
#[test]
fn a_non_finite_body_refuses() {
    let meter = Meter::unlimited();
    let mut avatar = h::avatar(&full());
    let human = ["m_Avatar", "m_Human", "data"];
    let nodes = rows(field(
        &mut avatar.value,
        &[&human[..], &["m_Skeleton", "data", "m_Node"]].concat(),
    ));
    for (k, node) in nodes.iter_mut().enumerate().skip(1) {
        set(node, "m_ParentId", V::Int(i32::try_from(k - 1).unwrap()));
    }
    let pose = rows(field(
        &mut avatar.value,
        &[&human[..], &["m_SkeletonPose", "data", "m_X"]].concat(),
    ));
    for x in pose.iter_mut().skip(1).take(9) {
        set(x, "s", xyz("float3", &[1e38; 3]));
    }
    let (_, file) = file(Layout::Unity2022, &[avatar]);
    let avatar = read_avatar(&file.object(AVATAR).unwrap(), &meter).unwrap();
    let body = body_avatar(&avatar, &meter).unwrap();
    assert_eq!(
        verdict(body.rest_points(&meter)),
        Err(refused(BodyModel::NonFiniteBody))
    );
    // A non-finite point or weight refuses in the centre and the frame.
    let mut weights = [0.; 25];
    weights[m::HIPS] = 1.;
    let mut p = local();
    p[m::HIPS] = Some([f64::INFINITY, 0., 0.]);
    let none = [None; 25];
    assert_eq!(
        verdict(m::centre(&p, &weights, &none, &[])),
        Err(refused(BodyModel::NonFiniteBody))
    );
    weights[m::HIPS] = f64::NAN;
    assert_eq!(
        verdict(m::centre(&local(), &weights, &none, &[])),
        Err(refused(BodyModel::NonFiniteBody))
    );
    p[m::LEFT_UPPER_ARM] = Some([f64::NAN, 0., 0.]);
    assert_eq!(
        verdict(m::body_frame(&p)),
        Err(refused(BodyModel::NonFiniteBody))
    );
    assert_eq!(
        verdict(m::frame_scope(&p)),
        Err(refused(BodyModel::NonFiniteBody))
    );
}

#[test]
fn collinear_frame_points_refuse() {
    // Normalizing up leaves a residual of 6 * 0.8 - 8 * 0.6 that would
    // become the forward axis.
    let p = points(&[
        ("LeftUpperLeg", [0., 0., 0.]),
        ("RightUpperLeg", [3., 4., 0.]),
        ("LeftUpperArm", [6., 8., 0.]),
        ("RightUpperArm", [9., 12., 0.]),
    ]);
    assert_eq!(
        verdict(m::body_frame(&p)),
        Err(refused(BodyModel::DegenerateFrame))
    );
    assert_eq!(
        verdict(m::frame_scope(&p)),
        Err(refused(BodyModel::UndefinedTwist))
    );
    // The threshold is a few ulps of the inputs: the five-bone rig is far
    // above it.
    let rest = local();
    let frame = [
        m::LEFT_UPPER_ARM,
        m::RIGHT_UPPER_ARM,
        m::LEFT_UPPER_LEG,
        m::RIGHT_UPPER_LEG,
    ]
    .map(|s| rest[s].unwrap());
    let threshold = m::frame_threshold(&frame, [0.6, 0., 0.], [0., 0.6, 0.]);
    assert!(threshold < 1e-13 && threshold > 0., "{threshold}");
}

#[test]
fn the_rest_frame_must_be_a_rotation() {
    let invalid = Err(refused(BodyModel::InvalidRestFrame));
    let mut nan = m::body_frame(&local()).unwrap();
    nan[1][2] = f64::NAN;
    for rest in [
        nan,
        [[0.; 3]; 3],
        [[1., 0., 0.], [0., 1., 0.], [0., 0., 2.]],
        [[1., 1e-9, 0.], [0., 1., 0.], [0., 0., 1.]],
    ] {
        assert_eq!(verdict(m::rest_frame_scope(&rest)), invalid);
        assert_eq!(verdict(m::body_rotation(&local(), &rest)), invalid);
    }
}

#[test]
fn the_body_rotation_against_a_turned_rest_frame() {
    // F(T) = R0, F(pose) = turn R0: the rotation is turn. A dropped
    // transpose (turn R0 R0) or a reversed product (R0^T turn R0) is not.
    let r0 = about([0.2, 0.9, -0.3], 63.);
    let turn = about([-0.5, 0.1, 0.7], 41.);
    let rest = m::body_frame(&turned(&local(), r0, |_| true)).unwrap();
    close(rest.as_flattened(), matrix(r0).as_flattened(), 1e-15);
    let pose = turned(&local(), m::multiply(turn, r0), |_| true);
    same_rotation(m::body_rotation(&pose, &rest).unwrap(), turn, 1e-14);
}

#[test]
fn the_twist_limit_is_inclusive() {
    let up = f64::from_bits(TWIST_LIMIT.to_bits() + 1);
    let down = f64::from_bits(TWIST_LIMIT.to_bits() - 1);
    assert_eq!(verdict(m::twist_scope(down)), Ok(()));
    assert_eq!(verdict(m::twist_scope(TWIST_LIMIT)), Ok(()));
    assert_eq!(
        verdict(m::twist_scope(up)),
        Err(refused(BodyModel::ShoulderHipTwist))
    );
    assert_eq!(
        verdict(m::twist_scope(f64::NAN)),
        Err(refused(BodyModel::ShoulderHipTwist))
    );
}

/// A zero-mass Eye with an identity rest interposed above Jaw keeps every
/// path and rest position, but the Avatar's chain then has the edge
/// `LeftEye>Jaw`, which the freeze never fitted.
#[test]
fn an_interposed_eye_refuses() {
    let meter = Meter::unlimited();
    let avatar = humanoid_with(&full(), None, Some((23, 21)));
    let body = body_avatar(&avatar, &meter).unwrap();
    let rest = body.rest_points(&meter).unwrap();
    close(&rest[m::JAW].unwrap(), &layout(23), 1e-6);
    assert_eq!(
        verdict(body.edges(&meter)),
        Err(refused(BodyModel::UnfittedEdge))
    );
}
