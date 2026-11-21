"""Round 12's seal and verdicts (round 12 design v4 section 4): the freeze record, the one-time claim keyed to the
intake digest, and the evaluation of B5a's and the twist verdicts behind round 12's allowlist.

    python3 -I tools/parity/anim_twist_seal.py freeze --intake-digest SHA --candidate none --reason TEXT --unityrs PATH

writes FREEZE_PATH once (an exclusive create), to be committed on its own after the code it pins.

**Threat model**, anim_body_seal's: accidental misuse (a wrong or changed run, swapped Editors, a sealed file read
early, a second evaluation, stale code or another reader), never a deliberately adversarial local user.

**Freeze** (`make_freeze`). It references the intake digest (anim_fixture_round12.intake) and pins, by git commit
and sha256 (`pinned_files`): the formula and its checker (every local module the entry points ENTRY_POINTS import,
transitively, and the data files they name: the twist model, the bound, the reference 2 muscle pose, completion,
the allowlist, this verdict rule), the Rust port (crates/unityrs-anim/src/humanoid), the evaluator's data (the
muscle pose goldens, freeze v8, the round 12 tables as the builder carries them), and the unityrs executable that
decodes the bundles by its sha256. It names the chosen candidate (an anim_twist_model limb and foot), or `none`
with its reason: no feasible candidate, an unresolved cell, or a twist-only collection failure. It records B5a's
pinned refusal reasons and the digest of its expected rows, recomputed and compared at evaluation.

**Binding** (`bind`): FREEZE_PATH itself (no symlink at it or above it in this checkout), tracked, added by exactly
one commit after the pinned code commit, unchanged since; no pinned file changed since the code commit, at HEAD or
in the work tree; the same pinned file list and digest; the same executable (anim_body_seal's git binding).

**Claim** (`claim`): one exclusive create (O_CREAT | O_EXCL) of `<intake digest>.claim` in LEDGER_DIR, under the
account's home from its passwd entry; an existing claim refuses. The confirmation partition opens only through a
view made after it (`require_claim`), and after evaluation every byte any view read must be one the intake
digest bound (anim_fixture_round12.consumed). After the claim any failure is a recorded failed attempt, and the
claim stays spent.

**Verdicts** (`evaluate`), per Editor, never pooled:
- B5a runs on every path: its twist-free rows, exactly the expected ones within their bounds, and the 25 refusal
  cases' rows each with its pinned reason (anim_fixture_round12.b5a_failures). A twist-active row never enters it.
- Twist: a freeze of `none` refuses twist everywhere and never evaluates it. Otherwise the frozen candidate's rule
  must pass in both Editors on every playback path; any sealed failure, an exception included, refuses all twist.
"""

import argparse
import hashlib
import json
import os
import pwd
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import anim_replay as replay  # noqa: E402

HERE = Path(__file__).resolve().parent
TOP = HERE.parents[1]
FORMAT = "unityrs.twist_freeze"
ENTRY_POINTS = ("anim_twist_seal.py", "anim_fixture_round12.py", "anim_twist_model.py", "anim_twist_bound.py",
                "anim_twist_witness.py", "anim_muscle_confirm.py", "anim_muscle_pose.py", "anim_muscle_round11.py")
FREEZE_PATH = HERE / "anim_twist_freeze.json"
RUST_PORT = "crates/unityrs-anim/src/humanoid"
DATA = ("crates/unityrs-anim/tests/humanoid/muscle_pose.json", "tools/parity/anim_body_freeze_v8.json",
        "tools/fixtures/unity/Editor/UnityrsRound12Tables.cs")
LEDGER_DIR = Path(pwd.getpwuid(os.getuid()).pw_dir) / ".local" / "state" / "unityrs" / "round12"
REASONS = ("no feasible candidate", "an unresolved cell", "a twist-only collection failure")
VERDICT = "both Editors pass on every playback path; any sealed failure refuses all twist; B5a runs on every path"


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def pinned_files(top=TOP):
    """Every pinned file, repository-relative and sorted: the entry points' transitive sources and named data
    (anim_body_seal.sources) but the freeze itself, the Rust port's files, and DATA."""
    import anim_body_seal as seal

    parity = [f"tools/parity/{name}" for name in seal.sources(HERE, ENTRY_POINTS) if name != FREEZE_PATH.name]
    rust = [f"{RUST_PORT}/{name}" for name in os.listdir(top / RUST_PORT) if name.endswith(".rs")]
    return sorted({*parity, *rust, *DATA})


def files_digest(files, top=TOP):
    return sha256(json.dumps([(name, sha256((top / name).read_bytes())) for name in files],
                             separators=(",", ":")).encode("utf-8"))


def code_state(top=TOP):
    """At the freeze: (commit, files, digest), refused unless every pinned file is tracked and unchanged from HEAD."""
    from anim_body_seal import git

    status, commit = git(top, "rev-parse", "HEAD")
    replay.require(status == 0 and commit, "the freeze records the code's git commit: run it in a checkout")
    files = pinned_files(top)
    replay.require(git(top, "ls-files", "--error-unmatch", "--", *files)[0] == 0,
                   "a pinned file of the freeze is not tracked by git")
    replay.require(git(top, "diff", "--quiet", "HEAD", "--", *files)[0] == 0,
                   "a pinned file of the freeze differs from HEAD: commit the code first")
    return commit, files, files_digest(files, top)


def candidate_of(value):
    """The frozen candidate: "none", or {"limb": [...], "foot": [...]} naming one of anim_twist_model.candidates()."""
    import anim_twist_model as model

    if value == "none":
        return value
    replay.require(isinstance(value, dict) and set(value) == {"limb", "foot"}, f"no candidate {value!r}")
    limb, foot = model.Limb(*value["limb"]), model.Foot(*value["foot"])
    replay.require((limb, foot) in model.candidates(), f"{value!r} is not one of anim_twist_model's candidates")
    return dict(limb=list(limb), foot=list(foot))


def b5a_pins():
    """B5a's frozen expectations: the 25 refusal cases' reasons and the sha256 of its expected row sets."""
    import anim_fixture_round12 as round12

    rows, refused = round12.b5a_expected()
    return dict(pinned=round12.pinned_refusals(),
                rows=sha256(round12.canonical_json(sorted(map(list, rows), key=str))),
                refused=sha256(round12.canonical_json(sorted(map(list, refused), key=str))))


def make_freeze(intake_digest, candidate, unityrs, reason=None, top=TOP):
    """The freeze record (anim_body_seal.executable resolves `unityrs`)."""
    from anim_body_seal import executable, reader_sha256
    from anim_muscle_confirm import EDITORS

    replay.require(isinstance(intake_digest, str) and re.fullmatch("[0-9a-f]{64}", intake_digest),
                   "the freeze names its intake digest")
    candidate = candidate_of(candidate)
    replay.require((candidate == "none") == (reason is not None) and reason in (None, *REASONS),
                   f"a `none` freeze names one of {REASONS}, and only it")
    commit, files, digest = code_state(top)
    return dict(format=FORMAT, version=1, intake_digest=intake_digest, candidate=candidate, reason=reason,
                code=dict(commit=commit, files=files, digest=digest),
                reader=dict(sha256=reader_sha256(executable(unityrs))), editors=sorted(EDITORS), verdict=VERDICT,
                b5a=b5a_pins())


def write_freeze(record, path=FREEZE_PATH):
    """Writes the freeze once: an exclusive create, never a replacement."""
    data = (json.dumps(record, indent=1, sort_keys=True, ensure_ascii=False) + "\n").encode("utf-8")
    try:
        handle = os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o644)
    except FileExistsError:
        raise ValueError(f"{path.name} exists: a freeze is written once") from None
    with os.fdopen(handle, "wb") as out:
        out.write(data)
    return sha256(data)


def read_freeze(data):
    record = replay.read_json(data.decode("utf-8"))
    replay.require(isinstance(record, dict) and record.get("format") == FORMAT and record.get("version") == 1
                   and set(record) == {"format", "version", "intake_digest", "candidate", "reason", "code", "reader",
                                       "editors", "verdict", "b5a"}, f"expected {FORMAT} version 1")
    replay.require(candidate_of(record["candidate"]) == record["candidate"]
                   and (record["candidate"] == "none") == (record["reason"] in REASONS),
                   "the freeze's candidate or reason is not a frozen one")
    return record


def canonical(path, top=TOP):
    """FREEZE_PATH itself, never through a link: no symlink at it or above it inside this checkout."""
    replay.require(path is not None and Path(path).absolute() == FREEZE_PATH,
                   f"the evaluation reads only the canonical freeze {FREEZE_PATH.name}")
    for part in (FREEZE_PATH, *FREEZE_PATH.parents):
        replay.require(not part.is_symlink(), f"the freeze path holds a symlink: {part.name}")
        if part == top:
            break


def bind(path, unityrs, top=TOP):
    """(record, its sha256, the executable): the canonical committed freeze, its code and reader unchanged."""
    from anim_body_seal import bind_artifact, code_history, executable, reader_sha256
    from anim_muscle_confirm import EDITORS

    canonical(path, top)
    replay.require(path.is_file(), "no freeze: the evaluation runs only after it")
    data = path.read_bytes()
    record = read_freeze(data)
    bind_artifact(path, record["code"]["commit"], data)
    files = record["code"]["files"]
    replay.require(files == pinned_files(top), "the freeze's pinned files are not the code's")
    code_history(record["code"]["commit"], files, top)
    replay.require(record["code"]["digest"] == files_digest(files, top), "the pinned files differ from the frozen ones")
    unityrs = executable(unityrs)
    replay.require(record["reader"]["sha256"] == reader_sha256(unityrs), "the unityrs executable is not the frozen one")
    replay.require(record["editors"] == sorted(EDITORS) and record["verdict"] == VERDICT
                   and record["b5a"] == b5a_pins(), "the freeze's Editors, verdict rule or B5a pins are not the code's")
    return record, sha256(data), unityrs


def claim(intake_digest, ledger_dir=None):
    """Claims the one evaluation of an intake: an exclusive create of its marker, refused if it exists."""
    replay.require(isinstance(intake_digest, str) and re.fullmatch("[0-9a-f]{64}", intake_digest),
                   "a claim names an intake digest")
    folder = Path(ledger_dir or LEDGER_DIR)
    folder.mkdir(parents=True, exist_ok=True)
    try:
        handle = os.open(folder / f"{intake_digest}.claim", os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
    except FileExistsError:
        raise ValueError("this intake was already evaluated (its claim exists): it is evaluated once") from None
    os.close(handle)


def require_claim(intake_digest, ledger_dir=None):
    """A sealed view's check before it is made: this intake's claim exists."""
    replay.require(isinstance(intake_digest, str) and re.fullmatch("[0-9a-f]{64}", intake_digest)
                   and (Path(ledger_dir or LEDGER_DIR) / f"{intake_digest}.claim").is_file(),
                   "the confirmation partition opens only after the intake's claim (anim_twist_seal.evaluate)")


def record(intake_digest, row, ledger_dir=None):
    """Appends one row to the ledger."""
    import datetime

    line = json.dumps(dict(row, intake_digest=intake_digest,
                           time=datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds")),
                      ensure_ascii=False, sort_keys=True) + "\n"
    with open(Path(ledger_dir or LEDGER_DIR) / "ledger.jsonl", "a", encoding="utf-8") as ledger:
        ledger.write(line)


def sealed_views(round12_runs, round11_runs, intake_digest, ledger_dir=None):
    """[(Editor, "round12" | "round11", View)]: every partition open, each view made after the claim."""
    import anim_fixture_round12 as round12

    def opened():
        return require_claim(intake_digest, ledger_dir)
    views = [(e, "round12", round12.round12_view(f, round12.PARTITIONS, opened)) for e, (f, _) in round12_runs.items()]
    return views + [(e, "round11", round12.round11_view(f, round12.PARTITIONS, opened))
                    for e, (f, _) in round11_runs.items()]


def twist_verdict(freeze, intake_record, views, unityrs, twist_rule):
    """The twist verdict: `none` refuses all twist unevaluated; otherwise `twist_rule(candidate, views, unityrs)`
    gives {Editor: failures}, and both Editors must pass. Any failure, an exception or a twist-only case a run
    lacks included, refuses all twist."""
    from anim_muscle_confirm import EDITORS

    if freeze["candidate"] == "none":
        return dict(admitted="none", failures=[f"twist refused everywhere: the freeze is none ({freeze['reason']})"])
    lacking = {e: row["twist_only_lacking"] for e, row in sorted(intake_record["round12"].items())
               if row["twist_only_lacking"]}
    if lacking:
        return dict(admitted="none", failures=[f"{e}: a twist-only collection failure: {len(cases)} case(s) lacking, "
                                               f"{cases[:3]}" for e, cases in lacking.items()])
    try:
        found = twist_rule(freeze["candidate"], views, unityrs)
        failures = [f"no twist verdict for Editor {e}" for e in EDITORS if e not in found]
        failures += [f"{e}: {line}" for e, lines in sorted(found.items()) for line in lines]
    except Exception as exc:
        failures = [f"the twist evaluation failed: {type(exc).__name__}: {exc}"[:500]]
    return dict(admitted="none" if failures else freeze["candidate"], failures=failures)


def evaluate_bound(freeze, round12_folders, round11_folders, unityrs, observe_b5a, twist_rule, ledger_dir=None,
                   expected=None):
    """The evaluation of a bound freeze: the folders' intake digest the frozen one; the claim; then B5a and twist
    through sealed views, every byte read the intake's. `observe_b5a(views, unityrs)` returns an
    anim_muscle_confirm.Observation of B5a's rows."""
    import anim_fixture_round12 as round12

    digest, intake_record = round12.intake(round12_folders, round11_folders)
    replay.require(digest == freeze["intake_digest"], "the runs are not the frozen intake's")
    claim(digest, ledger_dir)
    try:
        record(digest, dict(event="claimed", candidate=freeze["candidate"], intake=intake_record), ledger_dir)
        runs12 = {e: (Path(f), r) for e, f, r in _runs(intake_record["round12"], round12_folders)}
        runs11 = {e: (Path(f), r) for e, f, r in _runs(intake_record["round11"], round11_folders)}
        views = sealed_views(runs12, runs11, digest, ledger_dir)
        b5a = round12.b5a_failures(observe_b5a(views, unityrs), expected)
        twist = twist_verdict(freeze, intake_record, views, unityrs, twist_rule)
        round12.consumed(intake_record, views)
    except Exception as exc:
        record(digest, dict(event="failed", error=f"{type(exc).__name__}: {exc}"[:500]), ledger_dir)
        raise
    report = dict(b5a=dict(passed=not b5a, failures=b5a), twist=twist)
    record(digest, dict(event="evaluated", b5a=not b5a, twist=twist["admitted"]), ledger_dir)
    return report


def _runs(recorded, folders):
    """(Editor, folder, run id) of each folder, by the Editor its run.json declares, as the intake recorded it."""
    import anim_fixture_round12 as round12

    for folder in folders:
        editor, run_id = round12.editor_of_run(folder)
        replay.require(recorded[editor]["run_id"] == run_id, f"Editor {editor}'s run is not the intake's")
        yield editor, folder, run_id


def evaluate(round12_folders, round11_folders, unityrs, observe_b5a, twist_rule, path=FREEZE_PATH):
    """The one evaluation: the canonical committed freeze bound, then `evaluate_bound`."""
    freeze, _, unityrs = bind(path, unityrs)
    return evaluate_bound(freeze, round12_folders, round11_folders, unityrs, observe_b5a, twist_rule)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    freeze = sub.add_parser("freeze")
    freeze.add_argument("--intake-digest", required=True)
    freeze.add_argument("--candidate", required=True, help='"none" or JSON {"limb": [...], "foot": [...]}')
    freeze.add_argument("--reason", choices=REASONS)
    freeze.add_argument("--unityrs", required=True)
    args = parser.parse_args()
    try:
        candidate = args.candidate if args.candidate == "none" else replay.read_json(args.candidate)
        found = write_freeze(make_freeze(args.intake_digest, candidate, args.unityrs, args.reason))
        print(f"wrote {FREEZE_PATH.name} (sha256 {found}): commit it alone, after the code it pins")
    except (ValueError, OSError) as exc:
        parser.exit(1, f"anim_twist_seal: {exc}\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
