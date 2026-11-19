"""B2's body checker (docs/plans/humanoid-body.md 3.2 and 6): E0 to E4, the freeze, and E3b after it.

    python3 -I anim_body_freeze.py MODE RUN [RUN ...] [--freeze FILE] [--unityrs CLI] [--json REPORT]

Each RUN is a completed `<version>/<target>/` fixture folder (one per Editor
version); MODE is one of:

- `e0`: the Avatars (anim_body_check), with the coefficients of `--freeze`
  if given, or else fitted on the training rows;
- `e1`, `e2`, `e4`: on the training rows;
- `e3a`: the training checkpoint, the hips of every training sample within
  T_s with coefficients fitted on the training rows;
- `freeze`: E3a and E4, and when both pass, the freeze artifact at
  `--freeze` (a new file): the model choices (anim_body_model.HYPOTHESES),
  coefficients, scope (Editor versions, training rigs and their topologies,
  the lateral limit with its bracketing observations, the named refusals,
  cases, free combinations), bounds (unit roundoffs, operation depths,
  limits, the comparison) and E4 per configuration (variant, Editor
  version, root placement: the passing ones, the only ones B4 and B5
  implement, and those excluded without a read-back), with the training
  inputs (Editor version, case, rig and sha256 per file) and their digest.
  The artifact also records the unityrs CLI's sha256, the hold-out cases
  and the in-scope minimum. `--evidence` names other completed training runs (one per version, an
  earlier run of the same cases, say) as supporting evidence, never an
  input: E2 and E3 within the lateral limit with the training design and
  coefficients, each recorded with its run's digest, and the freeze refused
  unless they pass;
- `e3b`: the hold-out rig against the canonical freeze artifact
  (anim_body_seal.FREEZE_PATH, given as `--freeze`), once, with the frozen
  unityrs CLI (anim_body_seal).

E2 and E3a compare only poses inside the lateral limit T (plan section 8):
the largest spine Left-Right value of a training sample the model compared
and passed (never one the twist scope refused) that is at least
LATERAL_STEP below every sample failing either, an observed value, never
clamped (anim_body_check.lateral_limit). A required band sweep unrecorded
leaves no limit, as does one under LATERAL_FLOOR, and both fail. The
freeze records it with the passing sample at T and the smallest failing
one, E3b refuses past it, and a freeze whose training runs give another
limit is refused.

Every mode reads the run's bundles through `unityrs show` (`--unityrs`):
each recording's Avatar, for its masses (m_HumanBoneMass, decision 1), its
scope and its F(T) (m_SkeletonPose), and each clip case's built clip: for
E4, and for each clip sample's spine Left-Right muscles, which the lateral
limit and scope read as they read a known vector's (`with_clip_muscles`).

Order: E3a, then the freeze, then E3b. The hold-out stays out of everything
before E3b by construction: every mode but `e3b` loads only the training role
(anim_body_runs: by run.json and the case table, never by file name), which
never opens a hold-out file, and the fit refuses a hold-out row. No load
reads a hold-out file: completion (anim_replay.py complete) seals the
hold-out's cases, and only E3b unseals them, through completion itself.

**The freeze is immutable and binding.** It needs E3a passed on every
required training case of both Editor versions (2022 and 6000), E4 passed,
a design of full rank and every coefficient within |a| <= 1, and records
them with the git commit of the code and a digest of its sources: the
entry points and every local module they import (anim_body_seal.sources),
all tracked and unchanged from that commit. E3b refuses unless git binds
the artifact (anim_body_seal: the one commit that added it, after the code
commit, an ancestor of HEAD, the file identical; no source changed since
the code commit), its E3a and E4 passed, its model choices and bounds are
the code's, its training inputs are the runs given, and the training
design, fit and lateral limit reproduce it bit for bit. It then evaluates
the hold-out with the stored coefficients alone, each Editor version on its
own: E0 on its hold-out Avatar (E0b's fold only here) and E3, with at least
anim_body_seal.MIN_COMPARISONS in-scope comparisons.

Every check also fails when a run lacks a required case (Run.missing).

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
import anim_body_e4 as e4_check  # noqa: E402
import anim_body_seal as seal  # noqa: E402
import anim_replay as replay  # noqa: E402
from anim_body_model import ANGLE_FLOOR, HYPOTHESES, TWIST_LIMIT  # noqa: E402
from anim_body_runs import Design, digest, load, observe, role_of, solve  # noqa: E402
from anim_body_tolerance import OPERATIONS, U32, U64  # noqa: E402
from anim_fixture_body import BAND_CASES, BODY_INFO, HOLDOUT, rig_bones  # noqa: E402

FORMAT = "unityrs.anim_body_freeze"
VERSION = 5
MODES = ("e0", "e1", "e2", "e3a", "e4", "freeze", "e3b")
EDITORS = ("2022", "6000")


def bounds(design=None):
    """The frozen bounds: the code's constants, and the training design's rank and condition."""
    fixed = dict(condition_limit=adequacy.CONDITION_LIMIT, coefficient_limit=check.COEFFICIENT_LIMIT,
                 unit_roundoff_f32=U32, unit_roundoff_f64=U64, operations=dict(OPERATIONS),
                 fold_depth=check.FOLD_DEPTH, angle_floor=ANGLE_FLOOR, twist_limit=TWIST_LIMIT,
                 lateral_step=check.LATERAL_STEP, lateral_floor=check.LATERAL_FLOOR,
                 comparison="each hips translation component within T_s = E_unity,s + u_s + E_eval,s, "
                            "with u_s <= E_unity,s")
    return fixed if design is None else fixed | dict(condition=design.condition, rank=design.rank)


def with_clip_muscles(runs, rows, clip_of):
    """`rows` with each clip sample's spine Left-Right muscles from its built clip at its clock
    (anim_body_e4.clip_muscles): a clip records no applied muscles, and the lateral scope and limit need them."""
    documents = {f"{run.version}/{case}": (run, doc) for run in runs for case, doc in run.documents.items()}
    clips = {}
    for row in rows:
        state = row["state"]
        if state is None or state["sample"].get("muscles_f32"):
            continue
        if row["name"] not in clips:
            clips[row["name"]] = clip_of(*documents[row["name"]])
        state["clip_muscles"] = e4_check.clip_muscles(clips[row["name"]], state["muscle_names"],
                                                      float(state["sample"]["source"]))
    return rows


def observed(runs, avatar_of, clip_of, table=None):
    return with_clip_muscles(runs, observe(runs, table, avatar_of), clip_of)


def training(folders, avatar_of, clip_of):
    runs = load(folders, "training")
    rows = observed(runs, avatar_of, clip_of)
    replay.require(rows, "no training recordings")
    return runs, rows, Design(rows)


def inputs_of(runs):
    return [row for run in runs for row in run.inputs]


def missing(runs):
    return [f"{run.version}: required case {case} not recorded" for run in runs for case in run.missing]


def lateral(runs, rows, design, coefficients):
    """(found, failures): the training runs' lateral limit with its bracketing observations
    (anim_body_check.lateral_limit). With a required band sweep unrecorded, or below LATERAL_FLOOR, there is
    none (limit None): it fails, and the checks then compare every pose, so the failures that would set it
    show."""
    absent = [f"{run.version}: {case}" for run in runs for case in run.missing if case in BAND_CASES]
    if absent:
        return dict(limit=None, failing=None, passing=None), [
            "no spine lateral limit: the band sweeps that validate it are not recorded (" + ", ".join(absent) + ")"]
    found = check.lateral_limit(rows, design, coefficients)
    if found["limit"] >= check.LATERAL_FLOOR:
        return found, []
    return found | dict(limit=None), [f"no validated spine lateral domain: limit {found['limit']:.6g} below "
                                      f"{check.LATERAL_FLOOR} (smallest failing {found['failing']})"]


def e3a(folders, avatar_of, clip_of):
    runs, rows, design = training(folders, avatar_of, clip_of)
    coefficients = solve(design)
    found, failures = lateral(runs, rows, design, coefficients)
    report = check.e3(rows, design, coefficients, found["limit"])
    report["failures"] += failures + missing(runs)
    report["passed"] = not report["failures"]
    return report | dict(coefficients=coefficients, digest=digest(inputs_of(runs)), lateral=found,
                         lateral_limit=found["limit"]), runs, rows, design


def e4(runs, rows, clip_of):
    clips = {f"{run.version}/{case}": clip_of(run, doc) for run in runs for case, doc in run.documents.items()
             if BODY_INFO.get(case, {}).get("kind") == "clip"}
    return e4_check.e4(rows, clips)


def supporting(folders, design, coefficients, limit, avatar_of, clip_of):
    """Other training runs' E2 and E3 within the frozen limit, with the training design and coefficients: one
    entry per run, its digest and the counts. Their rows never reach the fit."""
    entries = []
    for run in load(folders, "training"):
        rows = observed([run], avatar_of, clip_of)
        e2, e3 = check.e2(rows, limit), check.e3(rows, design, coefficients, limit)
        entries.append(dict(run=run.version, digest=digest(run.inputs), cases=len(run.documents),
                            e2=dict(passed=e2["passed"], residuals=e2["residuals"], refused=e2["refused"],
                                    worst_ratio=e2["worst_ratio"]),
                            e3=dict(passed=e3["passed"], samples=e3["samples"], refused=e3["refused"],
                                    worst_ratio=e3["worst_ratio"]),
                            failures=(e2["failures"] + e3["failures"])[:10]))
    return entries


# Each named refusal of the frozen scope (plan section 8) and what bounds it.
REFUSALS = {"topology": "an Avatar outside the training rigs' recorded topologies (scope.topologies)",
            "frame": "a shoulder-hip twist over bounds.twist_limit, or an undefined one",
            "spine_lateral_range": "a spine Left-Right muscle over scope.lateral_limit"}


def freeze(folders, path, avatar_of, clip_of, evidence=(), unityrs=None):
    replay.require(path is not None and not path.exists(), "freeze writes a new artifact: give a new --freeze path")
    report, runs, rows, design = e3a(folders, avatar_of, clip_of)
    replay.require(report["passed"], "E3a must pass before the freeze")
    root = e4(runs, rows, clip_of)
    replay.require(root["passed"] and root["passing"], "E4 must pass, with a passing configuration, before the freeze")
    replay.require(not {str(f.resolve()) for f in folders} & {str(f.resolve()) for f in evidence},
                   "a supporting run is one of the freeze's own")
    support = supporting(evidence, design, report["coefficients"], report["lateral_limit"], avatar_of, clip_of)
    replay.require(all(entry["e2"]["passed"] and entry["e3"]["passed"] for entry in support),
                   "the supporting runs fail within the lateral limit")
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
    commit, files, source_digest = seal.code_state()
    rigs = sorted({row["rig"] for row in inputs} - {"p2b"})
    artifact = dict(
        format=FORMAT, version=VERSION, model=HYPOTHESES, coefficients=coefficients,
        scope=dict(editor_versions=versions, training_rigs=rigs, lateral_limit=report["lateral_limit"],
                   lateral=report["lateral"], refusals=REFUSALS,
                   topologies={rig: sorted(rig_bones(rig)) for rig in rigs if rig in adequacy.TRAINING_RIGS},
                   cases=sorted({row["case"] for row in inputs}), free=design.free, holdout=HOLDOUT,
                   holdout_cases=list(seal.HOLDOUT_CASES), holdout_minimum=seal.MIN_COMPARISONS),
        bounds=bounds(design),
        training=dict(digest=digest(inputs), inputs=sorted(inputs, key=lambda row: (row["run"], row["case"])),
                      holdout_rows=0),
        e3a=dict(passed=True, samples=report["samples"], worst_ratio=report["worst_ratio"],
                 refused=report["refused"]),
        e4=dict(passed=True, note="per configuration (variant/rig/Editor version/root placement): B4 and B5 "
                                  "implement the passing ones alone; excluded ones compared nothing",
                samples=root["samples"], start_pairs=root["start_pairs"], passing=root["passing"],
                excluded=root["excluded"], configurations=root["configurations"]),
        code=dict(commit=commit, digest=source_digest, files=files),
        reader=dict(note="the unityrs CLI that read every Avatar and built clip; E3b needs the same",
                    sha256=seal.reader_sha256(unityrs)),
        evidence=dict(note="supporting runs, never inputs: E2 and E3 within the lateral limit with the frozen "
                           "design and coefficients", runs=support))
    path.write_text(json.dumps(artifact, indent=2, ensure_ascii=False, allow_nan=False) + "\n", encoding="utf-8")
    return report


def read_freeze(path):
    """The freeze artifact, refused unless git binds it (anim_body_seal: its one freeze commit, its code commit,
    the sources), it came from training inputs alone after a passed E3a and E4, and the code's model and bounds
    are the frozen ones."""
    replay.require(path is not None and path.is_file(), "no freeze artifact: E3b runs only after the freeze")
    seal.bind_artifact(path, None)
    artifact = replay.read_json(path.read_text(encoding="utf-8"))
    replay.require(artifact.get("format") == FORMAT and artifact.get("version") == VERSION,
                   f"expected {FORMAT} version {VERSION}")
    seal.bind_artifact(path, artifact["code"]["commit"])
    seal.bind_code(artifact["code"])
    data = artifact["training"]
    inputs = data["inputs"]
    replay.require(data["holdout_rows"] == 0 and inputs
                   and all(row["rig"] != HOLDOUT and role_of(row["case"]) == "training" for row in inputs),
                   "the freeze artifact was produced from hold-out inputs")
    replay.require(data["digest"] == digest(inputs), "the freeze artifact's digest disagrees with its inputs")
    replay.require(artifact["e3a"].get("passed") is True, "the freeze artifact records no passed E3a")
    replay.require(artifact["e4"].get("passed") is True and artifact["e4"]["passing"],
                   "the freeze artifact records no passed E4")
    scope = artifact["scope"]
    replay.require(sorted(version.split(".")[0] for version in scope["editor_versions"]) == list(EDITORS),
                   f"the frozen Editor versions are not the code's {EDITORS}")
    replay.require((scope["holdout_cases"], scope["holdout_minimum"]) == (list(seal.HOLDOUT_CASES),
                                                                          seal.MIN_COMPARISONS),
                   "the frozen hold-out cases or minimum are not the code's")
    replay.require(isinstance(artifact["reader"]["sha256"], str), "the freeze artifact records no reader")
    replay.require(artifact["model"] == HYPOTHESES, "the model choices differ from the frozen ones")
    frozen = {key: value for key, value in artifact["bounds"].items() if key not in ("condition", "rank")}
    replay.require(frozen == json.loads(json.dumps(bounds())), "the bounds differ from the frozen ones")
    coefficients = artifact["coefficients"]
    replay.require(isinstance(coefficients, dict) and all(type(a) is float for a in coefficients.values()),
                   "malformed frozen coefficients")
    limit = artifact["scope"].get("lateral_limit")
    replay.require(type(limit) is float and limit >= check.LATERAL_FLOOR
                   and artifact["scope"]["lateral"]["limit"] == limit, "malformed frozen lateral limit")
    return artifact


def frozen_training(folders, artifact, avatar_of, clip_of):
    """The unseal's check: the training runs, rows and design, refused unless they are the frozen ones and
    reproduce its design, fit and lateral limit (with its bracketing observations) exactly."""
    runs = load(folders, "training")
    replay.require(digest(inputs_of(runs)) == artifact["training"]["digest"],
                   "the training inputs differ from the frozen ones")
    replay.require(sorted(run.version for run in runs) == artifact["scope"]["editor_versions"],
                   "the Editor versions differ from the frozen ones")
    rows = observed(runs, avatar_of, clip_of)
    design = Design(rows)
    coefficients = artifact["coefficients"]
    replay.require(sorted(coefficients) == design.keys, "the frozen coefficients do not match the training design")
    replay.require((design.rank, design.condition) == (artifact["bounds"]["rank"], artifact["bounds"]["condition"]),
                   "the training design's rank or condition differs from the frozen one")
    replay.require(solve(design) == coefficients, "the frozen coefficients are not the training fit")
    replay.require(lateral(runs, rows, design, coefficients)[0] == artifact["scope"]["lateral"],
                   "the training runs' lateral limit differs from the frozen one")
    return runs, rows, design


def completion():
    """Sealed completion, the one way E3b reads the hold-out."""
    from anim_replay_complete import complete_folder

    return complete_folder


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


def run_mode(mode, folders, freeze_path=None, avatar_of=None, clip_of=None, evidence=(), unityrs=None):
    replay.require(avatar_of is not None and clip_of is not None,
                   f"{mode} reads the Avatars (decision 1) and built clips from the bundles: give --unityrs")
    if mode == "e3a":
        return e3a(folders, avatar_of, clip_of)[0]
    if mode == "freeze":
        return freeze(folders, freeze_path, avatar_of, clip_of, evidence, unityrs)
    if mode == "e3b":
        return seal.e3b(folders, freeze_path, unityrs, readers, completion())
    runs, rows, design = training(folders, avatar_of, clip_of)
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
        found, failures = lateral(runs, rows, design, solve(design))
        report = check.e2(rows, found["limit"]) | dict(lateral_limit=found["limit"], lateral=found)
        report["failures"] += failures
    else:
        report = e4(runs, rows, clip_of)
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
    parser.add_argument("--evidence", type=Path, nargs="+", default=[],
                        help="freeze: other completed training runs, one per version, checked as supporting evidence")
    args = parser.parse_args(argv)
    try:
        replay.require(not (args.json and args.freeze and args.json.resolve() == args.freeze.resolve()),
                       "--json and --freeze name one file")
        avatar_of, clip_of = readers(args.unityrs) if args.unityrs else (None, None)
        replay.require(not args.evidence or args.mode == "freeze", "--evidence is for freeze")
        report = run_mode(args.mode, args.runs, args.freeze, avatar_of, clip_of, args.evidence, args.unityrs)
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
