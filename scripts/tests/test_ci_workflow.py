"""The CI workflows' own rules, read from the YAML as GitHub reads it.

- Every `uses` key, wherever it is (a step, a job calling a reusable
  workflow, a flow-style mapping such as `- {uses: ...}`), names a full
  commit SHA, with its release tag in a comment on the same line: a tag can
  be moved to other code, a SHA cannot.
- Every job has a `timeout-minutes`, so a hung step fails in minutes instead
  of GitHub's six hours.

Needs PyYAML (scripts/requirements-tests.txt), which CI's scripts-py job
installs; without it the tests skip, and CI's gate fails on a skip.
"""

import re
import unittest
from pathlib import Path

try:
    import yaml
except ImportError:  # pragma: no cover - outside CI's scripts venv
    yaml = None

WORKFLOWS = Path(__file__).resolve().parents[2] / ".github" / "workflows"
ACTION = re.compile(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_./-]+@[0-9a-f]{40}")
# What may follow the value on its line: closing flow brackets, then the tag comment.
TAG_COMMENT = re.compile(r"[ \t\]},]*# v\d+(?:\.\d+)*[ \t]*")


def uses_values(text: str) -> list:
    """The value node of every `uses` key in the YAML `text`, at any depth, in document order."""
    found, seen = [], set()

    def walk(node):
        if id(node) in seen:  # an alias points at a node already walked
            return
        seen.add(id(node))
        if isinstance(node, yaml.MappingNode):
            for key, value in node.value:
                if isinstance(key, yaml.ScalarNode) and key.value == "uses":
                    found.append(value)
                walk(key)
                walk(value)
        elif isinstance(node, yaml.SequenceNode):
            for item in node.value:
                walk(item)

    for document in yaml.compose_all(text, Loader=yaml.SafeLoader):
        walk(document)
    return found


def uses_problems(text: str) -> list[str]:
    """Each `uses` value in the YAML `text` that is not `owner/repo[/path]@SHA # vX.Y.Z`, as 'line N: why'."""
    lines, problems = text.splitlines(), []
    for value in uses_values(text):
        where = f"line {value.start_mark.line + 1}"
        end = value.end_mark
        if not isinstance(value, yaml.ScalarNode):
            problems.append(f"{where}: `uses` is not a plain value")
        elif not ACTION.fullmatch(value.value):
            problems.append(f"{where}: {value.value!r} is not owner/repo@<40-hex commit SHA>")
        elif end.line != value.start_mark.line or not TAG_COMMENT.fullmatch(lines[end.line][end.column :]):
            problems.append(f"{where}: {value.value} has no `# vX.Y.Z` tag comment after it on its line")
    return problems


def timeout_problems(text: str) -> list[str]:
    """Each job of the workflow `text` without a whole-minute `timeout-minutes`, and a workflow without jobs."""
    workflow = yaml.safe_load(text)
    jobs = workflow.get("jobs") if isinstance(workflow, dict) else None
    if not isinstance(jobs, dict) or not jobs:
        return ["no jobs"]
    return [
        f"job {job}: no timeout-minutes"
        for job, body in jobs.items()
        if not isinstance(body, dict) or type(body.get("timeout-minutes")) is not int or body["timeout-minutes"] < 1
    ]


@unittest.skipIf(yaml is None, "needs PyYAML (scripts/requirements-tests.txt)")
class WorkflowTests(unittest.TestCase):
    def workflows(self):
        found = sorted(WORKFLOWS.glob("*.y*ml"))
        self.assertTrue(found)
        return [(p.name, p.read_text(encoding="utf-8")) for p in found]

    def test_every_action_is_pinned_to_a_commit(self):
        for name, text in self.workflows():
            with self.subTest(name):
                self.assertTrue(uses_values(text), "the walk finds the workflow's actions")
                self.assertEqual(uses_problems(text), [])

    def test_every_job_has_a_timeout(self):
        for name, text in self.workflows():
            with self.subTest(name):
                self.assertEqual(timeout_problems(text), [])

    def test_the_rules_catch_what_they_are_for(self):
        sha = "11d5960a326750d5838078e36cf38b85af677262"
        good = f"jobs:\n  a:\n    timeout-minutes: 5\n    steps:\n      - uses: actions/checkout@{sha} # v4.4.0\n"
        self.assertEqual(uses_problems(good), [])
        self.assertEqual(timeout_problems(good), [])
        bad = {
            "a tag": "      - uses: actions/checkout@v4\n",
            "no tag comment": f"      - uses: actions/checkout@{sha}\n",
            "a flow-style step": "      - {uses: actions/checkout@v4}\n",
            "a flow-style step, quoted": "      - {name: x, 'uses': \"actions/checkout@v4\"}\n",
            "a job calling a reusable workflow": "  b:\n    uses: o/r/.github/workflows/w.yml@main\n",
            "an anchored step": "      - &s {uses: actions/cache@v4}\n      - *s\n",
            "a short SHA": "      - uses: actions/checkout@11d5960 # v4.4.0\n",
            "not a plain value": "      - uses: [actions/checkout@v4]\n",
        }
        for label, step in bad.items():
            with self.subTest(label):
                self.assertTrue(uses_problems(good + step), step)
        # A flow-style step pinned properly passes.
        self.assertEqual(uses_problems(good + f"      - {{uses: actions/checkout@{sha}}} # v4.4.0\n"), [])
        # A job without a timeout, a flow-style one included, fails.
        self.assertEqual(timeout_problems(good + "  b:\n    runs-on: x\n"), ["job b: no timeout-minutes"])
        self.assertEqual(timeout_problems(good + "  c: {runs-on: x, steps: []}\n"), ["job c: no timeout-minutes"])
        self.assertEqual(timeout_problems(good + "  d:\n    timeout-minutes: '5'\n"), ["job d: no timeout-minutes"])
        self.assertEqual(timeout_problems("on: push\n"), ["no jobs"])
        self.assertEqual(timeout_problems("- a\n"), ["no jobs"])


if __name__ == "__main__":
    unittest.main()
