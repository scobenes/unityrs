"""B2's body checker (docs/plans/humanoid-body.md 3.2 and 6): E0 to E4, the freeze, and E3b after it.

    python3 -I anim_body_freeze.py MODE RUN [RUN ...] [--freeze FILE] [--unityrs CLI] [--json REPORT]

Each RUN is a completed `<version>/<target>/` fixture folder (one per Editor
version); MODE is one of:

- `e0`: the Avatars (anim_body_check), with the coefficients of `--freeze`
  if given, or else fitted on the training rows;
- `e1`, `e2`, `e4`: on the training rows;
- `e3a`: the training checkpoint, the hips of every training sample within
  T_s with coefficients fitted on the training rows;

- `freeze`: E3a, and when it passes, the freeze artifact at `--freeze` (a
  new file): the model choices (anim_body_model.HYPOTHESES), coefficients,
  scope (Editor versions, training rigs and their topologies, the lateral
  limit, cases, free combinations) and bounds (unit roundoffs, operation depths, limits, the
  comparison), with the training inputs (Editor version, case, rig and
  sha256 per file) and their digest;
- `e3b`: the hold-out rig against the freeze artifact at `--freeze`.

E2 and E3a compare only poses inside the lateral limit (plan section 8):
the largest spine Left-Right value of a training sample the model compared
(never one the twist scope refused) below every sample failing either, and
at least LATERAL_STEP below them
(anim_body_check.lateral_limit). A required band sweep unrecorded leaves
no limit, as does one under LATERAL_FLOOR, and both fail. The
freeze records it, E3b refuses past it, and a freeze whose training runs
give another limit is refused.

Every mode reads the run's bundles through `unityrs show` (`--unityrs`):
each recording's Avatar, for its masses (m_HumanBoneMass, decision 1), its
scope and its F(T) (m_SkeletonPose), and for E4 each clip case's built clip.

Order: E3a, then the freeze, then E3b. The hold-out stays out of everything
before E3b by construction: every mode but `e3b` loads only the training role
(anim_body_runs: by run.json and the case table, never by file name), which
never opens a hold-out file, and the fit refuses a hold-out row. Completion
(anim_replay.py complete) seals the hold-out's cases too until a valid
freeze unseals them (`verify_unseal`).

**The freeze is immutable and binding.** It needs E3a passed on every
required training case of both Editor versions (2022 and 6000), a design of
full rank and every coefficient within |a| <= 1, and records them with the
git commit of the code and a digest of the model's source files
(MODEL_FILES). E3b refuses unless the artifact is tracked by git and
unmodified at HEAD, its E3a passed, its model choices, bounds and model
digest are the code's, its training inputs are the runs given, and the
training design and fit reproduce its rank, condition and coefficients
bit for bit. It then evaluates the hold-out with the stored coefficients
alone: E0 on the hold-out's Avatars (E0b's fold only here) and E3.

Every check also fails when a run lacks a required case (Run.missing).

Exit 0 when the check passes, 1 when it fails, 2 when it refuses or the
input is unreadable.
"""

import argparse
import hashlib
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import anim_body_adequacy as adequacy  # noqa: E402
import anim_body_check as check  # noqa: E402
import anim_body_e4 as e4_check  # noqa: E402
import anim_replay as replay  # noqa: E402
import child  # noqa: E402
from anim_body_model import ANGLE_FLOOR, HYPOTHESES, TWIST_LIMIT  # noqa: E402
from anim_body_runs import Design, digest, load, observe, role_of, solve  # noqa: E402
from anim_body_tolerance import OPERATIONS, U32, U64  # noqa: E402
from anim_fixture_body import BAND_CASES, BODY_INFO, HOLDOUT, rig_bones  # noqa: E402

FORMAT = "unityrs.anim_body_freeze"
VERSION = 3
MODES = ("e0", "e1", "e2", "e3a", "e4", "freeze", "e3b")
HERE = Path(__file__).resolve().parent
# The source files the model's equations, bounds, scope and checks live in.
MODEL_FILES = ("anim_body_adequacy.py", "anim_body_check.py", "anim_body_design.py", "anim_body_e4.py",
               "anim_body_freeze.py", "anim_body_model.py", "anim_body_runs.py", "anim_body_tolerance.py",
               "anim_fixture_body.py", "anim_reference.py")
EDITORS = ("2022", "6000")


def model_digest():
    rows = [(name, hashlib.sha256((HERE / name).read_bytes()).hexdigest()) for name in MODEL_FILES]
    return hashlib.sha256(json.dumps(rows, separators=(",", ":")).encode("utf-8")).hexdigest()


def git(folder, *args):
    """A git command's (exit status, stdout) in `folder`."""
    result = child.run(["git", "-C", str(folder), *args], capture_output=True, text=True)
    return result.returncode, result.stdout.strip()


def bounds(design=None):
    """The frozen bounds: the code's constants, and the training design's rank and condition."""
    fixed = dict(condition_limit=adequacy.CONDITION_LIMIT, coefficient_limit=check.COEFFICIENT_LIMIT,
                 unit_roundoff_f32=U32, unit_roundoff_f64=U64, operations=dict(OPERATIONS),
                 fold_depth=check.FOLD_DEPTH, angle_floor=ANGLE_FLOOR, twist_limit=TWIST_LIMIT,
                 lateral_step=check.LATERAL_STEP, lateral_floor=check.LATERAL_FLOOR,
                 comparison="each hips translation component within T_s = E_unity,s + u_s + E_eval,s, "
                            "with u_s <= E_unity,s")
    return fixed if design is None else fixed | dict(condition=design.condition, rank=design.rank)


def training(folders, avatar_of=None):
    runs = load(folders, "training")
    rows = observe(runs, avatar_of=avatar_of)
    replay.require(rows, "no training recordings")
    return runs, rows, Design(rows)


def inputs_of(runs):
    return [row for run in runs for row in run.inputs]


def missing(runs):
    return [f"{run.version}: required case {case} not recorded" for run in runs for case in run.missing]


def lateral(runs, rows, design, coefficients):
    """(limit, smallest failing value, failures): the training runs' lateral limit
    (anim_body_check.lateral_limit). With a required band sweep unrecorded, or below LATERAL_FLOOR, there is
    none: it fails, and the checks then compare every pose (limit None), so the failures that would set it
    show."""
    absent = [f"{run.version}: {case}" for run in runs for case in run.missing if case in BAND_CASES]
    if absent:
        return None, float("inf"), ["no spine lateral limit: the band sweeps that validate it are not recorded ("
                                    + ", ".join(absent) + ")"]
    limit, worst = check.lateral_limit(rows, design, coefficients)
    if limit >= check.LATERAL_FLOOR:
        return limit, worst, []
    return None, worst, [f"no validated spine lateral domain: limit {limit:.6g} below {check.LATERAL_FLOOR} "
                         f"(smallest failing spine Left-Right {worst:.6g})"]


def e3a(folders, avatar_of):
    runs, rows, design = training(folders, avatar_of)
    coefficients = solve(design)
    limit, worst, failures = lateral(runs, rows, design, coefficients)
    report = check.e3(rows, design, coefficients, limit)
    report["failures"] += failures + missing(runs)
    report["passed"] = not report["failures"]
    return report | dict(coefficients=coefficients, digest=digest(inputs_of(runs)), lateral_limit=limit,
                         lateral_failing=worst), runs, design


def freeze(folders, path, avatar_of):
    replay.require(path is not None and not path.exists(), "freeze writes a new artifact: give a new --freeze path")
    report, runs, design = e3a(folders, avatar_of)
    replay.require(report["passed"], "E3a must pass before the freeze")
    versions = sorted({run.version for run in runs})
    replay.require(sorted(v.split(".")[0] for v in versions) == list(EDITORS),
                   f"the freeze needs one run of each Editor version {EDITORS}, not {versions}")
    replay.require(design.rank == len(design.keys), "the freeze needs a design of full rank")
    coefficients = report["coefficients"]
    replay.require(all(abs(a) <= check.COEFFICIENT_LIMIT for a in coefficients.values()),
                   f"the freeze needs every coefficient within |a| <= {check.COEFFICIENT_LIMIT}")
    inputs = inputs_of(runs)
    replay.require(all(row["rig"] != HOLDOUT and role_of(row["case"]) == "training" for row in inputs),
                   "a hold-out input reached the freeze")
    status, commit = git(HERE, "rev-parse", "HEAD")
    replay.require(status == 0 and commit, "the freeze records the code's git commit: run it in a checkout")
    rigs = sorted({row["rig"] for row in inputs} - {"p2b"})
    artifact = dict(
        format=FORMAT, version=VERSION, model=HYPOTHESES, coefficients=coefficients,
        scope=dict(editor_versions=versions, training_rigs=rigs, lateral_limit=report["lateral_limit"],
                   topologies={rig: sorted(rig_bones(rig)) for rig in rigs if rig in adequacy.TRAINING_RIGS},
                   cases=sorted({row["case"] for row in inputs}), free=design.free, holdout=HOLDOUT),
        bounds=bounds(design),
        training=dict(digest=digest(inputs), inputs=sorted(inputs, key=lambda row: (row["run"], row["case"])),
                      holdout_rows=0),
        e3a=dict(passed=True, samples=report["samples"], worst_ratio=report["worst_ratio"],
                 refused=report["refused"]),
        code=dict(commit=commit, model_digest=model_digest(), files=list(MODEL_FILES)))
    path.write_text(json.dumps(artifact, indent=2, ensure_ascii=False, allow_nan=False) + "\n", encoding="utf-8")
    return report


def read_freeze(path):
    """The freeze artifact, refused unless git tracks it unmodified, it came from training inputs alone after a
    passed E3a, and the code's model, bounds and source files are the frozen ones."""
    replay.require(path is not None and path.is_file(), "no freeze artifact: E3b runs only after the freeze")
    folder, name = path.resolve().parent, path.name
    replay.require(git(folder, "ls-files", "--error-unmatch", "--", name)[0] == 0,
                   "the freeze artifact is not tracked by git")
    replay.require(git(folder, "diff", "--quiet", "HEAD", "--", name)[0] == 0,
                   "the freeze artifact differs from its committed version")
    artifact = replay.read_json(path.read_text(encoding="utf-8"))
    replay.require(artifact.get("format") == FORMAT and artifact.get("version") == VERSION,
                   f"expected {FORMAT} version {VERSION}")
    data = artifact["training"]
    inputs = data["inputs"]
    replay.require(data["holdout_rows"] == 0 and inputs
                   and all(row["rig"] != HOLDOUT and role_of(row["case"]) == "training" for row in inputs),
                   "the freeze artifact was produced from hold-out inputs")
    replay.require(data["digest"] == digest(inputs), "the freeze artifact's digest disagrees with its inputs")
    replay.require(artifact["e3a"].get("passed") is True, "the freeze artifact records no passed E3a")
    replay.require(artifact["model"] == HYPOTHESES, "the model choices differ from the frozen ones")
    frozen = {key: value for key, value in artifact["bounds"].items() if key not in ("condition", "rank")}
    replay.require(frozen == json.loads(json.dumps(bounds())), "the bounds differ from the frozen ones")
    replay.require(artifact["code"]["model_digest"] == model_digest(), "the model's source files differ from the frozen ones")
    coefficients = artifact["coefficients"]
    replay.require(isinstance(coefficients, dict) and all(type(a) is float for a in coefficients.values()),
                   "malformed frozen coefficients")
    limit = artifact["scope"].get("lateral_limit")
    replay.require(type(limit) is float and limit >= check.LATERAL_FLOOR, "malformed frozen lateral limit")
    return artifact


def frozen_training(folders, artifact, avatar_of):
    """The training runs, rows and design, refused unless they are the frozen ones and reproduce its fit."""
    runs = load(folders, "training")
    replay.require(digest(inputs_of(runs)) == artifact["training"]["digest"],
                   "the training inputs differ from the frozen ones")
    replay.require(sorted(run.version for run in runs) == artifact["scope"]["editor_versions"],
                   "the Editor versions differ from the frozen ones")
    rows = observe(runs, avatar_of=avatar_of)
    design = Design(rows)
    coefficients = artifact["coefficients"]
    replay.require(sorted(coefficients) == design.keys, "the frozen coefficients do not match the training design")
    replay.require((design.rank, design.condition) == (artifact["bounds"]["rank"], artifact["bounds"]["condition"]),
                   "the training design's rank or condition differs from the frozen one")
    replay.require(solve(design) == coefficients, "the frozen coefficients are not the training fit")
    replay.require(check.lateral_limit(rows, design, coefficients)[0] == artifact["scope"]["lateral_limit"],
                   "the training runs' lateral limit differs from the frozen one")
    return runs, rows, design


def e3b(folders, path, avatar_of):
    artifact = read_freeze(path)
    runs, _, design = frozen_training(folders, artifact, avatar_of)
    coefficients = artifact["coefficients"]
    holdout = load(folders, "holdout")
    replay.require(any(run.documents for run in holdout), "no hold-out recording")
    documents = [doc for run in runs for doc in run.documents.values()]
    table = adequacy.default_masses(documents)
    avatars = check.e0([run for run in holdout if run.documents], avatar_of, design, coefficients, table)
    report = check.e3(observe(holdout, table[:2], avatar_of), design, coefficients, artifact["scope"]["lateral_limit"])
    report["failures"] = avatars["failures"] + report["failures"]
    report["passed"] = not report["failures"]
    return report | dict(avatars=avatars["avatars"], digest=artifact["training"]["digest"],
                         holdout=digest(inputs_of(holdout)))


def verify_unseal(folder, path):
    """For completion's --unseal: the freeze is valid (read_freeze) and holds this run's training inputs."""
    artifact = read_freeze(path)
    run = load([folder], "training")[0]
    frozen = sorted((row["case"], row["sha256"]) for row in artifact["training"]["inputs"]
                    if row["run"] == run.version)
    replay.require(frozen and frozen == sorted((row["case"], row["sha256"]) for row in run.inputs),
                   f"the freeze does not hold this run's ({run.version}) training recordings")
    return artifact


def readers(unityrs):
    """(avatar_of, clip_of)(run, doc) through `unityrs show` on the run's bundles, each bundle checked against
    the recording's pinned sha256 and read once per run folder and digest."""
    from anim_replay_complete import BundleView, bundle_digest, read_manifest

    views = {}

    def view_of(run, doc):
        name = doc["spec"]["bundles"][0]["name"]
        path = (run.folder / "bundles" / name).resolve()
        found = bundle_digest(path)
        replay.require(doc["capture"]["bundle_sha256"] == [found],
                       f"bundle {name} of {run.folder} differs from the recording's pinned sha256")
        key = (str(path), found)
        if key not in views:
            views[key] = BundleView(path, found, 0, unityrs, None, replay, read_manifest(run.folder, replay)[1])
        return views[key]

    def avatar_of(run, doc):
        view = view_of(run, doc)
        return view.value(view.asset(doc["spec"]["avatar_asset"], 90))

    def clip_of(run, doc):
        view = view_of(run, doc)
        return view.value(view.asset(doc["spec"]["clip_asset"], 74))
    return avatar_of, clip_of


def run_mode(mode, folders, freeze_path=None, avatar_of=None, clip_of=None):
    replay.require(avatar_of is not None and (mode != "e4" or clip_of is not None),
                   f"{mode} reads the Avatars (decision 1) and built clips from the bundles: give --unityrs")
    if mode == "e3a":
        return e3a(folders, avatar_of)[0]
    if mode == "freeze":
        return freeze(folders, freeze_path, avatar_of)
    if mode == "e3b":
        return e3b(folders, freeze_path, avatar_of)
    runs, rows, design = training(folders, avatar_of)
    if mode == "e0":
        coefficients = read_freeze(freeze_path)["coefficients"] if freeze_path else solve(design)
        report = check.e0(runs, avatar_of, design, coefficients)
    elif mode == "e1":
        documents = [run.documents for run in runs]
        parents, _ = adequacy.bone_table([doc for docs in documents for doc in docs.values()])
        coverage = [adequacy.coverage_of(docs) for docs in documents]
        merged = dict(recorded=sorted({rig for row in coverage for rig in row["recorded"]}),
                      missing=sorted({f"{run.version}/{rig}" for run, row in zip(runs, coverage)
                                      for rig in row["missing"]}))
        planned = [f"{run.version}/{line}" for run, docs in zip(runs, documents)
                   for line in adequacy.missing_planned(docs)]
        report = check.e1(rows, design, adequacy.expected_inventory(parents), merged, planned)
    elif mode == "e2":
        limit, worst, failures = lateral(runs, rows, design, solve(design))
        report = check.e2(rows, limit) | dict(lateral_limit=limit, lateral_failing=worst)
        report["failures"] += failures
    else:
        clips = {f"{run.version}/{case}": clip_of(run, doc) for run in runs for case, doc in run.documents.items()
                 if BODY_INFO.get(case, {}).get("kind") == "clip"}
        report = e4_check.e4(rows, clips)
    report["failures"] += missing(runs)
    report["passed"] = not report["failures"]
    return report


def clean(value):
    """A report as JSON can hold it: an infinite or NaN ratio as its name."""
    if isinstance(value, dict):
        return {str(key): clean(item) for key, item in value.items()}
    if isinstance(value, (list, tuple)):
        return [clean(item) for item in value]
    if isinstance(value, float) and value != value or value in (float("inf"), float("-inf")):
        return repr(value)
    return value


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("mode", choices=MODES)
    parser.add_argument("runs", type=Path, nargs="+", help="completed <version>/<target>/ folders, one per version")
    parser.add_argument("--freeze", type=Path, help="the freeze artifact: written by freeze, read by e3b (and e0)")
    parser.add_argument("--unityrs", help="unityrs CLI for the Avatar and built clip reads")
    parser.add_argument("--json", type=Path, help="write the report here")
    args = parser.parse_args(argv)
    try:
        replay.require(not (args.json and args.freeze and args.json.resolve() == args.freeze.resolve()),
                       "--json and --freeze name one file")
        avatar_of, clip_of = readers(args.unityrs) if args.unityrs else (None, None)
        report = run_mode(args.mode, args.runs, args.freeze, avatar_of, clip_of)
    except (KeyError, TypeError, IndexError, ValueError, OSError, StopIteration, ZeroDivisionError) as exc:
        print(f"anim_body_freeze {args.mode}: refused: {exc}", file=sys.stderr)
        return 2
    text = json.dumps(clean(report), indent=2, ensure_ascii=False, allow_nan=False) + "\n"
    if args.json:
        args.json.write_text(text, encoding="utf-8")
    shown = report["failures"][:10]
    print(f"{args.mode}: " + ("pass" if report["passed"] else f"fail, {len(report['failures'])} failure(s)")
          + "".join(f"\n  {line}" for line in shown))
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())
