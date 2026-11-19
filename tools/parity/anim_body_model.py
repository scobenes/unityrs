"""B2's f64 reference of the humanoid body model (docs/plans/humanoid-body.md 2.1 to 2.4).

Every equation is one of the plan's named hypotheses (decision 2), listed in
HYPOTHESES; the freeze artifact records them as its model choices. Values
are f64 from the recorded f32 inputs, taken exactly. Standard library only.

- 2.1, `centre`: c = sum_i m_i (p_i + sum_j a_ij (p_j - p_i)) over the mapped
  bones and their edges (anim_body_tolerance.edges), m_i the Avatar's
  m_HumanBoneMass entry. `columns` is c's derivative in each a_ij, the fit's
  design column.
- 2.2, `body_rotation`: F(pose) F(T)^-1, F from anim_body_tolerance.body_frame.

**Scope (plan section 8, frozen on the training rigs).** `topology_scope`
refuses an Avatar whose Chest or Neck is absent or that maps exactly one
Shoulder: 2.1's per-edge fractions fit every other training rig jointly but
not these. It admits only a recorded pattern of absent body slots
(RECORDED_ABSENT, the in-scope training rigs'), so an unrecorded topology,
such as one mapped eye, refuses too. `frame_scope` refuses a pose whose
shoulder-hip twist (the angle about up between the upper-arm and upper-leg
laterals) exceeds TWIST_LIMIT: there 2.2's lateral sum leaves Unity's yaw
about up by more than the bound. A twist with a lateral along up, or no up,
is undefined and refuses as well. All of them, and solve_hips and
body_rotation through frame_scope, raise `Refused`.
- 2.3, `solve_hips`: H_A = B_A F(T) F_h^-1, P_A = h T_A - k H_A c_h with
  k = sigma_p s_h, t = sigma_p^-1 R_p^-1 (P_A - t_p), q = R_p^-1 H_A, and the
  hips keep their own scale s_h.
- 2.4, `cells`, `project` and `animator_body`: the root X_r = (r, y) per
  cell (bake: the start value; keep original: the original root, MotionT
  and MotionQ, with the level and orientation offset; otherwise the body,
  with them), and X_A = X_r^-1 o X_b: T_A = m_Scale y^-1 (RootT - r),
  B_A = y^-1 RootQ. A yaw is a rotation's twist about the up axis,
  2 atan2(q_y, q_w) (the training fixtures' body-yaw cells match it, not
  the forward's yaw atan2(f_x, f_z)). The m_StartAtOrigin rebase is taken as shared by the root
  and the body, so it cancels and the projection runs in the clip's own
  motion space; E4's start variants test that. Height from feet stays
  undecided (`Undecided`) until F4 tells candidates A and B apart, and an
  original-root cell without motion slots refuses (`Refused`).

**Bounds.** The hips path's f64 bound is anim_body_tolerance.evaluation_error
(E_eval), whose operation counts cover forward kinematics with scale, the
weighted sum, the frame and its normalizations and the solve. For 2.4,
`projection_error` bounds the inverse rigid composition at a unit
roundoff u: the subtraction at |RootT| + |r|, the rotation by y^-1 at its
operands (rotate_rounding), the product by m_Scale at its result, and the
yaw's own error turning the horizontal offset; `yaw_error` bounds a yaw
from a quaternion error dq: (q_y, q_w) moves by at most |dq| plus its
representation, which turns 2 atan2 by at most 2 |d| / (|(q_y, q_w)| - |d|),
plus gamma(2) of the angle, unbounded as (q_y, q_w) vanishes (a half turn
about a horizontal axis). `turn_angle` maps a
bound on how far a rotation turns a unit vector (a 2-norm, which the
Frobenius bounds of anim_body_tolerance dominate) to an angle,
2 asin(d / 2), and to NaN, which no comparison passes, from d >= 2; the angular metric (anim_reference.angle) adds
ANGLE_FLOOR.
"""

import math

from anim_body_tolerance import (
    BODY_SLOTS,
    FRAME_BONES,
    U32,
    U64,
    body_frame,
    compose_rounding,
    conjugate,
    cross,
    gamma,
    norm,
    ordered,
    product,
    quaternion_matrix,
    rotate,
    rotate_rounding,
    sub,
)
from anim_reference import multiply, normalize
from anim_replay import f32_from_bits as f32

HYPOTHESES = {
    "masses": "decision 1: each mapped body slot's Avatar m_HumanBoneMass entry, renormalized over the mapped "
              "slots; E0b compares it with the fold of HumanTrait's own default masses (the recorder's hierarchy "
              "masses less their children's)",
    "mass_points": "2.1 H-pos: a mass point is p_i + sum_j a_ij (p_j - p_i) over the edges to the nearest mapped "
                   "human descendants, one coefficient per edge, left and right separate",
    "body_position": "2.1: bodyPosition = c / m_Scale",
    "frame": "2.2: up = mid(upper arms) - mid(upper legs), lateral = (R - L UpperArm) + (R - L UpperLeg), "
             "forward = lateral x up, right = up x forward; bodyRotation = F(pose) F(T)^-1",
    "solve": "2.3: closed form, H_A = B_A F(T) F_h^-1, P_A = h T_A - k H_A c_h, k = sigma_p s_h",
    "root": "2.4: X_A = X_r^-1 o X_b, the m_StartAtOrigin rebase shared by root and body",
    "yaw": "2.4: a yaw is the twist about up, 2 atan2(q_y, q_w)",
    "height_from_feet": "2.4: undecided between candidates A and B; refused",
    "topology_scope": "section 8: refused when Chest or Neck is absent, exactly one Shoulder is mapped or the "
                      "absent body slots are not a recorded training pattern",
    "frame_scope": "section 8: refused when the shoulder-hip twist about up exceeds 4.25 degrees or is undefined",
}
TWIST_LIMIT = math.radians(4.25)
# The absent body slots of each in-scope training rig (anim_fixture_body.PROBE_RIGS): the only admitted patterns.
RECORDED_ABSENT = frozenset(frozenset(absent) for absent in (
    (), ("Jaw",), ("LeftEye", "RightEye"), ("Jaw", "LeftEye", "RightEye"),
    ("Jaw", "LeftEye", "RightEye", "UpperChest"), ("Jaw", "LeftEye", "RightEye", "LeftToes"),
    ("Jaw", "LeftEye", "RightEye", "RightToes"), ("Jaw", "LeftEye", "RightEye", "LeftToes", "RightToes"),
    ("Jaw", "LeftEye", "RightEye", "LeftShoulder", "RightShoulder"),
    ("Jaw", "LeftEye", "RightEye", "UpperChest", "LeftToes", "RightToes"),
    ("Jaw", "LeftEye", "RightEye", "UpperChest", "LeftToes"), ("Jaw", "LeftEye", "RightEye", "UpperChest", "RightToes")))
ANGLE_FLOOR = gamma(16, U64)


class Refused(ValueError):
    """A configuration the hypothesis refuses (plan section 8)."""


class Undecided(ValueError):
    """A cell whose rule the recordings have not decided yet (plan 2.4)."""


def centre(points, masses, links, coefficients):
    """2.1's c in the frame of `points` (name: position), with `coefficients` by "i>j"."""
    c = [0., 0., 0.]
    for i in ordered(masses):
        for k in range(3):
            c[k] += masses[i] * points[i][k]
    for i, j in links:
        a = coefficients.get(f"{i}>{j}", 0.)
        for k in range(3):
            c[k] += a * masses[i] * (points[j][k] - points[i][k])
    return tuple(c)


def columns(points, masses, links):
    """dc/da_ij for each edge: m_i (p_j - p_i)."""
    return {f"{i}>{j}": tuple(masses[i] * (b - a) for a, b in zip(points[i], points[j])) for i, j in links}


def quaternion(m):
    """A rotation matrix's unit quaternion (x, y, z, w), w >= 0, by Shepperd's method: the largest of
    the four components from the diagonal, the rest from off-diagonal sums and differences, so no
    component comes from the square root of a cancelled difference."""
    trace = m[0][0] + m[1][1] + m[2][2]
    pick = max(range(4), key=lambda k: trace if k == 3 else m[k][k])
    if pick == 3:
        s = 2 * math.sqrt(max(0., 1 + trace))
        q = ((m[2][1] - m[1][2]) / s, (m[0][2] - m[2][0]) / s, (m[1][0] - m[0][1]) / s, s / 4)
    else:
        i, j, k = pick, (pick + 1) % 3, (pick + 2) % 3
        s = 2 * math.sqrt(max(0., 1 + m[i][i] - m[j][j] - m[k][k]))
        q = [0.] * 4
        q[i], q[j], q[k], q[3] = s / 4, (m[j][i] + m[i][j]) / s, (m[k][i] + m[i][k]) / s, (m[k][j] - m[j][k]) / s
    q = normalize(q)
    return tuple(-v for v in q) if q[3] < 0 else q


def topology_scope(mapped):
    """Refuses the topologies outside the frozen scope; `mapped` holds the mapped body slots."""
    if "Chest" not in mapped:
        raise Refused("topology: Chest absent")
    if "Neck" not in mapped:
        raise Refused("topology: Neck absent")
    if ("LeftShoulder" in mapped) != ("RightShoulder" in mapped):
        raise Refused("topology: exactly one Shoulder")
    absent = BODY_SLOTS - set(mapped)
    if absent not in RECORDED_ABSENT:
        raise Refused(f"topology: unrecorded pattern (absent {sorted(absent)})")


def shoulder_hip_twist(points):
    """The angle about 2.2's up between the upper-arm and the upper-leg laterals (radians, unsigned)."""
    ul, ur, ll, lr = (points[name] for name in FRAME_BONES)
    up = sub(tuple(a + b for a, b in zip(ul, ur)), tuple(a + b for a, b in zip(ll, lr)))
    if not norm(up) > 0:
        raise Refused("frame: undefined shoulder-hip twist (no up)")
    up = tuple(v / norm(up) for v in up)

    def flat(v):
        along = sum(a * b for a, b in zip(v, up))
        moved = tuple(a - along * b for a, b in zip(v, up))
        # A lateral along up has no direction about it: the twist is undefined.
        if not norm(moved) > 64 * U64 * norm(v):
            raise Refused("frame: undefined shoulder-hip twist (a lateral along up)")
        return moved

    top, bottom = flat(sub(ur, ul)), flat(sub(lr, ll))
    return abs(math.atan2(sum(a * b for a, b in zip(cross(bottom, top), up)), sum(a * b for a, b in zip(bottom, top))))


def frame_scope(points):
    """Refuses a pose outside 2.2's frozen twist scope."""
    twist = shoulder_hip_twist(points)
    if not twist <= TWIST_LIMIT:
        raise Refused(f"frame: shoulder-hip twist {math.degrees(twist):.3f} deg over {math.degrees(TWIST_LIMIT)} deg")


def body_rotation(points, rest_frame):
    """2.2: F(pose) F(T)^-1 as a quaternion, F(T) given as its matrix."""
    frame_scope(points)
    return quaternion(product(body_frame(points), rest_frame, transpose=True))


def solve_hips(target, rotation, human_scale, local_points, masses, links, coefficients, rest_frame, parent,
               hips_scale):
    """2.3: the hips' local translation and rotation placing the body at (T_A, B_A) = (target, rotation).

    `local_points` are the mapped bones in the hips frame (hips at the origin, identity, unit scale);
    `parent` has the hips parent's position, rotation and uniform scale in A."""
    frame_scope(local_points)
    c_h = centre(local_points, masses, links, coefficients)
    solved = product(product(quaternion_matrix(rotation), rest_frame), body_frame(local_points), transpose=True)
    k = parent.scale * hips_scale
    turned = [sum(solved[r][c] * c_h[c] for c in range(3)) for r in range(3)]
    p_a = tuple(human_scale * t - k * h for t, h in zip(target, turned))
    inverse = conjugate(parent.rotation)
    t = tuple(v / parent.scale for v in rotate(inverse, sub(p_a, parent.position)))
    return t, normalize(multiply(inverse, quaternion(solved)))


def compose(points, rotation, position, scale=1.):
    """Points turned by `rotation`, scaled and moved to `position`: a pose placed by its hips."""
    return {name: tuple(a + scale * b for a, b in zip(position, rotate(rotation, p))) for name, p in points.items()}


# 2.4: the root projection.

def flag(value):
    if type(value) is bool or value in (0, 1):
        return bool(value)
    raise ValueError(f"serialized clip flag {value!r} is not a boolean")


def cells(serialized):
    """Each root component's rule from the built clip's m_MuscleClip fields: bake, original, body or feet."""
    if not flag(serialized.get("m_StartAtOrigin")):
        raise Refused("m_StartAtOrigin off: no recording covers it")
    rules = {}
    for cell, bake, keep in (("yaw", "m_LoopBlendOrientation", "m_KeepOriginalOrientation"),
                             ("height", "m_LoopBlendPositionY", "m_KeepOriginalPositionY"),
                             ("xz", "m_LoopBlendPositionXZ", "m_KeepOriginalPositionXZ")):
        rules[cell] = "bake" if flag(serialized[bake]) else "original" if flag(serialized[keep]) else "body"
    if rules["height"] == "body" and flag(serialized["m_HeightFromFeet"]):
        rules["height"] = "feet"
    return rules


def about_up(radians):
    return (0., math.sin(radians / 2), 0., math.cos(radians / 2))


def yaw(q):
    """The twist of q about the up axis (swing-twist), in radians."""
    if yaw_error(q) == math.inf:
        raise Refused("a half turn about a horizontal axis: no yaw")
    return 2 * math.atan2(q[1], q[3])


def project(rules, body, start, original=None, level=0., offset_degrees=0.):
    """The root (r, y) at one sample. `body` and `start` are (RootT, RootQ) at the sample and at the clip's
    start; `original` is (MotionT, MotionQ) at the sample, or None when the clip has no motion slots."""
    def source(rule):
        if rule == "original" and original is None:
            raise Refused("an original-root cell needs the clip's MotionT and MotionQ slots")
        return {"bake": start, "original": original, "body": body}[rule]

    if rules["height"] == "feet":
        raise Undecided("height from feet: lowest-foot candidates A and B are not yet told apart")
    offset = 0. if rules["yaw"] == "bake" else math.radians(offset_degrees)
    lift = 0. if rules["height"] == "bake" else level
    angle = yaw(source(rules["yaw"])[1]) + offset
    xz, height = source(rules["xz"])[0], source(rules["height"])[0]
    return (xz[0], height[1] + lift, xz[2]), about_up(angle)


def animator_body(body, root, human_scale):
    """X_A = X_r^-1 o X_b: (T_A in metres, B_A)."""
    (position, rotation), (r, y) = body, root
    inverse = conjugate(y)
    return (tuple(human_scale * v for v in rotate(inverse, sub(position, r))),
            normalize(multiply(inverse, rotation)))


def yaw_error(q, dq=0., unit=U64):
    """A bound on q's twist about up when q errs by dq (quaternion 2-norm) and the yaw rounds at `unit`."""
    across = math.hypot(q[1], q[3])
    moved = dq + unit * across
    if not across > 2 * moved:
        return math.inf
    return 2 * moved / (across - moved) + gamma(2, unit) * abs(2 * math.atan2(q[1], q[3])) + 2 * unit


def projection_error(body, root, human_scale, yaw_bound, unit=U64):
    """The inverse rigid composition's rounding at `unit` and a yaw error's turn: (|dT_A|, |dB_A| as a turn)."""
    (position, rotation), (r, y) = body, root
    d = sub(position, r)
    inverse = conjugate(y)
    turned = rotate(inverse, d)
    translation = human_scale * (unit * (norm(position) + norm(r)) + rotate_rounding(inverse, d, unit)
                                 + yaw_bound * math.hypot(d[0], d[2])) + unit * human_scale * norm(turned)
    return translation, 2 * compose_rounding(inverse, rotation, unit) + yaw_bound


def turn_angle(turn):
    """The angle of a rotation that moves a unit vector by at most `turn`; NaN when `turn` bounds nothing
    (2 or more, or not finite), so that every comparison against it fails rather than passing at pi."""
    return 2 * math.asin(turn / 2) if turn < 2 else math.nan


def curve(keys, time):
    """An authored Editor curve (anim_replay keys, unweighted) at `time`, cubic Hermite between keys, and the
    magnitude of its operands."""
    rows = sorted(((f32(key["time_f32"]), f32(key["value_f32"]), f32(key["in_tangent_f32"]),
                    f32(key["out_tangent_f32"])) for key in keys), key=lambda row: row[0])
    if not rows or any(key.get("weighted_mode", "None") != "None" for key in keys):
        raise Refused("a weighted or empty authored body curve")
    if time <= rows[0][0]:
        return rows[0][1], abs(rows[0][1])
    for (t0, a, _, m0), (t1, b, m1, _) in zip(rows, rows[1:]):
        if time <= t1:
            dt = t1 - t0
            s = (time - t0) / dt
            value = ((2 * s ** 3 - 3 * s * s + 1) * a + (s ** 3 - 2 * s * s + s) * dt * m0
                     + (3 * s * s - 2 * s ** 3) * b + (s ** 3 - s * s) * dt * m1)
            return value, abs(a) + abs(b) + dt * (abs(m0) + abs(m1))
    return rows[-1][1], abs(rows[-1][1])


def sample_curves(authored, prefix, time, unit=U32):
    """(T or Q from `prefix` + .x/.y/.z(/.w) curves, a bound on its components' error at `unit`)."""
    axes = "xyzw" if prefix.endswith("Q") else "xyz"
    if not all(f"{prefix}.{a}" in authored for a in axes):
        return None, 0.
    values = [curve(authored[f"{prefix}.{a}"]["keys"], time) for a in axes]
    error = gamma(12, unit) * norm(tuple(magnitude for _, magnitude in values))
    vector = tuple(value for value, _ in values)
    if len(vector) == 4:
        size = norm(vector)
        if not size > 4 * error:
            raise Refused(f"{prefix} is too close to zero to normalize")
        return normalize(vector), 2 * error / (size - error)
    return vector, error
