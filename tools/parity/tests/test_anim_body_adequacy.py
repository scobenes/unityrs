"""B1's tolerances and adequacy checkpoint for the humanoid body model, on synthetic rigs."""

import contextlib
import io
import json
import math
import unittest
from unittest import mock

import anim_body_support as support
from anim_body_support import body_doc
from anim_fixture_support import RUN, write

import anim_body_adequacy as adequacy  # noqa: E402  (test_anim_replay puts it on the path)
import anim_body_tolerance as tolerance  # noqa: E402
import anim_fixture_body as body  # noqa: E402
import anim_replay as replay  # noqa: E402
import scratch  # noqa: E402


class LinearAlgebraTests(unittest.TestCase):
    def test_singular_values_of_orthogonal_columns(self):
        # Columns of norms 3, 2 and 1, turned in the plane of the first two rows.
        c, s = math.cos(.3), math.sin(.3)
        rows = [[3 * c, -2 * s, 0], [3 * s, 2 * c, 0], [0, 0, 1], [0, 0, 0]]
        sigma, vectors = adequacy.jacobi_svd(adequacy.householder_r(rows, 3), 3)
        self.assertEqual([round(v, 12) for v in sorted(sigma)], [1, 2, 3])
        for k in range(3):
            self.assertAlmostEqual(sum(v * v for v in vectors[k]), 1, places=12)

    def test_a_dependent_column_is_rank_deficient(self):
        rows = [[1, 2, 3], [4, 5, 9], [7, 8, 15], [1, 0, 1]]
        sigma, _ = adequacy.jacobi_svd(adequacy.householder_r(rows, 3), 3)
        self.assertLess(min(sigma), max(sigma) * 1e-13)

    def test_uncertainty_is_the_pseudo_inverse_row_norm(self):
        # One coefficient: A_w^+ = A_w^T / |A_w|^2, so u = |J| * sum|A_w| / |A_w|^2.
        rows = [dict(fit=True, epsilon=.5, tau=1., parent_scale=1., name="r", local=0,
                     columns={"Hips>Spine": (1., 2., 0.)}, jacobian={"Hips>Spine": (.5, 0., 0.)})]
        report = adequacy.adequacy(rows, {"recorded": [], "missing": []})
        weighted = [2., 4., 0.]
        expected = .5 * sum(map(abs, weighted)) / sum(v * v for v in weighted)
        self.assertEqual(report["rank"], 1)
        self.assertEqual(report["condition"], 1)
        self.assertAlmostEqual(report["worst_ratio"], expected, places=15)
        self.assertEqual(report["necessary"], [])


class ToleranceTests(unittest.TestCase):
    def test_edges_and_folded_masses(self):
        parents = support.PARENT
        compact = body.rig_bones("compact")
        links = tolerance.edges(parents, compact)
        self.assertIn(("Hips", "Spine"), links)
        self.assertIn(("UpperChest", "Neck"), links)
        self.assertNotIn("LeftHand", {i for i, _ in links})
        mapped = body.rig_bones("no-chest")
        self.assertEqual({j for i, j in tolerance.edges(parents, mapped) if i == "Spine"},
                         {"Neck", "LeftShoulder", "RightShoulder"})
        masses = {name: support.MASSES[name] for name in tolerance.BODY_SLOTS}
        folded = tolerance.folded_masses(masses, parents, mapped)
        self.assertAlmostEqual(sum(folded.values()), 1, places=15)
        total = sum(masses.values())
        eyes_jaw = sum(masses[name] for name in ("LeftEye", "RightEye", "Jaw"))
        self.assertAlmostEqual(folded["Spine"], (masses["Spine"] + masses["Chest"] + masses["UpperChest"]) / total)
        self.assertAlmostEqual(folded["Head"], (masses["Head"] + eyes_jaw) / total)

    def test_bounds_are_positive_and_grow_with_depth(self):
        doc = body_doc("body-probes-compact", RUN, samples=1)
        nodes, sample = doc["spec"]["nodes"], doc["capture"]["samples"][0]
        frames = tolerance.kinematics(nodes, sample["pose"]["transforms"], replay.f32_from_bits)
        bones = tolerance.bone_frames(doc["capture"]["humanoid"]["bones"], nodes, frames)
        self.assertLess(tolerance.position_error(bones["Spine"]), tolerance.position_error(bones["LeftHand"]))
        self.assertEqual(bones["LeftHand"].depth, 8)  # Hips, Spine, Chest, UpperChest, Shoulder, three arm links
        links = tolerance.edges(support.PARENT, set(bones))
        masses = tolerance.folded_masses({n: support.MASSES[n] for n in tolerance.BODY_SLOTS}, support.PARENT,
                                         set(bones))
        epsilon = tolerance.measurement_tolerance(bones, masses, links, 1., (0., 1., 0.), (0., 1., 0.),
                                                  ((0., 0., 0.), 1.))
        tau = tolerance.hips_tolerance(bones, masses, links, 1., (0., 1., 0.), frames["0"], (0., .88, 0.))
        self.assertTrue(0 < epsilon < 1e-3 and 0 < tau < 1e-3, (epsilon, tau))
        with self.assertRaisesRegex(ValueError, "nonuniform"):
            tolerance.uniform((1., 1., 1.5))


class CheckpointTests(unittest.TestCase):
    def setUp(self):
        self.folder = scratch.for_case(self)
        (self.folder / "replays" / "body").mkdir(parents=True)

    def record(self, names, samples=10):
        for name in names:
            write(self.folder / "replays" / "body" / f"{name}.pending.json", body_doc(name, RUN, samples=samples))

    def run_checkpoint(self):
        out = io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(io.StringIO()):
            code = adequacy.main([str(self.folder), "--json", str(self.folder / "report.json")])
        report = json.loads((self.folder / "report.json").read_text(encoding="utf-8")) if code != 2 else None
        return code, report, out.getvalue()

    def test_every_rig_gives_full_rank(self):
        self.record([name for name, info in body.BODY_INFO.items() if info["kind"] == "probes"]
                    + ["body-chain-armature-clip", "body-targets-tall-offset"])
        code, report, text = self.run_checkpoint()
        self.assertIn(code, (0, 1))
        self.assertEqual(report["coverage"]["missing"], [])
        self.assertEqual(report["rank"], len(report["coefficients"]))
        self.assertIn("Spine>Neck", report["coefficients"])
        self.assertLess(report["condition"], adequacy.CONDITION_LIMIT)
        self.assertEqual(report["untrained"], [])
        self.assertEqual(report["adequate"], adequacy.adequate(report))
        self.assertIn(f"rank {report['rank']}/{report['rank']}", text)

    def test_a_coplanar_branch_stays_free(self):
        rigs = [name for name, info in body.BODY_INFO.items() if info["kind"] == "probes"]
        with mock.patch.object(support, "NONCOPLANAR", {"noncoplanar"}):
            self.record(rigs)
        code, report, _ = self.run_checkpoint()
        self.assertEqual(code, 1)
        self.assertEqual(report["rank"], len(report["coefficients"]) - 1)
        self.assertEqual(set(report["free"][0]), {"Spine>Neck", "Spine>LeftShoulder", "Spine>RightShoulder"})
        self.assertIsNone(report["condition"])
        self.assertFalse(report["adequate"])

    def test_reports_missing_topologies_and_untrained_edges(self):
        self.record(["body-probes-compact", "body-probes-holdout", "body-probes-eyes"], samples=6)
        (self.folder / "replays" / "body" / "body-probes-eyes.pending.json").rename(
            self.folder / "replays" / "body" / "body-probes-eyes.json")
        code, report, _ = self.run_checkpoint()
        self.assertEqual(code, 1)
        self.assertIn("no-chest", report["coverage"]["missing"])
        self.assertEqual(report["untrained"], [])
        doc = body_doc("body-probes-jaw", RUN, samples=4)
        doc["spec"]["evidence"]["editor"]["body_samples"] = []
        write(self.folder / "replays" / "body" / "body-probes-jaw.pending.json", doc)
        _, report, _ = self.run_checkpoint()
        self.assertTrue(report["untrained"] and all("Head>Jaw" in row for row in report["untrained"]))

    def test_unreadable_input_exits_two(self):
        self.assertEqual(self.run_checkpoint()[0], 2)
        self.record(["body-probes-compact"], samples=3)
        doc = body_doc("body-probes-tall", RUN, samples=3)
        doc["spec"]["evidence"]["editor"]["human_bones"][0]["mass"] = replay.f32_bits(.5)
        write(self.folder / "replays" / "body" / "body-probes-tall.pending.json", doc)
        self.assertEqual(self.run_checkpoint()[0], 2)


if __name__ == "__main__":
    unittest.main()
