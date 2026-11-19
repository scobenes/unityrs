"""B2's seal (docs/plans/humanoid-body.md 6): the freeze's git and source binding, and E3b's one-shot unseal.

**Sources.** The freeze binds every local module its entry points
(ENTRY_POINTS: the checker CLI, the checks, and this E3b path) import,
transitively, at any depth and inside functions, and every data file of
this folder those modules name (anim_replay.schema.json, anim_bounds.json),
all from the modules' syntax trees (`sources`): the model, the checks, run
loading, recording validation, case tables and Avatar decoding.
`source_digest` is the sha256 of their names and digests. The unityrs CLI
that read the Avatars and built clips is pinned by its sha256
(`reader_sha256`): E3b needs the same executable.

**Git binding** (`bind_artifact`, `bind_code`). E3b takes only the
canonical artifact, FREEZE_PATH (a constant of these sources), and
refuses unless:

- the artifact is tracked and its file is byte-identical to HEAD's;
- exactly one commit added that path (the freeze commit), nothing changed
  it since, and the freeze commit is an ancestor of HEAD;
- the code commit it records is an ancestor of HEAD (and of the freeze
  commit, in one repository), and no source file differs from it, at HEAD
  or in the working tree;
- the sources' digest is the recorded one, and the Editor versions, the
  hold-out cases and the minimum it records are the code's constants.

**One shot** (`e3b`). Before anything of the hold-out opens, E3b
reproduces the frozen training fit, design and lateral limit from the
frozen runs (anim_body_freeze.frozen_training) and checks, by listing
names only, that each Editor version's run records every hold-out case
(HOLDOUT_CASES) with its recording and bundle present. Then it claims the
shot: an exclusive create (O_CREAT | O_EXCL) of a marker named by the
freeze's sha256 in LEDGER_DIR, a fixed directory outside the repository; an
existing marker refuses. The claim appends a ledger row; after it, any
failure is a recorded failed attempt and the shot stays consumed. The
hold-out is read only through sealed completion
(anim_replay_complete.complete_folder with an `Unseal`), which validates
it (run identity, fixture topology, probe schedule and bundle) and hands it
over; a second row records the hold-out recordings' sha256 before any
evaluation, and a last one the result.

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
import shutil
from pathlib import Path

import anim_replay as replay
import child
from anim_body_runs import Run, digest, statuses
from anim_fixture_body import BODY_CASES, BODY_INFO, HOLDOUT

HERE = Path(__file__).resolve().parent
ENTRY_POINTS = ("anim_body_freeze.py", "anim_body_check.py", "anim_body_seal.py")
# The one artifact E3b reads (bound separately, so never a source), and E3b's fixed ledger directory.
FREEZE_PATH = HERE / "anim_body_freeze_v5.json"
LEDGER_DIR = Path.home() / ".local" / "state" / "unityrs" / "e3b"
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


def references(path):
    """(the module names a source file imports at any depth, the string constants it holds)."""
    tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
    names, strings = set(), set()
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            names.update(alias.name.split(".")[0] for alias in node.names)
        elif isinstance(node, ast.ImportFrom) and node.level == 0 and node.module:
            names.add(node.module.split(".")[0])
        elif isinstance(node, ast.Constant) and isinstance(node.value, str):
            strings.add(node.value)
    return names, strings


def sources(folder=HERE, entries=None):
    """The entry points (ENTRY_POINTS by default), every local module they import, transitively, and every
    other file of `folder` they name but the freeze artifact itself, sorted."""
    found, data, pending = set(), set(), list(entries or ENTRY_POINTS)
    while pending:
        name = pending.pop()
        if name in found:
            continue
        found.add(name)
        modules, strings = references(folder / name)
        pending += [module + ".py" for module in modules if (folder / (module + ".py")).is_file()]
        data |= {text for text in strings if "/" not in text and not text.endswith(".py")
                 and text != FREEZE_PATH.name and (folder / text).is_file()}
    return sorted(found | data)


def reader_sha256(unityrs):
    """The sha256 of the unityrs executable `unityrs` names (a path, or a name on PATH)."""
    replay.require(unityrs is not None, "the freeze and E3b pin the unityrs CLI: give --unityrs")
    path = Path(shutil.which(unityrs) or unityrs)
    replay.require(path.is_file(), f"no unityrs executable at {unityrs}")
    return hashlib.sha256(path.read_bytes()).hexdigest()


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


def claim(freeze_sha256, ledger_dir=None):
    """Claims E3b's one shot for a freeze: an exclusive create of its marker, refused if it exists."""
    folder = Path(ledger_dir or LEDGER_DIR)
    folder.mkdir(parents=True, exist_ok=True)
    try:
        handle = os.open(folder / f"{freeze_sha256}.claim", os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
    except FileExistsError:
        raise ValueError("E3b already ran on this freeze (its claim exists): it runs once") from None
    os.close(handle)


def record(freeze_sha256, row, ledger_dir=None):
    """Appends one row to the ledger (one JSON line per row)."""
    line = json.dumps(dict(row, freeze_sha256=freeze_sha256,
                           time=datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds")),
                      ensure_ascii=False, sort_keys=True) + "\n"
    with open(Path(ledger_dir or LEDGER_DIR) / "ledger.jsonl", "a", encoding="utf-8") as ledger:
        ledger.write(line)


def preflight(runs):
    """By names alone, never opening a hold-out file: every hold-out case recorded in each run, its recording
    and bundle present."""
    for run in runs:
        recorded = statuses(replay.read_json((run.folder / "run.json").read_text(encoding="utf-8")))
        files = set(os.listdir(run.folder / "replays" / "body")) if (run.folder / "replays" / "body").is_dir() else set()
        bundles = set(os.listdir(run.folder / "bundles")) if (run.folder / "bundles").is_dir() else set()
        absent = [case for case in HOLDOUT_CASES if recorded.get(case) != "recorded"
                  or f"{case}.pending.json" not in files or BODY_CASES[case].bundle not in bundles]
        replay.require(not absent, f"{run.version}: hold-out case(s) {absent} not recorded")


def e3b(folders, path, unityrs, readers, complete_folder):
    """E3b through sealed completion, once per freeze, per Editor version."""
    import anim_body_freeze as freeze

    replay.require(path is not None and path.resolve() == FREEZE_PATH.resolve(),
                   f"E3b reads only the canonical freeze artifact {FREEZE_PATH.name}")
    artifact = freeze.read_freeze(path)
    commit = bind_artifact(path, artifact["code"]["commit"])
    reader = reader_sha256(unityrs)
    replay.require(reader == artifact["reader"]["sha256"], "the unityrs CLI is not the frozen one")
    avatar_of, clip_of = readers(unityrs)
    runs, _, design = freeze.frozen_training(folders, artifact, avatar_of, clip_of)
    preflight(runs)
    freeze_sha256 = hashlib.sha256(path.read_bytes()).hexdigest()
    claim(freeze_sha256)
    record(freeze_sha256, dict(event="claimed", freeze_commit=commit, code_commit=artifact["code"]["commit"],
                               training=artifact["training"]["digest"], reader=reader,
                               runs=sorted(run.version for run in runs)))
    try:
        report = evaluate(runs, design, artifact, avatar_of, freeze_sha256, unityrs, complete_folder)
    except Exception as exc:
        record(freeze_sha256, dict(event="failed", error=f"{type(exc).__name__}: {exc}"[:500]))
        raise
    record(freeze_sha256, dict(event="evaluated", passed=report["passed"], failures=len(report["failures"])))
    return report


def evaluate(runs, design, artifact, avatar_of, freeze_sha256, unityrs, complete_folder):
    """After the claim: the hold-out through sealed completion, recorded, then each version alone."""
    import anim_body_check as check
    from anim_body_adequacy import default_masses
    from anim_body_runs import observe

    frozen = {(row["run"], row["case"], row["sha256"]) for row in artifact["training"]["inputs"]}

    def check_folder(folder):
        run = next(run for run in runs if str(run.folder.resolve()) == str(Path(folder).resolve()))
        replay.require({(row["run"], row["case"], row["sha256"]) for row in run.inputs} <= frozen,
                       f"the freeze does not hold this run's ({run.version}) training recordings")

    unseal = Unseal([run.folder for run in runs], check_folder)
    for run in runs:
        complete_folder(Path(run.folder), unityrs, None, replay, unseal)
    holdout = []
    for run in runs:
        documents = unseal.documents.get(str(run.folder.resolve()), {})
        absent = [case for case in HOLDOUT_CASES if case not in documents]
        replay.require(not absent, f"{run.version}: hold-out case(s) {absent} not recorded")
        holdout.append(Run.sealed(run.folder, run.version, documents))
    record(freeze_sha256, dict(event="unsealed", holdout={run.version: {row["case"]: row["sha256"]
                                                                        for row in run.inputs} for run in holdout}))
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
                digest=artifact["training"]["digest"], holdout=digest([row for run in holdout for row in run.inputs]))
