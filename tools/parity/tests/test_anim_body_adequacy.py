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
from anim_reference import euler  # noqa: E402
import anim_replay as replay  # noqa: E402
import scratch  # noqa: E402

G = tolerance.g


def row(name, column, jacobian, epsilon, tau, evaluation=0., fit=True):
    return dict(name=name, local=0, fit=fit, epsilon=epsilon, tau=tau, evaluation=evaluation,
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
        # against tau - E_eval = 1 - .25.
        report = checkpoint([row("r", (1., 2., 0.), (.5, 0., 0.), .5, 1., .25)])
        weighted = math.hypot(2., 4.)
        self.assertEqual((report["rank"], report["condition"]), (1, 1))
        self.assertAlmostEqual(report["worst_margin"], .75 / (.5 / weighted), places=13)
        self.assertEqual(report["margins"]["p0"], report["worst_margin"])

    def test_the_direct_check_needs_no_tau_against_epsilon_veto(self):
        # Sample b's own row is weak, but a's strong row pins the coefficient:
        # A_w^+ = (1, .1) / 1.01 over the two blocks, so u_b = .1 (1 + .1) / 1.01 = .109,
        # within tau_b - E_eval,b = .15, though tau_b < 2 epsilon_b.
        rows = [row("a", (1., 0., 0.), (1., 0., 0.), 1., 4.), row("b", (.1, 0., 0.), (.1, 0., 0.), 1., .25, .1)]
        report = checkpoint(rows)
        self.assertEqual(report["uncertainty_failures"], [])
        self.assertTrue(adequacy.adequate(report))
        self.assertLess(rows[1]["tau"], 2 * rows[1]["epsilon"])
        self.assertAlmostEqual(report["worst_margin"], min(4 / (1.1 / 1.01), .15 / (.11 / 1.01)), places=12)

    def test_a_short_design_is_reported_short(self):
        # u = .1 / .1 = 1: within tau alone, but not once E_eval takes its share.
        self.assertEqual(checkpoint([row("weak", (.1, 0., 0.), (.1, 0., 0.), 1., 1.)])["uncertainty_failures"], [])
        report = checkpoint([row("weak", (.1, 0., 0.), (.1, 0., 0.), 1., 1., .5)])
        self.assertEqual(len(report["uncertainty_failures"]), 1)
        self.assertAlmostEqual(report["worst_margin"], .5)
        self.assertFalse(adequacy.adequate(report))

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


class Bone:
    def __init__(self, position, depth=1, length=1.):
        self.position, self.depth, self.length = position, depth, length


class ToleranceTests(unittest.TestCase):
    def test_edges_and_folded_masses(self):
        parents = support.PARENT
        links = tolerance.edges(parents, body.rig_bones("compact"))
        self.assertIn(("Hips", "Spine"), links)
        self.assertIn(("UpperChest", "Neck"), links)
        self.assertNotIn("LeftHand", {i for i, _ in links})
        mapped = body.rig_bones("no-chest")
        self.assertEqual({j for i, j in tolerance.edges(parents, mapped) if i == "Spine"},
                         {"Neck", "LeftShoulder", "RightShoulder"})
        masses = {name: support.MASSES[name] for name in tolerance.BODY_SLOTS}
        folded = tolerance.folded_masses(masses, parents, mapped)
        self.assertEqual(list(folded), tolerance.ordered(mapped))
        self.assertAlmostEqual(sum(folded.values()), 1, places=15)
        total = sum(masses.values())
        eyes_jaw = sum(masses[name] for name in ("LeftEye", "RightEye", "Jaw"))
        self.assertAlmostEqual(folded["Spine"], (masses["Spine"] + masses["Chest"] + masses["UpperChest"]) / total)
        self.assertAlmostEqual(folded["Head"], (masses["Head"] + eyes_jaw) / total)

    def test_recorded_edges_follow_the_hierarchy(self):
        doc = body_doc("body-probes-no-chest", RUN, samples=1)
        human, nodes = doc["capture"]["humanoid"]["bones"], doc["spec"]["nodes"]
        mapped = body.rig_bones("no-chest")
        self.assertEqual(tolerance.recorded_edges(human, nodes), tolerance.edges(support.PARENT, mapped))

    def test_centre_of_mass_bound_by_hand(self):
        # Hips (two edges) and Spine (none): m_i((1 + n_i) E_i + sum_j E_j), plus the sum's own rounding.
        positions = {"Hips": (0, 1, 0), "Spine": (0, 2, 0), "LeftUpperLeg": (0, .5, 0)}
        errors = {"Hips": 1e-6, "Spine": 2e-6, "LeftUpperLeg": 3e-6}
        masses = {"Hips": .5, "Spine": .3, "LeftUpperLeg": .2}
        links = [("Hips", "Spine"), ("Hips", "LeftUpperLeg")]
        error, magnitude = tolerance.com_error(positions, masses, links, errors)
        want_magnitude = .5 * (3 * 1 + 2 + .5) + .3 * 2 + .2 * .5
        self.assertAlmostEqual(magnitude, want_magnitude, places=15)
        gamma = 13 * 2. ** -24 / (1 - 13 * 2. ** -24)  # widest 2, three bones: 2 + 6 + 5
        want = .5 * (3e-6 + 2e-6 + 3e-6) + .3 * 2e-6 + .2 * 3e-6 + gamma * want_magnitude
        self.assertAlmostEqual(error, want, delta=1e-20)

    def test_measurement_tolerance_counts_each_rounding_once(self):
        bones = {"Hips": Bone((0., 1., 0.), depth=2, length=1.)}
        world, h, t, s = (0., 1., 0.), 1., (2., 0., 0.), 2.
        fk = G("level", 2) * 1. + 48 * 2. ** -126
        com = fk + 7 * 2. ** -24 / (1 - 7 * 2. ** -24) * 1.  # one bone: 0 + 2 + 5 roundings
        want = (com + 2. ** -24 * h * 1. + G("readback") * (h * 1. + 2.)) / s
        self.assertAlmostEqual(tolerance.measurement_tolerance(bones, {"Hips": 1.}, [], h, world, t, s), want,
                               delta=1e-22)

    def test_frame_bound_by_hand(self):
        points = dict(zip(tolerance.FRAME_BONES, ((-1, 1, 0), (1, 1, 0), (-1, -1, 0), (1, -1, 0))))
        e = 1e-6
        errors = dict.fromkeys(tolerance.FRAME_BONES, e)
        size = 4 * math.sqrt(2)
        d_up = 2 * (4 * e / 2 + G("midpoint") * size / 2) / 2 + G("normalize")
        d_cross = 4 * e + G("lateral") * size + 4 * d_up + 2 * G("cross") * 4
        d_forward = 2 * d_cross / 4 + G("normalize")
        d_right = d_up + d_forward + 2 * G("cross")
        self.assertAlmostEqual(tolerance.frame_error(points, errors), math.sqrt(d_right ** 2 + d_up ** 2 + d_forward ** 2),
                               delta=1e-20)

    def test_frame_bound_grows_as_the_cross_product_vanishes(self):
        errors = dict.fromkeys(tolerance.FRAME_BONES, 1e-6)
        bounds = []
        for lean in (1., .1, .01):
            # Each side leans toward the up vector, so lateral x up shrinks with the lean.
            points = dict(zip(tolerance.FRAME_BONES, ((-lean, 1 - 1 / lean, 0.), (lean, 1 + 1 / lean, 0.),
                                                      (-lean, -1 - 1 / lean, 0.), (lean, -1 + 1 / lean, 0.))))
            bounds.append(tolerance.frame_error(points, errors))
        self.assertLess(bounds[0], bounds[1])
        self.assertLess(bounds[1], bounds[2])
        with self.assertRaisesRegex(ValueError, "lateral along up"):
            tolerance.frame_error(dict(zip(tolerance.FRAME_BONES, ((0, 2, 0), (0, 3, 0), (0, -2, 0), (0, -1, 0)))),
                                  errors)

    def test_the_model_evaluation_is_the_f64_path_and_one_output_rounding(self):
        self.assertEqual(G("solve", 1, tolerance.U64), 8 * 2. ** -53 / (1 - 8 * 2. ** -53))
        doc = replay.read_json(json.dumps(body_doc("body-probes-compact", RUN, samples=1)))
        parents, masses = adequacy.bone_table([doc])
        sample = next(adequacy.samples("body-probes-compact", doc, parents, masses))
        hips = doc["spec"]["nodes"].index(next(node for node in doc["spec"]["nodes"] if node["name"] == "Hips"))
        local = tolerance.decode(doc["capture"]["samples"][0]["pose"]["transforms"][hips]["translation"], 3,
                                 replay.f32_from_bits)
        rounding = tolerance.U32 * tolerance.norm(local)
        self.assertGreater(sample["evaluation"], rounding)
        self.assertLess(sample["evaluation"], rounding + sample["tau"] * 2. ** -25)

    def test_bounds_are_positive_and_grow_with_depth(self):
        doc = body_doc("body-probes-compact", RUN, samples=1)
        nodes, sample = doc["spec"]["nodes"], doc["capture"]["samples"][0]
        frames = tolerance.kinematics(nodes, sample["pose"]["transforms"], replay.f32_from_bits)
        world = tolerance.kinematics(nodes, sample["pose"]["transforms"], replay.f32_from_bits, "", include_base=True)
        self.assertEqual(world["0/0"].depth, frames["0/0"].depth + 2)
        bones = tolerance.bone_frames(doc["capture"]["humanoid"]["bones"], nodes, frames)
        self.assertLess(tolerance.position_error(bones["Spine"]), tolerance.position_error(bones["LeftHand"]))
        self.assertEqual(bones["LeftHand"].depth, 8)  # Hips, Spine, Chest, UpperChest, Shoulder, three arm links
        with self.assertRaisesRegex(ValueError, "nonuniform"):
            tolerance.uniform((1., 1., 1.5))


class CheckpointTests(unittest.TestCase):
    def setUp(self):
        self.folder = scratch.for_case(self)
        (self.folder / "replays" / "body").mkdir(parents=True)

    def record(self, names, samples=10, edit=None):
        for name in names:
            doc = body_doc(name, RUN, samples=samples)
            if edit:
                edit(doc)
            write(self.folder / "replays" / "body" / f"{name}.pending.json", doc)

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
        self.assertIn("Spine>Neck", report["coefficients"])
        self.assertLess(report["condition"], adequacy.CONDITION_LIMIT)
        self.assertEqual((report["untrained"], report["edge_mismatches"], report["missing_coefficients"]), ([], [], []))
        self.assertNotIn("necessary", report)
        self.assertEqual(report["adequate"], adequacy.adequate(report))
        self.assertIn(f"rank {report['rank']}/{report['rank']}", text)

    def test_a_coplanar_branch_stays_free(self):
        with mock.patch.object(support, "NONCOPLANAR", {"noncoplanar"}):
            self.record(self.design())
        code, report, _ = self.run_checkpoint()
        self.assertEqual(code, 1)
        self.assertEqual(report["rank"], len(report["coefficients"]) - 1)
        self.assertEqual(set(report["free"][0]), {"Spine>Neck", "Spine>LeftShoulder", "Spine>RightShoulder"})
        self.assertIsNone(report["condition"])
        self.assertFalse(report["adequate"])

    def test_coverage_comes_only_from_probe_cases(self):
        # A clip on the compact rig, a probe case without read-back and one whose
        # Avatar lost a bone credit nothing.
        self.record(["body-clip-level", "body-probes-holdout"], samples=6)
        self.record(["body-probes-eyes"], samples=4,
                    edit=lambda doc: doc["spec"]["evidence"]["editor"].update(body_samples=[]))

        def drop_jaw(doc):
            next(row for row in doc["capture"]["humanoid"]["bones"] if row["name"] == "Jaw")["node"] = -1
        self.record(["body-probes-jaw"], samples=4, edit=drop_jaw)
        code, report, _ = self.run_checkpoint()
        self.assertEqual(code, 1)
        self.assertEqual(report["coverage"]["recorded"], ["holdout"])
        self.assertIn("compact", report["coverage"]["missing"])
        self.assertIn("Spine>Neck", report["missing_coefficients"])
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

    def test_unreadable_input_exits_two(self):
        self.assertEqual(self.run_checkpoint()[0], 2)
        self.record(["body-probes-compact"], samples=3)
        self.record(["body-probes-tall"], samples=3,
                    edit=lambda doc: doc["spec"]["evidence"]["editor"]["human_bones"][0].update(mass=replay.f32_bits(.5)))
        self.assertEqual(self.run_checkpoint()[0], 2)


if __name__ == "__main__":
    unittest.main()
