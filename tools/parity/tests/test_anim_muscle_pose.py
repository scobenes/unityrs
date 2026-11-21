"""B5a's muscle-pose reference (anim_muscle_pose): reference 2's angle rule, anim_humanoid_qualify's equation
otherwise with its arithmetic order written out, the committed goldens Rust must equal, the axes table, clip
playback's muscle values, the derived agreement bound, and the agreement report."""

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


def upper_arm():
    """Unity's LeftUpperArm z axis: m_Sgn -1, limits -60 and +100 degrees."""
    return dict(pre=[0., 0., 0., 1.], post=[0., 0., 0., 1.], sign=[1., 1., -1.],
                min=[pose.f32(math.radians(v)) for v in (-90., -100., -60.)],
                max=[pose.f32(math.radians(v)) for v in (90., 100., 100.)])


class ReferenceTests(unittest.TestCase):
    def test_reference_one_is_the_qualification_hypothesis_with_its_order_fixed(self):
        # Under CPython up to 3.11 `sum` adds left to right, so the two agree bit for bit; from 3.12 `sum`
        # compensates and anim_humanoid_qualify.normalize may differ in its last bits, never the reference.
        compared = 0
        for bone in (7, 1, 13, 21, 24, 54):
            axes = pose.synthetic_axes(bone)
            for values in pose.schedule(bone):
                ours = pose.muscle_rotation(axes, values, pose.hypothesis_angles)
                theirs = qualify.muscle_rotation(record(axes), values)
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
        # A zero or NaN norm is None, as Rust's.
        self.assertIsNone(pose.normalize((0., 0., 0., 0.)))
        self.assertIsNone(pose.normalize((math.nan, 0., 0., 1.)))

    def test_the_angle_rule(self):
        # The limit by the value's sign (a zero of either sign takes the max), then the axis sign; a later
        # muscle on the same axis replaces an earlier one. Reference 1 applied the sign first.
        axes = dict(pre=[0., 0., 0., 1.], post=[0., 0., 0., 1.], sign=[1., -1., 1.], min=[-.5, -.25, 0.],
                    max=[1., .75, 0.])
        self.assertEqual(pose.angles(axes, [(.5, 0), (.5, 1)]), [.5, -.375, 0.])
        self.assertEqual(pose.angles(axes, [(-.5, 0), (-.5, 1), (1., 2)]), [-.25, .125, 0.])
        self.assertEqual(pose.angles(axes, [(.5, 0), (-1., 0)]), [-.5, 0., 0.])
        self.assertEqual(math.copysign(1, pose.angles(axes, [(-0., 0)])[0]), -1.)
        self.assertEqual(math.copysign(1, pose.angles(axes, [(0., 1)])[1]), -1.)
        self.assertEqual(pose.hypothesis_angles(axes, [(.5, 0), (.5, 1)]), [.5, -.125, 0.])
        self.assertEqual(pose.hypothesis_angles(axes, [(-.5, 0), (-.5, 1), (1., 2)]), [-.25, .375, 0.])

    def test_the_discriminating_samples(self):
        # The plan's: LeftUpperArm z at +1, -1, +.5, -.5 turns -100, +60, -50, +30 degrees; reference 1
        # turned -60, +100, -30, +50. The difference is -|v| * (m_Max + m_Min).
        axes = upper_arm()
        for v, now, before in ((1., -100., -60.), (-1., 60., 100.), (.5, -50., -30.), (-.5, 30., 50.)):
            new, old = pose.angles(axes, [(v, 2)])[2], pose.hypothesis_angles(axes, [(v, 2)])[2]
            self.assertAlmostEqual(math.degrees(new), now, places=5)
            self.assertAlmostEqual(math.degrees(old), before, places=5)
            self.assertAlmostEqual(new - old, -abs(v) * (axes["max"][2] + axes["min"][2]), places=12)

    def test_the_mirrored_pair_is_the_same_mapping(self):
        # Sign first with (-m_Max, -m_Min) as the limits on an m_Sgn -1 axis: every value, the same bits.
        axes = upper_arm()
        mirrored = dict(axes, min=[-v for v in axes["max"]], max=[-v for v in axes["min"]])
        for v in (*pose.VALUES, -2. ** -24, .3, -.7):
            self.assertEqual(struct.pack(">d", pose.angles(axes, [(v, 2)])[2]),
                             struct.pack(">d", pose.hypothesis_angles(mirrored, [(v, 2)])[2]))

    def test_nothing_changes_with_a_positive_sign_or_symmetric_limits(self):
        # The pre-registered no-change proof, bit for bit over every value and every synthetic bone's axes.
        unchanged = changed = 0
        for bone in sorted({qualify.muscle_bone(m) for m in range(qualify.MUSCLE_COUNT)}):
            axes = pose.synthetic_axes(bone)
            for values in pose.schedule(bone):
                for (value, axis) in values:
                    one = [(value, axis)]
                    same = struct.pack(">3d", *pose.angles(axes, one)) == struct.pack(
                        ">3d", *pose.hypothesis_angles(axes, one))
                    # Bitwise symmetric: limits of +0 and +0 are not (-m_Min is -0), and may differ in a zero's sign.
                    symmetric = struct.pack(">d", axes["max"][axis]) == struct.pack(">d", -axes["min"][axis])
                    if axes["sign"][axis] == 1. or symmetric:
                        self.assertTrue(same, (bone, value, axis))
                        unchanged += 1
                    elif value and axes["max"][axis] != -axes["min"][axis]:
                        self.assertFalse(same, (bone, value, axis))
                        changed += 1
        self.assertGreater(unchanged, 500)
        self.assertGreater(changed, 50)

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
        self.assertEqual(pose.goldens()["reference"], 2)


def key(time, value, slope=0.):
    return dict(time_f32=replay.f32_bits(time), value_f32=replay.f32_bits(value), in_tangent_f32=replay.f32_bits(
        slope), out_tangent_f32=replay.f32_bits(slope), weighted_mode="None")


class ClipPlaybackTests(unittest.TestCase):
    def doc(self, bindings):
        return {"spec": {"evidence": {"editor": {"bindings": [
            dict(path=path, type=kind, property=name, keys=keys) for path, kind, name, keys in bindings]}}}}

    def test_the_animator_property_of_a_muscle(self):
        self.assertEqual(pose.muscle_property("Left Thumb 1 Stretched"), "LeftHand.Thumb.1 Stretched")
        self.assertEqual(pose.muscle_property("Right Little Spread"), "RightHand.Little.Spread")
        self.assertEqual(pose.muscle_property("Left Arm Twist In-Out"), "Left Arm Twist In-Out")
        self.assertEqual(pose.muscle_property("Spine Front-Back"), "Spine Front-Back")

    def test_values_are_the_clips_own_curves(self):
        animator = "UnityEngine.Animator"
        doc = self.doc([("", animator, "Spine Front-Back", [key(0., 0.), key(1., .5)]),
                        ("", animator, "LeftHand.Thumb.1 Stretched", [key(0., -1.), key(1., -1.)]),
                        ("Hips", animator, "Chest Front-Back", [key(0., 1.), key(1., 1.)]),
                        ("", "UnityEngine.Transform", "Chest Left-Right", [key(0., 1.), key(1., 1.)])])
        names = ("Spine Front-Back", "Left Thumb 1 Stretched", "Chest Front-Back", "Chest Left-Right")
        sample = dict(local=.5, effective_source=.5)
        values, errors = pose.clip_values(doc, sample, names)
        # Hermite with zero slopes at its midpoint; a finger by its Animator property; a curve on another
        # path or type is not the Animator's, so its muscle is exactly 0.
        self.assertEqual(values, [.25, -1., 0., 0.])
        self.assertEqual(errors[2:], [0., 0.])
        self.assertTrue(0 < errors[0] < 1e-5 and 0 < errors[1] < 1e-5, errors)
        # The time's own f32 rounding joins the error.
        late = dict(local=.5, effective_source=.5 + 2. ** -30)
        self.assertGreater(pose.clip_values(doc, late, names)[1][0], errors[0])

    def test_curves_are_sampled_on_the_source_clock(self):
        doc = self.doc([("", "UnityEngine.Animator", "Spine Front-Back", [key(0., 0.), key(1., .5)])])
        # A clip whose source starts later than the playable: its curve at the source time, not the local one.
        values, _ = pose.clip_values(doc, dict(local=0., effective_source=.5), ("Spine Front-Back",))
        self.assertEqual(values, [.25])
        with self.assertRaisesRegex(ValueError, "source time"):
            pose.clip_values(doc, dict(local=.5), ("Spine Front-Back",))

    def test_a_weighted_curve_is_refused(self):
        weighted = dict(key(0., 0.), weighted_mode="Both")
        doc = self.doc([("", "UnityEngine.Animator", "Spine Front-Back", [weighted, key(1., 1.)])])
        with self.assertRaisesRegex(ValueError, "Spine Front-Back"):
            pose.clip_values(doc, dict(local=.5, effective_source=.5), ("Spine Front-Back",))

    def test_a_value_error_widens_the_bound(self):
        axes, rest = pose.synthetic_axes(7), f32_quaternion(pose._quaternion("muscle-pose/7/rest"))
        values = [(.5, 2), (.5, 1)]
        self.assertGreater(pose.unity_bound(rest, axes, values, [1e-6, 0.]), pose.unity_bound(rest, axes, values))
        self.assertEqual(pose.unity_bound(rest, axes, values, [0., 0.]), pose.unity_bound(rest, axes, values))

    def test_a_value_error_at_zero_reaches_either_limit(self):
        # m_Max 0 and m_Min -60 degrees: at v = 0 an evaluation error of either sign can turn the axis by the
        # larger limit's magnitude times the error, so the error widens the bound on both sides of 0.
        axes, rest = upper_arm(), [0., 0., 0., 1.]
        axes["max"][2] = 0.
        for value in (0., -0., 1e-3, -1e-3):
            self.assertGreater(pose.unity_bound(rest, axes, [(value, 2)], [1e-4]),
                               pose.unity_bound(rest, axes, [(value, 2)]) + .9e-4 * abs(axes["min"][2]), value)


class DomainTests(unittest.TestCase):
    def test_none_where_rust_gives_none(self):
        axes, rest = upper_arm(), [0., 0., 0., 1.]
        # A NaN or infinite muscle value gives a NaN tangent, as C's tan does, then no rotation: Rust's None.
        for value in (math.nan, math.inf, -math.inf):
            self.assertIsNone(pose.bone_rotation(rest, axes, [(value, 2)]), value)
        self.assertTrue(math.isnan(pose.tangents([math.inf, 0., 0.])[0]))
        # An axis past z, and a zero quaternion.
        self.assertIsNone(pose.angles(axes, [(1., 3)]))
        self.assertIsNone(pose.bone_rotation(rest, axes, [(1., 3)]))
        self.assertIsNone(pose.normalize((0., 0., 0., 0.)))
        self.assertIsNone(pose.bone_rotation(rest, dict(axes, pre=[0., 0., 0., 0.]), [(1., 0)]))
        self.assertIsNotNone(pose.bone_rotation(rest, axes, [(1., 2)]))
        with self.assertRaisesRegex(ValueError, "no rotation"):
            pose.vector_row(rest, axes, [(math.nan, 2)])

    def test_a_hand_the_avatar_does_not_map_is_none(self):
        bones = list(range(100, 155))
        human = {"m_HumanBoneIndex": bones[:25], "m_LeftHand": {"data": {"m_HandBoneIndex": []}},
                 "m_RightHand": {"data": {"m_HandBoneIndex": list(range(200, 215))}}}
        # An empty hand table: every finger of that hand is None, never an IndexError, as Rust's `bone_node`.
        self.assertEqual([pose.avatar_node(human, bone) for bone in range(24, 39)], [None] * 15)
        self.assertEqual([pose.avatar_node(human, bone) for bone in range(39, 54)], list(range(200, 215)))
        self.assertEqual((pose.avatar_node(human, 0), pose.avatar_node(human, 54)), (100, 109))
        with self.assertRaises(IndexError):
            qualify.bone_node(human, 24)


class AgreementTests(unittest.TestCase):
    def case(self, turn=0., rule=pose.angles, refusal=False, axes=None, editor="2022", confirmation=False):
        """One bone's case, its recording `rule`'s rotation in f32, turned by `turn` radians."""
        axes = axes or pose.synthetic_axes(7)
        rest = f32_quaternion(pose._quaternion("muscle-pose/7/rest"))
        rows = []
        for values in pose.schedule(7)[:20]:
            local = pose.bone_rotation(rest, axes, values, rule)
            recorded = f32_quaternion(pose.multiply(local, (math.sin(turn / 2), 0., 0., math.cos(turn / 2))))
            rows.append(pose.vector_row(rest, axes, values, recorded))
        return dict(format=pose.FORMAT, kind="cases", required={}, missing={},
                    entries=[dict(editor=editor, case="x", bone=7, name="Spine", refusal=refusal,
                                  confirmation=confirmation, axes=pose.axes_row(axes), rest=pose.f32_hexes(rest),
                                  vectors=rows)])

    def test_a_recording_within_its_bound_agrees(self):
        (key, (worst, bound, count, outside, _, _)), = pose.agreement(self.case()).items()
        self.assertEqual((key, count, outside), (("2022", "Spine"), 20, 0))
        # The f32 rounding of the recording alone, well inside the derived bound.
        self.assertLess(worst, bound)
        self.assertLess(bound, 1e-3)

    def test_a_turned_recording_disagrees(self):
        (_, (worst, bound, count, outside, _, _)), = pose.agreement(self.case(math.radians(.5))).items()
        self.assertEqual((count, outside), (20, 20))
        self.assertAlmostEqual(math.degrees(worst), .5, places=4)

    def test_a_refusal_case_is_reported_apart(self):
        document = self.case(math.radians(.5), refusal=True)
        self.assertEqual(pose.agreement(document), {})
        (_, (_, _, count, outside, _, _)), = pose.agreement(document, refusal=True).items()
        self.assertEqual((count, outside), (20, 20))

    def test_a_change_to_an_agreeing_bone_is_a_regression(self):
        # Synthetic Spine axes 7 have an m_Sgn -1 axis with unequal limits: recorded by reference 1, they
        # agreed then and change now, which the regression check names.
        axes = pose.synthetic_axes(7)
        self.assertTrue(any(s == -1. and lo != -hi for s, lo, hi in zip(axes["sign"], axes["min"], axes["max"])))
        rows = pose.agreement(self.case(rule=pose.hypothesis_angles))
        (_, (_, _, _, outside, changed, was)), = rows.items()
        self.assertEqual(was, 0)
        self.assertGreater(changed, 0)
        self.assertGreater(outside, 0)
        self.assertEqual(pose.regressions(rows), [("2022", "Spine")])
        # Recorded by reference 2 on symmetric limits, nothing changes and nothing regresses.
        symmetric = dict(axes, min=[-v for v in axes["max"]])
        rows = pose.agreement(self.case(axes=symmetric))
        self.assertEqual(next(iter(rows.values()))[3:], (0, 0, 0))
        self.assertEqual(pose.regressions(rows), [])

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


class FailureTests(unittest.TestCase):
    """`agree`'s exit status, acceptance alone: it fails unless both Editors compare acceptance samples, every
    one within its bound, and the acceptance runs lack no required case. Round 11 is `confirm`'s
    (test_anim_muscle_round11.ConfirmTests)."""

    def document(self, **changes):
        cases = AgreementTests.case
        entries = [cases(self, editor=editor)["entries"][0] for editor in pose.EDITORS]
        document = dict(format=pose.FORMAT, kind="cases", reference=pose.REFERENCE, entries=entries,
                        missing={editor: [] for editor in pose.EDITORS}, round11={})
        return document | changes

    def test_a_complete_agreeing_document_passes(self):
        self.assertEqual(pose.failures(self.document()), [])
        self.assertEqual(self.agree(self.document()), (0, ""))

    def test_no_comparison_fails(self):
        document = self.document()
        self.assertEqual(pose.failures(dict(document, entries=[])),
                         ["no acceptance comparison in Editor 2022", "no acceptance comparison in Editor 6000"])
        self.assertEqual(pose.failures(dict(document, entries=document["entries"][:1])),
                         ["no acceptance comparison in Editor 6000"])
        # Rows without a recording compare nothing.
        bare = [dict(entry, vectors=[{k: v for k, v in row.items() if k != "recorded"} for row in entry["vectors"]])
                for entry in document["entries"]]
        self.assertEqual(len(pose.failures(dict(document, entries=bare))), 2)
        code, message = self.agree(dict(document, entries=[]))
        self.assertEqual(code, 1)
        self.assertIn("no acceptance comparison", message)

    def test_any_sample_outside_its_bound_fails(self):
        turned = AgreementTests.case(self, math.radians(.5))["entries"][0]
        document = self.document()
        document["entries"][1] = dict(turned, editor="6000")
        self.assertEqual(pose.failures(document), ["acceptance samples outside their bound: 6000 Spine in 20"])
        # A bone reference 1 already failed on fails too: no row is excused.
        before = AgreementTests.case(self, math.radians(.5), rule=pose.hypothesis_angles)["entries"][0]
        document["entries"][1] = dict(before, editor="6000")
        (_, (_, _, _, outside, _, was)), = pose.agreement(dict(document, entries=document["entries"][1:])).items()
        self.assertGreater(min(outside, was), 0)
        self.assertEqual(len(pose.failures(document)), 1)
        # A row counts each sample it stands for.
        document["entries"][1] = dict(turned, editor="6000", vectors=[dict(turned["vectors"][0], samples=7)])
        self.assertEqual(pose.failures(document), ["acceptance samples outside their bound: 6000 Spine in 7"])
        # Round 11's rows and the refusal table never count toward acceptance, either way.
        for flags in (dict(confirmation=True), dict(refusal=True)):
            document["entries"][1:] = [dict(turned, editor="6000", **flags), self.document()["entries"][1]]
            self.assertEqual(pose.failures(document), [])
            document["entries"][2:] = []
            self.assertEqual(pose.failures(document), ["no acceptance comparison in Editor 6000"])
            self.assertEqual(self.agree(document)[0], 1)

    def test_scope_is_the_runtimes_twist_admission(self):
        # Out of scope exactly where the runtime refuses: a twist setting off neutral and the case animating a
        # bone it redistributes over (Rust's TWIST_BONES).
        self.assertEqual(qualify.LIMBS["m_ArmTwist"], (13, 14, 15, 16))
        neutral = {field: 0. for field in qualify.TWIST}
        refusals = {"humanoid-twist-refusal": "unsupported_humanoid_twist"}
        every = set(range(55))
        self.assertIsNone(pose.refusal_reason("body-probes-muscles-compact", neutral, refusals, every))
        for field in qualify.TWIST:
            twisted = dict(neutral, **{field: .5})
            for bone in qualify.LIMBS[field]:
                self.assertEqual(pose.refusal_reason("x", twisted, refusals, {7, bone}), "unsupported_humanoid_twist")
            # A clip that leaves those bones still is in scope on the same Avatar: a Spine-only one, say.
            self.assertIsNone(pose.refusal_reason("x", twisted, refusals, every - set(qualify.LIMBS[field])))
            self.assertIsNone(pose.refusal_reason("x", twisted, refusals, {7, 8, 54}))
        self.assertEqual(pose.refusal_reason("humanoid-twist-refusal", neutral, refusals, set()),
                         "unsupported_humanoid_twist")
        with self.assertRaisesRegex(ValueError, "lacks a twist setting"):
            pose.refusal_reason("x", {}, refusals, set())

    def test_a_missing_required_case_fails(self):
        document = self.document(missing={"2022": [], "6000": ["body-probes-tall"]})
        self.assertEqual(pose.failures(document), ["Editor 6000's runs lack 1 required case(s): body-probes-tall"])
        self.assertEqual(self.agree(document)[0], 1)

    def test_a_document_without_its_inventory_is_refused(self):
        for key in ("round11", "missing"):
            document = self.document()
            del document[key]
            for mode in ("agree", "confirm"):
                code, message = self.agree(document, mode)
                self.assertEqual(code, 1)
                self.assertIn("make it again", message)

    def agree(self, document, mode="agree"):
        """`agree`'s (or `mode`'s) exit status and stderr on `document`."""
        import contextlib
        import io
        import json

        import scratch

        path = scratch.for_case(self) / "cases.json"
        path.write_text(json.dumps(document), encoding="utf-8")
        argv, err = sys.argv, io.StringIO()
        sys.argv = ["anim_muscle_pose.py", mode, str(path)]
        try:
            with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(err):
                pose.main()
            code = 0
        except SystemExit as exit:
            code = exit.code
        finally:
            sys.argv = argv
        return code, err.getvalue()


if __name__ == "__main__":
    unittest.main()
