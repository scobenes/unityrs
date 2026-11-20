"""B1's error bounds for the humanoid body model (docs/plans/humanoid-body.md 2.5).

The bounds come from an explicit f32 operation model, in the manner of the
per-recording f32 tolerances of docs/anim_reference.md, applied operation by
operation at its actual operands: a value computed through `n` dependent
roundings errs by at most gamma(n) = n*u/(1-n*u) of the absolute values of
its operands, u = 2^-24 (f32, Unity) or 2^-53 (f64, the model), plus
2^-126 per operation for flush-to-zero. Recorded f32 values decode exactly.
Every rounding is counted once. Each bound is on a vector's Euclidean
norm, so a fitting row's three scalar equations form one block
(anim_body_adequacy.py propagates it as such). The tests check each bound by
hand, and the forward kinematics against chains emulated in f32.

**Rotations.** Unity turns `v` by `q` as `v + w t + u x t`, `t = 2 (u x v)`
(`unity_rotate`), six roundings deep at the absolute values of the cross
products' operands (`rotate_rounding`). An error `|dq|` in the quaternion
moves the result by at most `kappa(q) |dq| |v|`, `kappa = 2 sqrt(|u|^2 +
(|w| + 2|u|)^2)` (`turn_sensitivity`). A quaternion product rounds four deep
at the absolute values of its sixteen products (`compose_rounding`).

**Forward kinematics (`link`, `tracked`).** One link sets `p' = p + s R t`,
`q' = q q_link`, `s' = s s_link`. The offset's rotation rounds at `t`, its
scale product and the add at their results (the running position), the
rotation product at its products and the scale product once; the parent's
errors carry over, its rotation's through kappa. A chain is walked from an
exact identity at its base: the hips for 2.3's hips-local pose (so where
the hips are in A does not enter), the Animator node for the hips parent
`(t_p, R_p, sigma_p)`, the world for Unity's read-back.

**Centre of mass (`com_error`).** `c = sum_i m_i (p_i + sum_j a_ij (p_j -
p_i))` over the mapped bones and their edges. Under `|a_ij| <= 1` (B2
refits and checks it) input errors reach `c` as `sum_i m_i (|dp_i| + sum_j
|d(p_j - p_i)|)`. An edge difference's error walks the links from `i` to
`j` again from `i`'s state with no position error, so a translation common
to both cancels, plus the subtraction at its result (`body_states`). Its own
evaluation (the products, the inner sum with `p_i`, the mass product, the
outer sum and the normalized masses) errs by `gamma(n_max + 2N + 2)` of
`M = sum_i m_i (|p_i| + sum_j |p_j - p_i|)`.

**The frame (`frame_error`).** 2.2's up and lateral vectors round at the
absolute values of the four points, each cross product at its operands, and
each normalization multiplies its vector's error by `2 / |v|`, so the bound
grows without limit as `up` or `lateral x up` vanishes. `H_A = M(B_A) F(T)
F_h^T` rounds at its operands too (`orientation_rounding`).

**epsilon (`measurement_tolerance`).** Unity's world centre of mass (the
world walk) and the recorder's conversion (`readback_error`): GetHumanPose's
stored `w = c / h` and the recorder's `h w` round at the world value; the
subtraction of the Animator's world position rounds at the difference `d`,
the inverse rotation at `d`, and the division by the Animator's scale and
the stored division by `h` at the result. The Animator's tracked errors
carry through. For a scaled Animator the recorded value is also divided by its
scale once more than c / h (`readback_position`), which the checks undo, so
`epsilon = E_c / s + s readback_error`.

**E_unity (`hips_error` at f32).** Unity's own error in the recorded
hips-local translation of 2.3, in the hips parent's units: the recording's
noise floor.

- The target: a known vector's is the exact f32 body pose the recorder
  passed to SetHumanPose, so only `h T_A` rounds and `B_A` is exact. A
  clip's stands in by its read-back: epsilon for the position, and for the
  rotation Unity's world frame and T-pose frame, its matrix-to-quaternion
  conversion and store, and the recorder's product with the inverse
  Animator rotation (`readback_turn`).
- The hips-local centre of mass `c_h` (`E_ch`, magnitude `M_h`) and both
  frames, so `|dH| <= |dF_h| + |dF_T| + |dB_A| + rounding(H_A)`.
- The solve `P_A = h T_A - k H_A c_h`: `k (E_ch + |dH| M_h)`, the
  matrix-vector product at `|H_A| M_h`, the product by `k` and the
  subtraction at its result.
- The local conversion `sigma_p^-1 R_p^-1 (P_A - t_p)`: `t_p`'s error, the
  subtraction at its result `d` (Unity's own: `sigma_p R_p` times the
  recorded hips translation), `R_p`'s error acting on `d`, the inverse
  rotation at `d` and the division at its result. `sigma_p`'s error cancels
  in `k / sigma_p = s_h` and acts only on `h T_A - t_p`.

**E_eval (`evaluation_error`).** The model's own error at a sample, with
the fitted coefficients taken as exact: B2's f64 reference (E3) runs the
same path from the recorded f32 inputs taken exactly, at u = 2^-53, with
`h T_A` one f64 product. Storing the hips translation as f32 adds `2^-24`
of a bound on the model's pre-cast translation, `(h |T_A| + k M_h + |t_p|)
/ sigma_p` plus the path's own error. These are the model's roundings, not
Unity's, so none is in E_unity's count.

E3 compares within `T_s = E_unity,s + u_s + E_eval,s`; the checkpoint holds
the coefficient uncertainty to `u_s <= E_unity,s`.
"""

import math

from anim_reference import gamma, multiply

U32 = 2. ** -24
FLUSH = 2. ** -126
# The model's own evaluation: f64 throughout, from the recorded f32 inputs.
U64, FLUSH64 = 2. ** -53, 2. ** -1022
# Rounding depths of the operation model, each rounding counted once. A computed
# value errs by at most gamma(depth) of the absolute values of its operands.
OPERATIONS = {
    "rotate": 6,      # Unity's v + w t + u x t, t = 2 (u x v): a cross, a product, a cross and two adds
    "compose": 4,     # a quaternion product: four products summed per component
    "matrix": 3,      # one entry of a 3x3 product, or of a matrix times a vector
    "quaternion": 4,  # one entry of a rotation matrix from a quaternion
    "unquaternion": 8,  # Unity's rotation matrix to quaternion (square root, divisions) and its f32 store
    "midpoint": 2,    # (ul + ur) - (ll + lr) per component; the halving is exact
    "lateral": 2,     # (ur - ul) + (lr - ll) per component
    "cross": 2,       # two products and a subtraction per component
    "normalize": 6,   # three squares, two adds, square root and divide
}
NAMES = ("Hips", "LeftUpperLeg", "RightUpperLeg", "LeftLowerLeg", "RightLowerLeg", "LeftFoot", "RightFoot",
         "Spine", "Chest", "Neck", "Head", "LeftShoulder", "RightShoulder", "LeftUpperArm", "RightUpperArm",
         "LeftLowerArm", "RightLowerArm", "LeftHand", "RightHand", "LeftToes", "RightToes", "LeftEye",
         "RightEye", "Jaw", "UpperChest")
# The 25 body slots of m_HumanBoneIndex: HumanBodyBones 0-23 and UpperChest (54).
BODY_SLOTS = frozenset(NAMES)
FRAME_BONES = ("LeftUpperArm", "RightUpperArm", "LeftUpperLeg", "RightUpperLeg")


def g(stage, times=1, unit=U32):
    return gamma(OPERATIONS[stage] * times, unit)


def flush(unit):
    return FLUSH if unit == U32 else FLUSH64


def ordered(names):
    """Body slots in HumanBodyBones order, so every sum runs in one fixed order."""
    return sorted(names, key=NAMES.index)


def rotate(q, v):
    x, y, z, w = q
    r = multiply(multiply(q, (*v, 0.)), (-x, -y, -z, w))
    return r[:3]


def norm(v):
    return math.sqrt(sum(c * c for c in v))


def sub(a, b):
    return tuple(x - y for x, y in zip(a, b))


def decode(bits, width, f32):
    values = tuple(map(f32, bits))
    if len(values) != width or not all(map(math.isfinite, values)):
        raise ValueError("malformed recorded transform")
    return values


class Frame:
    """One node: position, rotation and uniform scale in the base frame, the
    chain's summed link lengths and its depth below that frame."""

    def __init__(self, position, rotation, scale, length, depth):
        self.position, self.rotation, self.scale, self.length, self.depth = position, rotation, scale, length, depth


def uniform(scale):
    if max(scale) - min(scale) > 1e-6 * max(map(abs, scale)) or min(scale) <= 0:
        raise ValueError("nonuniform or nonpositive scale in the Animator chain")
    return scale[0]


def kinematics(nodes, transforms, f32, base="0", *, include_base=False):
    """f64 forward kinematics of every node at or under `base`: in base's own frame
    (A for the Animator node), or with base's transform applied (the world, for "")."""
    index = {node["address"]: i for i, node in enumerate(nodes)}
    frames = {}
    root = Frame((0., 0., 0.), (0., 0., 0., 1.), 1., 0., 0)
    for address in sorted(index, key=lambda key: (key.count("/") if key else -1, key)):
        if address != base and not address.startswith(base + "/" if base else ""):
            continue
        if address == base and not include_base:
            frames[address] = root
            continue
        parent = root if address == base else frames[address.rpartition("/")[0]]
        row = transforms[index[address]]
        t = decode(row["translation"], 3, f32)
        q = decode(row["rotation"], 4, f32)
        s = uniform(decode(row["scale"], 3, f32))
        offset = tuple(parent.scale * c for c in rotate(parent.rotation, t))
        frames[address] = Frame(tuple(a + b for a, b in zip(parent.position, offset)), multiply(parent.rotation, q),
                                parent.scale * s, parent.length + norm(offset), parent.depth + 1)
    return frames


def bone_frames(human_bones, nodes, frames):
    """Mapped body-slot bones by name, from the captured humanoid bone rows."""
    result = {}
    for row in human_bones:
        if row["name"] in BODY_SLOTS and row["node"] >= 0:
            result[row["name"]] = frames[nodes[row["node"]]["address"]]
    if "Hips" not in result:
        raise ValueError("no mapped Hips in the Animator chain")
    return result


def edges(parents, mapped):
    """(i, j): j's nearest mapped body-slot ancestor under HumanTrait's parents is i."""
    found = []
    for j in ordered(mapped):
        i = parents.get(j)
        while i is not None and i not in mapped:
            i = parents.get(i)
        if i is not None:
            found.append((i, j))
    return found


def recorded_edges(human_bones, nodes):
    """(i, j): i is the nearest mapped body-slot ancestor of j in the recorded hierarchy."""
    by_address = {nodes[row["node"]]["address"]: row["name"] for row in human_bones
                  if row["name"] in BODY_SLOTS and row["node"] >= 0}
    found = []
    for address, j in by_address.items():
        parent = address
        while parent:
            parent = parent.rpartition("/")[0]
            if parent in by_address:
                found.append((by_address[parent], j))
                break
    return sorted(found, key=lambda edge: NAMES.index(edge[1]))


def folded_masses(masses, parents, mapped):
    """E0b: each mapped body slot's default mass plus those of the absent slots it
    is the nearest mapped ancestor of, over the total of the body slots."""
    folded = dict.fromkeys(ordered(mapped), 0.)
    for name in NAMES:
        owner = name
        while owner is not None and owner not in folded:
            owner = parents.get(owner)
        if owner is None:
            raise ValueError(f"absent body slot {name} has no mapped ancestor")
        folded[owner] += masses[name]
    total = sum(folded.values())
    if not total > 0:
        raise ValueError("body slot masses sum to zero")
    return {name: mass / total for name, mass in folded.items()}




# Rotations: Unity's quaternion-vector product and its sensitivities.

def conjugate(q):
    return (-q[0], -q[1], -q[2], q[3])


def cross(a, b):
    return (a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0])


def abs_cross(a, b):
    """|a| x |b| with every product taken positive: the operand bound of a cross product."""
    a, b = [abs(v) for v in a], [abs(v) for v in b]
    return (a[1] * b[2] + a[2] * b[1], a[2] * b[0] + a[0] * b[2], a[0] * b[1] + a[1] * b[0])


def unity_rotate(q, v):
    """Unity's quaternion-vector product, v + w t + u x t with t = 2 (u x v), exact for unit q."""
    u, w = q[:3], q[3]
    t = tuple(2 * c for c in cross(u, v))
    return tuple(a + w * b + c for a, b, c in zip(v, t, cross(u, t)))


def turn_sensitivity(q):
    """kappa(q): rotating v by q + dq instead of q moves Unity's v + 2w (u x v) + 2u x (u x v) by at
    most kappa |dq| |v| to first order. The derivative is 2 dw (u x v) + 2 w (du x v) + 2 du x (u x v)
    + 2 u x (du x v), so kappa = 2 sqrt(|u|^2 + (|w| + 2|u|)^2)."""
    u, w = norm(q[:3]), abs(q[3])
    return 2 * math.sqrt(u * u + (w + 2 * u) ** 2)


def rotate_rounding(q, v, unit=U32):
    """The rounding of Unity's rotation of `v` by `q`, at the absolute values of its operands."""
    u, w, a = [abs(c) for c in q[:3]], abs(q[3]), [abs(c) for c in v]
    t = tuple(2 * c for c in abs_cross(u, a))
    return g("rotate", 1, unit) * norm(tuple(a[k] + w * t[k] + c for k, c in enumerate(abs_cross(u, t))))


def compose_rounding(a, b, unit=U32):
    """The rounding of the quaternion product a b, at the absolute values of its products."""
    (ax, ay, az, aw), (bx, by, bz, bw) = [[abs(c) for c in q] for q in (a, b)]
    terms = (aw * bx + ax * bw + ay * bz + az * by, aw * by + ay * bw + az * bx + ax * bz,
             aw * bz + az * bw + ax * by + ay * bx, aw * bw + ax * bx + ay * by + az * bz)
    return g("compose", 1, unit) * norm(terms)


def quaternion_matrix(q):
    x, y, z, w = q
    return [[1 - 2 * (y * y + z * z), 2 * (x * y - z * w), 2 * (x * z + y * w)],
            [2 * (x * y + z * w), 1 - 2 * (x * x + z * z), 2 * (y * z - x * w)],
            [2 * (x * z - y * w), 2 * (y * z + x * w), 1 - 2 * (x * x + y * y)]]


def matrix_quaternion(m):
    """The unit quaternion (x, y, z, w), w >= 0, of a rotation matrix."""
    w = math.sqrt(max(0., 1 + m[0][0] + m[1][1] + m[2][2])) / 2
    x = math.copysign(math.sqrt(max(0., 1 + m[0][0] - m[1][1] - m[2][2])) / 2, m[2][1] - m[1][2])
    y = math.copysign(math.sqrt(max(0., 1 - m[0][0] + m[1][1] - m[2][2])) / 2, m[0][2] - m[2][0])
    z = math.copysign(math.sqrt(max(0., 1 - m[0][0] - m[1][1] + m[2][2])) / 2, m[1][0] - m[0][1])
    return x, y, z, w


def product(a, b, transpose=False):
    """a b, or a b^T."""
    return [[sum(a[r][k] * (b[c][k] if transpose else b[k][c]) for k in range(3)) for c in range(3)] for r in range(3)]


def frobenius(m):
    return math.sqrt(sum(v * v for row in m for v in row))


def absolute(m):
    return [[abs(v) for v in row] for row in m]


# Forward kinematics with its error bounds, link by link.

class Tracked:
    """One node's f64 position, rotation and scale in a frame, with bounds on how far the computed
    (f32 for Unity, f64 for the model) values are from them: |dp|, |dq| (quaternion 2-norm), |ds|."""

    def __init__(self, position, rotation, scale, e_position=0., e_rotation=0., e_scale=0.):
        self.position, self.rotation, self.scale = position, rotation, scale
        self.e_position, self.e_rotation, self.e_scale = e_position, e_rotation, e_scale


IDENTITY = Tracked((0., 0., 0.), (0., 0., 0., 1.), 1.)


def link(parent, t, q, s, unit=U32):
    """`parent + R_parent (s_parent t)`, rotation `R_parent q` and scale `s_parent s`.

    The rotation and scale of the offset round at the offset's own operands,
    the add at the running position, the product of rotations at its products
    and the product of scales once; the parent's errors carry over, its
    rotation's through kappa."""
    turned = unity_rotate(parent.rotation, t)
    offset = tuple(parent.scale * c for c in turned)
    position = tuple(a + b for a, b in zip(parent.position, offset))
    e_offset = (parent.scale * (turn_sensitivity(parent.rotation) * parent.e_rotation * norm(t)
                                + rotate_rounding(parent.rotation, t, unit))
                + parent.e_scale * norm(turned) + unit * norm(offset))
    return Tracked(position, multiply(parent.rotation, q), parent.scale * s,
                   parent.e_position + e_offset + unit * norm(position) + 10 * flush(unit),
                   parent.e_rotation * norm(q) + compose_rounding(parent.rotation, q, unit),
                   parent.e_scale * s + unit * parent.scale * s)


def tracked(nodes, transforms, f32, base, unit=U32):
    """Every node under `base` (an address, or "" for the world) from `base` exact at the identity."""
    index = {node["address"]: i for i, node in enumerate(nodes)}
    states = {base: IDENTITY} if base else {}
    for address in sorted(index, key=lambda key: (key.count("/"), key)):
        if address == base or not (address.startswith(base + "/") if base else True):
            continue
        parent = states.get(address.rpartition("/")[0], IDENTITY) if address.count("/") else IDENTITY
        row = transforms[index[address]]
        states[address] = link(parent, decode(row["translation"], 3, f32), decode(row["rotation"], 4, f32),
                               uniform(decode(row["scale"], 3, f32)), unit)
    return states


def body_states(nodes, transforms, f32, human_bones, links, base, unit=U32):
    """The mapped body bones' tracked states under `base`, by name, and each edge's difference
    `p_j - p_i` error: the links from i to j walked again from i's state with no position error, so
    a translation common to both cancels, plus the subtraction's rounding at the difference."""
    states = tracked(nodes, transforms, f32, base, unit)
    index = {node["address"]: i for i, node in enumerate(nodes)}
    address = {row["name"]: nodes[row["node"]]["address"] for row in human_bones
               if row["name"] in BODY_SLOTS and row["node"] >= 0}
    bones = {name: states[where] for name, where in address.items()}
    differences = {}
    for i, j in links:
        top, state = address[i], states[address[i]]
        state = Tracked(state.position, state.rotation, state.scale, 0., state.e_rotation, state.e_scale)
        steps = address[j][len(top) + 1:].split("/")
        for k in range(len(steps)):
            row = transforms[index["/".join([top, *steps[:k + 1]])]]
            state = link(state, decode(row["translation"], 3, f32), decode(row["rotation"], 4, f32),
                         uniform(decode(row["scale"], 3, f32)), unit)
        differences[i, j] = state.e_position + unit * norm(sub(bones[j].position, bones[i].position))
    return bones, differences


# The centre of mass and the frame.

def com_error(bones, masses, links, differences, unit=U32):
    """c = sum_i m_i (p_i + sum_j a_ij (p_j - p_i)) under |a_ij| <= 1: its error bound and magnitude
    M = sum_i m_i (|p_i| + sum_j |p_j - p_i|)."""
    children = {i: [] for i in masses}
    for i, j in links:
        children[i].append(j)
    total = magnitude = 0.
    for i in ordered(masses):
        kids = children[i]
        total += masses[i] * (bones[i].e_position + sum(differences[i, j] for j in kids))
        magnitude += masses[i] * (norm(bones[i].position)
                                  + sum(norm(sub(bones[j].position, bones[i].position)) for j in kids))
    # a (p_j - p_i), the inner sum with p_i, the mass product, the outer sum, and the normalized masses.
    widest = max(map(len, children.values()), default=0)
    return total + gamma(widest + 2 * len(masses) + 2, unit) * magnitude, magnitude


def body_frame(points):
    """2.2's frame F as rows of the matrix whose columns are (right, up, forward)."""
    ul, ur, ll, lr = (points[name] for name in FRAME_BONES)
    up = tuple((a + b - c - d) / 2 for a, b, c, d in zip(ul, ur, ll, lr))
    lateral = tuple((b - a) + (d - c) for a, b, c, d in zip(ul, ur, ll, lr))
    up = tuple(v / norm(up) for v in up)
    forward = cross(lateral, up)
    forward = tuple(v / norm(forward) for v in forward)
    right = cross(up, forward)
    return [[right[r], up[r], forward[r]] for r in range(3)]


def frame_error(points, errors, unit=U32):
    """A bound on the Frobenius change of 2.2's frame F = (right, up, forward) from position errors
    of the upper arms and legs and its own rounding at its operands, through both normalizations:
    a normalized vector's error is at most 2 |dv| / |v|, unbounded as v vanishes."""
    ul, ur, ll, lr = (points[name] for name in FRAME_BONES)
    e = sum(errors[name] for name in FRAME_BONES)
    operands = norm(tuple(sum(abs(p[k]) for p in (ul, ur, ll, lr)) for k in range(3)))
    up = tuple((a + b - c - d) / 2 for a, b, c, d in zip(ul, ur, ll, lr))
    lateral = tuple((b - a) + (d - c) for a, b, c, d in zip(ul, ur, ll, lr))
    if not norm(up) > 0:
        raise ValueError("degenerate body frame: no up vector")
    d_up = 2 * (e / 2 + g("midpoint", 1, unit) * operands / 2) / norm(up) + g("normalize", 1, unit)
    hat = tuple(v / norm(up) for v in up)
    turned = cross(lateral, hat)
    if not norm(turned) > 0:
        raise ValueError("degenerate body frame: lateral along up")
    d_cross = (e + g("lateral", 1, unit) * operands + norm(lateral) * d_up
               + g("cross", 1, unit) * norm(abs_cross(lateral, hat)))
    d_forward = 2 * d_cross / norm(turned) + g("normalize", 1, unit)
    forward = tuple(v / norm(turned) for v in turned)
    d_right = d_up + d_forward + g("cross", 1, unit) * norm(abs_cross(hat, forward))
    return math.sqrt(d_right ** 2 + d_up ** 2 + d_forward ** 2)


def orientation_rounding(body_rotation, rest_frame, pose_frame, unit=U32):
    """The rounding of H_A = M(B_A) F(T) F_h^T (2.3 step 2) at its operands: the quaternion's
    matrix, then two products. Returns the bound and H_A."""
    x, y, z, w = (abs(c) for c in body_rotation)
    operands = [[1 + 2 * (y * y + z * z), 2 * (x * y + z * w), 2 * (x * z + y * w)],
                [2 * (x * y + z * w), 1 + 2 * (x * x + z * z), 2 * (y * z + x * w)],
                [2 * (x * z + y * w), 2 * (y * z + x * w), 1 + 2 * (x * x + y * y)]]
    body = quaternion_matrix(body_rotation)
    turned = product(body, rest_frame)
    error = (g("quaternion", 1, unit) * frobenius(operands)
             + g("matrix", 1, unit) * frobenius(product(absolute(body), absolute(rest_frame)))
             + g("matrix", 1, unit) * frobenius(product(absolute(turned), absolute(pose_frame), transpose=True)))
    return error, product(turned, pose_frame, transpose=True)


# The read-back and the hips.

def readback_error(human_scale, world, root, unit=U32):
    """The recorder's read-back body position, a proxy for Unity's target in A (metres): GetHumanPose's
    stored `w = c / h` and the recorder's `h w` round at the world value; InverseTransformPoint
    subtracts the Animator's world position (rounding at the difference), turns it by the inverse
    Animator rotation and divides by its scale; the division by `h` is stored. `root` is the
    Animator node's tracked world state, whose own errors carry through."""
    d = tuple(human_scale * a - b for a, b in zip(world, root.position))
    local = norm(d) / root.scale
    error = (2 * unit * human_scale * norm(world) + root.e_position + unit * norm(d)
             + turn_sensitivity(root.rotation) * root.e_rotation * norm(d)
             + rotate_rounding(conjugate(root.rotation), d, unit))
    return error / root.scale + (root.e_scale / root.scale + 2 * unit) * local


def readback_position(readback, root, f32):
    """The read-back body position as c / h in A: the recorder's `readback_position` times the Animator's
    uniform scale. GetHumanPose's bodyPosition is already free of the Animator's scale (`h w` is the
    Animator's world position plus its rotation of c in A), and the recorder's InverseTransformPoint
    divides by that scale again, so it records c / (h s). `root` is the Animator's tracked world state
    and `f32` decodes the recorded bits."""
    return tuple(root.scale * v for v in decode(readback["readback_position"], 3, f32))


def measurement_tolerance(world_bones, differences, masses, links, human_scale, world, root):
    """epsilon for one fitting sample: a bound on the Euclidean error of c in A (metres), from Unity's
    world forward kinematics and centre of mass and the recorder's conversion, which
    `readback_position` scales back by the Animator's scale."""
    error, _ = com_error(world_bones, masses, links, differences)
    return error / root.scale + root.scale * readback_error(human_scale, world, root)


def readback_turn(world_bones, rest_turn, body_rotation, world_rotation, root, unit=U32):
    """A read-back body rotation as a proxy for Unity's target: Unity's world frame (2.2) and T-pose
    frame, its matrix-to-quaternion conversion and store, and the recorder's product with the
    inverse Animator rotation, as a bound on how far it turns a unit vector."""
    points = {name: state.position for name, state in world_bones.items()}
    errors = {name: state.e_position for name, state in world_bones.items()}
    quaternion = (g("unquaternion", 1, unit) + root.e_rotation
                  + compose_rounding(conjugate(root.rotation), world_rotation, unit))
    return frame_error(points, errors, unit) + rest_turn + turn_sensitivity(body_rotation) * quaternion


def hips_error(bones, differences, rest_bones, masses, links, human_scale, target, target_error, target_turn,
               body_rotation, rest_frame, parent, hips_scale, hips_local=None, unit=U32):
    """The hips-local translation's error bound along 2.3's path (in the hips parent's units), and a
    bound on its magnitude. `bones` are the hips-local tracked bones (from zero, the identity and unit
    scale at the hips), `parent` the hips parent's tracked state in A, and `hips_local` Unity's
    recorded translation, whose magnitude the local conversion rounds at; without it, the bound
    `(h |T_A| + k M_h + |t_p|) / sigma_p` stands in."""
    error, magnitude = com_error(bones, masses, links, differences, unit)
    positions = {name: state.position for name, state in bones.items()}
    rest = {name: state.position for name, state in rest_bones.items()}
    d_pose = frame_error(positions, {name: state.e_position for name, state in bones.items()}, unit)
    d_rest = frame_error(rest, {name: state.e_position for name, state in rest_bones.items()}, unit)
    rounding, solved = orientation_rounding(body_rotation, rest_frame, body_frame(positions), unit)
    turn = d_pose + d_rest + target_turn + rounding
    k = parent.scale * hips_scale
    body = human_scale * norm(target)
    # P_A = h T_A - k H_A c_h: H_A c_h (matrix), times k, the subtraction at its result.
    solve = (k * (error + turn * magnitude) + (g("matrix", 1, unit) * frobenius(absolute(solved)) + unit) * k * magnitude
             + unit * (body + k * magnitude))
    away = tuple(human_scale * a - b for a, b in zip(target, parent.position))
    if hips_local is not None:
        d = tuple(parent.scale * c for c in unity_rotate(parent.rotation, hips_local))
        turned = rotate_rounding(conjugate(parent.rotation), d, unit)
    else:
        d = (body + k * magnitude + norm(parent.position),)
        turned = g("rotate", 1, unit) * (2 + math.sqrt(2)) * math.sqrt(3) * norm(d)
    # t = sigma_p^-1 R_p^-1 (P_A - t_p): t_p's own error, the subtraction at its result, R_p's error
    # acting on P_A - t_p, the inverse rotation and the division. sigma_p's error cancels in
    # k / sigma_p = s_h and acts only on h T_A - t_p.
    local = (parent.e_position + 2 * unit * norm(d) + turned
             + turn_sensitivity(parent.rotation) * parent.e_rotation * norm(d))
    scale = parent.e_scale / parent.scale * norm(away)
    total = (target_error + solve + local + scale) / parent.scale
    return total, norm(d) / parent.scale


def evaluation_error(bones, differences, rest_bones, masses, links, human_scale, target, body_rotation,
                     rest_frame, parent, hips_scale):
    """E_eval at one sample (module docstring): the path at u = 2^-53 from the recorded inputs taken
    exactly, plus one f32 rounding of the model's pre-cast hips translation, bounded by
    (h |T_A| + k M_h + |t_p|) / sigma_p and the path's own error."""
    path, magnitude = hips_error(bones, differences, rest_bones, masses, links, human_scale, target,
                                 U64 * human_scale * norm(target), 0., body_rotation, rest_frame, parent,
                                 hips_scale, None, U64)
    return path + U32 * (magnitude + path)
