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
  scope (Editor versions, training rigs and their topologies, cases, free
  combinations) and bounds (unit roundoffs, operation depths, limits, the
  comparison), with the training inputs (Editor version, case, rig and
  sha256 per file) and their digest;
- `e3b`: the hold-out rig against the freeze artifact at `--freeze`.

The masses are each recording's Avatar m_HumanBoneMass (decision 1), read
from the run's bundles through `unityrs show` (`--unityrs`), which every
mode that fits or evaluates the hips needs (`e0`, `e1`, `e3a`, `freeze`,
`e3b`); `e2` and `e4` take the HumanTrait fold without it, which enters
only their bounds.

Order: E3a, then the freeze, then E3b. The hold-out stays out of everything
before E3b by construction: every mode but `e3b` loads only the training role
(anim_body_runs), which never opens a hold-out file, and the fit refuses a
hold-out row. E3b refuses to run without a freeze artifact, or with one
whose inputs name a hold-out case or rig or disagree with its digest, whose
model choices are not the code's, or whose training inputs differ from the
runs given. It never fits: the coefficients are the artifact's, and the
training rows serve only the uncertainty u_s, through the same design.

Exit 0 when the check passes, 1 when it fails, 2 when it refuses or the
input is unreadable.
"""

import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import anim_body_adequacy as adequacy  # noqa: E402
import anim_body_check as check  # noqa: E402
import anim_replay as replay  # noqa: E402
from anim_body_model import ANGLE_FLOOR, HYPOTHESES  # noqa: E402
from anim_body_runs import Design, digest, load, observe, role_of, solve  # noqa: E402
from anim_body_tolerance import OPERATIONS, U32, U64  # noqa: E402
from anim_fixture_body import HOLDOUT, rig_bones  # noqa: E402

FORMAT = "unityrs.anim_body_freeze"
VERSION = 1
MODES = ("e0", "e1", "e2", "e3a", "e4", "freeze", "e3b")


FITTED = ("e0", "e1", "e3a", "freeze", "e3b")


def training(folders, avatar_of=None):
    runs = load(folders, "training")
    rows = observe(runs, avatar_of=avatar_of)
    replay.require(rows, "no training recordings")
    return runs, rows, Design(rows)


def inputs_of(runs):
    return [row for run in runs for row in run.inputs]


def e3a(folders, avatar_of):
    runs, rows, design = training(folders, avatar_of)
    coefficients = solve(design)
    return check.e3(rows, design, coefficients) | dict(coefficients=coefficients,
                                                       digest=digest(inputs_of(runs))), runs, design


def freeze(folders, path, avatar_of):
    replay.require(path is not None and not path.exists(), "freeze writes a new artifact: give a new --freeze path")
    report, runs, design = e3a(folders, avatar_of)
    replay.require(report["passed"], "E3a must pass before the freeze")
    inputs = inputs_of(runs)
    replay.require(all(row["rig"] != HOLDOUT and role_of(row["case"]) == "training" for row in inputs),
                   "a hold-out input reached the freeze")
    rigs = sorted({row["rig"] for row in inputs} - {"p2b"})
    artifact = dict(
        format=FORMAT, version=VERSION, model=HYPOTHESES, coefficients=report["coefficients"],
        scope=dict(editor_versions=sorted({run.version for run in runs}), training_rigs=rigs,
                   topologies={rig: sorted(rig_bones(rig)) for rig in rigs if rig in adequacy.TRAINING_RIGS},
                   cases=sorted({row["case"] for row in inputs}), free=design.free, holdout=HOLDOUT),
        bounds=dict(condition_limit=adequacy.CONDITION_LIMIT, condition=design.condition, rank=design.rank,
                    coefficient_limit=check.COEFFICIENT_LIMIT, unit_roundoff_f32=U32, unit_roundoff_f64=U64,
                    operations=dict(OPERATIONS), fold_depth=check.FOLD_DEPTH, angle_floor=ANGLE_FLOOR,
                    comparison="each hips translation component within T_s = E_unity,s + u_s + E_eval,s"),
        training=dict(digest=digest(inputs), inputs=sorted(inputs, key=lambda row: (row["run"], row["case"])),
                      holdout_rows=0),
        e3a=dict(samples=report["samples"], worst_ratio=report["worst_ratio"]))
    path.write_text(json.dumps(artifact, indent=2, ensure_ascii=False, allow_nan=False) + "\n", encoding="utf-8")
    return report


def read_freeze(path):
    """The freeze artifact, refused unless it is whole and came from training inputs alone."""
    replay.require(path is not None and path.is_file(), "no freeze artifact: E3b runs only after the freeze")
    artifact = replay.read_json(path.read_text(encoding="utf-8"))
    replay.require(artifact.get("format") == FORMAT and artifact.get("version") == VERSION,
                   f"expected {FORMAT} version {VERSION}")
    data = artifact["training"]
    inputs = data["inputs"]
    replay.require(data["holdout_rows"] == 0 and inputs
                   and all(row["rig"] != HOLDOUT and role_of(row["case"]) == "training" for row in inputs),
                   "the freeze artifact was produced from hold-out inputs")
    replay.require(data["digest"] == digest(inputs), "the freeze artifact's digest disagrees with its inputs")
    replay.require(artifact["model"] == HYPOTHESES, "the model choices differ from the frozen ones")
    coefficients = artifact["coefficients"]
    replay.require(isinstance(coefficients, dict) and all(type(a) is float for a in coefficients.values()),
                   "malformed frozen coefficients")
    return artifact


def e3b(folders, path, avatar_of):
    artifact = read_freeze(path)
    runs = load(folders, "training")
    replay.require(digest(inputs_of(runs)) == artifact["training"]["digest"],
                   "the training inputs differ from the frozen ones")
    rows = observe(runs, avatar_of=avatar_of)
    design = Design(rows)
    coefficients = artifact["coefficients"]
    replay.require(sorted(coefficients) == design.keys, "the frozen coefficients do not match the training design")
    holdout = load(folders, "holdout")
    replay.require(any(run.documents for run in holdout), "no hold-out recording")
    table = adequacy.bone_table([doc for run in runs for doc in run.documents.values()])
    report = check.e3(observe(holdout, table, avatar_of), design, coefficients)
    return report | dict(digest=artifact["training"]["digest"], holdout=digest(inputs_of(holdout)))


def avatar_reader(unityrs):
    """avatar_of(run, doc) through `unityrs show` on the run's bundles."""
    from anim_replay_complete import BundleView, bundle_digest, read_manifest

    views = {}

    def avatar_of(run, doc):
        name = doc["spec"]["bundles"][0]["name"]
        if name not in views:
            path = run.folder / "bundles" / name
            views[name] = BundleView(path, bundle_digest(path), 0, unityrs, None, replay,
                                     read_manifest(run.folder, replay)[1])
        view = views[name]
        return view.value(view.asset(doc["spec"]["avatar_asset"], 90))
    return avatar_of


def run_mode(mode, folders, freeze_path=None, avatar_of=None):
    replay.require(mode not in FITTED or avatar_of is not None,
                   f"{mode} reads the masses from the Avatars (decision 1): give --unityrs")
    if mode == "e3a":
        return e3a(folders, avatar_of)[0]
    if mode == "freeze":
        return freeze(folders, freeze_path, avatar_of)
    if mode == "e3b":
        return e3b(folders, freeze_path, avatar_of)
    runs, rows, design = training(folders, avatar_of)
    if mode == "e0":
        coefficients = read_freeze(freeze_path)["coefficients"] if freeze_path else solve(design)
        return check.e0(runs, avatar_of, design, coefficients)
    if mode == "e1":
        documents = [run.documents for run in runs]
        parents, _ = adequacy.bone_table([doc for docs in documents for doc in docs.values()])
        coverage = [adequacy.coverage_of(docs) for docs in documents]
        merged = dict(recorded=sorted({rig for row in coverage for rig in row["recorded"]}),
                      missing=sorted({f"{run.version}/{rig}" for run, row in zip(runs, coverage)
                                      for rig in row["missing"]}))
        planned = [f"{run.version}/{line}" for run, docs in zip(runs, documents)
                   for line in adequacy.missing_planned(docs)]
        return check.e1(rows, design, adequacy.expected_inventory(parents), merged, planned)
    if mode == "e2":
        return check.e2(rows)
    documents = {f"{run.version}/{case}": doc for run in runs for case, doc in run.documents.items()}
    return check.e4(rows, documents)


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
    parser.add_argument("--unityrs", help="unityrs CLI for e0's Avatar reads")
    parser.add_argument("--json", type=Path, help="write the report here")
    args = parser.parse_args(argv)
    try:
        avatar_of = avatar_reader(args.unityrs) if args.unityrs else None
        report = run_mode(args.mode, args.runs, args.freeze, avatar_of)
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
