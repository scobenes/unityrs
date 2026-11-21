"""results.py: pairing (decision 4), the aggregation of rounds and categories, and the speedup's range (section 3),
on synthetic runs. Standard library only."""

import unittest

from support import NAMES, WORKLOADS, results, wall


class PairingTests(unittest.TestCase):
    def test_a_tier_a_row_both_sides_ok_with_one_digest_is_paired(self):
        self.assertEqual(results.pairings(wall(), WORKLOADS)["texture-DXT1"], (True, None))

    def test_a_tier_b_row_is_unpaired_whatever_its_runs(self):
        self.assertEqual(results.pairings(wall(), WORKLOADS)["texture-BC6H"], (False, "PARITY.md row is Tier B"))

    def test_a_failed_round_on_either_side_unpairs(self):
        doc = wall(status={("texture-DXT1", "unitypy", 3): "raised:ValueError"})
        self.assertEqual(results.pairings(doc, WORKLOADS)["texture-DXT1"], (False, "unitypy: raised:ValueError"))
        doc = wall(status={("texture-DXT1", "unityrs", 0): "raised:KeyError"})
        self.assertEqual(results.pairings(doc, WORKLOADS)["texture-DXT1"], (False, "unityrs: raised:KeyError"))

    def test_a_different_digest_in_any_round_unpairs(self):
        for impl in results.IMPLS:
            doc = wall(digest={("texture-DXT1", impl, 6): "other"})
            self.assertEqual(results.pairings(doc, WORKLOADS)["texture-DXT1"], (False, "the digests differ"))

    def test_a_side_with_no_runs_unpairs(self):
        doc = wall()
        doc["workloads"] = [e for e in doc["workloads"] if not (e["name"] == "texture-DXT1" and e["impl"] == "unityrs")]
        self.assertEqual(results.pairings(doc, WORKLOADS)["texture-DXT1"], (False, "no unityrs runs"))


class AggregationTests(unittest.TestCase):
    def test_a_workloads_figure_is_the_median_round_with_its_spread(self):
        doc = wall(times={("texture-DXT1", "unitypy"): [5, 1, 3, 2, 4, 7, 6]})
        fig = results.workload_figures(doc, "texture-DXT1")["unitypy"]
        self.assertEqual((fig["median"], fig["min"], fig["max"]), (4, 1, 7))

    def test_a_category_sums_each_round_then_takes_the_median(self):
        # The median of the round totals (3, 12, 12, ...) is not the sum of the medians (1 + 1).
        a = [1, 1, 1, 10, 10, 10, 1]
        b = [2, 11, 11, 1, 1, 1, 1]
        doc = wall(names=("typetree-objects-lz4", "typetree-objects-2019"),
                   times={("typetree-objects-lz4", "unitypy"): a, ("typetree-objects-2019", "unitypy"): b})
        row = next(r for r in results.summary(doc, WORKLOADS) if r["category"] == "typetree")
        totals = sorted(x + y for x, y in zip(a, b, strict=True))
        self.assertEqual(row["unitypy"], {"median": totals[3], "min": totals[0], "max": totals[-1]})
        self.assertNotEqual(row["unitypy"]["median"], 2)

    def test_the_speedup_is_the_ratio_of_medians_with_the_extremes_range(self):
        x = results.speedup([10, 12, 14], [1, 2, 4])
        self.assertEqual(x, {"ratio": 6.0, "low": 2.5, "high": 14.0})

    def test_memory_and_cpu_ratios_sum_each_round_like_times(self):
        names = ("typetree-objects-lz4", "typetree-objects-2019")
        doc = wall(names=names)
        for e in doc["workloads"]:
            for r in e["runs"]:
                big = e["impl"] == "unitypy"
                r["peak_pss_bytes"] = (300 if big else 100) * 2**20 if e["name"] == names[0] else 100 * 2**20
                r["cpu_s"] = 3.0 if big else 1.0
        row = next(r for r in results.summary(doc, WORKLOADS) if r["category"] == "typetree")
        self.assertEqual((row["memory"], row["cpu"]), (2.0, 3.0))  # (300 + 100) / (100 + 100); 6 / 2
        table = "\n".join(results.summary_table(doc, WORKLOADS, ranges=False))
        self.assertIn("| ×10.0 | ×2.0 | ×3.0 |", table)

    def test_a_missing_peak_pss_leaves_the_memory_ratio_out(self):
        doc = wall(names=("texture-DXT1",))
        next(e for e in doc["workloads"] if e["impl"] == "unityrs")["runs"][2]["peak_pss_bytes"] = None
        row = results.summary(doc, WORKLOADS)[0]
        self.assertIsNone(row["memory"])
        self.assertIsNotNone(row["cpu"])
        self.assertTrue(results.summary_table(doc, WORKLOADS, ranges=False)[2].endswith("| — | ×2.8 |"))

    def test_a_zero_round_total_or_an_unsampled_pss_never_fails_the_ratio(self):
        doc = wall(names=("texture-DXT1",))
        rs = next(e for e in doc["workloads"] if e["impl"] == "unityrs")
        rs["runs"][0]["cpu_s"] = 0.0  # a zero round total; the median stays positive
        row = results.summary(doc, WORKLOADS)[0]
        self.assertAlmostEqual(row["cpu"], 1.43 / 0.503)  # medians: UnityPy 1.03 + 0.4; unityrs 0, then 0.501 up
        self.assertEqual(results.ratio({"unitypy": [1, 2, 3], "unityrs": [0, 0, 1]}), None)  # a zero median
        rs["runs"][3]["peak_pss_bytes"] = 0  # the sampler caught no process: not sampled
        row = results.summary(doc, WORKLOADS)[0]
        self.assertIsNone(row["memory"])
        self.assertTrue(results.summary_table(doc, WORKLOADS, ranges=False)[2].endswith("| — | ×2.8 |"))
        self.assertEqual(results.workload_figures(doc, "texture-DXT1")["unityrs"]["peak_pss_bytes"], 64 * 2**20)

    def test_unpaired_workloads_count_in_no_category(self):
        doc = wall(NAMES, status={("texture-DXT1", "unitypy", 0): "raised:KeyError"})
        cats = [r["category"] for r in results.summary(doc, WORKLOADS)]
        self.assertNotIn("texture", cats)  # DXT1 failed and BC6H is Tier B: no paired texture row
        self.assertEqual(cats, ["load", "typetree"])


class SchemaTests(unittest.TestCase):
    def test_a_synthetic_run_is_valid(self):
        self.assertEqual(results.problems(wall(), WORKLOADS), [])

    def test_a_stored_pairing_must_be_the_computed_one(self):
        doc = wall()
        for e in doc["workloads"]:
            if e["name"] == "texture-BC6H":
                e["paired"], e["reason"] = True, None
        self.assertTrue(any("stored pairing" in p for p in results.problems(doc, WORKLOADS)))

    def test_missing_fields_runs_and_rounds_are_reported(self):
        doc = wall()
        del doc["machine"]
        self.assertEqual(results.problems(doc, WORKLOADS), ["missing fields: machine"])
        doc = wall()
        doc["workloads"][0]["runs"].pop()
        self.assertIn(f"{NAMES[0]}/unitypy: 6 runs, not 7", results.problems(doc, WORKLOADS))

    def test_a_host_path_is_refused(self):
        doc = wall()
        doc["tools"]["rustc"] = "rustc 1.99.1\nsysroot: /opt/runner/.rustup"
        self.assertTrue(any("host path" in p for p in results.problems(doc, WORKLOADS)))

    def test_a_status_is_ok_or_a_class_name_never_a_message(self):
        for status in ("ValueError: bad image mode", "exit 1: boom", "raised:Value Error", "raised:"):
            doc = wall(status={("texture-DXT1", "unitypy", 0): status})
            self.assertTrue(any("status is not" in p for p in results.problems(doc, WORKLOADS)), status)

    def test_each_round_appears_once(self):
        doc = wall()
        for r in doc["workloads"][0]["runs"]:
            r["round"] = 0
        self.assertIn(f"{NAMES[0]}/unitypy: its rounds are not 0 to 6, once each", results.problems(doc, WORKLOADS))

    def test_a_run_holds_its_fields_alone_each_well_typed_and_finite(self):
        for change in ({"extra": 1}, {"scope_s": float("nan")}, {"cpu_s": float("inf")}, {"round": "0"},
                       {"peak_pss_bytes": "64"}, {"counts": {"other": 1}}, {"digest": "digest"}):
            doc = wall()
            doc["workloads"][0]["runs"][0].update(change)
            self.assertTrue(any(NAMES[0] in p for p in results.problems(doc, WORKLOADS)), change)

    def test_an_emulated_or_partial_run_is_not_published(self):
        self.assertEqual(results.publish_problems(wall(), WORKLOADS), [])
        self.assertTrue(results.publish_problems(wall(emulated=True), WORKLOADS))
        doc = wall()
        doc["scale"] = "gate"
        self.assertTrue(results.publish_problems(doc, WORKLOADS))

    def test_a_full_run_missing_a_workload_or_rounds_is_not_published(self):
        doc = wall()
        doc["workloads"] = [e for e in doc["workloads"] if not (e["name"] == NAMES[0] and e["impl"] == "unityrs")]
        self.assertEqual(results.publish_problems(doc, WORKLOADS), [f"{NAMES[0]}/unityrs: missing"])
        doc = wall()
        doc["rounds"] = 3
        self.assertEqual(results.publish_problems(doc, WORKLOADS), ["3 rounds, not workloads.json's 7"])


if __name__ == "__main__":
    unittest.main()
