//! Freeze v8's hips solve (`humanoid::body_solve`): the f64 reference's
//! vectors (`tests/support/body_vectors.json`, synthetic rigs, written by
//! `tools/parity/vectors/test_anim_body_vectors.py`), its analytic solve
//! cases, the hips frame of a synthetic Avatar and the solve's refusals.
use super::body_model::{
    FREEZE, about, close, field, full, local, matrix, refused, rows, same_rotation, slot_named,
    turned, verdict, without,
};
use serde_json::Value;
use unityrs_anim::{
    AnimError, BodyModel, RigPaths,
    humanoid::{
        body_avatar,
        body_model::{self as m, COEFFICIENTS, Edges, Matrix, Point, Points, Quaternion},
        body_solve::{self as s, FREE_COMBINATIONS, HipsParent},
        read_avatar, rig_avatar, slot_bone,
    },
};
use unityrs_core::limits::Meter;
use unityrs_scene::{GraphOptions, graph_from_game_object};
use unityrs_testkit::{
    ngui::{Object, V, pptr},
    scene3d::{
        Layout,
        anim::humanoid::{self as h, ANIMATOR, AVATAR, Rig},
        pointers, set, xyz,
    },
};

const VECTORS: &str = include_str!("../support/body_vectors.json");

fn float(v: &Value) -> f64 {
    f64::from_bits(u64::from_str_radix(v.as_str().unwrap(), 16).unwrap())
}

fn floats<const N: usize>(v: &Value) -> [f64; N] {
    let items = v.as_array().unwrap();
    assert_eq!(items.len(), N);
    std::array::from_fn(|k| float(&items[k]))
}

fn named_points(v: &Value) -> Points {
    let mut out = [None; 25];
    for (name, p) in v.as_object().unwrap() {
        out[slot_named(name)] = Some(floats::<3>(p));
    }
    out
}

fn bits<const N: usize>(v: [f64; N]) -> [u64; N] {
    v.map(f64::to_bits)
}

fn frame_bits(v: &Value) -> [[u64; 3]; 3] {
    let rows = v.as_array().unwrap();
    std::array::from_fn(|r| bits(floats::<3>(&rows[r])))
}

/// Every reference vector: the centre, both frames and the hips' local
/// translation bit for bit; the twist (the platform's `atan2`) and the
/// quaternions (normalized by a square root, not `math.hypot`) within
/// four ulps; and the reference's refusals refused.
#[test]
fn the_reference_vectors() {
    let vectors: Value = serde_json::from_str(VECTORS).unwrap();
    assert_eq!(vectors["freeze"], "anim_body_freeze_v8.json");
    let (mut solved, mut refusals) = (0, 0);
    for case in vectors["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let mut weights = [0.; 25];
        for (bone, w) in case["weights"].as_object().unwrap() {
            weights[slot_named(bone)] = float(w);
        }
        let mut edges: Edges = [None; 25];
        for link in case["links"].as_array().unwrap() {
            let [i, j] = [0, 1].map(|k| slot_named(link[k].as_str().unwrap()));
            edges[j] = Some(i);
        }
        let (rest, pose) = (named_points(&case["rest"]), named_points(&case["local"]));
        let want = &case["expect"];
        let rest_frame = m::body_frame(&rest).unwrap();
        assert_eq!(
            rest_frame.map(bits),
            frame_bits(&want["rest_frame"]),
            "{name}"
        );
        let centre = m::centre(&pose, &weights, &edges, &COEFFICIENTS).unwrap();
        assert_eq!(bits(centre), bits(floats::<3>(&want["centre"])), "{name}");
        assert_eq!(
            m::body_frame(&pose).unwrap().map(bits),
            frame_bits(&want["frame"]),
            "{name}"
        );
        let twist = m::shoulder_hip_twist(&pose).unwrap();
        close(&[twist], &[float(&want["twist"])], 4. * twist * 2. * m::U64);
        let parent = HipsParent {
            position: floats(&case["parent"]["position"]),
            rotation: floats(&case["parent"]["rotation"]),
            scale: float(&case["parent"]["scale"]),
        };
        let found = s::solve_hips(
            floats(&case["target"]),
            floats(&case["rotation"]),
            float(&case["human_scale"]),
            &pose,
            &weights,
            &edges,
            &rest_frame,
            &parent,
            float(&case["hips_scale"]),
        );
        if want.get("refused").is_some() {
            assert_eq!(want["refused"], "shoulder_hip_twist");
            assert_eq!(
                verdict(found),
                Err(refused(BodyModel::ShoulderHipTwist)),
                "{name}"
            );
            refusals += 1;
            continue;
        }
        let rotation = m::body_rotation(&pose, &rest_frame).unwrap();
        close(&rotation, &floats::<4>(&want["body_rotation"]), 4. * m::U64);
        let (t, q) = found.unwrap();
        assert_eq!(bits(t), bits(floats::<3>(&want["t"])), "{name}");
        close(&q, &floats::<4>(&want["q"]), 8. * m::U64);
        solved += 1;
    }
    assert_eq!((solved, refusals), (24, 2));
}

/// The reference's five-bone rig (`SolveTests`): masses .2, every bone on
/// the hips with coefficient .5, `F(T)` the identity, `c_h = (0, .24, 0)`.
fn solve(
    rotation: Quaternion,
    parent: &HipsParent,
    hips_scale: f64,
    pose: &Points,
) -> Result<(Point, Quaternion), AnimError> {
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
    s::solve_hips_with(
        &table,
        [0.1, 0.62, -0.05],
        rotation,
        2.,
        pose,
        &weights,
        &edges,
        &m::body_frame(&rest).unwrap(),
        parent,
        hips_scale,
    )
}

const IDENTITY: Quaternion = [0., 0., 0., 1.];

/// Every node's rest scale one.
const ONES: fn(usize) -> Option<[f32; 3]> = |_| Some([1.; 3]);

#[test]
fn the_analytic_solves() {
    let id = HipsParent::IDENTITY;
    // P_A = h T_A - c_h.
    let (t, q) = solve(IDENTITY, &id, 1., &local()).unwrap();
    close(&t, &[0.2, 1., -0.1], 1e-14);
    same_rotation(q, IDENTITY, 1e-15);
    // t = R_y(-90) ((.2, 1, -.1) - (1, 2, 3)), q = R_y(-90).
    let yaw = about([0., 1., 0.], 90.);
    let turned_parent = HipsParent {
        position: [1., 2., 3.],
        rotation: yaw,
        scale: 1.,
    };
    let (t, q) = solve(IDENTITY, &turned_parent, 1., &local()).unwrap();
    close(&t, &[3.1, -1., -0.8], 1e-14);
    same_rotation(q, s::conjugate(yaw), 1e-15);
    // k = 2: P_A = (.2, 1.24 - .48, -.1), t = P_A / 2.
    let scaled = HipsParent { scale: 2., ..id };
    close(
        &solve(IDENTITY, &scaled, 1., &local()).unwrap().0,
        &[0.1, 0.38, -0.05],
        1e-14,
    );
    // k = s_h = 1.5: P_A = (.2, 1.24 - .36, -.1).
    close(
        &solve(IDENTITY, &id, 1.5, &local()).unwrap().0,
        &[0.2, 0.88, -0.1],
        1e-14,
    );
}

/// The pose turns its frame by `R_z(30)` and the body asks for `R_x(40)`:
/// `H_A = R_x(40) R_z(-30)`, not `R_z(-30) R_x(40)`, and placed by the
/// answer the pose's centre is the target and its rotation the request.
#[test]
fn a_noncommuting_solve_places_the_body() {
    let (spin, tilt) = (about([0., 0., 1.], 30.), about([1., 0., 0.], 40.));
    let pose = turned(&local(), spin, |_| true);
    let (t, q) = solve(tilt, &HipsParent::IDENTITY, 1., &pose).unwrap();
    let want = m::multiply(tilt, s::conjugate(spin));
    same_rotation(q, want, 1e-14);
    let reversed = m::multiply(s::conjugate(spin), tilt);
    assert!(q.iter().zip(&reversed).any(|(a, b)| (a - b).abs() > 0.05));
    let (c, sn) = (40f64.to_radians().cos(), 40f64.to_radians().sin());
    close(&t, &[0.2, 1.24 - 0.24 * c, -0.1 - 0.24 * sn], 1e-14);
    let placed = pose.map(|p| p.map(|p| add(t, s::rotate(q, p))));
    let rest = local();
    let mut weights = [0.; 25];
    let mut edges: Edges = [None; 25];
    let mut table = Vec::new();
    for slot in (0..25).filter(|&slot| rest[slot].is_some()) {
        weights[slot] = 0.2;
        if slot != m::HIPS {
            edges[slot] = Some(m::HIPS);
            table.push((m::HIPS, slot, 0.5));
        }
    }
    let centre = m::centre(&placed, &weights, &edges, &table).unwrap();
    close(&centre, &[0.2, 1.24, -0.1], 1e-14);
    let rotation = m::body_rotation(&placed, &m::body_frame(&rest).unwrap()).unwrap();
    same_rotation(rotation, tilt, 1e-14);
}

#[test]
fn the_solve_refuses_what_it_cannot_answer() {
    let id = HipsParent::IDENTITY;
    let root = Err(AnimError::UnsupportedAnimatorRoot.to_string());
    for scale in [0., -1., f64::NAN, f64::INFINITY] {
        let parent = HipsParent { scale, ..id };
        assert_eq!(verdict(solve(IDENTITY, &parent, 1., &local())), root);
        assert_eq!(verdict(solve(IDENTITY, &id, scale, &local())), root);
    }
    let non_finite = Err(refused(BodyModel::NonFiniteBody));
    let parent = HipsParent {
        position: [f64::NAN, 0., 0.],
        ..id
    };
    assert_eq!(verdict(solve(IDENTITY, &parent, 1., &local())), non_finite);
    assert_eq!(
        verdict(solve([0., f64::INFINITY, 0., 1.], &id, 1., &local())),
        non_finite
    );
    // A huge parent rotation with no translation left: its quaternion's
    // squares overflow, so dividing by their root would leave zeros.
    let (p_a, _) = solve(IDENTITY, &id, 1., &local()).unwrap();
    let huge = HipsParent {
        position: p_a,
        rotation: [0., 0., 0., 1e200],
        scale: 1.,
    };
    assert_eq!(
        verdict(solve(IDENTITY, &huge, 1., &local())),
        Err(refused(BodyModel::DegenerateSolve))
    );
    // A subnormal parent scale overflows the translation.
    let tiny = HipsParent {
        scale: 1e-320,
        ..id
    };
    assert_eq!(
        verdict(solve(IDENTITY, &tiny, 1., &local())),
        Err(refused(BodyModel::DegenerateSolve))
    );
    // Arms swung 10 degrees about up from the legs.
    let twisted = turned(&local(), about([0., 1., 0.], 10.), |slot| {
        slot == m::LEFT_UPPER_ARM || slot == m::RIGHT_UPPER_ARM
    });
    assert_eq!(
        verdict(solve(IDENTITY, &id, 1., &twisted)),
        Err(refused(BodyModel::ShoulderHipTwist))
    );
}

#[test]
fn freeze_v8_leaves_no_free_combination() {
    // The freeze's `scope.free` is empty, and its design has full rank.
    let free = FREEZE.find("\"free\": ").unwrap() + 8;
    assert!(
        FREEZE[free..].starts_with("[]"),
        "{}",
        &FREEZE[free..free + 40]
    );
    assert_eq!(FREE_COMBINATIONS.len(), 0);
    let mut edges: Edges = [None; 25];
    for &(i, j, _) in &COEFFICIENTS {
        edges[j] = Some(i);
    }
    assert_eq!(verdict(s::free_scope(&edges)), Ok(()));
}

fn avatar(rig: &Rig) -> unityrs_anim::humanoid::Avatar {
    let (_, file) = super::file(Layout::Unity2022, &[h::avatar(rig)]);
    read_avatar(&file.object(AVATAR).unwrap(), &Meter::unlimited()).unwrap()
}

/// The testkit's hierarchy rig: every bone at its T-pose under its
/// `HumanTrait` parent, the hips at (0, 1, 0).
#[test]
fn the_hips_frame_of_a_hierarchy() {
    let meter = Meter::unlimited();
    for rig in [full(), without(&["UpperChest", "LeftToes"])] {
        let avatar = avatar(&Rig {
            hierarchy: true,
            ..rig
        });
        let body = body_avatar(&avatar, &meter).unwrap();
        let rest = body.rest_points(&meter).unwrap();
        let hips = rest[m::HIPS].unwrap();
        let local = body.hips_points(&|_| IDENTITY, &ONES, &meter).unwrap();
        for (a, b) in local.iter().zip(&rest) {
            assert_eq!(*a, b.map(|b| sub(b, hips)));
        }
        // Solved for a request, the body lands on it.
        let (edges, weights) = (body.edges(&meter).unwrap(), body.weights());
        let rest_frame = m::body_frame(&rest).unwrap();
        let (target, request) = ([0.25, 1., -0.5], about([0.1, 1., 0.2], 70.));
        let (t, q) = s::solve_hips(
            target,
            request,
            1.,
            &local,
            &weights,
            &edges,
            &rest_frame,
            &HipsParent::IDENTITY,
            1.,
        )
        .unwrap();
        let placed = local.map(|p| p.map(|p| add(t, s::rotate(q, p))));
        let centre = m::centre(&placed, &weights, &edges, &COEFFICIENTS).unwrap();
        close(&centre, &target, 1e-14);
        same_rotation(
            m::body_rotation(&placed, &rest_frame).unwrap(),
            request,
            1e-14,
        );
        // The left upper arm's node turned: its arm turns about it, nothing
        // else moves.
        let arm = body.nodes[m::LEFT_UPPER_ARM].unwrap();
        let q = about([0.3, -0.2, 0.9], 50.);
        let posed = body
            .hips_points(&|k| if k == arm { q } else { IDENTITY }, &ONES, &meter)
            .unwrap();
        let pivot = local[m::LEFT_UPPER_ARM].unwrap();
        for (slot, (a, b)) in posed.iter().zip(&local).enumerate() {
            let below = [m::LEFT_LOWER_ARM, m::LEFT_HAND].contains(&slot);
            let want = b.map(|b| {
                if below {
                    add(pivot, m::unity_rotate(q, sub(b, pivot)))
                } else {
                    b
                }
            });
            close(&a.unwrap_or_default(), &want.unwrap_or_default(), 1e-15);
        }
    }
}

/// A turned rest frame against the identity: the matrix's quaternion.
#[test]
fn the_quaternion_matrix_and_product() {
    let (a, b) = (about([0.2, 0.9, -0.3], 63.), about([-0.5, 0.1, 0.7], 41.));
    let (ma, mb): (Matrix, Matrix) = (s::quaternion_matrix(a), s::quaternion_matrix(b));
    close(ma.as_flattened(), matrix(a).as_flattened(), 1e-15);
    let ab = s::product(&ma, &mb);
    close(
        ab.as_flattened(),
        matrix(m::multiply(a, b)).as_flattened(),
        1e-15,
    );
    let v = [0.3, -1.2, 2.];
    close(&s::rotate(a, v), &m::unity_rotate(a, v), 1e-15);
}

fn add(a: Point, b: Point) -> Point {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub(a: Point, b: Point) -> Point {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// The rest scales are the rig's Transforms', through `m_TOS`: one scaled
/// lower arm moves the hand, and a node without a scale refuses.
#[test]
fn the_rest_scales_come_from_the_rig() {
    let meter = Meter::unlimited();
    let rig = Rig {
        hierarchy: true,
        ..full()
    };
    let mut objects = h::prefab(&rig);
    let mut bones = h::BONES
        .iter()
        .map(|&(bone, _)| bone)
        .chain(rig.optional.iter().copied());
    let lower = 102
        + bones
            .position(|bone| bone == slot_bone(m::LEFT_LOWER_ARM))
            .unwrap();
    let lower = i64::try_from(lower).unwrap();
    let transform = objects.iter_mut().find(|o| o.path_id == lower).unwrap();
    set(
        &mut transform.value,
        "m_LocalScale",
        xyz("Vector3f", &[2.; 3]),
    );
    let (env, file) = super::file(Layout::Unity2022, &objects);
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
    let scales = body.rest_scales(&paths, &meter).unwrap();
    let scales = scales.as_slice();
    let arm = body.nodes[m::LEFT_LOWER_ARM].unwrap();
    for (k, scale) in scales.iter().enumerate() {
        assert_eq!(
            *scale,
            Some(if k == arm { [2.; 3] } else { [1.; 3] }),
            "node {k}"
        );
    }
    let rest_scale = |k: usize| scales.get(k).copied().flatten();
    assert_eq!(body.hips_scale(&rest_scale).unwrap(), 1.);
    let local = body
        .hips_points(&|_| IDENTITY, &rest_scale, &meter)
        .unwrap();
    let unscaled = body.hips_points(&|_| IDENTITY, &ONES, &meter).unwrap();
    let (elbow, hand) = (
        unscaled[m::LEFT_LOWER_ARM].unwrap(),
        unscaled[m::LEFT_HAND].unwrap(),
    );
    close(
        &local[m::LEFT_HAND].unwrap(),
        &add(elbow, sub(hand, elbow).map(|c| 2. * c)),
        1e-15,
    );
    // No scale for the lower arm, or for the hips: refused, never one.
    let missing = |k: usize| if k == arm { None } else { Some([1.; 3]) };
    assert_eq!(
        verdict(body.hips_points(&|_| IDENTITY, &missing, &meter)),
        Err(refused(BodyModel::MissingRestScale))
    );
    let hips = body.nodes[m::HIPS].unwrap();
    let no_hips = |k: usize| if k == hips { None } else { Some([1.; 3]) };
    assert_eq!(
        verdict(body.hips_scale(&no_hips)),
        Err(refused(BodyModel::MissingRestScale))
    );
}

/// A Head parented to the Animator node, with no Eyes or Jaw, has no
/// hips-local state: refused, not left out of the centre.
#[test]
fn a_mapped_bone_outside_the_hips_refuses() {
    let meter = Meter::unlimited();
    let rig = Rig {
        hierarchy: true,
        ..without(&["LeftEye", "RightEye", "Jaw"])
    };
    let mut object = h::avatar(&rig);
    let human = ["m_Avatar", "m_Human", "data"];
    let index = rows(field(
        &mut object.value,
        &[&human[..], &["m_HumanBoneIndex"]].concat(),
    ));
    let V::Int(head) = index[11] else {
        panic!("m_HumanBoneIndex")
    };
    let head = usize::try_from(head).unwrap();
    let nodes = rows(field(
        &mut object.value,
        &[&human[..], &["m_Skeleton", "data", "m_Node"]].concat(),
    ));
    set(&mut nodes[head], "m_ParentId", V::Int(0));
    let (_, file) = super::file(Layout::Unity2022, &[object]);
    let avatar = read_avatar(&file.object(AVATAR).unwrap(), &meter).unwrap();
    let body = body_avatar(&avatar, &meter).unwrap();
    assert_eq!(body.nodes[m::HEAD], Some(head));
    assert_eq!(
        verdict(body.hips_points(&|_| IDENTITY, &ONES, &meter)),
        Err(refused(BodyModel::DetachedBone))
    );
}

/// The rest-scale lookup's verdict on a hierarchy rig's prefab as `edit`
/// leaves its objects: the Transforms (`GameObject` `i + 2`'s is `i + 102`,
/// the Animator node's 101) and the Avatar.
fn lookup(edit: impl FnOnce(&mut [Object])) -> Result<(), String> {
    let meter = Meter::unlimited();
    let rig = Rig {
        hierarchy: true,
        ..full()
    };
    let mut objects = h::prefab(&rig);
    edit(&mut objects);
    let (env, file) = super::file(Layout::Unity2022, &objects);
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
    verdict(body.rest_scales(&paths, &meter))
}

/// A rig bone's Transform ID in `h::prefab`.
fn transform_of(bone: usize) -> i64 {
    let index = h::BONES.iter().position(|&(b, _)| b == bone).unwrap();
    102 + i64::try_from(index).unwrap()
}

fn skeleton_ids(objects: &mut [Object]) -> &mut Vec<V> {
    let avatar = objects.iter_mut().find(|o| o.path_id == AVATAR).unwrap();
    rows(field(
        &mut avatar.value,
        &["m_Avatar", "m_Human", "data", "m_Skeleton", "data", "m_ID"],
    ))
}

/// Two skeleton nodes resolving to one Transform refuse, even when one is
/// unmapped: here the Animator node's row repeats the last bone's `m_ID`.
#[test]
fn a_transform_shared_by_two_skeleton_nodes_refuses() {
    assert_eq!(lookup(|_| ()), Ok(()));
    assert_eq!(
        lookup(|objects| {
            let ids = skeleton_ids(objects);
            assert_eq!(ids[0], V::UInt(0));
            ids[0] = ids[ids.len() - 1].clone();
        }),
        Err(refused(BodyModel::SharedTransform))
    );
}

/// A Transform elsewhere under the Animator for a bone the Avatar places
/// below the hips refuses: the left hand's Transform moved onto the
/// Animator node, its `m_TOS` path and `m_ID` following it, while the
/// skeleton keeps it under the lower arm.
#[test]
fn a_transform_outside_its_skeleton_parent_refuses() {
    let (hand, arm) = (transform_of(17), transform_of(15));
    let new = b"LeftHand";
    assert_eq!(
        lookup(|objects| {
            for object in objects.iter_mut() {
                match object.path_id {
                    101 => {
                        let V::Vector(_, _, children) = field(&mut object.value, &["m_Children"])
                        else {
                            panic!("m_Children")
                        };
                        children.push(pptr("Transform", (0, hand)));
                    }
                    id if id == arm => {
                        set(&mut object.value, "m_Children", pointers("Transform", &[]));
                    }
                    id if id == hand => {
                        set(&mut object.value, "m_Father", pptr("Transform", (0, 101)));
                    }
                    _ => {}
                }
            }
            let avatar = objects.iter_mut().find(|o| o.path_id == AVATAR).unwrap();
            let mut old = None;
            for row in rows(field(&mut avatar.value, &["m_TOS"])) {
                if let V::Pair(id, path) = row
                    && let V::Str(text) = &**path
                    && text.ends_with(b"/LeftLowerArm/LeftHand")
                {
                    old = Some((**id).clone());
                    (**id, **path) = (V::UInt(h::crc32(new)), V::Str(new.to_vec()));
                }
            }
            let old = old.expect("the left hand's m_TOS entry");
            let ids = skeleton_ids(objects);
            let row = ids.iter().position(|id| *id == old).unwrap();
            ids[row] = V::UInt(h::crc32(new));
        }),
        Err(refused(BodyModel::MisparentedTransform))
    );
}
