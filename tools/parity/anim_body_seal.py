"""B2's seal (docs/plans/humanoid-body.md 6): the freeze's git and source binding, and E3b's one-shot unseal.

**Sources.** The freeze binds every local module its entry points
(ENTRY_POINTS: the checker CLI, the checks, and this E3b path) import,
transitively, at any depth and inside functions (`sources`, from the
modules' syntax trees): the model, the checks, run loading, recording
validation, case tables and Avatar decoding. `source_digest` is the sha256
of their names and digests.

**Git binding** (`bind_artifact`, `bind_code`). E3b refuses unless:

- the artifact is tracked and its file is byte-identical to HEAD's;
- exactly one commit added it (the freeze commit), nothing changed it
  since, and the freeze commit is an ancestor of HEAD;
- the code commit it records is an ancestor of HEAD (and of the freeze
  commit, in one repository), and no source file differs from it, at HEAD
  or in the working tree;
- the sources' digest is the recorded one.

**One-shot unseal** (`e3b`). The hold-out is read only through sealed
completion (anim_replay_complete.complete_folder with an `Unseal`): its
recording is validated there (run identity, fixture topology, probe
schedule and bundle) and handed to E3b, which never opens a hold-out file
itself. Before any hold-out file opens, the unseal reproduces the frozen
training fit, design and lateral limit from the frozen runs
(anim_body_freeze.frozen_training). Each Editor version must hold every
hold-out case (HOLDOUT_CASES). Then, before evaluating anything, E3b
appends an entry to the private ledger (`--ledger`): the freeze file's
sha256, its commit, each hold-out recording's sha256 and the time. A
ledger that already holds the freeze refuses: E3b runs once per freeze.

**Per version.** E3b evaluates each Editor version alone: E0 on its
hold-out Avatar, and E3 within the frozen lateral limit, with at least
MIN_COMPARISONS in-scope comparisons (stated from the hold-out's design
below). Every version must pass; nothing pools across them.
"""

import ast
import datetime
import hashlib
import json
import os
from pathlib import Path

import anim_replay as replay
import child
from anim_body_runs import Run, digest
from anim_fixture_body import BODY_INFO, HOLDOUT

HERE = Path(__file__).resolve().parent
ENTRY_POINTS = ("anim_body_freeze.py", "anim_body_check.py", "anim_body_seal.py")
HOLDOUT_CASES = tuple(sorted(name for name, info in BODY_INFO.items() if info["rig"] == HOLDOUT))
# The in-scope comparisons each Editor version's hold-out must reach, from its design: body-probes-holdout's
# schedule (anim_fixture_body.probe_schedule) has 45 known vectors on the compact topology. Seven bend the spine
# sideways by .25 or more (the three single Left-Right probes, the three combined vectors and the noncommuting
# anchor), past any lateral limit under .25, and 2.2's frame scope refuses five more by design (the single Twist
# probes of Spine, Chest and UpperChest, and each shoulder's Front-Back): IN_SCOPE_BY_DESIGN. The minimum allows
# three more frame refusals for the hold-out's longer arms, and no fewer.
FRAME_REFUSED = ("Spine Twist Left-Right", "Chest Twist Left-Right", "UpperChest Twist Left-Right",
                 "Left Shoulder Front-Back", "Right Shoulder Front-Back")
IN_SCOPE_BY_DESIGN = 33
MIN_COMPARISONS = IN_SCOPE_BY_DESIGN - 3


def imports(path):
    """The module names a source file imports, at any depth."""
    tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
    names = set()
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            names.update(alias.name.split(".")[0] for alias in node.names)
        elif isinstance(node, ast.ImportFrom) and node.level == 0 and node.module:
            names.add(node.module.split(".")[0])
    return names


def sources(folder=HERE, entries=None):
    """The entry points (ENTRY_POINTS by default) and every local module they import, transitively, sorted."""
    found, pending = set(), list(entries or ENTRY_POINTS)
    while pending:
        name = pending.pop()
        if name in found:
            continue
        found.add(name)
        pending += [module + ".py" for module in imports(folder / name) if (folder / (module + ".py")).is_file()]
    return sorted(found)


def source_digest(folder=HERE, files=None):
    rows = [(name, hashlib.sha256((folder / name).read_bytes()).hexdigest()) for name in files or sources(folder)]
    return hashlib.sha256(json.dumps(rows, separators=(",", ":")).encode("utf-8")).hexdigest()


def git(folder, *args):
    """A git command's (exit status, stdout) in `folder`."""
    result = child.run(["git", "-C", str(folder), *args], capture_output=True, text=True)
    return result.returncode, result.stdout.strip()


def code_history(commit, files, folder=HERE):
    """The recorded code commit an ancestor of HEAD, and no source changed since it, at HEAD or in the work tree."""
    replay.require(git(folder, "merge-base", "--is-ancestor", commit, "HEAD")[0] == 0,
                   "the freeze's code commit is not an ancestor of HEAD")
    for against in ((commit, "HEAD"), (commit,)):
        replay.require(git(folder, "diff", "--quiet", *against, "--", *files)[0] == 0,
                       "a source file changed since the freeze's code commit")


def code_state(folder=HERE):
    """At the freeze: (commit, files, digest) of the code, refused unless every source is tracked and unchanged
    from HEAD."""
    status, commit = git(folder, "rev-parse", "HEAD")
    replay.require(status == 0 and commit, "the freeze records the code's git commit: run it in a checkout")
    files = sources(folder)
    replay.require(git(folder, "ls-files", "--error-unmatch", "--", *files)[0] == 0,
                   "a source file of the freeze is not tracked by git")
    replay.require(git(folder, "diff", "--quiet", "HEAD", "--", *files)[0] == 0,
                   "a source file of the freeze differs from HEAD: commit the code first")
    return commit, files, source_digest(folder, files)


def bind_code(code, folder=HERE):
    """E3b's code binding: the recorded commit an ancestor of HEAD, no source changed since it, the same
    sources and digest."""
    commit, files = code["commit"], code["files"]
    replay.require(files == sources(folder), "the freeze's source files are not the code's")
    code_history(commit, files, folder)
    replay.require(code["digest"] == source_digest(folder, files), "the sources differ from the frozen ones")


def bind_artifact(path, code_commit, code_folder=HERE):
    """E3b's artifact binding, returning the freeze commit: tracked, identical to HEAD's and to the one commit
    that added it, unchanged since, that commit an ancestor of HEAD and, given the code commit and in the code's
    repository, a descendant of it."""
    folder, name = path.resolve().parent, path.name
    replay.require(git(folder, "ls-files", "--error-unmatch", "--", name)[0] == 0,
                   "the freeze artifact is not tracked by git")
    replay.require(git(folder, "diff", "--quiet", "HEAD", "--", name)[0] == 0,
                   "the freeze artifact differs from its committed version")
    status, added = git(folder, "log", "--format=%H", "--diff-filter=A", "--", name)
    replay.require(status == 0 and len(added.split()) == 1, "the freeze artifact was not added by exactly one commit")
    commit = added.split()[0]
    replay.require(git(folder, "log", "--format=%H", f"{commit}..HEAD", "--", name) == (0, ""),
                   "the freeze artifact changed after its freeze commit")
    replay.require(git(folder, "merge-base", "--is-ancestor", commit, "HEAD")[0] == 0,
                   "the freeze commit is not an ancestor of HEAD")
    _, relative = git(folder, "ls-files", "--full-name", "--", name)
    shown = child.run(["git", "-C", str(folder), "show", f"{commit}:{relative}"], capture_output=True)
    replay.require(shown.returncode == 0 and shown.stdout == path.read_bytes(),
                   "the freeze artifact differs from the one its freeze commit added")
    same = git(folder, "rev-parse", "--show-toplevel") == git(code_folder, "rev-parse", "--show-toplevel")
    replay.require(code_commit is None or not same or (code_commit != commit
                                and git(folder, "merge-base", "--is-ancestor", code_commit, commit)[0] == 0),
                   "the freeze's code commit does not precede its freeze commit")
    return commit


class Unseal:
    """complete_folder's unseal: `verify(folder)` before any hold-out file opens, `accept` for each validated
    hold-out recording."""

    def __init__(self, folders, check_folder):
        self.folders = {str(Path(folder).resolve()) for folder in folders}
        self.check_folder = check_folder
        self.documents = {}

    def verify(self, folder):
        replay.require(str(Path(folder).resolve()) in self.folders, "the unsealed run is not one of the frozen runs")
        self.check_folder(folder)

    def accept(self, folder, case, sha256, document):
        replay.require(BODY_INFO.get(case, {}).get("rig") == HOLDOUT, f"{case} is not a hold-out case")
        self.documents.setdefault(str(Path(folder).resolve()), {})[case] = (sha256, document)


def ledger_rows(ledger, freeze_sha256):
    """The ledger's entries, refused if one is this freeze's: E3b runs once per freeze."""
    rows = replay.read_json(ledger.read_text(encoding="utf-8")) if ledger.exists() else []
    replay.require(isinstance(rows, list) and all(isinstance(row, dict) for row in rows), "malformed E3b ledger")
    replay.require(all(row.get("freeze_sha256") != freeze_sha256 for row in rows),
                   "E3b already ran on this freeze (the ledger holds it): it runs once")
    return rows


def ledger_entry(ledger, freeze_sha256, entry):
    """Appends `entry` to the ledger, refused if it already holds this freeze; written before any evaluation."""
    rows = ledger_rows(ledger, freeze_sha256) + [entry]
    staged = ledger.with_name(f".{ledger.name}.{os.getpid()}.tmp")
    staged.write_text(json.dumps(rows, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    os.replace(staged, ledger)


def e3b(folders, path, unityrs, ledger, readers, complete_folder):
    """E3b through sealed completion, once per freeze, per Editor version."""
    import anim_body_check as check
    import anim_body_freeze as freeze
    from anim_body_adequacy import default_masses
    from anim_body_runs import observe

    replay.require(ledger is not None, "E3b needs --ledger, its one-shot record")
    artifact = freeze.read_freeze(path)
    freeze_sha256 = hashlib.sha256(path.read_bytes()).hexdigest()
    ledger_rows(Path(ledger), freeze_sha256)
    avatar_of, _ = readers(unityrs)
    runs, _, design = freeze.frozen_training(folders, artifact, avatar_of)
    frozen = {(row["run"], row["case"], row["sha256"]) for row in artifact["training"]["inputs"]}

    def check_folder(folder):
        run = next(run for run in runs if str(run.folder.resolve()) == str(Path(folder).resolve()))
        replay.require({(row["run"], row["case"], row["sha256"]) for row in run.inputs} <= frozen,
                       f"the freeze does not hold this run's ({run.version}) training recordings")

    unseal = Unseal(folders, check_folder)
    for folder in folders:
        complete_folder(Path(folder), unityrs, None, replay, unseal)
    holdout = []
    for run in runs:
        documents = unseal.documents.get(str(run.folder.resolve()), {})
        absent = [case for case in HOLDOUT_CASES if case not in documents]
        replay.require(not absent, f"{run.version}: hold-out case(s) {absent} not recorded")
        holdout.append(Run.sealed(run.folder, run.version, documents))
    entry = dict(freeze_sha256=freeze_sha256, freeze_commit=bind_artifact(path, artifact["code"]["commit"]),
                 code_commit=artifact["code"]["commit"], training=artifact["training"]["digest"],
                 holdout={run.version: {row["case"]: row["sha256"] for row in run.inputs} for run in holdout},
                 time=datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds"))
    ledger_entry(Path(ledger), freeze_sha256, entry)
    table = default_masses([doc for run in runs for doc in run.documents.values()])
    limit = artifact["scope"]["lateral_limit"]
    versions, failures = {}, []
    for run in holdout:
        avatars = check.e0([run], avatar_of, design, artifact["coefficients"], table)
        report = check.e3(observe([run], table[:2], avatar_of), design, artifact["coefficients"], limit)
        short = [] if report["samples"] >= MIN_COMPARISONS else [
            f"{run.version}: {report['samples']} in-scope comparison(s), under the minimum {MIN_COMPARISONS}"]
        found = [f"{run.version}: {line}" for line in avatars["failures"] + report["failures"]] + short
        versions[run.version] = dict(passed=not found, samples=report["samples"], refused=report["refused"],
                                     worst_ratio=report["worst_ratio"], avatars=avatars["avatars"])
        failures += found
    return dict(passed=not failures, failures=failures, versions=versions, minimum=MIN_COMPARISONS,
                digest=artifact["training"]["digest"], holdout=digest([row for run in holdout for row in run.inputs]),
                ledger=entry)
