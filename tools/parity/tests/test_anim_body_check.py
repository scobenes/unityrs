"""B2's checks E0-E4 on synthetic recordings that follow the f64 model, and on one value perturbed."""

import copy
import json
import math
import unittest

import anim_body_support as support
from anim_body_model_support import BITS, F32, avatar, coefficients, recording, write_run

import anim_body_adequacy as adequacy  # noqa: E402  (test_anim_replay puts it on the path)
import anim_body_check as check  # noqa: E402
import anim_body_freeze as freeze  # noqa: E402
import anim_body_model as model  # noqa: E402
import anim_body_runs as runs  # noqa: E402
import anim_body_tolerance as tolerance  # noqa: E402
import anim_fixture_body as body  # noqa: E402
import scratch  # noqa: E402
from anim_reference import euler, multiply  # noqa: E402

CASES = ("body-probes-compact", "body-probes-no-upper-chest", "body-targets-tall", "body-chain-armature-targets",
         "body-chain-hips-scale-targets", "body-chain-scaled-animator-targets", "body-clip-keep-o0-y0-xz0",
         "body-clip-keep-o0-y0-xz0-start", "body-clip-bake-o1-y1-xz1-offset", "body-clip-level",
         "body-clip-keep-o1-y1-xz1-motion", "body-clip-keep-o1-y0-xz0", "body-clip-height-from-feet")


def hips_index(doc):
    return next(row["node"] for row in doc["capture"]["humanoid"]["bones"] if row["name"] == "Hips")


def avatars(run, doc):
    return avatar(doc)


class Checks(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.docs = {name: recording(name, samples=12) for name in CASES}

    def run_mode(self, mode, docs=None, avatar_of=avatars, folder=None):
        folder = folder or scratch.for_case(self)
        write_run(folder, docs if docs is not None else self.docs)
        return freeze.run_mode(mode, [folder], avatar_of=avatar_of)

    def edited(self, name, edit):
        docs = copy.deepcopy(self.docs)
        edit(docs[name])
        return docs

    def test_every_check_passes_on_the_model(self):
        for mode in ("e0", "e2", "e3a", "e4"):
            with self.subTest(mode=mode):
                report = self.run_mode(mode)
                self.assertEqual(report["failures"], [])
                self.assertTrue(report["passed"])
        report = self.run_mode("e4")
        self.assertEqual(report["combinations"]["keep-o0-y0-xz0"], dict(status="pass", start_offset="pass"))
        self.assertEqual(report["combinations"]["keep-o1-y1-xz1-motion"]["status"], "pass")
        self.assertEqual(report["combinations"]["keep-o1-y0-xz0"]["status"], "refused")
        self.assertEqual(report["combinations"]["height-from-feet"]["status"], "undecided")
        self.assertEqual(report["start_pairs"], 6)

    def test_e0_refuses_or_fails_a_wrong_avatar(self):
        def edited(change):
            def read(run, doc):
                value = avatar(doc)
                change(check.human_of(value))
                return value
            return read

        def mass(human, slot, value):
            masses = [F32(human["m_HumanBoneMass"]["v"][i:i + 8]) for i in range(0, 200, 8)]
            masses[slot] = value
            human["m_HumanBoneMass"]["v"] = "".join(BITS(v) for v in masses)

        report = self.run_mode("e0", avatar_of=edited(lambda human: mass(human, 7, F32(BITS(.051)))))
        e0b = [line for line in report["failures"] if "E0b" in line]
        self.assertTrue(e0b and all("E0b Spine" in line for line in e0b))
        # An absent slot must hold no mass.
        report = self.run_mode("e0", avatar_of=edited(lambda human: mass(human, 22, 1e-3)))
        self.assertTrue(any("E0b LeftEye" in line for line in report["failures"]))
        report = self.run_mode("e0", avatar_of=edited(lambda human: human["m_HumanBoneIndex"].__setitem__(22, 3)))
        self.assertTrue(any("E0a slot mapping ['LeftEye']" in line for line in report["failures"]))
        report = self.run_mode("e0", avatar_of=edited(
            lambda human: human["m_RootX"]["t"].update(y=human["m_RootX"]["t"]["y"] + 1e-4)))
        self.assertTrue(any("E0c m_RootX.t" in line for line in report["failures"]))
        report = self.run_mode("e0", avatar_of=edited(lambda human: human.update(m_Scale=human["m_Scale"] * 1.001)))
        self.assertTrue(any("E0c m_Scale" in line for line in report["failures"]))
        for malformed in ({"t": "f32s", "v": "00000000"}, {"t": "f32s", "v": "7f800000" * 25}):
            with self.assertRaisesRegex(ValueError, "m_HumanBoneMass"):
                self.run_mode("e0", avatar_of=edited(
                    lambda human, value=malformed: human.update(m_HumanBoneMass=value)))
        with self.assertRaisesRegex(ValueError, "give --unityrs"):
            self.run_mode("e0", avatar_of=None)

    def test_e2_fails_a_turned_read_back(self):
        def turn(doc):
            row = doc["spec"]["evidence"]["editor"]["body_samples"][3]
            rotation = multiply(euler((0, .1, 0), 4), [F32(b) for b in row["readback_rotation"]])
            row["readback_rotation"] = [BITS(v) for v in rotation]

        report = self.run_mode("e2", self.edited("body-targets-tall", turn))
        self.assertEqual(len(report["failures"]), 2)
        self.assertIn("model to read-back", report["failures"][0])
        self.assertIn("request to read-back", report["failures"][1])
        # A clip has no request: only the model residual.
        report = self.run_mode("e2", self.edited("body-clip-level", turn))
        self.assertEqual(len(report["failures"]), 1)

    def test_e3a_fails_a_moved_or_turned_hips(self):
        def move(doc):
            pose = doc["capture"]["samples"][4]["pose"]["transforms"][hips_index(doc)]
            pose["translation"][0] = BITS(F32(pose["translation"][0]) + 5e-4)

        report = self.run_mode("e3a", self.edited("body-probes-compact", move))
        self.assertEqual(len(report["failures"]), 1)
        self.assertIn("hips translation off", report["failures"][0])
        self.assertGreater(report["worst_ratio"], 1)

        def turn(doc):
            pose = doc["capture"]["samples"][2]["pose"]["transforms"][hips_index(doc)]
            pose["rotation"] = [BITS(v) for v in multiply([F32(b) for b in pose["rotation"]], euler((.1, 0, 0), 4))]

        report = self.run_mode("e3a", self.edited("body-chain-armature-targets", turn))
        self.assertTrue(any("hips rotation off" in line for line in report["failures"]))
        with self.assertRaisesRegex(ValueError, "give --unityrs"):
            self.run_mode("e3a", avatar_of=None)

    def test_e4_fails_an_off_read_back_and_a_moved_start(self):
        def shift(index, delta):
            def edit(doc):
                row = doc["spec"]["evidence"]["editor"]["body_samples"][index]
                row["readback_position"][2] = BITS(F32(row["readback_position"][2]) + delta)
            return edit

        report = self.run_mode("e4", self.edited("body-clip-level", shift(2, 1e-3)))
        self.assertEqual(report["combinations"]["level"]["status"], "fail")
        self.assertEqual(len(report["failures"]), 1)
        # The start variant moved alone: its own sample and its twin pair both fail.
        report = self.run_mode("e4", self.edited("body-clip-keep-o0-y0-xz0-start", shift(1, 1e-3)))
        self.assertEqual(report["combinations"]["keep-o0-y0-xz0"], dict(status="fail", start_offset="fail"))
        self.assertTrue(any("start offset changed" in line for line in report["failures"]))
        # An unsupported combination's pair only informs.
        docs = dict(self.docs, **{"body-clip-keep-o1-y0-xz0-start": recording("body-clip-keep-o1-y0-xz0-start", 12)})
        docs = copy.deepcopy(docs)
        shift(1, 1e-2)(docs["body-clip-keep-o1-y0-xz0-start"])
        report = self.run_mode("e4", docs)
        self.assertTrue(report["passed"])
        self.assertEqual(report["combinations"]["keep-o1-y0-xz0"]["start_offset"], "fail")

    def test_e4_refuses_an_incomplete_clip(self):
        def strip(doc):
            del doc["spec"]["evidence"]["body_settings"]

        with self.assertRaisesRegex(ValueError, "not completed"):
            self.run_mode("e4", self.edited("body-clip-level", strip))

    def test_e3_against_a_known_vector_needs_its_target(self):
        rows = [dict(state=dict(target_kind=None))]
        with self.assertRaisesRegex(ValueError, "no sample with a body target"):
            check.e3(rows, None, {})


class SmokeCauses(unittest.TestCase):
    """The two input conventions the first run on owner recordings exposed, on synthetic recordings."""

    def test_the_recorded_masses_are_hierarchy_masses(self):
        doc = recording("body-probes-no-upper-chest", samples=1)
        rows = doc["spec"]["evidence"]["editor"]["human_bones"]
        hips = next(row for row in rows if row["name"] == "Hips")
        # GetBoneDefaultHierarchyMass: the Hips hold the whole body's default mass.
        self.assertEqual(F32(hips["mass"]), sum(support.MASSES[name] for name in tolerance.BODY_SLOTS))
        parents, masses = adequacy.bone_table([doc])
        self.assertEqual(masses, {name: support.MASSES[name] for name in tolerance.BODY_SLOTS})
        # The finger bones are not read (Unity 6 returns unspecified values for them).
        next(row for row in rows if row["name"] == "Finger 0")["mass"] = "7fc00000"
        self.assertEqual(adequacy.bone_table([doc])[1], masses)
        # A table whose Hips hold less than their children is not a hierarchy table.
        hips["mass"] = BITS(support.MASSES["Hips"])
        with self.assertRaisesRegex(ValueError, "Hips is below its children"):
            adequacy.bone_table([doc])

    def test_a_scaled_animators_read_back_is_scaled_back(self):
        doc = recording("body-chain-scaled-animator-targets", samples=4)
        parents, masses = adequacy.bone_table([doc])
        rows = list(adequacy.samples("body-chain-scaled-animator-targets", doc, parents, masses))
        for row in rows:
            state = row["state"]
            self.assertEqual(state["root"].scale, 1.25)
            points = {name: frame.position for name, frame in state["bones"].items()}
            want = model.centre(points, state["weights"], state["links"], coefficients(state["links"]))
            recorded = tolerance.decode(state["readback"]["readback_position"], 3, F32)
            read = tolerance.readback_position(state["readback"], state["root"], F32)
            self.assertLess(max(abs(state["scale"] * a - b) for a, b in zip(read, want)), 1e-6)
            self.assertGreater(max(abs(state["scale"] * a - b) for a, b in zip(recorded, want)), .1)
            # So E1's right-hand side is the model's coefficient columns at the true coefficients.
            a = coefficients(state["links"])
            fitted = [sum(a[key] * column[k] for key, column in row["columns"].items()) for k in range(3)]
            self.assertLess(math.dist(fitted, runs.rhs(row)), 1e-6)


class FitCheck(unittest.TestCase):
    """E1 on a design that reaches every training rig's edges."""

    @classmethod
    def setUpClass(cls):
        names = [name for name, info in body.BODY_INFO.items()
                 if info["kind"] == "probes" and info["rig"] != body.HOLDOUT]
        full = {"body-probes-compact", *body.EXTRA_CASES}
        cls.docs = {name: recording(name, samples=None if name in full else 10) for name in names}
        cls.docs["body-targets-tall-offset"] = recording("body-targets-tall-offset")
        cls.docs["body-chain-armature-clip"] = recording("body-chain-armature-clip")

    def e1(self, docs):
        folder = scratch.for_case(self)
        write_run(folder, docs)
        return freeze.run_mode("e1", [folder], avatar_of=avatars)

    def test_e1_passes_and_recovers_the_coefficients(self):
        report = self.e1(self.docs)
        self.assertEqual(report["failures"], [])
        self.assertEqual(report["rank"], len(report["coefficients"]))
        self.assertLess(report["condition"], 1e6)
        self.assertTrue(all(abs(a - .4) < 1e-4 for a in report["coefficients"].values()))
        self.assertGreater(report["terminal_samples"], 5)
        self.assertLess(report["worst_residual"], 1)

    def test_e1_fails_an_off_read_back_a_large_coefficient_and_a_moving_terminal(self):
        docs = copy.deepcopy(self.docs)
        row = docs["body-probes-tall"]["spec"]["evidence"]["editor"]["body_samples"][5]
        row["readback_position"][0] = BITS(F32(row["readback_position"][0]) + 1e-3)
        report = self.e1(docs)
        self.assertTrue(any("fit residual" in line for line in report["failures"]))
        # Coefficients of 1.5 are outside the centre-of-mass bound's |a| <= 1.
        docs = {name: recording(name, samples=6, value=1.5) for name in ("body-probes-compact", "body-probes-tall")}
        report = self.e1(docs)
        self.assertTrue(any("outside |a| <= 1.0" in line for line in report["failures"]))
        # A sample turning only the left hand moved the hips.
        docs = copy.deepcopy(self.docs)
        doc = docs["body-probes-compact"]
        names = [m["name"] for m in doc["capture"]["humanoid"]["muscles"]]
        index = next(i for i, sample in enumerate(doc["capture"]["samples"])
                     if {names[k] for k, b in enumerate(sample["muscles_f32"]) if F32(b)} == {"Left Hand Down-Up"})
        pose = doc["capture"]["samples"][index]["pose"]["transforms"][hips_index(doc)]
        pose["translation"][1] = BITS(F32(pose["translation"][1]) + 1e-3)
        report = self.e1(docs)
        self.assertTrue(any("terminal ['LeftHand'] moved the hips" in line for line in report["failures"]))

    def test_e1_reports_a_short_design(self):
        report = self.e1({name: self.docs[name] for name in ("body-probes-compact",)})
        self.assertFalse(report["passed"])
        self.assertIn("the adequacy checkpoint is short", report["failures"])
        self.assertTrue(report["checkpoint"]["coverage"]["missing"])
        json.dumps(freeze.clean(report), allow_nan=False)
        self.assertTrue(math.isfinite(report["worst_residual"]))


if __name__ == "__main__":
    unittest.main()
