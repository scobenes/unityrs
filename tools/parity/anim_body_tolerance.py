"""B1's tolerances for the humanoid body model (docs/plans/humanoid-body.md 2.5).

The bounds come from an explicit f32 operation model, in the manner of the
per-recording f32 tolerances of docs/anim_reference.md. A stage of n
roundings errs by at most gamma32(n) = n*u/(1-n*u), u = 2^-24, of the
magnitudes it combines, plus n*2^-126 for flush-to-zero. Recorded f32 values
decode exactly into f64, and this module's own f64 arithmetic errs by far
less than one f32 ulp. Every rounding is counted once, at the stage that
makes it; a stage's last rounding is the f32 value it stores.

Each bound is on a vector's Euclidean norm, so a fitting row's three scalar
equations form one block (anim_body_adequacy.py propagates it as such).

**Forward kinematics.** One link adds `parent + R_parent (s_parent t)`:
a quaternion-vector rotation, a scale and an add, `level` roundings, each
relative to the link's own offset or to the running position, both at most
the chain's summed link lengths `L`. The accumulated parent rotation's own
error turns each later offset by at most its rounding. A bone at depth `d`
from the frame's origin therefore errs by `E = gamma32(level d) L`.

**Centre of mass.** `c = sum_i m_i ((1 - sum_j a_ij) p_i + sum_j a_ij p_j)`
over the mapped bones `i` and their `n_i` edges. With `|a_ij| <= 1` (B2
refits and checks it), input errors reach `c` as
`sum_i m_i ((1 + n_i) E_i + sum_j E_j)`. Its own evaluation (normalized
masses, `N + 1` roundings; differences, products and the inner and outer
sums, `n_max + N + 4`) errs by `gamma32(n_max + 2N + 5)` of
`M = sum_i m_i ((1 + n_i) |p_i| + sum_j |p_j|)`.

**epsilon (`measurement_tolerance`).** Unity's centre of mass in the world
frame, from forward kinematics through the prefab root and Animator node.
GetHumanPose divides it by the human scale `h` (one rounding of the stored
world read-back `w`). The recorder multiplies by `h`, applies the inverse
Animator transform and divides by `h` (`readback` roundings, the last its
stored value), relative to `h |w| + |t_root|`. Into the Animator frame A
every world error divides by the root scale `s`:
`epsilon = (E_c + u h |w| + gamma32(readback) (h |w| + |t_root|)) / s`.

**E_unity (`unity_error`).** Unity's own error in the recorded hips-local
translation of 2.3, in the hips parent's units: the recording's noise floor.

- positions about the hips (`about_hips`): Unity's links add to the running
  absolute position, so a bone `d` links below the hips errs by
  `gamma32(level d)` of its whole chain length from A's origin, plus one
  rounding (`relative`) of `|p| + |p_hips|` for the difference; the hips'
  own error is common to both and cancels. Then the centre of mass `c_h`
  about the hips, as above (`E_ch`, magnitude `M_h`);
- the frame of 2.2, propagated through both normalized frames
  (`frame_error`), for the pose and for the T-pose, and the target body
  rotation `B_A` (`target_turn`: a read-back turned by Unity's inverse
  Animator rotation, `rotation_error(compose d_root + turn)`), so
  `|dH| <= |dF_h| + |dF_T| + gamma32(rotation) + |dB_A|`;
- the target path: `target_error`, the caller's bound on `h T_A`;
- the hips parent's composed transform at depth `d_p` below the Animator
  node: `|dt_p| <= gamma32(level d_p) L_p`, `|dR_p| <=
  rotation_error(compose d_p)` and `|dsigma_p| / sigma_p <= gamma32(scale
  d_p)`, each over `S = h |T_A| + M_h + |t_p|`, and `k = sigma_p s_h` errs
  by `gamma32(scale (d_p + 1))` of `M_h`;
- the solve `P_A = h T_A - k H c_h` (`solve` roundings over `h |T_A| + M_h`);
- the local conversion `sigma_p^-1 R_p^-1 (P_A - t_p)` (`local` roundings,
  the last the stored hips translation, over `S`).

`E_unity = (target_error + E_ch + (|dH| + gamma32(scale (d_p + 1))) M_h
+ gamma32(solve)(h|T_A| + M_h) + gamma32(local) S + |dt_p|
+ (|dR_p| + |dsigma_p| / sigma_p) S) / sigma_p`.

`rotation_error(n)`: a quaternion whose components each err by
`gamma(n)` of unit magnitude errs by `|dq| <= 2 gamma(n)`, and rotating a
unit vector by it moves the result by at most `4 |dq|`.

**E_eval (`evaluation_error`).** The model's own error at a sample, with
the fitted coefficients taken as exact: B2's f64 reference (E3) runs the
same path from the recorded f32 rotations and target, so each stage has
the same rounding count at u = 2^-53 (flush 2^-1022), with `h T_A` one f64
product and the read-back rotation exact. Storing the hips translation as
f32 adds `u32` of a bound on the model's pre-cast translation:
`S / sigma_p` plus the path's own error. These are the model's roundings,
not Unity's, so neither is in E_unity's count.

E3 compares within `T_s = E_unity,s + u_s + E_eval,s`; the checkpoint holds
the coefficient uncertainty to `u_s <= E_unity,s`.
"""

import math

from anim_reference import gamma, multiply

U32 = 2. ** -24
FLUSH = 2. ** -126
# The model's own evaluation: f64 throughout, from the recorded f32 inputs.
U64, FLUSH64 = 2. ** -53, 2. ** -1022
# Roundings per stage of the operation model, each counted once.
OPERATIONS = {
    "level": 24,      # one link: quaternion-vector rotate (18), scale (3), add (3)
    "readback": 30,   # times human scale, the inverse Animator matrix and point, divide (stored)
    "midpoint": 3,    # two adds and a subtraction per component of the up vector
    "lateral": 2,     # two subtractions per component, then their sum is the third
    "cross": 3,       # two products and a subtraction per component
    "normalize": 6,   # three squares, two adds, square root and divide
    "rotation": 12,   # H_A = B_A F_T F_h^-1: two rotation products and the conversion
    "solve": 8,       # rotate c_h (5), scale by k, subtract from h T_A
    "local": 7,       # subtract t_p, inverse rotate (5), divide by sigma_p (stored)
    "compose": 4,     # one quaternion product: four products summed per component
    "scale": 1,       # one product of uniform scales
    "relative": 1,    # a bone's position less the hips' position, per component
    "turn": 5,        # the recorder's read-back rotation: the inverse Animator rotation times it (4), stored
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


def fk_error(depth, length, unit=U32):
    """Forward kinematics of one bone `depth` links and `length` from its origin, in f32 (Unity) or f64."""
    return g("level", depth, unit) * length + OPERATIONS["level"] * depth * (FLUSH if unit == U32 else FLUSH64)


def position_error(frame):
    return fk_error(frame.depth, frame.length)


def com_error(positions, masses, links, errors, unit=U32):
    """The centre of mass's error bound and magnitude M, given each bone's position and error.

    `positions` and `errors` are by bone name, about the frame the sum runs in."""
    children = {i: [] for i in masses}
    for i, j in links:
        children[i].append(j)
    total = magnitude = 0.
    for i in ordered(masses):
        mass, kids = masses[i], children[i]
        total += mass * ((1 + len(kids)) * errors[i] + sum(errors[j] for j in kids))
        magnitude += mass * ((1 + len(kids)) * norm(positions[i]) + sum(norm(positions[j]) for j in kids))
    widest = max(map(len, children.values()), default=0)
    return total + gamma(widest + 2 * len(masses) + 5, unit) * magnitude, magnitude


def measurement_tolerance(world_bones, masses, links, human_scale, world, root_translation, root_scale):
    """epsilon for one fitting sample: a bound on the Euclidean error of c in A (metres).

    `world_bones` are the bones' Frames in the world (prefab root and Animator
    node included), `world` the raw world read-back (normalized)."""
    positions = {name: frame.position for name, frame in world_bones.items()}
    errors = {name: position_error(frame) for name, frame in world_bones.items()}
    error, _ = com_error(positions, masses, links, errors)
    stored = U32 * human_scale * norm(world)
    conversion = g("readback") * (human_scale * norm(world) + norm(root_translation))
    return (error + stored + conversion) / root_scale


def readback_error(human_scale, world, root_translation, root_scale):
    """The output part of epsilon alone: a target body position read back from Unity."""
    return (U32 * human_scale * norm(world)
            + g("readback") * (human_scale * norm(world) + norm(root_translation))) / root_scale


def frame_error(points, errors, unit=U32):
    """A bound on the Frobenius change of 2.2's frame F = (right, up, forward) from
    position errors of the upper arms and legs, through both normalizations."""
    ul, ur, ll, lr = (points[name] for name in FRAME_BONES)
    e = sum(errors[name] for name in FRAME_BONES)
    size = sum(norm(points[name]) for name in FRAME_BONES)
    up = tuple((a + b - c - d) / 2 for a, b, c, d in zip(ul, ur, ll, lr))
    lateral = tuple((b - a) + (d - c) for a, b, c, d in zip(ul, ur, ll, lr))
    if not norm(up) > 0:
        raise ValueError("degenerate body frame: no up vector")
    d_up = 2 * (e / 2 + g("midpoint", 1, unit) * size / 2) / norm(up) + g("normalize", 1, unit)
    hat = tuple(v / norm(up) for v in up)
    cross = (lateral[1] * hat[2] - lateral[2] * hat[1], lateral[2] * hat[0] - lateral[0] * hat[2],
             lateral[0] * hat[1] - lateral[1] * hat[0])
    if not norm(cross) > 0:
        raise ValueError("degenerate body frame: lateral along up")
    d_cross = e + g("lateral", 1, unit) * size + norm(lateral) * d_up + 2 * g("cross", 1, unit) * norm(lateral)
    d_forward = 2 * d_cross / norm(cross) + g("normalize", 1, unit)
    d_right = d_up + d_forward + 2 * g("cross", 1, unit)
    return math.sqrt(d_right ** 2 + d_up ** 2 + d_forward ** 2)


def rotation_error(roundings, unit=U32):
    """A bound on how far rotating by a quaternion moves a unit vector, when each of the quaternion's
    components errs by gamma(roundings) of unit magnitude: |dq| <= 2 gamma, and |dR| <= 4 |dq|."""
    return 8 * gamma(roundings, unit) if roundings else 0.


def cross(a, b):
    return (a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0])


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


def quaternion_matrix(q):
    x, y, z, w = q
    return [[1 - 2 * (y * y + z * z), 2 * (x * y - z * w), 2 * (x * z + y * w)],
            [2 * (x * y + z * w), 1 - 2 * (x * x + z * z), 2 * (y * z - x * w)],
            [2 * (x * z - y * w), 2 * (y * z + x * w), 1 - 2 * (x * x + y * y)]]


def product(a, b, transpose=False):
    """a b, or a b^T."""
    return [[sum(a[r][k] * (b[c][k] if transpose else b[k][c]) for k in range(3)) for c in range(3)] for r in range(3)]


def about_hips(bones, unit=U32):
    """Positions about the hips and their error bounds, by bone name.

    The links below the hips add to the running absolute position, so each
    rounds relative to the absolute magnitudes (the chain's whole length from
    A's origin), and the difference from the hips position rounds once more,
    relative to both. The hips' own error is common to both and cancels."""
    hips = bones["Hips"]
    positions = {name: sub(frame.position, hips.position) for name, frame in bones.items()}
    errors = {name: 0. if name == "Hips" else
              fk_error(frame.depth - hips.depth, max(frame.length, norm(frame.position)), unit)
              + g("relative", 1, unit) * (norm(frame.position) + norm(hips.position))
              for name, frame in bones.items()}
    return positions, errors


def _hips_path(bones, rest_bones, masses, links, human_scale, target, target_error, target_turn, hips_parent,
               unit):
    """The hips-local translation's error bound along 2.3's path at `unit`, and a bound on its magnitude."""
    positions, errors = about_hips(bones, unit)
    error, magnitude = com_error(positions, masses, links, errors, unit)
    rest = frame_error(*about_hips(rest_bones, unit), unit)
    turn = frame_error(positions, errors, unit) + rest + g("rotation", 1, unit) + target_turn
    body = human_scale * norm(target)
    # The hips parent's composed transform in A: t_p from forward kinematics, R_p from one
    # quaternion product per link, sigma_p from one scale product per link, and k = sigma_p s_h one more.
    depth, offset = hips_parent.depth, norm(hips_parent.position)
    span = body + magnitude + offset
    parent = (fk_error(depth, max(hips_parent.length, offset), unit)
              + (rotation_error(OPERATIONS["compose"] * depth, unit) + gamma(OPERATIONS["scale"] * depth, unit)) * span)
    scale = gamma(OPERATIONS["scale"] * (depth + 1), unit) * magnitude
    total = (target_error + error + turn * magnitude + scale + g("solve", 1, unit) * (body + magnitude)
             + g("local", 1, unit) * span + parent)
    return total / hips_parent.scale, span / hips_parent.scale


def unity_error(bones, rest_bones, masses, links, human_scale, target, target_error, target_turn, hips_parent):
    """E_unity for one sample's hips-local translation (in the hips parent's units): Unity's f32 path.

    `bones` and `rest_bones` are the sample's and the T-pose's Frames in A,
    `target` the body position in A (normalized) with `target_error` bounding
    `h T_A`, `target_turn` bounding the target body rotation's error (as
    `rotation_error`), and `hips_parent` the hips parent's Frame in A."""
    return _hips_path(bones, rest_bones, masses, links, human_scale, target, target_error, target_turn,
                      hips_parent, U32)[0]


def evaluation_error(bones, rest_bones, masses, links, human_scale, target, hips_parent):
    """E_eval at one sample (module docstring): E_unity's operation counts at u = 2^-53 from an
    exactly recorded target, plus one f32 rounding of the model's pre-cast hips translation, whose
    magnitude is bounded by (h |T_A| + M_h + |t_p|) / sigma_p and the path's own error."""
    path, magnitude = _hips_path(bones, rest_bones, masses, links, human_scale, target,
                                 U64 * human_scale * norm(target), 0., hips_parent, U64)
    return path + U32 * (magnitude + path)
