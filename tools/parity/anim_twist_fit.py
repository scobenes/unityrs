"""Round 12's exploration fit (round 12 design v4 section 2, "The fit"), per Editor, never pooled.

**Feasible** means within bound on every exploration row. A row is one exploration case's scheduled vector at
its capture sample index (anim_twist_tables.sample_index), the muscles the capture records exactly the table's.
On it a joint candidate (anim_twist_model: 126 limb x 5 foot) predicts every chain bone of both sides
(`row_chains`, on the case's own built Avatar read from its bundle, at the Avatar's twist settings, which must be
the tuple's). Each prediction must lie within its derived bound (anim_twist_bound) of the recorded local
rotation: the separation less its own f64 error at most `angle_bound` of the prediction's radius, so the
separation's error never makes a candidate infeasible. A candidate that refuses a row (a pole, or S1 and S2 apart
under a merged key) predicts nothing there, and is not feasible.

**Ambiguity.** Two or more feasible candidates that are not key-equal (`canonical`) on a proposed admission cell
(anim_twist_witness.ADMISSION) leave that cell unresolved. The fit records those cells, so selection can open once.
Residual size never chooses: the slacks are reported, never compared. No two feasible candidates have a witness
on an exploration row, since a witness puts their predictions further apart than both bounds and no recording
meets both.

It opens only round 12's exploration view (anim_fixture_round12.round12_view's default, completed by
`complete`), and, for the cells' keys, round 11's allowlisted Avatars (anim_twist_witness.scoped_rigs). It calls
nothing of the seal and writes nothing to its ledger.

    python3 -I tools/parity/anim_twist_fit.py rows --unityrs PATH --shard K N --out FILE <round 12 run folder>
        One Editor's exploration rows, every Nth from the Kth, against all 630 joint candidates.
    python3 -I tools/parity/anim_twist_fit.py combine --unityrs PATH --round11 FOLDER --out FILE SHARD...
        One Editor's shards, all N of one run, with that Editor's round 11 Avatars for the cells' keys: the feasible
        candidates, each cell's equations among them, the unresolved cells, and the slacks.
"""

import argparse
import json
import math
import platform
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import anim_replay as replay  # noqa: E402
import anim_twist_bound as bound  # noqa: E402
import anim_twist_model as model  # noqa: E402
import anim_twist_tables as tables  # noqa: E402
import anim_twist_witness as witness  # noqa: E402

FORMAT = "unityrs.twist_fit"
# Each twist setting: the Avatar's m_Human field (anim_twist_model.FIELDS order) and the capture's own record.
SETTINGS = (("m_ArmTwist", "upper_arm_twist"), ("m_ForeArmTwist", "lower_arm_twist"),
            ("m_UpperLegTwist", "upper_leg_twist"), ("m_LegTwist", "lower_leg_twist"))
MUSCLES = 95
ZERO = (0.,) * MUSCLES


def exploration_cases():
    return sorted(case for case, (_, _, _, part) in tables.cases().items() if part == "exploration")


def settings_of(case, row, capture, avatar):
    """The case's twist settings in FIELDS order: the Avatar's (as f32), the capture's record and the tuple's,
    all three the same."""
    from anim_body_runs import human_of

    human = human_of(avatar)
    wanted = tables.TUPLES[row["tuple"]]
    for (field, recorded), value in zip(SETTINGS, wanted):
        stored = human.get(field)
        replay.require(type(stored) in (int, float) and bound._f32(float(stored)) == value,
                       f"{case}: the Avatar's {field} is {stored!r}, not the tuple {row['tuple']}'s {value}")
        replay.require(replay.f32_from_bits(str(capture["humanoid"].get(recorded, ""))) == value,
                       f"{case}: the capture records {recorded} other than the tuple {row['tuple']}'s {value}")
    return wanted


def case_rows(case, row, capture, rig):
    """[(capture sample index, the 95 muscle values, {chain bone: recorded local rotation})]: each of the case's
    table vectors at its scheduled sample, whose recorded muscles must be exactly the vector's."""
    names = [entry["name"] for entry in capture["humanoid"]["muscles"]]
    replay.require(len(names) == MUSCLES and all(names[m] == name for name, m in tables.NAMES.items()),
                   f"{case}: the muscle names differ from anim_twist_tables.NAMES")
    vectors = tables.SETS[row["set"]]()
    samples = capture["samples"]
    replay.require(len(samples) == len(vectors) + 3, f"{case} records {len(samples)} samples, not the "
                                                     f"{len(vectors)} vectors and three anchors")
    bones = capture["humanoid"]["bones"]
    nodes = {bone: bones[bone]["node"] for bone in rig["rig"]}
    found = []
    for k, vector in enumerate(vectors):
        index = tables.sample_index(k, len(vectors))
        sample = samples[index]
        expected = [0.] * MUSCLES
        for m, value in vector.items():
            expected[m] = value
        values = [replay.f32_from_bits(v) for v in sample["muscles_f32"]]
        replay.require(values == expected, f"{case}: sample {index} records muscles other than vector {k}'s")
        transforms = sample["pose"]["transforms"]
        recorded = {bone: [replay.f32_from_bits(v) for v in transforms[node]["rotation"]]
                    for bone, node in nodes.items()}
        found.append((index, tuple(expected), recorded))
    return found


def load(folder, unityrs):
    """(the run's identity, {case: (rig, settings, rows)}) for every exploration case of one run: completed
    (anim_fixture_round12.complete) through round 12's exploration view and nothing else, each rig from its own
    case's bundle (anim_twist_witness.rig_of)."""
    import anim_fixture_round12 as round12

    view = round12.round12_view(folder)
    replay.require(view.allowed == ("exploration",), "the fit opens the exploration partition alone")
    readers = round12.readers(unityrs)
    captures = round12.complete(view, readers)
    replay.require(sorted(captures) == exploration_cases(),
                   f"the run completes {sorted(captures)}, not exploration's cases")
    found = {}
    for case, doc in sorted(captures.items()):
        row = round12.rows()[case]
        replay.require(row["partition"] == "exploration", f"{case} is not an exploration case")
        avatar = readers[0](view, case, doc)
        rig = witness.rig_of(case, doc["spec"], doc["capture"], avatar, row["rig"])
        found[case] = (rig, settings_of(case, row, doc["capture"], avatar), case_rows(case, row, doc["capture"], rig))
    run = json.loads(view.run_file("run.json"))["run"]
    return dict(run_id=run["run_id"], editor=run["editor_version"]), found


def slack(predictions, recorded):
    """(the smallest slack in radians, its bone) over the predicted bones: each bound less the separation net of
    its own f64 error, so negative is out of bound."""
    worst = None
    for bone, (q, error) in sorted(predictions.items()):
        replay.require(bone in recorded, f"bone {bone} is predicted but not recorded")
        turn, own = bound.separation(q, recorded[bone])
        found = bound.angle_bound(error) - (turn - own)
        if worst is None or found < worst[0]:
            worst = (found, bone)
    return worst


def fit_rows(cases, shard=(0, 1), log=print):
    """Every joint candidate on this shard's rows (every Nth of the cases' rows in order, from the Kth): per
    candidate its row-sides out of bound and refused, and its worst slack's row."""
    k, n = shard
    replay.require(0 <= k < n, f"no shard {k} of {n}")
    joint = model.candidates()
    out, refused, worst = [0] * len(joint), [0] * len(joint), [None] * len(joint)
    plan = [(case, row) for case in sorted(cases) for row in cases[case][2]]
    mine = plan[k::n]
    started = time.time()
    for done, (case, (sample, values, recorded)) in enumerate(mine):
        rig, settings = cases[case][0], cases[case][1]
        for j, (limb, foot) in enumerate(joint):
            for side in model.SIDES:
                try:
                    predictions = model.row_chains(rig["rig"], list(values), list(ZERO), settings, limb, foot, side)
                except bound.Pole:
                    refused[j] += 1
                    continue
                found, bone = slack(predictions, recorded)
                out[j] += found < 0
                if worst[j] is None or found < worst[j][0]:
                    worst[j] = (found, case, sample, side, bone)
        if done % 20 == 0:
            log(f"shard {k}/{n}: row {done}/{len(mine)} ({time.time() - started:.0f} s)")
    return dict(shard=[k, n], rows=len(mine), total_rows=len(plan),
                candidates=[dict(out=o, refused=r, worst=list(w) if w else None)
                            for o, r, w in zip(out, refused, worst)])


def rig_summary(rig):
    return dict(toes=rig["toes"], signs=rig["signs"], reach={side: {str(b): r for b, r in found.items()}
                                                             for side, found in rig["reach"].items()})


def merge(shards):
    """One run's shards, each of K of the same N once, the same run and rows: per candidate, summed counts and the
    worst slack over all."""
    first = shards[0]
    n = first["shard"][1]
    replay.require(sorted(s["shard"][0] for s in shards) == list(range(n)) and all(s["shard"][1] == n for s in shards),
                   f"the shards are not each of 0..{n - 1} of {n} once")
    for key in ("format", "run", "tables", "candidates_count", "total_rows", "rigs"):
        replay.require(all(s[key] == first[key] for s in shards), f"the shards differ in {key}")
    replay.require(sum(s["rows"] for s in shards) == first["total_rows"], "the shards do not cover every row")
    merged = []
    for j in range(first["candidates_count"]):
        rows = [s["candidates"][j] for s in shards]
        worsts = [r["worst"] for r in rows if r["worst"] is not None]
        merged.append(dict(out=sum(r["out"] for r in rows), refused=sum(r["refused"] for r in rows),
                           worst=min(worsts, key=lambda w: w[0]) if worsts else None))
    return merged


def describe(j):
    limb, foot = model.candidates()[j]
    return dict(index=j, limb=limb._asdict(), foot=foot._asdict())


def cells(feasible, rigs):
    """Per proposed admission cell: the feasible candidates' equations there (`canonical`), each its members."""
    joint = model.candidates()
    found = []
    for name, rig_names in witness.ADMISSION:
        settings = tables.TUPLES[name]
        for rig_name in rig_names:
            rig = rigs[rig_name]
            groups = {}
            for j in feasible:
                limb, foot = joint[j]
                key = model.canonical(limb, foot, settings, rig["toes"], rig["signs"], rig["reach"])
                groups.setdefault(key, []).append(j)
            found.append(dict(tuple=name, rig=rig_name, equations=len(groups),
                              members=sorted(groups.values()), unresolved=len(groups) > 1))
    return found


def combine(shards, rigs, round11_run):
    """The fit of one Editor: its shards merged, every cell's equations among the feasible candidates (round 11's
    Avatars of the same Editor, for the cells' keys), the unresolved cells, and the slacks in degrees."""
    first = shards[0]
    replay.require(round11_run["editor_version"] == first["run"]["editor"],
                   f"round 11's run is Editor {round11_run['editor_version']}, not {first['run']['editor']}")
    merged = merge(shards)
    feasible = [j for j, c in enumerate(merged) if c["out"] == 0 and c["refused"] == 0]
    found = cells(feasible, rigs)
    unresolved = [f"{c['tuple']}/{c['rig']}" for c in found if c["unresolved"]]
    # Round 12's own exploration Avatars against round 11's of the same rig: the cells' keys read round 11's.
    same = {rig: rig_summary(rigs[rig]) == summary for rig, summary in first["rigs"].items() if rig in rigs}

    def degrees(w):
        return None if w is None else dict(degrees=math.degrees(w[0]), case=w[1], sample=w[2], side=w[3], bone=w[4])
    near = sorted((j for j, c in enumerate(merged) if j not in feasible and c["worst"] is not None),
                  key=lambda j: -merged[j]["worst"][0])[:10]
    return dict(format=FORMAT, editor=first["run"]["editor"], run_id=first["run"]["run_id"],
                round11_run_id=round11_run["run_id"], tables=first["tables"], python=first["python"],
                rows=first["total_rows"], candidates=len(merged), feasible=len(feasible),
                feasible_candidates=[dict(describe(j), out=merged[j]["out"], slack=degrees(merged[j]["worst"]))
                                     for j in feasible],
                refusing=sum(1 for c in merged if c["refused"]),
                cells=found, unresolved_cells=unresolved, resolved=bool(feasible) and not unresolved,
                round12_rigs_match_round11=same,
                nearest_infeasible=[dict(describe(j), out=merged[j]["out"], refused=merged[j]["refused"],
                                         slack=degrees(merged[j]["worst"])) for j in near])


def rows_main(args):
    identity, cases = load(args.folder, str(Path(args.unityrs).resolve()))
    result = fit_rows(cases, tuple(args.shard), log=lambda line: print(line, file=sys.stderr, flush=True))
    # Each probe rig's Avatar summary, the same in every case of the rig.
    rigs = {}
    for case, (rig, _, _) in sorted(cases.items()):
        name = tables.cases()[case][0]
        replay.require(rigs.setdefault(name, rig_summary(rig)) == rig_summary(rig),
                       f"{case}'s Avatar differs from its rig's other cases")
    return dict(format=FORMAT, run=identity, tables=tables.digest(), python=platform.python_version(),
                candidates_count=len(model.candidates()), rigs=rigs, **result)


def combine_main(args):
    shards = [json.loads(path.read_text(encoding="utf-8")) for path in args.shards]
    replay.require(shards and all(s.get("format") == FORMAT for s in shards), "not twist fit shards")
    identity, rigs = witness.scoped_rigs(args.round11, str(Path(args.unityrs).resolve()),
                                         log=lambda line: print(line, file=sys.stderr, flush=True))
    return combine(shards, rigs, identity)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    rows = sub.add_parser("rows")
    rows.add_argument("--unityrs", required=True)
    rows.add_argument("--shard", nargs=2, type=int, default=(0, 1))
    rows.add_argument("--out", type=Path, required=True)
    rows.add_argument("folder", type=Path)
    joined = sub.add_parser("combine")
    joined.add_argument("--unityrs", required=True)
    joined.add_argument("--round11", type=Path, required=True)
    joined.add_argument("--out", type=Path, required=True)
    joined.add_argument("shards", type=Path, nargs="+")
    args = parser.parse_args()
    try:
        result = rows_main(args) if args.command == "rows" else combine_main(args)
        args.out.write_text(json.dumps(result, separators=(",", ":"), ensure_ascii=False) + "\n", encoding="utf-8")
    except (ValueError, OSError) as exc:
        parser.exit(1, f"anim_twist_fit: {exc}\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
