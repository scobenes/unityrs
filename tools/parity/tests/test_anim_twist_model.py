"""Round 12's limb-twist candidates (anim_twist_model), their bound (anim_twist_bound) and tables (anim_twist_tables).

The bound is checked against an f32 emulation of every candidate's whole chain: each flop rounded to f32, on random
and adversarial inputs (zero crossings, near the tangent's pole, collinear axes)."""

import math
import random
import struct
import unittest
from unittest import mock

import test_anim_replay_coverage  # noqa: F401  (puts tools/parity on the path)

import anim_muscle_pose as pose  # noqa: E402
import anim_twist_bound as b  # noqa: E402
import anim_twist_model as model  # noqa: E402
import anim_twist_tables as tables  # noqa: E402
from anim_humanoid_qualify import muscle_bone  # noqa: E402
from anim_reference import angle  # noqa: E402

CHAIN = (1, 2, 3, 4, 5, 6, 13, 14, 15, 16, 17, 18, 19, 20)


def f32(x):
    return struct.unpack("<f", struct.pack("<f", x))[0]


def synthetic_rig(seed, toes=True, identity=False, wide=False):
    """Synthetic f32 axes and rests for the chain bones; `identity` collinear axes (every pre, post and rest the
    identity); `wide` limits as anim_muscle_pose's goldens make them (out to pi). Otherwise limits within 2 rad,
    as Avatars' are."""
    rng = random.Random(seed)

    def quaternion():
        q = [rng.uniform(-1, 1) for _ in range(4)]
        n = math.sqrt(sum(x * x for x in q))
        return [f32(x / n) for x in q]
    rig = {}
    for bone in CHAIN:
        if bone in (19, 20) and not toes:
            continue
        if wide:
            axes = pose.synthetic_axes(bone)
        else:
            axes = dict(pre=quaternion(), post=quaternion(), sign=[rng.choice((-1., 1.)) for _ in range(3)],
                        min=[f32(-rng.uniform(.1, 2)) for _ in range(3)], max=[f32(rng.uniform(.1, 2)) for _ in range(3)])
        rest = quaternion()
        if identity:
            axes["pre"], axes["post"], rest = [0., 0., 0., 1.], [0., 0., 0., 1.], [0., 0., 0., 1.]
        rig[bone] = dict(axes=axes, rest=rest)
    return rig


def chain_muscles():
    return sorted(m for m in range(95) if muscle_bone(m) in CHAIN)


class F32Ops:
    """anim_twist_bound's operations with every flop rounded to f32 and no radius: Unity's path, emulated."""

    @staticmethod
    def multiply(a, b_):
        ax, ay, az, aw = a
        bx, by, bz, bw = b_

        def dot(*terms):
            total = f32(terms[0][0] * terms[0][1])
            for sign, (x, y) in terms[1:]:
                total = f32(total + sign * f32(x * y))
            return total
        return (dot((aw, bx), (1, (ax, bw)), (1, (ay, bz)), (-1, (az, by))),
                dot((aw, by), (-1, (ax, bz)), (1, (ay, bw)), (1, (az, bx))),
                dot((aw, bz), (1, (ax, by)), (-1, (ay, bx)), (1, (az, bw))),
                dot((aw, bw), (-1, (ax, bx)), (-1, (ay, by)), (-1, (az, bz))))

    @staticmethod
    def norm(q):
        total = 0.
        for x in q:
            total = f32(total + f32(x * x))
        return f32(math.sqrt(total))

    def patches(self):
        ops = self

        def limit_angle(axes, axis, value, value_error=0.):
            limit = axes["max"][axis] if value >= 0 else -axes["min"][axis]
            return (f32(axes["sign"][axis] * f32(value * limit)), 0.)

        def unit(a):
            n = ops.norm(a[0])
            if not n > 0:
                raise b.Pole("zero")
            return (tuple(f32(x / n) for x in a[0]), 0.)

        def unit3(a):
            n = ops.norm(a[0])
            return (tuple(f32(x / n) for x in a[0]), 0.)

        def tan_half(a):
            if abs(a[0]) >= math.pi:
                raise b.Pole("pole")
            return (f32(math.tan(f32(a[0] / 2))), 0.)

        def axis_angle(axis, phi):
            half = f32(phi[0] / 2)
            s, c = f32(math.sin(half)), f32(math.cos(half))
            return ((f32(s * axis[0][0]), f32(s * axis[0][1]), f32(s * axis[0][2]), c), 0.)
        return dict(limit_angle=limit_angle, unit=unit, unit3=unit3, tan_half=tan_half, axis_angle=axis_angle,
                    scale=lambda x, y: (f32(x[0] * y[0]), 0.), add=lambda x, y: (f32(x[0] + y[0]), 0.),
                    one_minus=lambda w: (f32(1. - w), 0.), atan2_half=lambda t: (f32(2 * f32(math.atan(t[0]))), 0.),
                    product=lambda x, y: (ops.multiply(x[0], y[0]), 0.),
                    exact=lambda v: (tuple(f32(x) for x in v) if isinstance(v, tuple) else v, 0.))


def emulate(rig, values, settings, limb, foot, side):
    with mock.patch.multiple(b, **F32Ops().patches()):
        return model.chains(rig, values, [0.] * 95, settings, limb, foot, side)


class CandidateTests(unittest.TestCase):
    def test_the_candidate_counts(self):
        self.assertEqual((len(model.LIMBS), len(model.FEET), len(model.candidates())), (126, 5, 630))
        self.assertTrue(all(limb.r == 1 for limb in model.LIMBS if limb.c == 1))
        self.assertEqual(len(set(model.LIMBS)), 126)

    def test_zero_twist_is_b5a_bit_for_bit(self):
        # Every limb-twist value 0: every candidate's chain is reference 2's bone_rotation exactly, and its bound
        # covers unity_bound's (the perturbed norms only add).
        rig = synthetic_rig(1)
        rng = random.Random(2)
        values = [0.] * 95
        for m in chain_muscles():
            if m not in model.LIMB_TWISTS:
                values[m] = f32(rng.uniform(-1, 1))
        for limb, foot in random.Random(3).sample(model.candidates(), 40):
            for side in ("Left", "Right"):
                found = model.chains(rig, values, [0.] * 95, (.25, .5, .75, 1.), limb, foot, side)
                for bone, (q, error) in found.items():
                    own = [(values[m], pose.MUSCLE_AXES[m]) for m in pose.bone_muscles(bone)]
                    expected = pose.bone_rotation(rig[bone]["rest"], rig[bone]["axes"], own)
                    self.assertEqual(list(q), list(expected), (limb, foot, bone))
                    self.assertGreaterEqual(b.angle_bound(error),
                                            pose.unity_bound(rig[bone]["rest"], rig[bone]["axes"], own))

    def test_the_bound_covers_unitys_f32_path(self):
        # Random rigs, values and settings, every candidate's chain in f32 against the f64 prediction.
        rng = random.Random(4)
        muscles = chain_muscles()
        worst, refused = 0., 0
        for trial in range(60):
            rig = synthetic_rig(100 + trial, toes=trial % 3 != 0, identity=trial % 7 == 0)
            values = [0.] * 95
            for m in muscles:
                values[m] = f32(rng.choice((rng.uniform(-1, 1), 2. ** -24, -2. ** -24, 1., -1.)))
            settings = tuple(f32(rng.choice((0., 1., .5, rng.random()))) for _ in range(4))
            for limb, foot in rng.sample(model.candidates(), 12):
                for side in ("Left", "Right"):
                    try:
                        exact = model.chains(rig, values, [0.] * 95, settings, limb, foot, side)
                    except b.Pole:
                        refused += 1
                        continue
                    unity = emulate(rig, values, settings, limb, foot, side)
                    for bone, (q, error) in exact.items():
                        turn = angle(q, unity[bone][0])
                        self.assertLessEqual(turn, b.angle_bound(error), (trial, limb, foot, bone))
                        worst = max(worst, turn / b.angle_bound(error))
        self.assertGreater(worst, 0)
        self.assertLess(refused, 60 * 12 * 2 // 10)

    def test_a_pole_refuses(self):
        # A limit of pi at value 1 puts theta/2 on the tangent's pole: no bound exists.
        rig = synthetic_rig(5)
        rig[13]["axes"]["max"] = [f32(math.pi)] * 3
        values = [0.] * 95
        values[41] = 1.
        with self.assertRaises(b.Pole):
            model.chains(rig, values, [0.] * 95, (1., 1., 1., 1.), model.LIMBS[0], model.FEET[0], "Left")
        with self.assertRaises(b.Pole):
            b.tan_half((3., .2))
        self.assertEqual(b.tan_half((0., 0.))[0], 0.)

    def test_perturbed_norm_products(self):
        a, c = ((0., 0., 0., 2.), .5), ((0., 0., 0., 3.), .25)
        value, error = b.product(a, c)
        self.assertEqual(value, (0., 0., 0., 6.))
        self.assertAlmostEqual(error, 2 * .25 + 3 * .5 + .5 * .25 + 2 * b.GAMMA4 * 2.5 * 3.25)
        with self.assertRaises(b.Pole):
            b.unit(((0., 0., 0., .1), .2))


def predictions(limb, foot, settings, rig, values, side="Left"):
    return model.chains(rig, values, [0.] * 95, settings, limb, foot, side)


class EquivalenceTests(unittest.TestCase):
    """`canonical`'s rules: equal keys give the same chain (to rounding), and some unequal keys visibly differ."""

    def setUp(self):
        self.rig = synthetic_rig(7)
        rng = random.Random(8)
        self.values = [0.] * 95
        for m in chain_muscles():
            self.values[m] = f32(rng.uniform(-.9, .9))
        for bone in (13, 1):
            self.rig[bone]["axes"]["sign"][0] = -self.rig[bone + 2]["axes"]["sign"][0]
        for bone in (15, 3):
            self.rig[bone]["axes"]["sign"][0] = self.rig[bone + 2]["axes"]["sign"][0]

    @property
    def signs(self):
        # The source's x sign times the receiver's, per pair (arm, forearm, upper leg, leg), as the witness reads
        # them from an Avatar.
        pairs = ((13, 15), (15, 17), (1, 3), (3, 5))
        return {side: [self.rig[s + k]["axes"]["sign"][0] * self.rig[r + k]["axes"]["sign"][0] for s, r in pairs]
                for side, k in (("Left", 0), ("Right", 1))}

    def key(self, limb, foot, settings, toes=True):
        return model.canonical(limb, foot, settings, {"Left": toes, "Right": toes}, self.signs)

    def assertSame(self, first, second, settings, toes=True):
        rig = self.rig if toes else {k: v for k, v in self.rig.items() if k not in (19, 20)}
        self.assertEqual(self.key(*first, settings, toes), self.key(*second, settings, toes))
        a = predictions(*first, settings, rig, self.values)
        c = predictions(*second, settings, rig, self.values)
        for bone in a:
            self.assertLess(angle(a[bone][0], c[bone][0]), 1e-12, bone)

    def assertDiffer(self, first, second, settings, toes=True):
        rig = self.rig if toes else {k: v for k, v in self.rig.items() if k not in (19, 20)}
        self.assertNotEqual(self.key(*first, settings, toes), self.key(*second, settings, toes))
        a = predictions(*first, settings, rig, self.values)
        c = predictions(*second, settings, rig, self.values)
        self.assertGreater(max(angle(a[bone][0], c[bone][0]) for bone in a), 1e-3)

    def test_direction_meets_at_one_half(self):
        f1 = model.FEET[0]
        self.assertSame((model.Limb(1, 1, 2, "+", 0), f1), (model.Limb(2, 1, 2, "+", 0), f1), (.5,) * 4)
        self.assertDiffer((model.Limb(1, 1, 2, "+", 0), f1), (model.Limb(2, 1, 2, "+", 0), f1), (0.,) * 4)

    def test_keep_everything_drops_the_receiver(self):
        f1 = model.FEET[0]
        self.assertSame((model.Limb(1, 1, 2, "+", 0), f1), (model.Limb(1, 3, 5, "-", 1 * 0), f1), (1.,) * 4)
        self.assertSame((model.Limb(2, 2, 4, "s", 0), f1), (model.Limb(2, 1, 1, "+", 1), f1), (0.,) * 4)

    def test_zero_weight_joins_s1_and_s2_only(self):
        f1 = model.FEET[0]
        self.assertSame((model.Limb(1, 1, 3, "+", 0), f1), (model.Limb(1, 2, 3, "+", 0), f1), (0.,) * 4)
        self.assertDiffer((model.Limb(1, 1, 3, "+", 0), f1), (model.Limb(1, 3, 3, "+", 0), f1), (0.,) * 4)
        self.assertDiffer((model.Limb(1, 1, 3, "+", 0), f1), (model.Limb(1, 2, 3, "+", 0), f1), (.25,) * 4)

    def test_signs_resolve_per_side(self):
        f1 = model.FEET[0]
        # setUp makes the left products -1, +1, -1, +1: "s" is "+" on the forearm and leg pairs alone.
        self.assertEqual(self.signs["Left"], [-1., 1., -1., 1.])
        self.assertDiffer((model.Limb(1, 1, 3, "s", 0), f1), (model.Limb(1, 1, 3, "+", 0), f1), (.25,) * 4)
        for bone in CHAIN:
            self.rig[bone]["axes"]["sign"][0] = 1.
        self.assertSame((model.Limb(1, 1, 3, "s", 0), f1), (model.Limb(1, 1, 3, "+", 0), f1), (.25,) * 4)
        for bone in CHAIN:
            self.rig[bone]["axes"]["sign"][0] = -1.
        self.assertSame((model.Limb(1, 1, 3, "s", 0), f1), (model.Limb(1, 1, 3, "+", 0), f1), (.25,) * 4)
        self.assertDiffer((model.Limb(1, 1, 3, "s", 0), f1), (model.Limb(1, 1, 3, "-", 0), f1), (.25,) * 4)

    def test_c1_is_r1_where_nothing_moves_on(self):
        f1 = model.FEET[0]
        # Forearm and leg settings 1 keep the received share on the LowerArm and LowerLeg.
        self.assertSame((model.Limb(1, 1, 1, "+", 0), f1), (model.Limb(1, 1, 1, "+", 1), f1), (.25, 1., .25, 1.))
        self.assertDiffer((model.Limb(1, 1, 1, "+", 0), f1), (model.Limb(1, 1, 1, "+", 1), f1), (.25,) * 4)

    def test_the_foot_rules(self):
        limb = model.LIMBS[0]
        f1, f2p, f2m, f3p, f3m = model.FEET
        self.assertSame((limb, f2p), (limb, f3p), (.5,) * 4)
        self.assertDiffer((limb, f2p), (limb, f2m), (.5,) * 4)
        self.assertSame((limb, f2p), (limb, f2m), (.5,) * 4, toes=False)
        self.assertSame((limb, f3m), (limb, f1), (.5,) * 4, toes=False)
        self.assertDiffer((limb, f2p), (limb, f3p), (.5,) * 4, toes=False)
        self.assertDiffer((limb, f1), (limb, f2p), (.5,) * 4)


class TableTests(unittest.TestCase):
    def test_the_bindings(self):
        # Each name's ordinal turns its bone on its axis: twists on x but the Foot's (y).
        bones = {"Upper Leg": (1, 2), "Lower Leg": (3, 4), "Foot": (5, 6), "Toes": (19, 20), "Arm": (13, 14),
                 "Forearm": (15, 16), "Hand": (17, 18)}
        for name, m in tables.NAMES.items():
            side, rest = name.split(" ", 1)
            part = next(p for p in sorted(bones, key=len, reverse=True) if rest.startswith(p + " "))
            self.assertEqual(muscle_bone(m), bones[part][side == "Right"], name)
            if "Twist" in name:
                self.assertEqual(pose.MUSCLE_AXES[m], 1 if part == "Foot" else 0, name)
        self.assertEqual(sorted(m for n, m in tables.NAMES.items() if "Twist" in n), list(model.LIMB_TWISTS))

    def test_the_sets(self):
        core, full = tables.core(), tables.full()
        self.assertEqual((len(core), len(full), len(tables.toe_absent())), (113, 211, 29))
        self.assertEqual(full[:len(core)], core)
        self.assertEqual(len({tuple(sorted(v.items())) for v in full}), 211)
        self.assertIn({}, core)
        self.assertIn({m: .5 for m in model.LIMB_TWISTS}, core)
        self.assertTrue(all(tables.NAMES["Left Toes Up-Down"] not in v and tables.NAMES["Right Toes Up-Down"] not in v
                            for v in tables.toe_absent()))

    def test_the_partitions(self):
        cases = tables.cases()
        parts = {}
        for case, (rig, name, kind, part) in cases.items():
            parts.setdefault(part, []).append(case)
        self.assertEqual({k: len(v) for k, v in parts.items()},
                         {"exploration": 8, "selection": 3, "confirmation": 13})
        self.assertEqual(cases["body-twist-compact-m1"][3], "confirmation")
        self.assertEqual(cases["body-twist-tall-t1"][3], "selection")
        self.assertEqual(len(tables.digest()), 64)
        self.assertEqual(tables.digest(), tables.digest())


if __name__ == "__main__":
    unittest.main()
