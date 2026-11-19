"""B2's seal (docs/plans/humanoid-body.md 6): the freeze's git and source binding, and E3b's one-shot unseal.

**Threat model.** These guards stop accidental re-runs, stale inputs and
honest mistakes: a second E3b on one freeze, a replaced or edited artifact,
changed code, another reader, a run that changed underneath. They do not
stop a deliberately adversarial local user, who could edit this code
anyway.

**Sources.** The freeze binds every local module its entry points
(ENTRY_POINTS: the checker CLI, the checks, and this E3b path) import,
transitively, at any depth and inside functions, and every data file of
this folder those modules name (anim_replay.schema.json, anim_bounds.json),
all from the modules' syntax trees (`sources`): the model, the checks, run
loading, recording validation, case tables and Avatar decoding.
`source_digest` is the sha256 of their names and digests. The unityrs CLI
that read the Avatars and built clips is resolved once to an absolute real
path (`executable`), which is hashed and given to every child, and pinned by
its sha256 (`reader_sha256`): E3b needs the same executable.

**Git binding** (`canonical`, `bind_artifact`, `bind_code`). E3b takes only
the canonical artifact, FREEZE_PATH (a constant of these sources), with no
symlink at it or its parents inside this code's checkout, reads its bytes
once (their sha256 is the claim key, and the same bytes are parsed and
compared with git), and refuses unless:

- the artifact is tracked and its file is byte-identical to HEAD's;
- exactly one commit added that path (the freeze commit), nothing changed
  it since, and the freeze commit is an ancestor of HEAD;
- the code commit it records precedes the freeze commit and is an ancestor
  of HEAD, and no source file differs from it, at HEAD or in the work tree;
- the sources' digest is the recorded one, and the Editor versions, the
  hold-out cases and the minimum it records are the code's constants.

**One shot** (`e3b`). Before anything of the hold-out opens, E3b
reproduces the frozen training fit, design and lateral limit from the
frozen runs (anim_body_freeze.frozen_training) and checks, by listing
names only, that each Editor version's run records every hold-out case
(HOLDOUT_CASES) with its recording and bundle present. Then it claims the
shot: an exclusive create (O_CREAT | O_EXCL) of a marker named by the
freeze's sha256 in LEDGER_DIR, a fixed directory under the account's home
(from its passwd entry, never $HOME); an existing marker refuses. After
the claim, any failure is a recorded failed attempt and the shot stays
consumed. The first ledger row records each run's run.json and hold-out
recordings by sha256. The hold-out is read only through sealed completion
(anim_replay_complete.complete_folder with the freeze's sha256), which
itself refuses unless that claim exists (`require_claim`), validates the
hold-out (run identity, fixture topology, probe schedule and bundle) and
returns it with the sha256 of the bytes it read; E3b evaluates those
documents only if their sha256 and the run's run.json are the recorded
ones, so a run swapped underneath fails closed.

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
import pwd
import shutil
from pathlib import Path

import anim_replay as replay
import child
from anim_body_runs import Run, digest, statuses
from anim_fixture_body import BODY_CASES, BODY_INFO, HOLDOUT

HERE = Path(__file__).resolve().parent
ENTRY_POINTS = ("anim_body_freeze.py", "anim_body_check.py", "anim_body_seal.py")
# The one artifact E3b reads (bound separately, so never a source), and E3b's fixed ledger directory.
FREEZE_PATH = HERE / "anim_body_freeze_v7.json"
# The account's own home (its passwd entry, never $HOME).
LEDGER_DIR = Path(pwd.getpwuid(os.getuid()).pw_dir) / ".local" / "state" / "unityrs" / "e3b"
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


def executable(unityrs):
    """The unityrs CLI `unityrs` names (a path, or a name on PATH) as an absolute real path, resolved once: the
    one path hashed and given to every child."""
    replay.require(unityrs is not None, "the freeze and E3b pin the unityrs CLI: give --unityrs")
    path = Path(shutil.which(unityrs) or unityrs).resolve()
    replay.require(path.is_file(), f"no unityrs executable at {unityrs}")
    return str(path)


def reader_sha256(unityrs):
    """The sha256 of the unityrs executable at the absolute path `unityrs` (`executable`)."""
    replay.require(unityrs is not None and Path(unityrs).is_absolute(), "the unityrs CLI is not resolved")
    return hashlib.sha256(Path(unityrs).read_bytes()).hexdigest()


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


def code_repo():
    """The top of the git checkout holding this code."""
    return Path(git(HERE, "rev-parse", "--show-toplevel")[1]).resolve()


def canonical(path):
    """Refuses unless `path` is FREEZE_PATH itself (never resolved through a link), with no symlink at it or at
    any of its parents inside the checkout, and that checkout this code's."""
    replay.require(path is not None and Path(path).absolute() == FREEZE_PATH,
                   f"E3b reads only the canonical freeze artifact {FREEZE_PATH.name}")
    status, top = git(FREEZE_PATH.parent, "rev-parse", "--show-toplevel")
    replay.require(status == 0 and Path(top).resolve() == code_repo(),
                   "the freeze artifact is not in this code's checkout")
    for part in (FREEZE_PATH, *FREEZE_PATH.parents):
        replay.require(not part.is_symlink(), f"the freeze path holds a symlink: {part.name}")
        if part.resolve() == code_repo():
            break


def bind_artifact(path, code_commit, data=None):
    """E3b's artifact binding, returning the freeze commit: tracked, identical to HEAD's, and `data` (its bytes
    as read once, by default read now) those of the one commit that added it, unchanged since, that commit an
    ancestor of HEAD and, given the code commit, a descendant of it."""
    folder, name = path.parent, path.name
    data = path.read_bytes() if data is None else data
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
    replay.require(shown.returncode == 0 and shown.stdout == data,
                   "the freeze artifact differs from the one its freeze commit added")
    replay.require(code_commit is None or (code_commit != commit
                                           and git(folder, "merge-base", "--is-ancestor", code_commit, commit)[0] == 0),
                   "the freeze's code commit does not precede its freeze commit")
    return commit


def claim(freeze_sha256, ledger_dir=None):
    """Claims E3b's one shot for a freeze: an exclusive create of its marker, refused if it exists."""
    folder = Path(ledger_dir or LEDGER_DIR)
    folder.mkdir(parents=True, exist_ok=True)
    try:
        handle = os.open(folder / f"{freeze_sha256}.claim", os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
    except FileExistsError:
        raise ValueError("E3b already ran on this freeze (its claim exists): it runs once") from None
    os.close(handle)


def require_claim(freeze_sha256, ledger_dir=None):
    """Sealed completion's check before it opens a hold-out file: this freeze's claim exists."""
    replay.require(isinstance(freeze_sha256, str) and len(freeze_sha256) == 64
                   and (Path(ledger_dir or LEDGER_DIR) / f"{freeze_sha256}.claim").is_file(),
                   "the hold-out opens only after E3b claims its shot (anim_body_freeze.py e3b)")


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


def sha256_of(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def run_files(runs):
    """{version: {run.json and each hold-out recording: sha256}} from the bytes read now."""
    return {run.version: {"run.json": sha256_of(run.folder / "run.json"),
                          **{case: sha256_of(run.folder / "replays" / "body" / f"{case}.pending.json")
                             for case in HOLDOUT_CASES}} for run in runs}


def e3b(folders, path, unityrs, readers, complete_folder):
    """E3b through sealed completion, once per freeze, per Editor version. `unityrs` is `executable`'s."""
    import anim_body_freeze as freeze

    canonical(path)
    replay.require(path.is_file(), "no freeze artifact: E3b runs only after the freeze")
    data = path.read_bytes()
    freeze_sha256 = hashlib.sha256(data).hexdigest()
    artifact = freeze.read_freeze(path, data)
    commit = bind_artifact(path, artifact["code"]["commit"], data)
    reader = reader_sha256(unityrs)
    replay.require(reader == artifact["reader"]["sha256"], "the unityrs CLI is not the frozen one")
    avatar_of, clip_of = readers(unityrs)
    runs, _, design = freeze.frozen_training(folders, artifact, avatar_of, clip_of)
    preflight(runs)
    claim(freeze_sha256)
    try:
        files = run_files(runs)
        record(freeze_sha256, dict(event="claimed", freeze_commit=commit, code_commit=artifact["code"]["commit"],
                                   training=artifact["training"]["digest"], reader=reader, files=files))
        report = evaluate(runs, design, artifact, avatar_of, freeze_sha256, files, unityrs, complete_folder)
    except Exception as exc:
        record(freeze_sha256, dict(event="failed", error=f"{type(exc).__name__}: {exc}"[:500]))
        raise
    record(freeze_sha256, dict(event="evaluated", passed=report["passed"], failures=len(report["failures"])))
    return report


def evaluate(runs, design, artifact, avatar_of, freeze_sha256, files, unityrs, complete_folder):
    """After the claim: the hold-out through sealed completion, its bytes those recorded at the claim and its run
    unchanged, then each version alone."""
    import anim_body_check as check
    from anim_body_adequacy import default_masses
    from anim_body_runs import observe

    holdout = []
    for run in runs:
        _, _, unsealed = complete_folder(Path(run.folder), unityrs, None, replay, freeze_sha256)
        documents = {name: (sha256, document) for name, sha256, document in unsealed}
        replay.require(sorted(documents) == sorted(HOLDOUT_CASES), f"{run.version}: hold-out case(s) not recorded")
        replay.require({name: sha256 for name, (sha256, _) in documents.items()} | {
            "run.json": sha256_of(run.folder / "run.json")} == files[run.version],
                       f"{run.version}: the run changed after the claim")
        holdout.append(Run.sealed(run.folder, run.version, documents))
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
