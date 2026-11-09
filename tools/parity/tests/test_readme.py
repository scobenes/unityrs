"""tools/parity/README.md's two tables list every file in tools/parity and in its tests/, and no other.

Review round 13: the file table left out `value_cases.py` (G2's driver, named only in prose), `child.py` and
`scratch.py`, which the tools and tests run their children and scratch folders through, and `shader_survey.py`,
`ruff.toml` and `unitypy_samples.json`; the self-test table left out `test_child.py`, `test_doc_tiers.py`,
`test_g3b.py` and `test_value_cases.py`. Each check reads its table's first column, not the whole README, so a
file the prose names but the table does not is still missing. A name in that column may be a glob
(`test_rows_*.py`); a folder is `name/`. Standard library only.
"""

import fnmatch
import re
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parents[1]

README = HERE / "README.md"
FILES = "| file | role |"
TESTS = "| module | covers | needs |"
NAME = re.compile(r"`([^`]+)`")


def listed(text: str, header: str) -> list[str]:
    """Every name in the first column of the table headed `header`, in order."""
    lines = text.splitlines()
    start = lines.index(header)
    names = []
    for line in lines[start + 2 :]:
        if not line.startswith("|"):
            break
        found = NAME.findall(line.split(" | ")[0])
        if not found:
            raise AssertionError(f"a row that names no file: {line!r}")
        names += found
    return names


def present(folder: Path) -> list[str]:
    """Every file in `folder` (a folder as `name/`), but the README itself, dotfiles and `__pycache__`."""
    out = []
    for p in sorted(folder.iterdir()):
        if p.name.startswith(".") or p.name == "__pycache__" or p == README:
            continue
        out.append(p.name + "/" if p.is_dir() else p.name)
    return out


def unlisted(folder: Path, names: list[str]) -> list[str]:
    """The files in `folder` no name matches."""
    return [f for f in present(folder) if not any(fnmatch.fnmatchcase(f, n) for n in names)]


def stale(folder: Path, names: list[str]) -> list[str]:
    """The names that match no file in `folder`."""
    files = present(folder)
    return [n for n in names if not any(fnmatch.fnmatchcase(f, n) for f in files)]


class ReadmeTableTests(unittest.TestCase):
    def check(self, folder: Path, header: str):
        names = listed(README.read_text(encoding="utf-8"), header)
        self.assertEqual(len(names), len(set(names)), f"{header}: a file listed twice")
        self.assertEqual(unlisted(folder, names), [], f"{header}: files the table does not list")
        self.assertEqual(stale(folder, names), [], f"{header}: rows naming no file")

    def test_the_file_table_lists_every_file_and_no_other(self):
        self.check(HERE, FILES)

    def test_the_self_test_table_lists_every_module_and_no_other(self):
        self.check(HERE / "tests", TESTS)

    def test_a_file_named_only_in_prose_is_missing(self):
        """The check's own input: a table without a row the prose still names is caught."""
        text = README.read_text(encoding="utf-8")
        row = next(line for line in text.splitlines() if line.startswith("| `value_cases.py` |"))
        cut = text.replace(row + "\n", "")
        self.assertIn("`value_cases.py`", cut)
        self.assertEqual(unlisted(HERE, listed(cut, FILES)), ["value_cases.py"])


if __name__ == "__main__":
    unittest.main()
