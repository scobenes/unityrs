"""Every module under tools/fixtures (a file with no `__main__` block, which a fixture script imports) runs on
its own: run as a program, as plan section 10's G4 line runs every `tools/fixtures/*.py` with `--check`, it
puts on its path what it imports itself, never relying on the path of the script that imports it
(integration 14: `env_race.py` found `scratch` only through `env.py`'s path, and failed run alone).
Standard library only.
"""

import os
import re
import sys
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parents[1]
ROOT = HERE.parents[1]
sys.path.insert(0, str(HERE))

import child  # noqa: E402

FIXTURES = ROOT / "tools" / "fixtures"
# A fixture script, which CI runs with --check (scripts/tests/test_ci_workflow.py's MAIN); the rest are modules.
MAIN = re.compile(r"^if __name__ == [\"']__main__[\"']:", re.M)


def modules() -> list[Path]:
    """The files under tools/fixtures that are modules, not scripts."""
    return sorted(p for p in FIXTURES.glob("*.py") if not MAIN.search(p.read_text(encoding="utf-8")))


class FixtureModulesRunAlone(unittest.TestCase):
    def test_each_module_runs_on_its_own(self):
        found = modules()
        self.assertTrue(found, f"no module under {FIXTURES}")
        # Only the interpreter's own path and the module's folder: nothing the caller's PYTHONPATH adds.
        env = {k: v for k, v in os.environ.items() if k != "PYTHONPATH"}
        for path in found:
            with self.subTest(module=path.name):
                proc = child.run(
                    [sys.executable, str(path), "--check"], cwd=ROOT, env=env, capture_output=True, text=True
                )
                self.assertEqual(
                    proc.returncode, 0, f"{path.name} run on its own exited {proc.returncode}:\n{proc.stderr[-2000:]}"
                )


if __name__ == "__main__":
    unittest.main()
