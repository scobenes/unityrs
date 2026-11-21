"""An independent oracle for round 12's limb and foot chains (anim_twist_model), coded apart from it.

The oracle restates each candidate from the round 12 design's words in 3x3 rotation matrices (Rodrigues' formula,
no quaternion product, no shared helper but B5a's muscle tables), so a frame, sign or multiplication-order bug in
the model cannot pass both paths. It runs every one of the 630 candidates on synthetic rigs:

- with exact inputs, the model's f64 chain and the oracle's agree to f64 rounding (one equation);
- with nonzero input errors, the oracle at perturbed values (each muscle at an end of its radius, or inside it) stays
  within the model's derived bound, so the bound carries input errors through the whole chain.
"""

import math
import random
import unittest

import test_anim_replay_coverage  # noqa: F401  (puts tools/parity on the path)

import anim_twist_bound as b  # noqa: E402
import anim_twist_model as model  # noqa: E402
from anim_muscle_pose import MUSCLE_AXES, bone_muscles  # noqa: E402
from test_anim_twist_model import chain_muscles, f32, synthetic_rig  # noqa: E402

# The design's chains, restated: (UpperArm, LowerArm, Hand) and (UpperLeg, LowerLeg, Foot) per side, each pair's
# setting by position (arm, forearm, upper leg, leg), and the Toes.
ARM = {"Left": (13, 15, 17), "Right": (14, 16, 18)}
LEG = {"Left": (1, 3, 5), "Right": (2, 4, 6)}
TOES = {"Left": 19, "Right": 20}
EX, EY = (1., 0., 0.), (0., 1., 0.)


def matmul(a, c):
    return [[sum(a[i][k] * c[k][j] for k in range(3)) for j in range(3)] for i in range(3)]


def transpose(a):
    return [list(row) for row in zip(*a)]


def apply(a, v):
    return [sum(a[i][k] * v[k] for k in range(3)) for i in range(3)]


def rotation(axis, angle):
    """Rodrigues: the rotation by `angle` about `axis` (normalized here)."""
    n = math.sqrt(sum(x * x for x in axis))
    if n == 0 or angle == 0:
        return [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]]
    x, y, z = (v / n for v in axis)
    c, s = math.cos(angle), math.sin(angle)
    t = 1 - c
    return [[c + x * x * t, x * y * t - z * s, x * z * t + y * s],
            [y * x * t + z * s, c + y * y * t, y * z * t - x * s],
            [z * x * t - y * s, z * y * t + x * s, c + z * z * t]]


def matrix(q):
    """A quaternion (x, y, z, w), normalized here, as the rotation it applies to a column vector."""
    n = math.sqrt(sum(v * v for v in q))
    x, y, z, w = (v / n for v in q)
    return [[1 - 2 * (y * y + z * z), 2 * (x * y - z * w), 2 * (x * z + y * w)],
            [2 * (x * y + z * w), 1 - 2 * (x * x + z * z), 2 * (y * z - x * w)],
            [2 * (x * z - y * w), 2 * (y * z + x * w), 1 - 2 * (x * x + y * y)]]


def turn(a, c):
    """The rotation angle of a^T c: atan2 of its axial part's length and (trace - 1) / 2."""
    r = matmul(transpose(a), c)
    sine = math.hypot(r[2][1] - r[1][2], r[0][2] - r[2][0], r[1][0] - r[0][1]) / 2
    return math.atan2(sine, (r[0][0] + r[1][1] + r[2][2] - 1) / 2)


def angles(bone, axes, values):
    """Each axis's angle: its muscle's value times the limit on the value's side, times the axis sign."""
    out = [0., 0., 0.]
    for m in bone_muscles(bone):
        a, v = MUSCLE_AXES[m], values[m]
        out[a] = axes["sign"][a] * v * (axes["max"][a] if v >= 0 else -axes["min"][a])
    return out


def swing_twist(x, y, z):
    """B5a's joint as rotations: the swing about (0, tan(y/2), tan(z/2)) by 2 atan of its length, after the twist
    about x by x."""
    ty, tz = math.tan(y / 2), math.tan(z / 2)
    return matmul(rotation((0., ty, tz), 2 * math.atan(math.hypot(ty, tz))), rotation(EX, x))


def oracle(rig, values, settings, limb, foot, side):
    """{bone: rotation matrix} for one side's arm, leg and Toes under a candidate."""
    out = {}
    toes = TOES[side] if TOES[side] in rig else None
    foot_y = 0.
    for chain, fields in ((ARM[side], (0, 1)), (LEG[side], (2, 3))):
        carry = None  # (signed angle, source bone) the previous bone sends on
        for k, bone in enumerate(chain):
            axes, rest = rig[bone]["axes"], rig[bone]["rest"]
            pre, post = matrix(axes["pre"]), matrix(axes["post"])
            x, y, z = angles(bone, axes, values)
            if k == 2 and chain is LEG[side]:
                foot_y = y
                if foot.kind == 2 or (foot.kind == 3 and toes is not None):
                    y = 0.
            if carry is not None and limb.c == 1:
                x, carry = x + carry[0], None
            sent = None
            if k < 2:
                t = settings[fields[k]]
                w = t if limb.d == 1 else 1 - t
                if limb.s == 1:
                    joint, sent = swing_twist(w * x, y, z), (1 - w) * x
                elif limb.s == 2:
                    half = math.tan(x / 2)
                    joint, sent = swing_twist(2 * math.atan(w * half), y, z), 2 * math.atan((1 - w) * half)
                else:
                    sent = (1 - w) * x
                    joint = matmul(rotation(EX, -sent), swing_twist(x, y, z))
            else:
                joint = swing_twist(x, y, z)
            local = matmul(matmul(matmul(matrix(rest), pre), joint), transpose(post))
            if carry is not None:
                phi, source = carry
                s_axes, s_rest = rig[source]["axes"], rig[source]["rest"]
                held = apply(matmul(matrix(s_rest), matrix(s_axes["pre"])), EX)
                around = {1: apply(post, EX), 2: apply(matmul(matrix(rest), pre), EX),
                          3: apply(matrix(s_axes["post"]), EX), 5: apply(matrix(s_axes["post"]), EX),
                          4: apply(transpose(out[source]), held), 6: apply(transpose(out[source]), held)}[limb.r]
                spin = rotation(around, phi)
                local = matmul(local, spin) if limb.r in (1, 5, 6) else matmul(spin, local)
            out[bone] = local
            carry = None
            if sent is not None:
                receiver = chain[k + 1]
                sign = {"+": 1., "-": -1.}.get(limb.sigma)
                if sign is None:
                    sign = axes["sign"][0] * rig[receiver]["axes"]["sign"][0]
                carry = (sign * sent, bone)
    if toes is not None:
        axes, rest = rig[toes]["axes"], rig[toes]["rest"]
        local = matmul(matmul(matmul(matrix(rest), matrix(axes["pre"])), swing_twist(*angles(toes, axes, values))),
                       transpose(matrix(axes["post"])))
        if foot.kind != 1:
            around = apply(matmul(matrix(rest), matrix(axes["pre"])), EY)
            local = matmul(rotation(around, foot.sign * foot_y), local)
        out[toes] = local
    return out


def inputs(rng):
    values = [0.] * 95
    for m in chain_muscles():
        values[m] = f32(rng.uniform(-.95, .95))
    return values


class OracleTests(unittest.TestCase):
    SETTINGS = ((.25, .75, .5, 1.), (0., .5, 1., .25), (.75, 0., .25, .5))

    def test_every_candidate_is_the_designs_equation(self):
        worst = 0.
        for trial, settings in enumerate(self.SETTINGS):
            rig = synthetic_rig(300 + trial, toes=trial != 2)
            values = inputs(random.Random(400 + trial))
            for limb, foot in model.candidates():
                for side in ("Left", "Right"):
                    try:
                        found = model.chains(rig, values, [0.] * 95, settings, limb, foot, side)
                    except b.Pole:
                        continue
                    want = oracle(rig, values, settings, limb, foot, side)
                    self.assertEqual(set(found), set(want))
                    for bone, (q, _) in found.items():
                        worst = max(worst, turn(matrix(q), want[bone]))
                        self.assertLess(turn(matrix(q), want[bone]), 1e-9, (limb, foot, side, bone))
        self.assertLess(worst, 1e-9)

    def test_the_bound_carries_input_errors(self):
        rng = random.Random(500)
        checked = 0
        for trial, settings in enumerate(self.SETTINGS):
            rig = synthetic_rig(310 + trial, toes=trial != 1)
            values = inputs(rng)
            errors = [0.] * 95
            for m in chain_muscles():
                errors[m] = rng.choice((1e-3, 2. ** -12, 1e-5))
            for limb, foot in rng.sample(model.candidates(), 160):
                side = rng.choice(("Left", "Right"))
                try:
                    found = model.chains(rig, values, errors, settings, limb, foot, side)
                except b.Pole:
                    continue
                for draw in range(3):
                    moved = [v + rng.choice((-e, e, rng.uniform(-e, e))) for v, e in zip(values, errors)]
                    want = oracle(rig, moved, settings, limb, foot, side)
                    for bone, (q, radius) in found.items():
                        self.assertLessEqual(turn(matrix(q), want[bone]), b.angle_bound(radius),
                                             (trial, limb, foot, side, bone, draw))
                        checked += 1
        self.assertGreater(checked, 3000)


if __name__ == "__main__":
    unittest.main()
