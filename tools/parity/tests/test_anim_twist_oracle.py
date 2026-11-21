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
import struct
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
# Every arithmetic result passes through r: the identity (f64), or f32 rounding (`in_f32`).
ROUND = [lambda x: x]


def r(x):
    return ROUND[0](x)


def dot(terms):
    total = r(terms[0][0] * terms[0][1])
    for x, y in terms[1:]:
        total = r(total + r(x * y))
    return total


def matmul(a, c):
    return [[dot([(a[i][k], c[k][j]) for k in range(3)]) for j in range(3)] for i in range(3)]


def transpose(a):
    return [list(row) for row in zip(*a)]


def apply(a, v):
    return [dot([(a[i][k], v[k]) for k in range(3)]) for i in range(3)]


def rotation(axis, angle):
    """Rodrigues: the rotation by `angle` about `axis` (normalized here)."""
    n = r(math.sqrt(dot([(v, v) for v in axis])))
    if n == 0 or angle == 0:
        return [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]]
    x, y, z = (r(v / n) for v in axis)
    c, s = r(math.cos(angle)), r(math.sin(angle))
    t = r(1 - c)
    rows = [[(c, 0, 0, 1), (x * y, t, z, -1), (x * z, t, y, 1)],
            [(y * x, t, z, 1), (c, 0, 0, 1), (y * z, t, x, -1)],
            [(z * x, t, y, -1), (z * y, t, x, 1), (c, 0, 0, 1)]]
    out = [[0.] * 3 for _ in range(3)]
    for i in range(3):
        for j in range(3):
            if i == j:
                square = (x, y, z)[i]
                out[i][j] = r(c + r(r(square * square) * t))
            else:
                (_, _, k, sign), (a, b_) = rows[i][j], ((x, y, z)[i], (x, y, z)[j])
                out[i][j] = r(r(r(a * b_) * t) + sign * r(k * s))
    return out


def matrix(q):
    """A quaternion (x, y, z, w), normalized here, as the rotation it applies to a column vector."""
    n = r(math.sqrt(dot([(v, v) for v in q])))
    x, y, z, w = (r(v / n) for v in q)

    def two(a, b_, c, d, sign):
        return r(2 * r(r(a * b_) + sign * r(c * d)))

    def diagonal(a, b_):
        return r(1 - r(2 * r(r(a * a) + r(b_ * b_))))
    return [[diagonal(y, z), two(x, y, z, w, -1), two(x, z, y, w, 1)],
            [two(x, y, z, w, 1), diagonal(x, z), two(y, z, x, w, -1)],
            [two(x, z, y, w, -1), two(y, z, x, w, 1), diagonal(x, y)]]


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
        out[a] = r(axes["sign"][a] * r(v * (axes["max"][a] if v >= 0 else -axes["min"][a])))
    return out


def swing_twist(x, y, z):
    """B5a's joint as rotations: the swing about (0, tan(y/2), tan(z/2)) by 2 atan of its length, after the twist
    about x by x."""
    ty, tz = r(math.tan(r(y / 2))), r(math.tan(r(z / 2)))
    return matmul(rotation((0., ty, tz), r(2 * r(math.atan(r(math.sqrt(dot([(ty, ty), (tz, tz)]))))))),
                  rotation(EX, x))


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
                x, carry = r(x + carry[0]), None
            sent = None
            if k < 2:
                t = settings[fields[k]]
                w = t if limb.d == 1 else r(1 - t)
                if limb.s == 1:
                    joint, sent = swing_twist(r(w * x), y, z), r(r(1 - w) * x)
                elif limb.s == 2:
                    half = r(math.tan(r(x / 2)))
                    joint = swing_twist(r(2 * r(math.atan(r(w * half)))), y, z)
                    sent = r(2 * r(math.atan(r(r(1 - w) * half))))
                else:
                    sent = r(r(1 - w) * x)
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
                carry = (r(sign * sent), bone)
    if toes is not None:
        axes, rest = rig[toes]["axes"], rig[toes]["rest"]
        local = matmul(matmul(matmul(matrix(rest), matrix(axes["pre"])), swing_twist(*angles(toes, axes, values))),
                       transpose(matrix(axes["post"])))
        if foot.kind != 1:
            around = apply(matmul(matrix(rest), matrix(axes["pre"])), EY)
            local = matmul(rotation(around, r(foot.sign * foot_y)), local)
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

    def test_an_f32_oracle_at_perturbed_inputs_stays_within_the_bound(self):
        # The oracle in f32 (every product, sum, division, root and function rounded), at inputs moved to the ends
        # of nonzero radii and inside them: the model's bound covers both the input errors and an independent f32
        # evaluation's rounding.
        rng = random.Random(600)
        checked = 0
        ROUND[0] = f32
        try:
            for trial, settings in enumerate(self.SETTINGS):
                rig = synthetic_rig(320 + trial, toes=trial != 0)
                values = inputs(rng)
                errors = [0.] * 95
                for m in chain_muscles():
                    errors[m] = rng.choice((2. ** -12, 1e-5, 2. ** -20))
                for limb, foot in rng.sample(model.candidates(), 120):
                    side = rng.choice(("Left", "Right"))
                    try:
                        found = model.chains(rig, values, errors, settings, limb, foot, side)
                    except b.Pole:
                        continue
                    for draw in range(2):
                        moved = [inside(v + rng.choice((-e, e, rng.uniform(-e, e))), v, e)
                                 for v, e in zip(values, errors)]
                        want = oracle(rig, moved, settings, limb, foot, side)
                        for bone, (q, radius) in found.items():
                            ROUND[0] = lambda x: x
                            measured = turn(matrix(q), want[bone])
                            ROUND[0] = f32
                            self.assertLessEqual(measured, b.angle_bound(radius), (trial, limb, foot, side, bone))
                            checked += 1
        finally:
            ROUND[0] = lambda x: x
        self.assertGreater(checked, 2000)


def inside(x, v, e):
    """x rounded to f32, then stepped an f32 ulp at a time toward v while the rounding left [v - e, v + e]: v is an
    f32 inside it, so the sample stays inside the declared interval."""
    y = f32(x)
    while abs(y - v) > e:
        if y == 0:
            y = math.copysign(2. ** -149, v)
            continue
        bits = struct.unpack("<I", struct.pack("<f", y))[0]
        bits += 1 if (v > y) == (y > 0) else -1
        y = struct.unpack("<f", struct.pack("<I", bits))[0]
    return y


if __name__ == "__main__":
    unittest.main()
