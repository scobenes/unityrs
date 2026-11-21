"""Round 12's seal and verdicts (round 12 design v4 section 4): the freeze record, the one-time selection
transition and claim, both keyed to the intake digest, and the evaluation of B5a's and the twist verdicts behind
round 12's allowlist.

    python3 -I tools/parity/anim_twist_seal.py freeze --intake-digest SHA --candidate none --reason TEXT
        --observe-b5a MODULE:FUNCTION [--twist-rule MODULE:FUNCTION] --reader-out PATH

builds the unityrs executable from the pinned commit (READER_BUILD), writes it to PATH and FREEZE_PATH once (an
exclusive create), to be committed on its own after the code it pins.

**Threat model**, anim_body_seal's: accidental misuse (a wrong or changed run, swapped Editors, a sealed file read
early, a second evaluation, stale code or another reader), never a deliberately adversarial local user.

**Freeze** (`make_freeze`). It references the intake digest (anim_fixture_round12.intake) and pins, by git commit
and each file's sha256 (`pinned_files`): the formula and its checker (every local module the entry points
ENTRY_POINTS and the hooks' modules import, transitively, and the data files they name: the twist model, the
bound, the reference 2 muscle pose, completion, the allowlist, this verdict rule), the Rust port
(crates/unityrs-anim/src/humanoid), the evaluator's data (the muscle pose goldens, freeze v8, the round 12 tables
as the builder carries them), and the unityrs executable that decodes the bundles: built by the freeze itself
from the pinned commit's tree (`build_reader`, READER_BUILD, recorded), its sha256 recorded. It names the
evaluation's hooks, `module:function` in a pinned module (B5a's observer; the twist rule, unless `none`); the
evaluation loads each from the exact pinned bytes (`load_hook`) and takes no other callable. It names the chosen
candidate (an anim_twist_model limb and foot), or `none` with its reason. It records B5a's pinned refusal reasons
and the digest of its expected rows, recomputed and compared at evaluation.

**Binding** (`bind`): FREEZE_PATH itself (no symlink at it or above it in this checkout), tracked, added by exactly
one commit after the pinned code commit, unchanged since; no pinned file changed since the code commit, at HEAD or
in the work tree; the same pinned files and sha256s; the same executable (anim_body_seal's git binding).

**Selection** (`open_selection`): before the claim, the selection partition opens through a one-time transition,
an exclusive create of `<intake digest>.selection` recording the ambiguity exploration left (unresolved cells or
pairs); none refuses it, and a second refuses.

**Claim** (`claim`): one exclusive create (O_CREAT | O_EXCL) of `<intake digest>.claim` in LEDGER_DIR, under the
account's home from its passwd entry, its state written in it and fsynced; an existing marker refuses. Its state
moves by atomic replacement (claimed, b5a, evaluated or failed). A marker without a complete state, from a process
that died while claiming, is detected and refuses everything that needs the claim, and a new claim too: it is
inspected and removed by hand, never silently claimed again. The confirmation partition opens only through a view
made after it (`require_claim`), and every byte any view read must be one the intake digest bound
(anim_fixture_round12.consumed). After the claim any failure is a recorded failed attempt, and the claim stays
spent.

**Verdicts** (`evaluate`), per Editor, never pooled, each through its own views:
- B5a first, on every path, recorded before twist begins: its twist-free rows, exactly the expected ones within
  their bounds, and the 25 refusal cases' rows each with its pinned reason (anim_twist_b5a.b5a_failures). Its
  failure, an exception or bytes the intake never bound included, is B5a's alone.
- Twist: a freeze of `none` refuses twist everywhere and never evaluates it. Otherwise the frozen rule must pass
  in both Editors on every playback path; any sealed failure (an exception, a twist-only case a run lacks or
  authored wrongly, bytes the intake never bound) refuses all twist, never B5a.
"""

import argparse
import hashlib
import io
import json
import os
import pwd
import re
import sys
import types
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import anim_replay as replay  # noqa: E402

HERE = Path(__file__).resolve().parent
TOP = HERE.parents[1]
FORMAT = "unityrs.twist_freeze"
ENTRY_POINTS = ("anim_twist_seal.py", "anim_fixture_round12.py", "anim_twist_b5a.py", "anim_twist_model.py",
                "anim_twist_bound.py", "anim_twist_witness.py", "anim_muscle_confirm.py", "anim_muscle_pose.py",
                "anim_muscle_round11.py")
FREEZE_PATH = HERE / "anim_twist_freeze.json"
RUST_PORT = "crates/unityrs-anim/src/humanoid"
DATA = ("crates/unityrs-anim/tests/humanoid/muscle_pose.json", "tools/parity/anim_body_freeze_v8.json",
        "tools/fixtures/unity/Editor/UnityrsRound12Tables.cs")
LEDGER_DIR = Path(pwd.getpwuid(os.getuid()).pw_dir) / ".local" / "state" / "unityrs" / "round12"
REASONS = ("no feasible candidate", "an unresolved cell", "a twist-only collection failure")
VERDICT = ("B5a first and recorded on every path; twist: both Editors pass on every playback path, and any sealed "
           "failure refuses all twist")
# The reader's build from the pinned commit's tree (`git archive`), its paths remapped so the tree's location
# leaves no trace in the executable.
READER_BUILD = ("cargo", "build", "--release", "--locked", "-p", "unityrs-cli", "--bin", "unityrs")
HOOKS = ("observe_b5a", "twist_rule")
STATES = ("claimed", "b5a", "evaluated", "failed")
DIGEST = re.compile("[0-9a-f]{64}")


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def digest_of(value):
    replay.require(isinstance(value, str) and DIGEST.fullmatch(value), "an intake digest is 64 lowercase hex")
    return value


def hook_modules(hooks):
    """The pinned module file of each named hook, `module:function`."""
    found = []
    for name, spec in sorted((hooks or {}).items()):
        replay.require(name in HOOKS, f"no hook {name}")
        if spec is None:
            continue
        module, _, function = str(spec).partition(":")
        replay.require(re.fullmatch(r"[a-z0-9_]+", module) and re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", function),
                       f"{name} names no module:function ({spec!r})")
        found.append(f"{module}.py")
    return found


def pinned_files(top=TOP, hooks=None):
    """Every pinned file, repository-relative and sorted: the entry points' and the hooks' modules' transitive
    sources and named data (anim_body_seal.sources) but the freeze itself, the Rust port's files, and DATA."""
    import anim_body_seal as seal

    entries = (*ENTRY_POINTS, *hook_modules(hooks))
    parity = [f"tools/parity/{name}" for name in seal.sources(HERE, entries) if name != FREEZE_PATH.name]
    rust = [f"{RUST_PORT}/{name}" for name in os.listdir(top / RUST_PORT) if name.endswith(".rs")]
    return sorted({*parity, *rust, *DATA})


def file_digests(files, top=TOP):
    return {name: sha256((top / name).read_bytes()) for name in files}


def code_state(hooks, top=TOP):
    """At the freeze: {commit, files, sha256}, refused unless every pinned file is tracked and unchanged from HEAD."""
    from anim_body_seal import git

    status, commit = git(top, "rev-parse", "HEAD")
    replay.require(status == 0 and commit, "the freeze records the code's git commit: run it in a checkout")
    files = pinned_files(top, hooks)
    replay.require(git(top, "ls-files", "--error-unmatch", "--", *files)[0] == 0,
                   "a pinned file of the freeze is not tracked by git")
    replay.require(git(top, "diff", "--quiet", "HEAD", "--", *files)[0] == 0,
                   "a pinned file of the freeze differs from HEAD: commit the code first")
    return dict(commit=commit, files=files, sha256=file_digests(files, top))


def build_reader(commit, out, top=TOP):
    """Builds the unityrs executable from `commit`'s tree (READER_BUILD), writes it to `out` and returns its
    sha256 and the recorded build."""
    import tarfile

    import child
    import scratch

    archive = child.run(["git", "-C", str(top), "archive", "--format=tar", commit], capture_output=True)
    replay.require(archive.returncode == 0, f"git archive {commit} failed")
    with scratch.folder("unityrs-reader-") as folder:
        tree = folder / "tree"
        with tarfile.open(fileobj=io.BytesIO(archive.stdout)) as tar:
            tar.extractall(tree, filter="data")
        env = dict(os.environ, CARGO_TARGET_DIR=str(folder / "target"),
                   RUSTFLAGS=f"--remap-path-prefix={tree}=/unityrs")
        built = child.run(list(READER_BUILD), cwd=tree, env=env, capture_output=True, text=True)
        replay.require(built.returncode == 0, f"the reader's build failed: {built.stderr[-500:]}")
        data = (folder / "target" / "release" / "unityrs").read_bytes()
    Path(out).write_bytes(data)
    os.chmod(out, 0o755)
    return dict(sha256=sha256(data), commit=commit, command=list(READER_BUILD), remap="--remap-path-prefix=<tree>=/unityrs")


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
    import anim_twist_b5a as b5a

    rows, refused = b5a.b5a_expected()
    return dict(pinned=b5a.pinned_refusals(),
                rows=sha256(round12.canonical_json(sorted(map(list, rows), key=str))),
                refused=sha256(round12.canonical_json(sorted(map(list, refused), key=str))))


def make_freeze(intake_digest, candidate, hooks, reader_out, reason=None, top=TOP):
    """The freeze record: its code, its hooks (each loaded once from the pinned bytes), and the reader built from
    the pinned commit into `reader_out`."""
    from anim_muscle_confirm import EDITORS

    digest_of(intake_digest)
    candidate = candidate_of(candidate)
    replay.require((candidate == "none") == (reason is not None) and reason in (None, *REASONS),
                   f"a `none` freeze names one of {REASONS}, and only it")
    hooks = {name: (hooks or {}).get(name) for name in HOOKS}
    replay.require(hooks["observe_b5a"] is not None and (hooks["twist_rule"] is not None or candidate == "none"),
                   "the freeze names B5a's observer, and the twist rule unless `none`")
    code = code_state(hooks, top)
    record = dict(format=FORMAT, version=1, intake_digest=intake_digest, candidate=candidate, reason=reason,
                  hooks=hooks, code=code, reader=build_reader(code["commit"], reader_out, top),
                  editors=sorted(EDITORS), verdict=VERDICT, b5a=b5a_pins())
    for name in HOOKS:
        load_hook(record, name, top)
    return record


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
                   and set(record) == {"format", "version", "intake_digest", "candidate", "reason", "hooks", "code",
                                       "reader", "editors", "verdict", "b5a"}, f"expected {FORMAT} version 1")
    replay.require(candidate_of(record["candidate"]) == record["candidate"]
                   and (record["candidate"] == "none") == (record["reason"] in REASONS),
                   "the freeze's candidate or reason is not a frozen one")
    replay.require(isinstance(record["hooks"], dict) and sorted(record["hooks"]) == sorted(HOOKS)
                   and record["hooks"]["observe_b5a"] is not None
                   and (record["hooks"]["twist_rule"] is not None or record["candidate"] == "none"),
                   "the freeze's hooks are not B5a's observer and the twist rule")
    digest_of(record["intake_digest"])
    return record


def load_hook(freeze, name, top=TOP):
    """A frozen hook, from the exact pinned bytes of its module: refused unless the freeze names it and the
    module's bytes are its pinned sha256. Its module's own imports are pinned files too (`bind`)."""
    spec = freeze["hooks"].get(name)
    if spec is None:
        return None
    replay.require(isinstance(spec, str), f"hook {name} is not a pinned module:function ({spec!r})")
    module, _, function = spec.partition(":")
    path = f"tools/parity/{module}.py"
    replay.require(path in freeze["code"]["files"], f"hook {name} is not in a pinned module ({path})")
    data = (top / path).read_bytes()
    replay.require(sha256(data) == freeze["code"]["sha256"].get(path), f"hook {name}'s module {path} changed")
    loaded = types.ModuleType(f"_round12_hook_{module}")
    loaded.__file__ = str(top / path)
    exec(compile(data, str(top / path), "exec"), loaded.__dict__)  # noqa: S102 (the pinned bytes)
    found = getattr(loaded, function, None)
    replay.require(callable(found), f"hook {name}: {spec} is not a function of its module")
    return found


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
    code = record["code"]
    bind_artifact(path, code["commit"], data)
    replay.require(code["files"] == pinned_files(top, record["hooks"]), "the freeze's pinned files are not the code's")
    code_history(code["commit"], code["files"], top)
    replay.require(code["sha256"] == file_digests(code["files"], top), "the pinned files differ from the frozen ones")
    replay.require(record["reader"]["commit"] == code["commit"] and record["reader"]["command"] == list(READER_BUILD),
                   "the freeze's reader was not built from its code commit")
    unityrs = executable(unityrs)
    replay.require(record["reader"]["sha256"] == reader_sha256(unityrs),
                   "the unityrs executable is not the one built from the pinned commit")
    replay.require(record["editors"] == sorted(EDITORS) and record["verdict"] == VERDICT
                   and record["b5a"] == b5a_pins(), "the freeze's Editors, verdict rule or B5a pins are not the code's")
    return record, sha256(data), unityrs


def _marker(intake_digest, kind, ledger_dir):
    return Path(ledger_dir or LEDGER_DIR) / f"{digest_of(intake_digest)}.{kind}"


def _create(path, content):
    """An exclusive create of a marker holding `content`, written and fsynced; an existing one refuses."""
    path.parent.mkdir(parents=True, exist_ok=True)
    handle = os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
    try:
        os.write(handle, (json.dumps(content, sort_keys=True) + "\n").encode("utf-8"))
        os.fsync(handle)
    finally:
        os.close(handle)


def _state(path, intake_digest):
    """A marker's content, refused unless complete: a marker a dying process left half-written refuses."""
    replay.require(path.is_file(), f"no {path.suffix[1:]} marker for this intake")
    try:
        content = json.loads(path.read_text(encoding="utf-8"))
    except ValueError:
        content = None
    replay.require(isinstance(content, dict) and content.get("intake_digest") == intake_digest
                   and content.get("state") in (*STATES, "opened"),
                   f"{path.name} is half-written (a process died while writing it): inspect it and remove it by hand")
    return content


def open_selection(intake_digest, ambiguity, ledger_dir=None):
    """The one-time selection transition: refused unless exploration left an ambiguity (a non-empty list of its
    unresolved cells or pairs), and refused a second time."""
    replay.require(isinstance(ambiguity, list) and ambiguity, "selection opens only on an ambiguity exploration left")
    path = _marker(intake_digest, "selection", ledger_dir)
    try:
        _create(path, dict(intake_digest=intake_digest, state="opened", ambiguity=ambiguity))
    except FileExistsError:
        raise ValueError("the selection partition was already opened for this intake: it opens once") from None


def require_selection(intake_digest, ledger_dir=None):
    _state(_marker(intake_digest, "selection", ledger_dir), intake_digest)


def claim(intake_digest, ledger_dir=None):
    """Claims the one evaluation of an intake: an exclusive create of its marker in state `claimed`."""
    path = _marker(intake_digest, "claim", ledger_dir)
    try:
        _create(path, dict(intake_digest=intake_digest, state="claimed"))
    except FileExistsError:
        _state(path, intake_digest)
        raise ValueError("this intake was already evaluated (its claim exists): it is evaluated once") from None


def require_claim(intake_digest, ledger_dir=None):
    """A sealed view's check before it is made: this intake's claim exists, complete."""
    try:
        path = _marker(intake_digest, "claim", ledger_dir)
    except ValueError:
        path = None
    replay.require(path is not None and path.is_file(),
                   "the confirmation partition opens only after the intake's claim (anim_twist_seal.evaluate)")
    _state(path, intake_digest)


def mark(intake_digest, state, ledger_dir=None):
    """Moves a complete claim to `state` by atomic replacement."""
    path = _marker(intake_digest, "claim", ledger_dir)
    _state(path, intake_digest)
    staged = path.with_name(f".{path.name}.{os.getpid()}")
    staged.write_text(json.dumps(dict(intake_digest=intake_digest, state=state), sort_keys=True) + "\n")
    os.replace(staged, path)


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
    views = [(e, "round12", round12.round12_view(f, round12.PARTITIONS, opened)) for e, f in round12_runs.items()]
    return views + [(e, "round11", round12.round11_view(f, round12.PARTITIONS, opened))
                    for e, f in round11_runs.items()]


def b5a_verdict(observe, views, unityrs, intake_record, expected=None):
    """B5a's failures through its own views: its observer's rows judged (anim_twist_b5a.b5a_failures), every byte
    read the intake's. An exception is a B5a failure."""
    import anim_fixture_round12 as round12
    import anim_twist_b5a as b5a

    try:
        found = b5a.b5a_failures(observe(views, unityrs), expected)
        round12.consumed(intake_record, views)
        return found
    except Exception as exc:
        return [f"the B5a evaluation failed: {type(exc).__name__}: {exc}"[:500]]


def twist_verdict(freeze, intake_record, views, unityrs, twist_rule):
    """The twist verdict through its own views: `none` refuses all twist unevaluated; otherwise the frozen rule
    gives {Editor: failures}, both Editors must pass and every byte read must be the intake's. Any failure, an
    exception or a twist-only case a run lacks included, refuses all twist."""
    import anim_fixture_round12 as round12
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
        round12.consumed(intake_record, views)
    except Exception as exc:
        failures = [f"the twist evaluation failed: {type(exc).__name__}: {exc}"[:500]]
    return dict(admitted="none" if failures else freeze["candidate"], failures=failures)


def evaluate_bound(freeze, round12_folders, round11_folders, unityrs, ledger_dir=None, expected=None, top=TOP):
    """The evaluation of a bound freeze: its hooks loaded from the pinned bytes; the folders' intake digest the
    frozen one; the claim; B5a, recorded; then twist."""
    import anim_fixture_round12 as round12

    observe, twist_rule = (load_hook(freeze, name, top) for name in HOOKS)
    digest, intake_record = round12.intake(round12_folders, round11_folders)
    replay.require(digest == freeze["intake_digest"], "the runs are not the frozen intake's")
    claim(digest, ledger_dir)
    try:
        record(digest, dict(event="claimed", candidate=freeze["candidate"], intake=intake_record), ledger_dir)
        runs12 = {e: f for e, f, _ in _runs(intake_record["round12"], round12_folders)}
        runs11 = {e: f for e, f, _ in _runs(intake_record["round11"], round11_folders)}
        b5a = b5a_verdict(observe, sealed_views(runs12, runs11, digest, ledger_dir), unityrs, intake_record, expected)
        record(digest, dict(event="b5a", passed=not b5a, failures=b5a), ledger_dir)
        mark(digest, "b5a", ledger_dir)
        twist = twist_verdict(freeze, intake_record, sealed_views(runs12, runs11, digest, ledger_dir), unityrs,
                              twist_rule)
    except Exception as exc:
        record(digest, dict(event="failed", error=f"{type(exc).__name__}: {exc}"[:500]), ledger_dir)
        mark(digest, "failed", ledger_dir)
        raise
    record(digest, dict(event="evaluated", b5a=not b5a, twist=twist["admitted"]), ledger_dir)
    mark(digest, "evaluated", ledger_dir)
    return dict(b5a=dict(passed=not b5a, failures=b5a), twist=twist)


def _runs(recorded, folders):
    """(Editor, folder, run id) of each folder, by the Editor its run.json declares, as the intake recorded it."""
    import anim_fixture_round12 as round12

    for folder in folders:
        editor, run_id, _ = round12.editor_of_run(folder)
        replay.require(recorded[editor]["run_id"] == run_id, f"Editor {editor}'s run is not the intake's")
        yield editor, Path(folder), run_id


def evaluate(round12_folders, round11_folders, unityrs, path=FREEZE_PATH):
    """The one evaluation: the canonical committed freeze bound, then `evaluate_bound` with its frozen hooks."""
    freeze, _, unityrs = bind(path, unityrs)
    return evaluate_bound(freeze, round12_folders, round11_folders, unityrs)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    freeze = sub.add_parser("freeze")
    freeze.add_argument("--intake-digest", required=True)
    freeze.add_argument("--candidate", required=True, help='"none" or JSON {"limb": [...], "foot": [...]}')
    freeze.add_argument("--reason", choices=REASONS)
    freeze.add_argument("--observe-b5a", required=True)
    freeze.add_argument("--twist-rule")
    freeze.add_argument("--reader-out", type=Path, required=True)
    args = parser.parse_args()
    try:
        candidate = args.candidate if args.candidate == "none" else replay.read_json(args.candidate)
        hooks = dict(observe_b5a=args.observe_b5a, twist_rule=args.twist_rule)
        found = write_freeze(make_freeze(args.intake_digest, candidate, hooks, args.reader_out, args.reason))
        print(f"wrote {FREEZE_PATH.name} (sha256 {found}) and the reader: commit the freeze alone, after the code")
    except (ValueError, OSError) as exc:
        parser.exit(1, f"anim_twist_seal: {exc}\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
