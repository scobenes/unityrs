"""tools/bench/README.md's two tables list every file in tools/bench and in its tests/, and no other.

tools/parity/tests/test_readme.py is hard-wired to its own folder (docs/plans/bench.md section 5.3), so this folder
has its own. Each check reads its table's first column, not the whole README, so a file the prose names but the
table does not is still missing. A folder is `name/`. Standard library only.
"""

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
    return [
        p.name + "/" if p.is_dir() else p.name
        for p in sorted(folder.iterdir())
        if not (p.name.startswith(".") or p.name == "__pycache__" or p == README)
    ]


class ReadmeTableTests(unittest.TestCase):
    def check(self, folder: Path, header: str):
        names = listed(README.read_text(encoding="utf-8"), header)
        self.assertEqual(len(names), len(set(names)), f"{header}: a file listed twice")
        files = present(folder)
        self.assertEqual([f for f in files if f not in names], [], f"{header}: files the table does not list")
        self.assertEqual([n for n in names if n not in files], [], f"{header}: rows naming no file")

    def test_the_file_table_lists_every_file_and_no_other(self):
        self.check(HERE, FILES)

    def test_the_self_test_table_lists_every_module_and_no_other(self):
        self.check(HERE / "tests", TESTS)

    def test_a_file_named_only_in_prose_is_missing(self):
        """The check's own input: a table without a row the prose still names is caught."""
        text = README.read_text(encoding="utf-8")
        row = next(line for line in text.splitlines() if line.startswith("| `py_workloads.py` |"))
        cut = text.replace(row + "\n", "")
        self.assertIn("`py_workloads.py`", cut)
        self.assertNotIn("py_workloads.py", listed(cut, FILES))


if __name__ == "__main__":
    unittest.main()
