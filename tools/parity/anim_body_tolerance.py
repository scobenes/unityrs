"""B1's tolerances for the humanoid body model (docs/plans/humanoid-body.md 2.5).

Two bounds, each from an explicit f32 operation model in the manner of the
per-recording f32 tolerances of docs/anim_reference.md: an operation of a
stage with n roundings errs by at most gamma32(n) = n*u/(1-n*u), u = 2^-24,
of the magnitudes it combines, plus n*2^-126 for flush-to-zero.

- `measurement_tolerance` (epsilon_r) bounds one fitting row: the Editor's
  f32 forward kinematics and centre-of-mass sum for the recorded pose, the
  f32 GetHumanPose output and the recorder's conversion to the Animator
  frame, as an absolute error in metres per component of `c` in A.
- `hips_tolerance` (tau_s) bounds one predicted sample's hips-local
  translation along the body path of 2.3: forward kinematics below the
  hips, the weighted sum, the frame, the solve and the f32 output.

The model's unknown coefficients enter through the assumption
|a_ij| <= 1 for each edge (B2 refits and checks it). Every input is the
recorded f32 data, read exactly into f64; the f64 arithmetic of this
module errs by far less than one f32 ulp, as in anim_reference.md.
"""

import math

from anim_reference import gamma, multiply, ulp32

U32 = 2. ** -24
FLUSH = 2. ** -126
# Roundings per stage of the operation model, each counted once per use.
OPERATIONS = {
    "level": 24,      # one link: rotate (quaternion-vector, 18), scale (3), add (3)
    "sum": 2,         # one weighted term of the centre of mass: multiply, add
    "readback": 24,   # recorder: times human scale, inverse Animator transform, divide
    "output": 1,      # division by human scale in GetHumanPose
    "frame": 40,      # two midpoints, two differences, cross products, normalization
    "solve": 30,      # H_A from three quaternions, rotate c_h, scale by k, subtract
    "local": 24,      # inverse parent rotation and scale, translation
}
NAMES = ("Hips", "LeftUpperLeg", "RightUpperLeg", "LeftLowerLeg", "RightLowerLeg", "LeftFoot", "RightFoot",
         "Spine", "Chest", "Neck", "Head", "LeftShoulder", "RightShoulder", "LeftUpperArm", "RightUpperArm",
         "LeftLowerArm", "RightLowerArm", "LeftHand", "RightHand", "LeftToes", "RightToes", "LeftEye",
         "RightEye", "Jaw", "UpperChest")
# The 25 body slots of m_HumanBoneIndex: HumanBodyBones 0-23 and UpperChest (54).
BODY_SLOTS = frozenset(NAMES)


def g(stage, times=1):
    n = OPERATIONS[stage] * times
    return gamma(n, U32)


def rotate(q, v):
    x, y, z, w = q
    r = multiply(multiply(q, (*v, 0.)), (-x, -y, -z, w))
    return r[:3]


def norm(v):
    return math.sqrt(sum(c * c for c in v))


def decode(bits, width, f32):
    values = tuple(map(f32, bits))
    if len(values) != width or not all(map(math.isfinite, values)):
        raise ValueError("malformed recorded transform")
    return values


class Frame:
    """One node in the Animator frame A: position, rotation, uniform scale,
    the chain's summed link lengths and its depth below A."""

    def __init__(self, position, rotation, scale, length, depth):
        self.position, self.rotation, self.scale, self.length, self.depth = position, rotation, scale, length, depth


def uniform(scale):
    if max(scale) - min(scale) > 1e-6 * max(map(abs, scale)) or min(scale) <= 0:
        raise ValueError("nonuniform or nonpositive scale in the Animator chain")
    return scale[0]


def kinematics(nodes, transforms, f32, animator="0"):
    """Forward kinematics in f64 of every node at or under the Animator node."""
    index = {node["address"]: i for i, node in enumerate(nodes)}
    frames = {animator: Frame((0., 0., 0.), (0., 0., 0., 1.), 1., 0., 0)}
    for address in sorted(index, key=lambda key: key.count("/")):
        if not address.startswith(animator + "/"):
            continue
        parent = frames[address.rpartition("/")[0]]
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
    """(i, j): j's nearest mapped body-slot ancestor is i. `parents` maps a name to its HumanTrait parent name."""
    found = []
    for j in sorted(mapped):
        i = parents.get(j)
        while i is not None and i not in mapped:
            i = parents.get(i)
        if i is not None:
            found.append((i, j))
    return found


def folded_masses(masses, parents, mapped):
    """E0b: each mapped body slot's default mass plus those of the absent slots it
    is the nearest mapped ancestor of, over the total of the body slots."""
    folded = dict.fromkeys(mapped, 0.)
    for name in BODY_SLOTS:
        owner = name
        while owner is not None and owner not in mapped:
            owner = parents.get(owner)
        if owner is None:
            raise ValueError(f"absent body slot {name} has no mapped ancestor")
        folded[owner] += masses[name]
    total = sum(folded.values())
    if not total > 0:
        raise ValueError("body slot masses sum to zero")
    return {name: mass / total for name, mass in folded.items()}


def position_error(frame):
    """Unity's f32 forward kinematics of one bone, against f64."""
    return g("level", frame.depth) * frame.length + OPERATIONS["level"] * frame.depth * FLUSH


def com_error(bones, masses, links, error):
    """Propagate per-bone errors and the sum's own rounding through c, with |a_ij| <= 1."""
    children = {}
    for i, j in links:
        children.setdefault(i, []).append(j)
    total, magnitude = 0., 0.
    for i, mass in masses.items():
        reach = max((error(bones[j]) for j in children.get(i, ())), default=0.)
        span = max((norm(bones[j].position) for j in children.get(i, ())), default=0.)
        total += mass * (error(bones[i]) + 2 * reach)
        magnitude += mass * (norm(bones[i].position) + 2 * span)
    return total + g("sum", 2 * len(masses) + len(links)) * magnitude


def measurement_tolerance(bones, masses, links, human_scale, readback, world, root):
    """epsilon_r, per component of c in A (metres), for one fitting sample.

    `readback` is the recorded body position in A (normalized), `world` the
    raw world read-back (normalized) and `root` the Animator node's world
    translation and uniform scale."""
    c = com_error(bones, masses, links, position_error)
    output = human_scale * (max(ulp32(abs(v)) for v in readback) / 2 + g("output") * norm(world))
    root_translation, root_scale = root
    conversion = g("readback") * (human_scale * norm(world) + norm(root_translation)) / root_scale
    return c * (1 + g("output")) + output + conversion


def hips_tolerance(bones, masses, links, human_scale, target, hips_parent, hips_local):
    """tau_s for one sample's hips-local translation (in the hips parent's units).

    `target` is the body position in A (normalized), `hips_parent` the hips
    parent's Frame in A and `hips_local` the recorded hips translation."""
    hips = bones["Hips"]

    def below(frame):
        depth = frame.depth - hips.depth
        length = max(frame.length - hips.length, 0.)
        return g("level", depth) * length + OPERATIONS["level"] * depth * FLUSH

    c_h = com_error(bones, masses, links, below)
    lever = max(norm(tuple(a - b for a, b in zip(bone.position, hips.position))) for bone in bones.values())
    sides = [bones.get(side + part) for side in ("Left", "Right") for part in ("UpperArm", "UpperLeg")]
    if any(side is None for side in sides):
        raise ValueError("the body frame needs both upper arms and upper legs")
    ul, ur, ll, lr = (side.position for side in sides)
    up = norm(tuple((a + b - c - d) / 2 for a, b, c, d in zip(ul, ur, ll, lr)))
    lateral = norm(tuple((b - a) + (d - c) for a, b, c, d in zip(ul, ur, ll, lr)))
    if not min(up, lateral) > 0:
        raise ValueError("degenerate body frame")
    point = max(below(side) for side in sides)
    turn = 4 * point / min(up, lateral) + g("frame")
    body = human_scale * norm(target)
    solve = g("solve") * (body + lever) + turn * lever + c_h
    target_error = human_scale * max(ulp32(abs(v)) for v in target) / 2
    position = (solve + target_error) / hips_parent.scale
    local = g("local") * (body + lever + norm(hips_parent.position)) / hips_parent.scale
    output = max(ulp32(abs(v)) for v in hips_local) / 2
    return position + local + output
