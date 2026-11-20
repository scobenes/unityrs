"""The gate's verdict (docs/plans/bench.md decisions 6 and 7, section 5.1): the two-sided band, the base branch's
modes and tolerance against the PR's counts, what fails and what only reports; gate.json's schema; reading the
base's file; and the committed gate.json against the workload table. Standard library only."""

import copy
import json
import unittest

from support import GATE, WORKLOADS, bench
import callgrind

NAMES = [w["name"] for w in WORKLOADS["workloads"]]
N = "texture-DXT1"


def doc(mode="calibrating", tol=1.0, **counts) -> dict:
    """A gate.json of every workload, `N`'s counts replaced by `counts` (ir_own, alloc_bytes, malloc, digest)."""
    g = copy.deepcopy(GATE)
    g["tolerance_pct"] = tol
    for e in g["workloads"].values():
        e["mode"] = mode
    e = g["workloads"][N]
    e.update(ir_own=counts.get("ir_own", 1_000_000), alloc_bytes=counts.get("alloc_bytes", 50_000),
             digest=counts.get("digest", "ab" * 32))
    e["allocs"] = {**e["allocs"], "malloc": counts.get("malloc", 100)}
    return g


def verdict(base, pr, measured, problems=None):
    return callgrind.compare(base, pr, measured, problems or {}, NAMES)


class BandTests(unittest.TestCase):
    def test_the_band_is_two_sided_and_inclusive(self):
        for value, ok in ((1_010_000, True), (990_000, True), (1_010_001, False), (989_999, False)):
            self.assertEqual(callgrind.in_band(1_000_000, value, 1.0), ok, value)
        self.assertTrue(callgrind.in_band(0, 0, 1.0))
        self.assertFalse(callgrind.in_band(0, 1, 1.0))

    def test_deltas_and_spread(self):
        self.assertEqual(callgrind.delta(1000, 1015), "+1.500%")
        self.assertEqual(callgrind.delta(None, 5), "—")
        self.assertEqual(callgrind.delta(0, 5), "new")
        self.assertAlmostEqual(callgrind.spread([100, 101, 100]), 1.0)


class CompareTests(unittest.TestCase):
    def test_in_band_passes_in_either_mode(self):
        for mode in callgrind.MODES:
            _, fails = verdict(doc(mode), doc(mode), doc(mode, ir_own=1_009_000, malloc=99))
            self.assertEqual(fails, [], mode)

    def test_an_enforcing_workload_out_of_band_fails_either_way(self):
        for ir in (1_020_000, 980_000):
            rows, fails = verdict(doc("enforcing"), doc(), doc(ir_own=ir))
            self.assertEqual(len(fails), 1)
            self.assertIn(f"{N}: ir_own", fails[0])
            self.assertTrue(fails[0].endswith("out of band: refresh the baseline"))
            self.assertIn(callgrind.REFRESH, [r[-1] for r in rows if r[0] == N and r[2] == "ir_own"])

    def test_every_gated_count_is_held(self):
        for edit in ({"malloc": 120}, {"alloc_bytes": 60_000}, {"digest": "cd" * 32}):
            _, fails = verdict(doc("enforcing"), doc(), doc(**edit))
            self.assertEqual(len(fails), 1, edit)

    def test_a_calibrating_workload_only_reports(self):
        rows, fails = verdict(doc(), doc(), doc(ir_own=2_000_000, digest="cd" * 32))
        self.assertEqual(fails, [])
        verdicts = {r[2]: r[-1] for r in rows if r[0] == N}
        self.assertEqual(verdicts["ir_own"], "out of band (calibrating: reported)")
        self.assertEqual(verdicts["digest"], "changed (calibrating: reported)")

    def test_the_mode_and_tolerance_are_the_base_branch_s(self):
        """A PR that marks itself calibrating, or widens its band, does not loosen its own gate."""
        _, fails = verdict(doc("enforcing", tol=1.0), doc("calibrating", tol=50.0), doc(ir_own=1_050_000))
        self.assertEqual(len(fails), 1)
        _, fails = verdict(doc("calibrating"), doc("enforcing"), doc(ir_own=1_050_000))
        self.assertEqual(fails, [])
        _, fails = verdict(doc("enforcing", tol=10.0), doc(tol=1.0), doc(ir_own=1_050_000))
        self.assertEqual(fails, [])

    def test_the_counts_compared_are_the_pr_s_own(self):
        """A refresh: the PR's gate.json carries the new counts, and the base's old ones only show as a delta."""
        rows, fails = verdict(doc("enforcing"), doc(ir_own=1_500_000), doc(ir_own=1_500_000))
        self.assertEqual(fails, [])
        row = next(r for r in rows if r[0] == N and r[2] == "ir_own")
        self.assertEqual(row[3:8], ("1,000,000", "1,500,000", "1,500,000", "+0.000%", "+50.000%"))

    def test_without_a_base_gate_json_every_workload_is_calibrating(self):
        _, fails = verdict(None, doc("enforcing"), doc(ir_own=3_000_000))
        self.assertEqual(fails, [])

    def test_a_workload_missing_from_the_pr_or_unknown_to_the_table_fails(self):
        pr = doc()
        del pr["workloads"][N]
        _, fails = verdict(None, pr, doc())
        self.assertEqual(fails, [f"{N}: missing from the PR's gate.json"])
        pr = doc()
        pr["workloads"]["texture-XYZ"] = pr["workloads"][N]
        _, fails = verdict(None, pr, doc())
        self.assertEqual(fails, ["texture-XYZ: in the PR's gate.json but not a workload"])

    def test_a_base_enforcing_workload_the_pr_drops_fails(self):
        """Dropped from both the PR's table and its gate.json: the base's enforcing entry still holds."""
        base = doc()
        base["workloads"]["texture-OLD"] = {**base["workloads"][N], "mode": "enforcing"}
        _, fails = verdict(base, doc(), doc())
        self.assertEqual(fails, ["texture-OLD: enforcing in the base's gate.json but not a workload"])
        base["workloads"]["texture-OLD"]["mode"] = "calibrating"
        self.assertEqual(verdict(base, doc(), doc())[1], [])

    def test_an_expected_workload_with_no_measured_row_fails(self):
        measured = doc()
        del measured["workloads"][N]
        rows, fails = verdict(None, doc(), measured)
        self.assertEqual(fails, [f"{N}: not measured"])
        self.assertIn((N, "calibrating", "run", "", "", "", "", "", "not measured"), rows)

    def test_a_rejected_run_fails_in_any_mode(self):
        measured = doc()
        del measured["workloads"][N]
        rows, fails = verdict(None, doc(), measured, {N: ["collection was on outside the scope"]})
        self.assertEqual(fails, [f"{N}: collection was on outside the scope"])
        self.assertTrue(any(r[0] == N and r[-1].startswith("rejected") for r in rows))

    def test_a_fingerprint_difference_fails_only_once_a_workload_enforces(self):
        measured = doc(mode="enforcing")
        measured["fingerprint"] = {**measured["fingerprint"], "guest_cpu": "sse4.2 avx avx2"}
        rows, fails = verdict(doc(), doc(), measured)
        self.assertEqual(fails, [])
        self.assertIn("guest_cpu", rows[0][-1])
        _, fails = verdict(doc("enforcing"), doc(), measured)
        self.assertEqual(len(fails), 1)
        self.assertIn("guest_cpu", fails[0])
        _, fails = verdict(doc(), doc(), doc(), {"(fingerprint)": ["the guest CPU differed between runs"]})
        self.assertEqual(fails, ["(fingerprint): the guest CPU differed between runs"])

    def test_the_summary_lists_every_workload_and_the_failures(self):
        rows, fails = verdict(doc("enforcing"), doc(), doc(ir_own=2_000_000))
        text = callgrind.summary(rows, fails, 1.0, True)
        self.assertIn("**Failed:**", text)
        for n in NAMES:
            self.assertIn(f"| {n} | ", text)
        self.assertIn("**Passed.**", callgrind.summary(*verdict(None, doc(), doc()), 1.0, False))
        warned = callgrind.summary([], [], 1.0, True, ["tools/bench/callgrind.py changed"])
        self.assertIn("> [!WARNING]", warned)
        self.assertIn("> - tools/bench/callgrind.py changed", warned)
        self.assertNotIn("WARNING", text)


class SchemaTests(unittest.TestCase):
    def test_the_committed_gate_json_is_valid_for_every_workload(self):
        """Its schema and legal modes and tolerance, whatever they are: a modes-only promotion changes no test."""
        self.assertEqual(callgrind.gate_problems(GATE), [])
        self.assertEqual(sorted(GATE["workloads"]), sorted(NAMES))
        self.assertLessEqual({e["mode"] for e in GATE["workloads"].values()}, set(callgrind.MODES))
        self.assertTrue(0 <= GATE["tolerance_pct"] <= 100)
        self.assertEqual({e["calls"] for e in GATE["workloads"].values()}, {1})
        self.assertTrue(all(e["ir_own"] >= callgrind.MIN_IR for e in GATE["workloads"].values()))

    def test_malformed_entries_are_named(self):
        cases = {
            "not exactly schema_version": lambda g: g.pop("tolerance_pct"),
            "fingerprint is not exactly": lambda g: g["fingerprint"].pop("guest_cpu"),
            f"{N}: mode 'strict'": lambda g: g["workloads"][N].update(mode="strict"),
            f"{N}: allocs is not exactly": lambda g: g["workloads"][N]["allocs"].pop("calloc"),
            f"{N}: a count is not": lambda g: g["workloads"][N].update(ir_own=-1),
            f"{N}: digest is not": lambda g: g["workloads"][N].update(digest=None),
            f"{N}: ir_own is 0": lambda g: g["workloads"][N].update(ir_own=0),
            "tolerance_pct is not": lambda g: g.update(tolerance_pct=-1),
            "schema_version 2": lambda g: g.update(schema_version=2),
        }
        for want, edit in cases.items():
            g = copy.deepcopy(GATE)
            edit(g)
            bad = callgrind.gate_problems(g)
            self.assertTrue(any(want in b for b in bad), (want, bad))


class BaseTests(unittest.TestCase):
    def fake_git(self, commit=True, listed="benchmarks/gate.json\n", text=None, tree_rc=0):
        def git(*args):
            if args[0] == "cat-file":
                return (0 if commit else 128), ""
            if args[0] == "ls-tree":
                return tree_rc, (listed if tree_rc == 0 else "")
            return (0, text) if text is not None else (128, "")
        return git

    def test_the_base_s_file_is_read_and_checked(self):
        got, bad = bench.base_gate("abc", git=self.fake_git(text=json.dumps(GATE)))
        self.assertEqual((got, bad), (GATE, []))
        got, bad = bench.base_gate("abc", git=self.fake_git(text="{"))
        self.assertIsNone(got)
        self.assertTrue(bad[0].startswith("the base's gate.json"))

    def test_a_base_without_the_file_is_none(self):
        self.assertEqual(bench.base_gate("abc", git=self.fake_git(listed="")), (None, []))
        self.assertEqual(bench.base_gate(None, git=self.fake_git()), (None, []))

    def test_an_unknown_or_unreadable_base_fails_closed(self):
        for git, why in ((self.fake_git(commit=False), "not in the checkout"),
                         (self.fake_git(tree_rc=128), "tree cannot be read"),
                         (self.fake_git(text=None), "gate.json cannot be read")):
            got, bad = bench.base_gate("abc", git=git)
            self.assertIsNone(got)
            self.assertIn(why, bad[0])


class ChangeTests(unittest.TestCase):
    """The warning when a change edits the gate's own checker or policy (decision 7)."""

    def files(self):
        from support import ROOT

        return {p: (ROOT / p).read_text(encoding="utf-8") for p in
                ("tools/bench/callgrind.py", "tools/bench/install_valgrind.sh", "tools/bench/bench.py", bench.CI)}

    def changes(self, head: dict, base_doc=GATE, pr_doc=GATE) -> list:
        base = self.files()

        def git(cmd, spec):
            ref, path = spec.split(":", 1)
            tree = base if ref == "BASE" else head
            return (0, tree[path]) if path in tree else (128, "")
        return bench.gate_changes("BASE", base_doc, pr_doc, git=git)

    def test_an_unchanged_gate_raises_nothing(self):
        self.assertEqual(self.changes(self.files()), [])

    def test_each_edit_of_the_checker_or_policy_is_named(self):
        head = self.files()
        head["tools/bench/callgrind.py"] += "\n# edited\n"
        head["tools/bench/bench.py"] = head["tools/bench/bench.py"].replace(
            "def cmd_gate(args) -> int:\n", "def cmd_gate(args) -> int:\n    pass\n")
        head[bench.CI] = head[bench.CI].replace("timeout-minutes: 40", "timeout-minutes: 41")
        pr = copy.deepcopy(GATE)
        pr["tolerance_pct"] = 5.0
        pr["workloads"][N]["mode"] = "enforcing"
        self.assertEqual(self.changes(head, pr_doc=pr), [
            "tools/bench/callgrind.py changed",
            "tools/bench/bench.py's gate functions changed: cmd_gate",
            f"{bench.CI}'s bench-gate job changed",
            "gate.json's tolerance_pct changed: 1.0 to 5.0",
            f"gate.json's modes changed: {N}",
        ])

    def test_edits_outside_the_gate_raise_nothing(self):
        head = self.files()
        head["tools/bench/bench.py"] = head["tools/bench/bench.py"].replace(
            "def cmd_render(args) -> int:\n", "def cmd_render(args) -> int:\n    pass\n")
        head[bench.CI] = head[bench.CI].replace("timeout-minutes: 10", "timeout-minutes: 11", 1)
        self.assertEqual(self.changes(head), [])

    def test_the_job_s_text_runs_to_the_next_job(self):
        text = "jobs:\n  a:\n    x: 1\n  bench-gate:\n    y: 2\n\n  c:\n    z: 3\n"
        self.assertEqual(bench.job_text(text), "  bench-gate:\n    y: 2")
        self.assertIsNone(bench.job_text("jobs:\n  a:\n"))


if __name__ == "__main__":
    unittest.main()
