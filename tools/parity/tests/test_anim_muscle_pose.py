"""B5a's muscle-pose reference (anim_muscle_pose): the hypothesis of anim_humanoid_qualify with its arithmetic
order written out, the committed goldens Rust must equal, the axes table, the derived agreement bound, and the
agreement report."""

import math
import platform
import struct
import sys
import unittest

import test_anim_replay_coverage  # noqa: F401  (puts tools/parity on the path)

import anim_humanoid_qualify as qualify  # noqa: E402
import anim_muscle_pose as pose  # noqa: E402
import anim_replay as replay  # noqa: E402
from anim_reference import angle  # noqa: E402


def record(axes):
    return {"m_PreQ": axes["pre"], "m_PostQ": axes["post"], "m_Sgn": axes["sign"],
            "m_Limit": {"m_Min": axes["min"], "m_Max": axes["max"]}}


def f32_quaternion(q):
    return [pose.f32(v) for v in q]


class ReferenceTests(unittest.TestCase):
    def test_it_is_the_qualification_hypothesis_with_its_order_fixed(self):
        # Under CPython up to 3.11 `sum` adds left to right, so the two agree bit for bit; from 3.12 `sum`
        # compensates and anim_humanoid_qualify.normalize may differ in its last bits, never the reference.
        compared = 0
        for bone in (7, 1, 13, 21, 24, 54):
            axes = pose.synthetic_axes(bone)
            for values in pose.schedule(bone):
                ours, theirs = pose.muscle_rotation(axes, values), qualify.muscle_rotation(record(axes), values)
                if sys.version_info < (3, 12):
                    self.assertEqual(ours, theirs)
                else:
                    self.assertLess(angle(ours, theirs), 1e-14)
                compared += 1
        self.assertGreater(compared, 100)

    def test_the_norm_adds_left_to_right(self):
        # Three terms whose compensated sum differs from the left-to-right one in the last bit.
        q = (1., 2. ** -27, 2. ** -27, 2. ** -27)
        naive = math.sqrt(((q[0] * q[0] + q[1] * q[1]) + q[2] * q[2]) + q[3] * q[3])
        self.assertEqual(pose.normalize(q), tuple(x / naive for x in q))
        with self.assertRaisesRegex(ValueError, "zero quaternion"):
            pose.normalize((0., 0., 0., 0.))

    def test_the_angle_rule(self):
        # d = sign * value; d >= 0 takes the max limit, a negative d the min's magnitude; a later muscle on
        # the same axis replaces an earlier one.
        axes = dict(pre=[0., 0., 0., 1.], post=[0., 0., 0., 1.], sign=[1., -1., 1.], min=[-.5, -.25, 0.],
                    max=[1., .75, 0.])
        self.assertEqual(pose.angles(axes, [(.5, 0), (.5, 1)]), [.5, -.125, 0.])
        self.assertEqual(pose.angles(axes, [(-.5, 0), (-.5, 1), (1., 2)]), [-.25, .375, 0.])
        self.assertEqual(pose.angles(axes, [(.5, 0), (-1., 0)]), [-.5, 0., 0.])
        self.assertEqual(math.copysign(1, pose.angles(axes, [(-0., 0)])[0]), -1.)

    def test_the_axes_table(self):
        self.assertEqual(len(pose.MUSCLE_AXES), qualify.MUSCLE_COUNT)
        bones = {}
        for muscle, axis in enumerate(pose.MUSCLE_AXES):
            bones.setdefault(qualify.muscle_bone(muscle), []).append(axis)
        # Each bone's muscles turn distinct axes, z first, then y, then x.
        self.assertTrue(all(axes == sorted(set(axes), reverse=True) for axes in bones.values()))
        self.assertEqual(bones[7], [2, 1, 0])
        self.assertEqual(bones[3], [2, 0])
        self.assertEqual(bones[24], [2, 1])
        self.assertEqual(len(bones), 54)

    def test_the_goldens_cover_every_muscle(self):
        document = pose.goldens()
        self.assertEqual(document["muscle_axes"], list(pose.MUSCLE_AXES))
        # Each muscle alone at every value, then four joint vectors per bone.
        alone = set()
        for entry in document["entries"]:
            count = len(pose.bone_muscles(entry["bone"]))
            self.assertEqual(len(entry["vectors"]), len(pose.VALUES) * count + 4)
            alone |= {(entry["bone"], tuple(map(tuple, row["values"])))
                      for row in entry["vectors"][:len(pose.VALUES) * count]}
        self.assertEqual(len(alone), len(pose.VALUES) * qualify.MUSCLE_COUNT)
        self.assertTrue({1., -1., 0., 1.5, -1.5} <= set(pose.VALUES))
        signs = {sign for entry in document["entries"] for sign in pose.axes_of(entry["axes"])["sign"]}
        self.assertEqual(signs, {-1., 1.})
        limits = [pose.axes_of(entry["axes"]) for entry in document["entries"]]
        self.assertTrue(any(a["min"][k] == a["max"][k] == 0 for a in limits for k in range(3)))
        self.assertTrue(any(a["max"][k] == pose.f32(math.pi) for a in limits for k in range(3)))
        self.assertTrue(any(-a["min"][k] != a["max"][k] != 0 for a in limits for k in range(3)))

    @unittest.skipUnless(platform.libc_ver()[0] == "glibc", "the goldens' tangents are glibc's")
    def test_the_committed_goldens_are_these(self):
        self.assertEqual(pose.GOLDENS.read_text(encoding="utf-8"), pose.dump(pose.goldens()))


class AgreementTests(unittest.TestCase):
    def case(self, turn=0.):
        """One bone's case, its recording the reference's rotation in f32, turned by `turn` radians."""
        axes, rest = pose.synthetic_axes(7), f32_quaternion(pose._quaternion("muscle-pose/7/rest"))
        rows = []
        for values in pose.schedule(7)[:20]:
            local = pose.bone_rotation(rest, axes, values)
            recorded = f32_quaternion(pose.multiply(local, (math.sin(turn / 2), 0., 0., math.cos(turn / 2))))
            rows.append(pose.vector_row(rest, axes, values, recorded))
        return dict(format=pose.FORMAT, kind="cases", entries=[dict(editor="2022", case="x", bone=7, name="Spine",
                                                                   axes=pose.axes_row(axes),
                                                                   rest=pose.f32_hexes(rest), vectors=rows)])

    def test_a_recording_within_its_bound_agrees(self):
        (key, (worst, bound, count, outside)), = pose.agreement(self.case()).items()
        self.assertEqual((key, count, outside), (("2022", "Spine"), 20, 0))
        # The f32 rounding of the recording alone, well inside the derived bound.
        self.assertLess(worst, bound)
        self.assertLess(bound, 1e-3)

    def test_a_turned_recording_disagrees(self):
        (_, (worst, bound, count, outside)), = pose.agreement(self.case(math.radians(.5))).items()
        self.assertEqual((count, outside), (20, 20))
        self.assertAlmostEqual(math.degrees(worst), .5, places=4)

    def test_the_bound_is_derived_at_the_operands(self):
        axes, rest = pose.synthetic_axes(7), f32_quaternion(pose._quaternion("muscle-pose/7/rest"))
        small = pose.unity_bound(rest, axes, [(.125, 2)])
        large = pose.unity_bound(rest, axes, [(1., 2), (1., 1), (1., 0)])
        # Every operation's rounding counts at its operands: more turn, more error, all of it f32-sized.
        self.assertGreater(large, small)
        self.assertGreater(small, 100 * pose.U32)
        self.assertLess(large, 2e4 * pose.U32)
        self.assertEqual(pose.U32, struct.unpack(">f", bytes.fromhex("33800000"))[0])
        self.assertGreater(small, replay.F64_ANGLE_FLOOR)


if __name__ == "__main__":
    unittest.main()
