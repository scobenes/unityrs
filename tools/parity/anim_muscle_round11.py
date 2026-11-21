"""B5a's round 11 inventory: the cases its confirmation requires of each Editor's round 11 run, and the reader
of their recordings.

docs/plans/humanoid-body.md, B5a (e), froze them before any round 11 data: all four muscle sweeps and every
full-body clip, in both Editors. They are round 11's builder's, read from its tables
(anim_muscle_confirm.b5a_cases, anim_fixture_round11.ROUND11_CASES' bundles), never copied: 4 sweeps, 25
refusal-evidence and 318 in-scope full-body clips. Freeze v8's `anim_body_runs.Run` lists neither and is not
edited; `Recordings` reads them beside it, from the same run folder. A run declares round 11 in `round.json`.
Each run's required cases are this inventory together with any full-body clip its `run.json` lists, so neither
can drop one unseen. Each capture's own metadata names its case, run, Editor version and bundle.
anim_muscle_confirm.py judges them. No round 11 rig is a hold-out rig. Each opens lazily through a view
(anim_fixture_round12.View): round 11's own by default, round 12's sealed ones where round 12 reuses them.
"""

import os
import sys
from collections.abc import Mapping
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import anim_replay as replay  # noqa: E402

ROUND = 11


def _cases():
    import anim_fixture_round11 as r11
    from anim_muscle_confirm import b5a_cases

    return {case: r11.ROUND11_CASES[case].bundle for case in b5a_cases()}


# case: bundle
CASES = _cases()
FULL_CLIPS = {case: bundle for case, bundle in CASES.items() if case.startswith("body-clip-")}


def full_clip_bundle(name):
    """A full-body clip's bundle, `body-clips-<rig>`, from its name; None for any other case."""
    if not name.startswith("body-clip-"):
        return None
    for marker in ("-full-", "-scoped-"):
        rig, found, _ = name.removeprefix("body-clip-").partition(marker)
        if found:
            return "body-clips-" + rig + marker[:-1]
    return None


def declared_round(folder):
    """ROUND for a run whose `round.json` declares round 11, None for a run without one (rounds 1 to 10)."""
    path = folder / "round.json"
    if not path.is_file():
        return None
    manifest = replay.read_json((folder / "run.json").read_text(encoding="utf-8"))
    declared = replay.read_json(path.read_text(encoding="utf-8"))
    replay.require(declared == {"format": "unityrs.fixture_round", "round": ROUND,
                                "run_id": manifest["run"].get("run_id")},
                   f"{folder.name}: round.json does not declare this round 11 run")
    return ROUND


def required(statuses):
    """{case: bundle}: the inventory and every full-body clip `run.json` lists."""
    listed = {name: full_clip_bundle(name) for name in statuses if full_clip_bundle(name)}
    return dict(sorted((listed | CASES).items()))


class Recordings:
    """One round 11 training run's B5a cases through a view (anim_fixture_round12.View): Unity's recordings
    (`.pending.json`, as `Run(pending=True)` reads them) by case, opened lazily through `view` only when a document
    is read, and the required cases it lacks, by names alone. The default view is round 11's own, permissive one
    (anim_fixture_round12.round11_own_view), for round 11's own confirmation and case export; round 12 passes its
    sealed views, which refuse a case they do not allow when it is read, never opening it."""

    def __init__(self, folder, view=None):
        import anim_body_runs as runs
        import anim_fixture_round12 as round12

        self.view = view or round12.round11_own_view(folder)
        replay.require(Path(self.view.folder) == Path(folder), f"the view is another folder's than {folder}")
        manifest = replay.read_json(self.view.run_file("run.json").decode("utf-8"))
        self.folder, self.version, self.run_id = folder, manifest["run"]["editor_version"], manifest["run"]["run_id"]
        statuses = runs.statuses(manifest)
        self.required = required(statuses)
        top = folder / "replays" / "body"
        names = set(os.listdir(top)) if top.is_dir() else set()
        present = [case for case in self.required
                   if statuses.get(case) == "recorded" and f"{case}.pending.json" in names]
        self.missing = [case for case in self.required if case not in present]
        self.documents = _Lazy(present, self._read)

    def _read(self, case):
        doc = self.view.capture(case)
        bundle = self.required[case]
        # The capture's own identity, never its file name: its case, run, Editor and bundle.
        editor = doc["spec"]["evidence"]["editor"]
        replay.require(editor.get("fixture_case") == case, f"{case}.pending.json records another case than {case}")
        replay.require(editor.get("run_id") == self.run_id, f"{case} was recorded by another run")
        replay.require(doc["capture"].get("editor_version") == self.version,
                       f"{case} was recorded by another Editor version than {self.version}")
        replay.require([row["name"] for row in doc["spec"]["bundles"]] == [bundle],
                       f"{case} plays another bundle than {bundle}")
        return doc


class _Lazy(Mapping):
    """{case: document} over the present cases, each read once, on first access."""

    def __init__(self, cases, read):
        self._cases, self._read, self._docs = tuple(cases), read, {}

    def __getitem__(self, case):
        if case not in self._cases:
            raise KeyError(case)
        if case not in self._docs:
            self._docs[case] = self._read(case)
        return self._docs[case]

    def __iter__(self):
        return iter(self._cases)

    def __len__(self):
        return len(self._cases)
