"""Round 12's exploration fit (anim_twist_fit) on synthetic rigs: a candidate whose equation made the recording is
feasible, one out of bound on any bone is not, feasible candidates apart in key leave a cell unresolved, and the
rows, settings and shards it reads are refused when they are not exactly the tables' and the run's."""

import re
import struct
import unittest
from pathlib import Path
from unittest import mock

import test_anim_replay_coverage  # noqa: F401  (puts tools/parity on the path)

import anim_twist_bound as bound  # noqa: E402
import anim_twist_fit as fit  # noqa: E402
import anim_twist_model as model  # noqa: E402
import anim_twist_tables as tables  # noqa: E402
from test_anim_twist_witness import rig_entry  # noqa: E402

LIMBS = (model.Limb(1, 1, 2, "+", 0), model.Limb(1, 2, 2, "+", 0), model.Limb(1, 3, 2, "+", 0))
FEET = (model.Foot(1, 0), model.Foot(2, 1))
CASE = "body-twist-compact-t5"


def bits(value):
    return struct.pack(">f", value).hex()


def vectors():
    twist = {m: .5 for m in model.LIMB_TWISTS}
    return ({}, twist, {41: -1., 39: .5, 23: .5, 27: .5}, {43: .5, 25: -.5})


def recording(rig, settings, limb, foot, values):
    """Every chain bone's rotation as `limb` and `foot` predict it, both sides."""
    found = {}
    for side in model.SIDES:
        found |= {bone: list(q) for bone, (q, _) in
                  model.row_chains(rig["rig"], list(values), [0.] * 95, settings, limb, foot, side).items()}
    return found


def as_values(vector):
    values = [0.] * 95
    for m, v in vector.items():
        values[m] = v
    return tuple(values)


def cases_of(rig, settings, limb, foot):
    rows = [(1 + k, as_values(v), recording(rig, settings, limb, foot, as_values(v)))
            for k, v in enumerate(vectors())]
    return {CASE: (rig, settings, rows)}


class FitTests(unittest.TestCase):
    def setUp(self):
        patch = mock.patch.multiple(model, LIMBS=LIMBS, FEET=FEET)
        patch.start()
        self.addCleanup(patch.stop)
        self.rig, self.settings = rig_entry(11), tables.TUPLES["t5"]

    def shard(self, cases, k, n):
        found = fit.fit_rows(cases, (k, n), log=lambda *_: None)
        return dict(format=fit.FORMAT, run=dict(run_id="r", editor="2022.3.62f3"), tables="t", python="3",
                    candidates_count=len(model.candidates()), rigs={}, **found)

    def test_the_recording_candidate_is_feasible(self):
        cases = cases_of(self.rig, self.settings, LIMBS[0], FEET[0])
        merged = fit.merge([self.shard(cases, k, 2) for k in (1, 0)])
        self.assertEqual(merged[0]["out"] + merged[0]["refused"], 0)
        self.assertGreater(merged[0]["worst"][0], 0)
        # S3 splits otherwise at w = .5: out of bound somewhere.
        self.assertGreater(merged[model.candidates().index((LIMBS[2], FEET[0]))]["out"], 0)

    def test_one_bone_out_of_bound_makes_it_infeasible(self):
        cases = cases_of(self.rig, self.settings, LIMBS[0], FEET[0])
        recorded = cases[CASE][2][2][2]
        recorded[15] = list(bound.multiply(recorded[15], (0., 0., .0005, 1.)))
        found = fit.fit_rows(cases, log=lambda *_: None)["candidates"][0]
        self.assertEqual(found["out"], 1)
        self.assertEqual(found["worst"][1:], [CASE, 3, "Left", 15])
        self.assertLess(found["worst"][0], 0)

    def test_feasible_candidates_apart_in_key_leave_the_cell_unresolved(self):
        rigs = {"compact": self.rig}
        with mock.patch.object(fit.witness, "ADMISSION", (("t0", ("compact",)), ("t25", ("compact",)))):
            found = {c["tuple"]: c for c in fit.cells([0, 2], rigs)}
        # D1's S1 and S2 are one equation at w = 0 and two at w = .25.
        self.assertEqual((found["t0"]["equations"], found["t0"]["unresolved"]), (1, False))
        self.assertEqual((found["t25"]["equations"], found["t25"]["unresolved"]), (2, True))
        self.assertEqual(found["t25"]["members"], [[0], [2]])

    def test_combine_reports_one_editor(self):
        cases = cases_of(self.rig, self.settings, LIMBS[0], FEET[0])
        shards = [self.shard(cases, 0, 1)]
        with mock.patch.object(fit.witness, "ADMISSION", (("t0", ("compact",)),)):
            result = fit.combine(shards, {"compact": self.rig}, dict(editor_version="2022.3.62f3", run_id="q"))
            self.assertIn(0, [c["index"] for c in result["feasible_candidates"]])
            with self.assertRaisesRegex(ValueError, "not 2022"):
                fit.combine(shards, {"compact": self.rig}, dict(editor_version="6000.6.9f1", run_id="q"))

    def test_shards_must_cover_one_run_once(self):
        cases = cases_of(self.rig, self.settings, LIMBS[0], FEET[0])
        first, second = self.shard(cases, 0, 2), self.shard(cases, 1, 2)
        with self.assertRaisesRegex(ValueError, "each of"):
            fit.merge([first, first])
        with self.assertRaisesRegex(ValueError, "each of"):
            fit.merge([first])
        with self.assertRaisesRegex(ValueError, "differ in run"):
            fit.merge([first, dict(second, run=dict(run_id="other", editor="2022.3.62f3"))])


class RowTests(unittest.TestCase):
    ROW = dict(set="toe-absent", tuple="t0")

    def capture(self):
        names = [f"m{k}" for k in range(95)]
        for name, m in tables.NAMES.items():
            names[m] = name
        samples = []
        found = tables.toe_absent()
        for k in range(len(found) + 3):
            vector = {}
            for j, v in enumerate(found):
                if tables.sample_index(j, len(found)) == k:
                    vector = v
            samples.append(dict(muscles_f32=[bits(x) for x in as_values(vector)],
                                pose=dict(transforms=[dict(rotation=[bits(0.), bits(0.), bits(0.), bits(1.)])])))
        humanoid = dict(muscles=[dict(name=n) for n in names], bones=[dict(node=0)] * 55,
                        upper_arm_twist=bits(0.), lower_arm_twist=bits(0.), upper_leg_twist=bits(0.),
                        lower_leg_twist=bits(0.))
        return dict(samples=samples, humanoid=humanoid)

    def test_rows_are_the_tables_vectors_at_their_samples(self):
        rows = fit.case_rows("c", self.ROW, self.capture(), dict(rig={1: None, 5: None}))
        self.assertEqual(len(rows), len(tables.toe_absent()))
        self.assertEqual(rows[0][0], 1)
        self.assertEqual(rows[0][1], as_values(tables.toe_absent()[0]))
        self.assertEqual(rows[0][2], {1: [0., 0., 0., 1.], 5: [0., 0., 0., 1.]})

    def test_a_sample_other_than_its_vector_refuses(self):
        capture = self.capture()
        capture["samples"][2]["muscles_f32"][27] = bits(.75)
        with self.assertRaisesRegex(ValueError, "sample 2 records muscles other than vector 1"):
            fit.case_rows("c", self.ROW, capture, dict(rig={1: None}))

    def test_settings_must_be_the_tuples(self):
        capture = self.capture()
        avatar = dict(m_Avatar=dict(m_Human=dict(m_ArmTwist=0., m_ForeArmTwist=0., m_UpperLegTwist=0.,
                                                 m_LegTwist=0.)))
        self.assertEqual(fit.settings_of("c", self.ROW, capture, avatar), (0.,) * 4)
        avatar["m_Avatar"]["m_Human"]["m_LegTwist"] = .5
        with self.assertRaisesRegex(ValueError, "m_LegTwist"):
            fit.settings_of("c", self.ROW, capture, avatar)
        avatar["m_Avatar"]["m_Human"]["m_LegTwist"] = 0.
        capture["humanoid"]["upper_arm_twist"] = bits(.5)
        with self.assertRaisesRegex(ValueError, "upper_arm_twist"):
            fit.settings_of("c", self.ROW, capture, avatar)

    def test_the_fit_touches_no_seal_and_no_sealed_partition(self):
        source = Path(fit.__file__).read_text(encoding="utf-8")
        code = source.split('"""', 2)[2]
        self.assertIsNone(re.search(r"\b(record_exploration|open_selection|claim|bind|_evaluate|evaluate|"
                                    r"require_claim|sealed_views|round11_own_view)\b", code))
        self.assertNotIn("allowed=", code)
        self.assertEqual(fit.exploration_cases(), sorted(
            case for case, row in tables.cases().items() if row[3] == "exploration"))


if __name__ == "__main__":
    unittest.main()
