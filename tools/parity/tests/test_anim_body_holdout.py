"""The hold-out's adequacy predicted from its spec alone (anim_body_holdout.py).

The stand-in is the hold-out rig's own pose: its base rig's recorded
rotations on the builder's layout. Its rows give the same E_unity,s and
J_s, and so the same u_s and margin at every sample, as a recording of
the hold-out rig that follows the model, so the freeze's margin flags a
sample E3b would fail. With UNITYRS_E3B_ROUND9 (the two round-9 run folders,
joined by the path separator) and UNITYRS_E3B_READER (the unityrs CLI freeze
v7 pins), the prediction is also checked on the recorded training runs: it
flags round 9's failing hold-out sample from the spec alone, reading no
hold-out file."""

import copy
import json
import os
import unittest
from unittest import mock

import anim_body_model_support as support
from anim_body_support import body_doc
from anim_fixture_support import RUN

import anim_body_adequacy as adequacy  # noqa: E402  (test_anim_replay puts it on the path)
import anim_body_check as check  # noqa: E402
import anim_body_holdout as holdout  # noqa: E402
import anim_replay as replay  # noqa: E402
from anim_body_runs import Design  # noqa: E402
from anim_fixture_body import HOLDOUT, HOLDOUT_RIGS, primary_probes  # noqa: E402

NAME, SOURCE = primary_probes(HOLDOUT), primary_probes(holdout.source_rig(HOLDOUT))


def rows_of(name):
    def rows(doc, _):
        parents, masses = adequacy.bone_table([doc])
        return list(adequacy.samples(name, doc, parents, masses))
    return rows


class PredictionTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.source = replay.read_json(json.dumps(body_doc(SOURCE, RUN)))
        cls.own = replay.read_json(json.dumps(body_doc(NAME, RUN)))

    def test_the_standin_is_the_hold_out_rigs_own_pose(self):
        doc, avatar = holdout.standin(NAME, self.source, support.avatar(support.consistent(copy.deepcopy(self.source))))
        hips = next(row["node"] for row in doc["capture"]["humanoid"]["bones"] if row["name"] == "Hips")
        self.assertEqual(doc["capture"]["rest"]["transforms"], self.own["capture"]["rest"]["transforms"])
        self.assertEqual(len(doc["capture"]["samples"]), len(self.own["capture"]["samples"]))
        for ours, theirs in zip(doc["capture"]["samples"], self.own["capture"]["samples"], strict=True):
            self.assertEqual(ours["muscles_f32"], theirs["muscles_f32"])
            self.assertEqual([t for i, t in enumerate(ours["pose"]["transforms"]) if i != hips],
                             [t for i, t in enumerate(theirs["pose"]["transforms"]) if i != hips])
        self.assertNotIn("body_samples", doc["spec"]["evidence"]["editor"])
        # The Avatar's skeleton takes the hold-out rig's translations too.
        own = support.avatar(self.own)["m_Avatar"]["m_Human"]["data"]["m_SkeletonPose"]["data"]["m_X"]
        self.assertEqual([x["t"] for x in avatar["m_Avatar"]["m_Human"]["data"]["m_SkeletonPose"]["data"]["m_X"]],
                         [x["t"] for x in own])

    def test_the_prediction_reproduces_the_hold_outs_criterion(self):
        coefficients = support.coefficients(support.layout(self.source)[5])
        planned = holdout.planned(NAME, self.source, coefficients, rows_of(NAME))
        # The frame scope refuses the noncommuting anchor and a draw or two, as it would the recording's.
        framed = [row["in_frame"] for row in planned]
        self.assertLessEqual(framed.count(False), 3)
        # The hold-out recorded as the model has it, at the human scale and body target Unity gives the rig.
        recorded = copy.deepcopy(self.own)
        state = planned[0]["state"]
        scale = state["scale"]
        for target in recorded["spec"]["evidence"]["editor"]["body_targets"]:
            target["position"] = [replay.f32_bits(v) for v in state["target"]]
        with mock.patch.object(support, "HUMAN_SCALE", scale):
            recorded = support.consistent(recorded)
        actual = rows_of(NAME)(recorded, None)
        parents, masses = adequacy.bone_table([self.source])
        design = Design(list(adequacy.samples(SOURCE, support.consistent(copy.deepcopy(self.source)), parents, masses)))
        actual = [dict(row, in_frame=in_frame) for row, in_frame in zip(actual, framed, strict=True)]
        ours, theirs = (holdout.margins(rows, design.hips_uncertainty, NAME) for rows in (planned, actual))
        for a, b in zip(ours["samples"], theirs["samples"], strict=True):
            self.assertEqual(a["local"], b["local"])
            self.assertAlmostEqual(a["unity"] / b["unity"], 1, delta=1e-3)
            self.assertAlmostEqual(a["uncertainty"] / b["uncertainty"], 1, delta=1e-3)
        # A margin just over the worst flags the same one sample in both.
        worst = min(ours["samples"], key=lambda entry: entry["margin"])
        flagged = [holdout.margins(rows, design.hips_uncertainty, NAME, worst["margin"] * (1 + 1e-6))["failures"]
                   for rows in (planned, actual)]
        self.assertEqual([len(lines) for lines in flagged], [1, 1])
        self.assertTrue(all(lines[0].startswith(f"{NAME}@{worst['local']}:") for lines in flagged))

    def test_only_what_e3b_would_compare_counts_toward_its_minimum(self):
        coefficients = support.coefficients(support.layout(self.source)[5])
        planned = holdout.planned(NAME, self.source, coefficients, rows_of(NAME))
        framed = [row for row in planned if row["in_frame"]]
        planned[1]["refused"] = "topology"
        late = framed[-1]["local"]
        with mock.patch.object(check, "outside", lambda row, limit: limit is not None and row["local"] == late):
            found = holdout.margins(planned, lambda row: row["unity"] / 2, NAME, limit=.1, minimum=len(framed))
        self.assertEqual(found["in_frame"], len(framed) - 1 - (planned[1]["in_frame"]))
        self.assertNotIn(late, [entry["local"] for entry in found["samples"]])
        self.assertEqual(found["failures"], [f"{NAME}: {found['in_frame']} predicted in-scope comparison(s), under "
                                             f"the minimum {len(framed)}"])
        self.assertEqual(holdout.margins(planned, lambda row: row["unity"] / 2, NAME)["in_frame"],
                         len(framed) - planned[1]["in_frame"])

    def test_a_target_that_moves_or_a_rig_of_its_own_topology_is_refused(self):
        moved = copy.deepcopy(self.source)
        with mock.patch.object(holdout, "standin", lambda *args: (self.moved(), None)):
            with self.assertRaisesRegex(ValueError, "a hold-out target moves"):
                holdout.planned(NAME, moved, {}, rows_of(NAME))
        self.assertEqual({holdout.source_rig(rig) for rig in HOLDOUT_RIGS}, {"compact"})
        with mock.patch.dict(holdout.PROBE_RIGS, {HOLDOUT: ("compact", ("UpperChest",), ())}):
            with self.assertRaisesRegex(ValueError, "maps other bones than its base rig"):
                holdout.source_rig(HOLDOUT)

    def moved(self):
        doc = copy.deepcopy(self.own)
        doc["spec"]["evidence"]["editor"]["body_targets"][1]["offset"][0] = replay.f32_bits(.1)
        return doc


@unittest.skipUnless(os.environ.get("UNITYRS_E3B_ROUND9") and os.environ.get("UNITYRS_E3B_READER"),
                     "the round-9 training runs are not given")
class Round9Tests(unittest.TestCase):
    def test_the_prediction_flags_round_9s_failing_sample(self):
        import pathlib

        import anim_body_freeze as freeze
        import anim_body_seal as seal

        folders = [pathlib.Path(path) for path in os.environ["UNITYRS_E3B_ROUND9"].split(os.pathsep)]
        artifact = json.loads((seal.HERE / "anim_body_freeze_v7.json").read_text(encoding="utf-8"))
        avatar_of, clip_of = freeze.readers(os.environ["UNITYRS_E3B_READER"])
        runs, _, design = freeze.frozen_training(folders, artifact, avatar_of, clip_of)
        found = holdout.predicted(runs, design, artifact["coefficients"], avatar_of, rig=HOLDOUT_RIGS[0])
        self.assertEqual(sorted(found), artifact["scope"]["editor_versions"])
        for entry in found.values():
            [sample] = [row for row in entry["samples"] if row["local"] == 0.651947021484375]
            self.assertLess(sample["margin"], 1)
            self.assertTrue(any("@0.651947021484375:" in line for line in entry["failures"]))
