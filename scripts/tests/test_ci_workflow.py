"""The CI workflow's own rules, read line by line (no YAML parser in the standard library).

- Every `uses:` names a full commit SHA, with its release tag in a comment: a
  tag can be moved to other code, a SHA cannot.
- Every job has a `timeout-minutes`, so a hung step fails in minutes instead
  of GitHub's six hours.
"""

import re
import unittest
from pathlib import Path

WORKFLOWS = Path(__file__).resolve().parents[2] / ".github" / "workflows"
PINNED = re.compile(r"^\s*(?:- )?uses: [A-Za-z0-9_.-]+/[A-Za-z0-9_./-]+@[0-9a-f]{40} # v\d+(?:\.\d+)*$")


def jobs(text: str) -> dict[str, list[str]]:
    """{job id: its lines} for the workflow's top-level `jobs:` block (jobs indented by two spaces)."""
    out, current, inside = {}, None, False
    for line in text.splitlines():
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        if not line.startswith(" "):
            inside, current = line.startswith("jobs:"), None
            continue
        m = re.match(r"^  ([A-Za-z0-9_-]+):\s*$", line)
        if inside and m:
            current = out.setdefault(m.group(1), [])
        elif inside and current is not None:
            current.append(line)
    return out


class WorkflowTests(unittest.TestCase):
    def workflows(self):
        found = sorted(WORKFLOWS.glob("*.y*ml"))
        self.assertTrue(found)
        return [(p.name, p.read_text(encoding="utf-8")) for p in found]

    def test_every_action_is_pinned_to_a_commit(self):
        for name, text in self.workflows():
            for n, line in enumerate(text.splitlines(), 1):
                if re.match(r"^\s*(?:- )?uses:", line):
                    with self.subTest(f"{name}:{n}"):
                        self.assertRegex(line, PINNED)

    def test_every_job_has_a_timeout(self):
        for name, text in self.workflows():
            found = jobs(text)
            self.assertTrue(found, name)
            for job, lines in found.items():
                with self.subTest(f"{name}: {job}"):
                    self.assertTrue(any(re.match(r"^    timeout-minutes: \d+$", line) for line in lines))

    def test_the_rules_catch_what_they_are_for(self):
        self.assertNotRegex("      - uses: actions/checkout@v4", PINNED)
        self.assertNotRegex("      - uses: actions/checkout@11d5960a326750d5838078e36cf38b85af677262", PINNED)
        self.assertRegex("      - uses: actions/cache/save@0057852bfaa89a56745cba8c7296529d2fc39830 # v4.3.0", PINNED)
        text = "on: push\njobs:\n  a:\n    runs-on: x\n    timeout-minutes: 5\n  b:\n    runs-on: x\n"
        self.assertEqual(jobs(text), {"a": ["    runs-on: x", "    timeout-minutes: 5"], "b": ["    runs-on: x"]})


if __name__ == "__main__":
    unittest.main()
