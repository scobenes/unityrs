"""B1's tolerances and adequacy checkpoint for the humanoid body model, on synthetic rigs."""

import contextlib
import copy
import io
import json
import math
import unittest
from unittest import mock

import anim_body_support as support
from anim_body_support import body_doc
from anim_fixture_support import RUN, write

import anim_body_adequacy as adequacy  # noqa: E402  (test_anim_replay puts it on the path)
import anim_body_design as design  # noqa: E402
import anim_body_tolerance as tolerance  # noqa: E402
import anim_fixture_body as body  # noqa: E402
from anim_reference import angle, euler  # noqa: E402
import anim_replay as replay  # noqa: E402
import scratch  # noqa: E402

G = tolerance.g


def row(name, column, jacobian, epsilon, unity, evaluation=0., fit=True):
    return dict(name=name, local=0, fit=fit, epsilon=epsilon, unity=unity, evaluation=evaluation,
                columns={"Hips>Spine": column}, jacobian={"Hips>Spine": jacobian})


def checkpoint(rows):
    return adequacy.adequacy(rows, {"recorded": [], "missing": []})


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

    def test_uncertainty_is_the_block_norm_of_the_pseudo_inverse(self):
        # One coefficient: A_w^+ = A_w^T / |A_w|^2, one block, so u = |J| |A_w| / |A_w|^2 = |J| / |A_w|,
        # against E_unity = 1; E3's tolerance is T = E_unity + u + E_eval.
        report = checkpoint([row("r", (1., 2., 0.), (.5, 0., 0.), .5, 1., .25)])
        u = .5 / math.hypot(2., 4.)
        self.assertEqual((report["rank"], report["condition"]), (1, 1))
        self.assertAlmostEqual(report["worst_margin"], 1 / u, places=13)
        self.assertEqual(report["margins"]["p0"], report["worst_margin"])
        self.assertAlmostEqual(report["samples"][0]["tolerance"], 1 + u + .25, places=15)

    def test_the_direct_check_needs_no_unity_error_against_epsilon_veto(self):
        # Sample b's own row is weak, but a's strong row pins the coefficient:
        # A_w^+ = (1, .1) / 1.01 over the two blocks, so u_b = .1 (1 + .1) / 1.01 = .109,
        # within E_unity,b = .25, though E_unity,b < 2 epsilon_b.
        rows = [row("a", (1., 0., 0.), (1., 0., 0.), 1., 4.), row("b", (.1, 0., 0.), (.1, 0., 0.), 1., .25, .1)]
        report = checkpoint(rows)
        self.assertEqual(report["uncertainty_failures"], [])
        self.assertTrue(adequacy.adequate(report))
        self.assertLess(rows[1]["unity"], 2 * rows[1]["epsilon"])
        self.assertAlmostEqual(report["worst_margin"], min(4 / (1.1 / 1.01), .25 / (.11 / 1.01)), places=12)

    def test_a_short_design_is_reported_short(self):
        # u = .1 / .1 = 1: within an E_unity of 1, but not of .5. E_eval is T's, not the criterion's.
        self.assertEqual(checkpoint([row("weak", (.1, 0., 0.), (.1, 0., 0.), 1., 1., 5.)])["uncertainty_failures"], [])
        report = checkpoint([row("weak", (.1, 0., 0.), (.1, 0., 0.), 1., .5)])
        self.assertEqual(len(report["uncertainty_failures"]), 1)
        self.assertAlmostEqual(report["worst_margin"], .5)
        self.assertFalse(adequacy.adequate(report))
        # The inequality itself decides: u = 0 passes, with no finite margin; a NaN bound fails.
        rows = [row("pinned", (1., 0., 0.), (1., 0., 0.), 1., 2.), row("still", (0., 1., 0.), (0., 0., 0.), 1., 0.)]
        report = checkpoint(rows)
        self.assertEqual(report["uncertainty_failures"], [])
        self.assertEqual(len(report["samples"]), 2)
        self.assertEqual(len(checkpoint([row("nan", (1., 0., 0.), (1., 0., 0.), 1., math.nan)])["uncertainty_failures"]), 1)

    def test_the_jacobian_is_the_hips_local_derivative_through_a_turned_scaled_parent(self):
        parent = tolerance.Frame((.3, .2, -.1), euler((-90, 30, 0), 4), .8, 0., 1)
        column = (.02, -.05, .03)
        inverse = (*(-v for v in parent.rotation[:3]), parent.rotation[3])

        def hips_local(a):
            # P_A = h T_A - (c_A(a) - p_hips) with c_A linear in a; t = sigma_p^-1 R_p^-1 (P_A - t_p).
            p = tuple(1. - (a * v) for v in column)
            return tuple(v / parent.scale for v in tolerance.rotate(inverse, tolerance.sub(p, parent.position)))

        step = 1e-3
        numeric = tuple((b - a) / step for a, b in zip(hips_local(0.), hips_local(step)))
        for want, got in zip(numeric, adequacy.hips_sensitivity(column, parent)):
            self.assertAlmostEqual(want, got, places=10)
        # With the solved orientation, the column turns by H_A H_rec^-1 first.
        solved = tolerance.quaternion_matrix(euler((0, 0, 40), 4))
        turned = tuple(sum(solved[r][k] * column[k] for k in range(3)) for r in range(3))
        for want, got in zip(adequacy.hips_sensitivity(turned, parent), adequacy.hips_sensitivity(column, parent, solved)):
            self.assertAlmostEqual(want, got, places=15)

    def test_the_solved_orientation_follows_the_target_body_rotation(self):
        # At the T-pose, H_A H_rec^-1 = B_A F(T) F(T)^-1 = B_A.
        doc = body_doc("body-probes-compact", RUN, samples=1)
        nodes = doc["spec"]["nodes"]
        rest = tolerance.bone_frames(doc["capture"]["humanoid"]["bones"], nodes,
                                     tolerance.kinematics(nodes, doc["capture"]["rest"]["transforms"], replay.f32_from_bits))
        frame = tolerance.body_frame({name: bone.position for name, bone in rest.items()})
        body_rotation = euler((5, 30, -10), 4)
        got = adequacy.solved_turn(body_rotation, frame, rest)
        for want_row, got_row in zip(tolerance.quaternion_matrix(body_rotation), got):
            for want, value in zip(want_row, got_row):
                self.assertAlmostEqual(want, value, places=12)
        # The synthetic read-back is the model's own body rotation, so the solved and recorded hips agree.
        parents, masses = adequacy.bone_table([replay.read_json(json.dumps(doc))])
        sample = next(adequacy.samples("body-probes-compact", replay.read_json(json.dumps(doc)), parents, masses))
        self.assertGreater(sample["unity"], 0)


class CheckpointTests(unittest.TestCase):
    def setUp(self):
        self.folder = scratch.for_case(self)
        (self.folder / "replays" / "body").mkdir(parents=True)
        self.recorded = set()

    def record(self, names, samples=10, edit=None):
        """Each case's recording at its case table path, and run.json listing every one recorded so far."""
        for name in names:
            doc = body_doc(name, RUN, samples=samples)
            doc["spec"]["bundles"] = [dict(name=body.BODY_CASES[name].bundle)]
            if edit:
                edit(doc)
            write(self.folder / "replays" / "body" / f"{name}.pending.json", doc)
            self.recorded.add(name)
        cases = [dict(name=name, status="recorded", reason="") for name in sorted(self.recorded)]
        write(self.folder / "run.json", dict(format="unityrs.fixture_run", version=1, run=RUN, cases=cases))

    def run_checkpoint(self):
        out = io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(io.StringIO()):
            code = adequacy.main([str(self.folder), "--json", str(self.folder / "report.json")])
        report = json.loads((self.folder / "report.json").read_text(encoding="utf-8")) if code != 2 else None
        return code, report, out.getvalue()

    def probes(self):
        return [name for name, info in body.BODY_INFO.items() if info["kind"] == "probes"]

    def design(self):
        # Ten samples of each probe case reach the trunk probes; the targets' combined vectors move the limbs.
        return self.probes() + ["body-chain-armature-clip", "body-targets-tall-offset"]

    def test_every_rig_gives_full_rank_and_the_whole_inventory(self):
        self.record(self.design())
        code, report, text = self.run_checkpoint()
        self.assertIn(code, (0, 1))
        self.assertEqual(report["coverage"]["missing"], [])
        self.assertEqual(report["rank"], len(report["coefficients"]))
        self.assertIn("Chest>Neck", report["coefficients"])
        self.assertLess(report["condition"], adequacy.CONDITION_LIMIT)
        self.assertEqual((report["untrained"], report["edge_mismatches"], report["missing_coefficients"]), ([], [], []))
        self.assertNotIn("necessary", report)
        self.assertEqual(report["adequate"], adequacy.adequate(report))
        self.assertIn(f"rank {report['rank']}/{report['rank']}", text)

    def test_out_of_scope_rigs_are_refused_not_fitted(self):
        # The frozen topology scope: Chest or Neck absent, or one Shoulder, is never fitted or credited.
        self.record(self.design())
        _, report, _ = self.run_checkpoint()
        self.assertEqual(adequacy.TRAINING_RIGS & {"no-chest", "no-neck", "no-left-shoulder", "no-right-shoulder"},
                         set())
        refused = {line.split(": ", 1)[1] for line in report["refused"]}
        self.assertEqual(refused, {"topology: Chest absent", "topology: Neck absent", "topology: exactly one Shoulder"})
        self.assertFalse({"Spine>Neck", "UpperChest>Head"} & set(report["coefficients"]))
        self.assertFalse(any(sample["name"].startswith("body-probes-no-neck") for sample in report["samples"]))

    def test_coverage_comes_only_from_probe_cases(self):
        # A clip on the compact rig, the hold-out (never read), a probe case without
        # read-back and one whose Avatar lost a bone credit nothing.
        self.record(["body-clip-level", "body-probes-holdout"], samples=6)
        self.record(["body-probes-eyes"], samples=4,
                    edit=lambda doc: doc["spec"]["evidence"]["editor"].update(body_samples=[]))

        def drop_jaw(doc):
            next(row for row in doc["capture"]["humanoid"]["bones"] if row["name"] == "Jaw")["node"] = -1
        self.record(["body-probes-jaw"], samples=4, edit=drop_jaw)
        code, report, _ = self.run_checkpoint()
        self.assertEqual(code, 1)
        self.assertEqual(report["coverage"]["recorded"], [])
        self.assertIn("compact", report["coverage"]["missing"])
        self.assertNotIn("holdout", report["coverage"]["missing"])
        self.assertIn("Chest>Neck", report["missing_coefficients"])
        self.assertTrue(report["untrained"] and all("Head>" in line for line in report["untrained"]))

    def test_an_edge_off_the_hierarchy_is_reported(self):
        def swap(doc):
            bones = {row["name"]: row for row in doc["capture"]["humanoid"]["bones"]}
            bones["LeftHand"]["node"], bones["LeftLowerArm"]["node"] = (bones["LeftLowerArm"]["node"],
                                                                       bones["LeftHand"]["node"])
        self.record(["body-probes-compact"], samples=4, edit=swap)
        _, report, _ = self.run_checkpoint()
        self.assertTrue(report["edge_mismatches"])
        self.assertFalse(report["adequate"])

    def test_the_design_mode_predicts_an_extra_probe_case_from_its_rig(self):
        # Synthetic poses turn each bone by its own muscles, as Unity does, so the prediction is the recording.
        name = sorted(body.EXTRA_CASES)[0]
        rig = body.EXTRA_RIGS[name]
        source = replay.read_json(json.dumps(body_doc(f"body-probes-{rig}", RUN)))
        predicted = design.predict(name, source)
        recorded = body_doc(name, RUN)
        nodes = {row["name"]: row["node"] for row in recorded["capture"]["humanoid"]["bones"] if row["node"] >= 0}
        self.assertEqual([point["local"] for point in predicted["spec"]["schedule"]],
                         [point["local"] for point in recorded["spec"]["schedule"]])
        for got, want in zip(predicted["capture"]["samples"], recorded["capture"]["samples"]):
            self.assertEqual(got["muscles_f32"], want["muscles_f32"])
            for bone, node in nodes.items():
                if bone != "Hips":
                    self.assertEqual(got["pose"]["transforms"][node]["rotation"],
                                     want["pose"]["transforms"][node]["rotation"], (got["local"], bone))
        # A pose the rig's probe case never recorded cannot be predicted.
        short = replay.read_json(json.dumps(body_doc(f"body-probes-{rig}", RUN, samples=4)))
        with self.assertRaisesRegex(ValueError, "no recorded pose of"):
            design.predict(name, short)

    def test_the_design_mode_predicts_a_range_sweep_from_its_rigs_probes(self):
        # Each spine bone turns by Unity's muscle map rebuilt from its single-muscle probes; the rest stay neutral.
        name, rig = "body-probes-range-compact", "compact"
        source = replay.read_json(json.dumps(body_doc(f"body-probes-{rig}", RUN)))
        capture = source["capture"]
        bones = {row["name"]: row["node"] for row in capture["humanoid"]["bones"] if row["node"] >= 0}
        spine = design.muscle_map(capture, bones)
        self.assertEqual(set(spine), {"Spine", "Chest", "UpperChest"})
        for sample in capture["samples"]:
            vector = [replay.f32_from_bits(bits) for bits in sample["muscles_f32"]]
            for bone, entry in spine.items():
                if sum(1 for i in entry[3] if vector[i]) == 1 and sum(1 for v in vector if v) == 1:
                    want = [replay.f32_from_bits(b) for b in sample["pose"]["transforms"][bones[bone]]["rotation"]]
                    self.assertLess(angle(design.spine_rotation(entry, vector), want), 1e-6, (sample["local"], bone))
        predicted = design.predict(name, source)
        recorded = body_doc(name, RUN)
        self.assertEqual([point["local"] for point in predicted["spec"]["schedule"]],
                         [point["local"] for point in recorded["spec"]["schedule"]])
        neutral = capture["samples"][0]["pose"]["transforms"]
        for got, want in zip(predicted["capture"]["samples"], recorded["capture"]["samples"]):
            self.assertEqual(got["muscles_f32"], want["muscles_f32"])
            vector = [replay.f32_from_bits(bits) for bits in got["muscles_f32"]]
            for bone, node in bones.items():
                rotation = got["pose"]["transforms"][node]["rotation"]
                if bone in spine:
                    self.assertEqual(rotation, [replay.f32_bits(v) for v in design.spine_rotation(spine[bone], vector)])
                elif bone != "Hips":
                    self.assertEqual(rotation, neutral[node]["rotation"], bone)
        parents, masses = adequacy.bone_table([source])
        self.assertTrue(all(row["fit"] for row in adequacy.samples(name, predicted, parents, masses)))
        # The bone's own limit row comes first; a probe that misses its limit angle refuses.
        rows = {row["name"]: row for row in capture["humanoid"]["muscles"]}
        own = copy.deepcopy(source)
        for row in own["capture"]["humanoid"]["muscles"]:
            if row["name"].startswith("Spine "):
                row.update(min=replay.f32_bits(-30), max=replay.f32_bits(30))
        with self.assertRaisesRegex(ValueError, "does not turn by its limit angle"):
            design.muscle_map(own["capture"], bones)
        limits = [rows[muscle][key] for muscle in ("Spine Twist Left-Right", "Spine Left-Right", "Spine Front-Back")
                  for key in ("min", "max")]
        own["capture"]["humanoid"]["limits"] = [dict(human="Spine", defaults=False, min=limits[0::2], max=limits[1::2])]
        self.assertEqual(design.muscle_map(own["capture"], bones)["Spine"][2], spine["Spine"][2])

    def test_the_envelope_bounds_a_planned_pose_conservatively(self):
        # Every recorded identity-root pose of the rig lends its hips and read-back; the merged row takes
        # the largest epsilon and E_eval and the smallest E_unity.
        name = sorted(body.EXTRA_CASES)[0]
        rig = body.EXTRA_RIGS[name]
        docs = {case: replay.read_json(json.dumps(body_doc(case, RUN, samples=None if case.startswith("body-probes")
                                                                     else 4)))
                for case in (f"body-probes-{rig}", f"body-targets-{rig}", f"body-targets-{rig}-offset",
                             "body-probes-holdout")}
        pairs = design.envelope(docs, rig)
        self.assertEqual(len(pairs), len(docs[f"body-probes-{rig}"]["capture"]["samples"]) + 4)
        variants = design.planned(docs)[name]
        self.assertEqual(len(variants), len(pairs))
        hips = next(row["node"] for row in docs[f"body-probes-{rig}"]["capture"]["humanoid"]["bones"]
                    if row["name"] == "Hips")
        self.assertEqual([doc["capture"]["samples"][0]["pose"]["transforms"][hips] for doc in variants],
                         [transform for transform, _ in pairs])
        with self.assertRaisesRegex(ValueError, "hold-out"):
            design.envelope(docs, body.HOLDOUT)
        rows = [[row("a", (1, 0, 0), (1, 0, 0), e, u, v)] for e, u, v in ((1, 5, 0), (3, 4, 2), (2, 6, 1))]
        merged = design.conservative(rows)
        self.assertEqual([(r["epsilon"], r["unity"], r["evaluation"]) for r in merged], [(3, 4, 2)])

    def test_the_hold_out_cannot_change_the_verdict(self):
        self.record([name for name in self.design() if name != "body-probes-holdout"])
        _, before, _ = self.run_checkpoint()

        def poison(doc):
            # A foreign bone table and a broken pose would fail every check that read them.
            doc["spec"]["evidence"]["editor"]["human_bones"][0].update(mass=replay.f32_bits(.5))
            doc["capture"]["samples"][1]["pose"]["transforms"][1]["rotation"] = [replay.f32_bits(.6)] * 4
        self.record(["body-probes-holdout"], edit=poison)
        _, after, _ = self.run_checkpoint()
        self.assertEqual(after, before)
        self.assertNotIn("body-probes-holdout", {row["name"] for row in after["samples"]})

    def test_the_twist_sweeps_take_no_part(self):
        # They measure the frame for B2: recorded or not, the checkpoint is the same, here and predicted.
        self.record([name for name in self.design() if name != "body-probes-holdout"])
        _, before, _ = self.run_checkpoint()
        self.record(sorted(body.TWIST_CASES))
        _, after, _ = self.run_checkpoint()
        self.assertEqual(after, before)
        self.assertFalse(body.TWIST_CASES & {row["name"] for row in after["samples"]})
        self.assertFalse(body.TWIST_CASES & set(design.planned(adequacy.load(self.folder))))

    def test_every_planned_sample_is_required(self):
        name = sorted(body.EXTRA_CASES)[0]
        self.record([case for case in self.design() if case not in body.RANGE_CASES | {name}])
        _, report, text = self.run_checkpoint()
        self.assertEqual(report["missing_planned"], [f"{case}: not recorded" for case in sorted(body.RANGE_CASES | {name})])
        self.assertIn(f"missing planned samples ['{sorted(body.RANGE_CASES)[0]}: not recorded'", text)
        self.assertFalse(report["adequate"])
        self.record([name, *body.RANGE_CASES], samples=5)
        _, report, _ = self.run_checkpoint()
        want = []
        for case in sorted(body.RANGE_CASES | {name}):
            rig = body.BODY_INFO[case]["rig"]
            want += [f"{case}@{t}" for t, _ in body.probe_schedule(rig, list(support.MUSCLES), False, case)[5:]]
        self.assertEqual(report["missing_planned"], want)
        self.assertFalse(report["adequate"])

    def test_design_only_adds_every_unrecorded_extra_case(self):
        name = sorted(body.EXTRA_CASES)[0]
        self.record([f"body-probes-{body.EXTRA_RIGS[name]}"], samples=None)
        self.record(["body-probes-compact"], samples=4)
        out = io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(io.StringIO()):
            code = adequacy.main([str(self.folder), "--design-only", "--json", str(self.folder / "report.json")])
        report = json.loads((self.folder / "report.json").read_text(encoding="utf-8"))
        self.assertIn(code, (0, 1))
        # The tall rig's planned cases are predicted; the compact rig's probe case is short, and the noncoplanar
        # rig's unrecorded, so their range sweeps stay missing.
        self.assertEqual(report["predicted"], ["body-probes-range-tall", name])
        self.assertIn(f"(predicted with body-probes-range-tall, {name})", out.getvalue())
        self.assertIn(name, {row["name"] for row in report["samples"]})
        self.assertEqual(report["missing_planned"], ["body-probes-range-compact: not recorded",
                                                     "body-probes-range-noncoplanar: not recorded"])
        # Without the flag the checkpoint reads only what was recorded.
        self.assertNotIn(name, {row["name"] for row in self.run_checkpoint()[1]["samples"]})

    def test_unreadable_input_exits_two(self):
        self.assertEqual(self.run_checkpoint()[0], 2)
        self.record(["body-probes-compact"], samples=3)
        self.record(["body-probes-tall"], samples=3,
                    edit=lambda doc: doc["spec"]["evidence"]["editor"]["human_bones"][0].update(mass=replay.f32_bits(.5)))
        self.assertEqual(self.run_checkpoint()[0], 2)


if __name__ == "__main__":
    unittest.main()
