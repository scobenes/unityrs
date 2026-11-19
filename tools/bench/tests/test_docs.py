"""`render` (decision 9): the committed README section and BENCHMARKS.md are what wall.json and gate.json render
to; without wall.json the section reads "Results pending" and BENCHMARKS.md does not exist; a missing gate.json is
tolerated; an emulated or malformed wall.json is refused. Standard library only."""

import unittest

from support import ROOT, bench, results, scratch_tree, wall


class CommittedDocsTests(unittest.TestCase):
    def test_the_repository_renders_as_committed(self):
        self.assertEqual(bench.render(ROOT, check=True), [])

    def test_the_section_reads_results_pending_exactly_when_no_run_is_committed(self):
        published = (ROOT / "benchmarks" / "wall.json").exists()
        text = (ROOT / "README.md").read_text(encoding="utf-8")
        inner = text.split(results.BEGIN)[1].split(results.END)[0]
        self.assertEqual("Results pending" in inner, not published)
        self.assertEqual(inner.strip() == results.PENDING.strip(), not published)
        self.assertEqual((ROOT / "BENCHMARKS.md").exists(), published)
        self.assertEqual(text.count("\n## Performance\n"), 1)
        self.assertLess(text.index("\n## Goals\n"), text.index("\n## Performance\n"))
        self.assertLess(text.index("\n## Performance\n"), text.index(results.BEGIN))


class RenderTests(unittest.TestCase):
    def test_a_synthetic_run_renders_both_documents_and_checks_clean(self):
        root = scratch_tree(self, wall())
        self.assertEqual(bench.render(root, check=True)[0][:9], "README.md")
        self.assertEqual(bench.render(root, check=False), [])
        self.assertEqual(bench.render(root, check=True), [])
        readme = (root / "README.md").read_text(encoding="utf-8")
        inner = readme.split(results.BEGIN)[1].split(results.END)[0]
        self.assertNotIn("Results pending", inner)
        self.assertIn("| load and list objects | objects-lz4 (20,000 objects), objects-lzma (20,000 objects), "
                      "objects-2019 (20,000 objects), many-blocks (64.0 MiB entry) |", inner)
        self.assertIn("| read every typetree | 40,000 objects in 2 layouts |", inner)
        self.assertIn("| read and decode textures | 15 formats, 113.2 Mpixel |", inner)
        self.assertIn("×10.0 |", inner)
        md = (root / "BENCHMARKS.md").read_text(encoding="utf-8")
        for n in range(1, 9):
            self.assertIn(f"\n## {n}. ", md)
        self.assertIn("| texture-BC6H | PARITY.md row is Tier B |", md)
        self.assertIn("the gate lands with BN-2", md)

    def test_a_stale_section_or_benchmarks_md_fails_the_check(self):
        root = scratch_tree(self, wall())
        bench.render(root, check=False)
        (root / "BENCHMARKS.md").write_text("edited\n", encoding="utf-8")
        self.assertEqual(len(bench.render(root, check=True)), 1)
        bench.render(root, check=False)
        readme = root / "README.md"
        readme.write_text(readme.read_text(encoding="utf-8").replace("×10.0", "×99.0"), encoding="utf-8")
        self.assertTrue(bench.render(root, check=True)[0].startswith("README.md"))

    def test_benchmarks_md_without_wall_json_fails_the_check(self):
        root = scratch_tree(self)
        (root / "BENCHMARKS.md").write_text("orphan\n", encoding="utf-8")
        self.assertEqual(bench.render(root, check=True), ["BENCHMARKS.md exists without benchmarks/wall.json"])

    def test_a_gate_json_renders_its_table_and_a_missing_one_is_tolerated(self):
        gate = {"schema_version": 1, "tolerance_pct": 1.0, "fingerprint": {}, "workloads": {
            "load-objects-lz4": {"mode": "calibrating", "ir_own": 123456, "ir_libc": 789, "digest": "ab" * 32,
                                 "allocs": {}, "alloc_bytes": 0, "calls": 1}}}
        root = scratch_tree(self, wall(), gate)
        self.assertEqual(bench.render(root, check=False), [])
        md = (root / "BENCHMARKS.md").read_text(encoding="utf-8")
        self.assertIn("| load-objects-lz4 | calibrating | 123,456 | 789 | `abababababababab` |", md)

    def test_an_emulated_run_is_refused(self):
        root = scratch_tree(self, wall(emulated=True))
        bad = bench.render(root, check=False)
        self.assertTrue(bad and "emulated" in bad[0])
        self.assertFalse((root / "BENCHMARKS.md").exists())

    def test_a_malformed_wall_json_is_refused(self):
        doc = wall()
        del doc["tools"]["pillow"]
        self.assertEqual(bench.render(scratch_tree(self, doc), check=True), ["benchmarks/wall.json: tools.pillow missing"])

    def test_the_markers_must_occur_once_each_in_order(self):
        for text in ("no markers", f"{results.END}\n{results.BEGIN}", f"{results.BEGIN}{results.BEGIN}{results.END}"):
            with self.assertRaises(ValueError):
                results.splice(text, "x\n")
        self.assertEqual(results.splice(f"a\n{results.BEGIN}\nold\n{results.END}\nb", "new\n"),
                         f"a\n{results.BEGIN}\nnew\n{results.END}\nb")


if __name__ == "__main__":
    unittest.main()
