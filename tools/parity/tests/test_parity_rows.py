"""PARITY.md's test column and the row tests it names agree (standard library).

The row tests are PARITY.md rows a review disputed, each run against the oracle
on the smallest input that shows it: test_parity_rows_files.py,
test_parity_rows_objects.py and test_parity_rows_media.py (parity_rows.py holds
what they share). Each test names the PARITY.md rows it keeps honest; a row's
test column names the test back, and the checks here hold both ways.
"""

import re
import sys
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE / "tests"))

import inventory  # noqa: E402


class TestColumn(unittest.TestCase):
    """PARITY.md's test column and the tests it names agree (standard library)."""

    CITED = re.compile(r"`(test_[a-z_]+\.py)` ([a-z0-9_]+)")
    RUST = re.compile(r"`(crates/[a-z0-9_/-]+\.rs)` ([a-z0-9_]+)")

    def test_every_cited_test_exists_and_every_row_test_is_cited(self):
        text = (HERE.parent.parent / "PARITY.md").read_text(encoding="utf-8")
        cited = set(self.CITED.findall(text))
        for module, name in sorted(cited):
            source = (HERE / "tests" / module).read_text(encoding="utf-8")
            self.assertIn(f"def test_{name}(", source, f"PARITY.md cites {module} {name}, which does not exist")
        # This module and the row tests' modules: a row names every test in them.
        modules = sorted((HERE / "tests").glob("test_parity_rows*.py"))
        self.assertEqual(
            [m.name for m in modules],
            ["test_parity_rows.py", "test_parity_rows_files.py", "test_parity_rows_media.py", "test_parity_rows_objects.py"],
        )
        for path in modules:
            defined = set(re.findall(r"def test_([a-z0-9_]+)\(", path.read_text(encoding="utf-8")))
            defined.discard("every_cited_test_exists_and_every_row_test_is_cited")
            named = {name for module, name in cited if module == path.name}
            self.assertEqual(sorted(defined - named), [], f"tests in {path.name} no PARITY.md row names")

    def test_every_status_agrees_with_its_test_column(self):
        """Section 0's status rules: a row that names a test is pinned or verified, one that names none is
        unverified (source reading), n/a (tier —) or a pointer to another row; `planned` is section 17.1's;
        a verified P1 row names a Rust test, and that test exists."""
        text = (HERE.parent.parent / "PARITY.md").read_text(encoding="utf-8")
        seen = 0
        for header, rows in inventory.tables(text):
            if "status" not in header or "test" not in header:
                continue
            test, status = header.index("test"), header.index("status")
            for cells in rows:
                seen += 1
                cites = bool(self.CITED.search(cells[test]) or re.search(r"`test_[a-z_]+\.py` \(all\)", cells[test]))
                state = cells[status]
                with self.subTest(row=cells[0][:60], status=state):
                    if state.startswith(("pinned P0", "verified P0")):
                        self.assertTrue(cites or "corpus dump" in cells[test], "a probed row names what probes it")
                    elif state.startswith("verified P1"):
                        rust = self.RUST.findall(cells[test])
                        self.assertTrue(rust, "a verified P1 row names the Rust test that pins it")
                        for path, name in rust:
                            source = (HERE.parent.parent / path).read_text(encoding="utf-8")
                            self.assertRegex(source, rf"\bfn {name}\(", f"{path} has no test {name}")
                    elif state.startswith("unverified (source reading); impl P"):
                        self.assertFalse(cites, "a row a test runs is not unverified")
                    elif state.startswith("planned ("):
                        self.assertIn("reference", header, "only section 17.1's references are planned")
                    elif state == "n/a":
                        self.assertEqual(cells[header.index("tier")], "—")
                    elif state == "as above":
                        pass
                    else:
                        self.assertEqual(state, "", "an unknown status")
                        self.assertEqual(set(cells[2:]), {""}, "a row with no status points at another section")
        self.assertGreater(seen, 500)


if __name__ == "__main__":
    unittest.main()
