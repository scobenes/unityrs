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

**Keys.** A cell's keys come from round 12's own built Avatar where exploration holds it (the compact and no-toes
probe rigs, from their bundles), and from round 11's allowlisted Avatar of the rig elsewhere (`keyed_rigs`):
round 12 builds those only in its sealed partitions. Where both rounds build a rig, a difference in the keys'
inputs (toes, x signs, x reaches) is reported (`key_differences`).

It opens only round 12's exploration view (anim_fixture_round12.round12_view's default, completed by
`complete`) and round 11's allowlisted Avatars (anim_twist_witness.scoped_rigs). It calls nothing of the seal and
writes nothing to its ledger.

**Witnesses** (`witnesses`, v4 section 2 at intake). Every pair of candidates whose keys differ on a proposed cell
needs a named row in each Editor where their predictions lie further apart than both bounds
(anim_twist_witness.search), here among exploration's rows on round 12's own built Avatars (their rests the
Avatar's m_SkeletonPose, never a capture's). Every pair's witness is written out (case, sample, side, bone,
margin, and the cells its keys separate it on); `combine` recomputes each one and the cells a pair without one
leaves open (`check_witnesses`), so selection opens from the pairs, never from a supplied list.

Every output records the interpreter (anim_twist_seal.interpreter: CPython's exact version and the container
image's id); shards from different interpreters never merge, and `combine` runs only under the shards' own.

    python3 -I tools/parity/anim_twist_fit.py rows --unityrs PATH --image ID --shard K N --out FILE <round 12 run>
        One Editor's exploration rows, every Nth from the Kth, against all 630 joint candidates.
    python3 -I tools/parity/anim_twist_fit.py witness --unityrs PATH --image ID --round11 RUN --out FILE <run>
        One Editor's witness rule on its exploration rows and Avatars: every joint pair's witness.
    python3 -I tools/parity/anim_twist_fit.py combine --unityrs PATH --image ID --round11 RUN --witness FILE
            --out FILE <round 12 run> --shards SHARD...
        One Editor's shards, all N of one run, with its witness result: the feasible candidates, each cell's
        equations among them, the unresolved and unwitnessed cells, whether selection opens, every pair's checked
        witness, and the slacks (each worst one's bound and separation apart). A FILE ending .gz is gzip.
"""

import argparse
import json
import math
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
# Where a cell's keys come from (`keyed_rigs`).
ROUND12, ROUND11 = "round 12", "round 11 (round 12's is sealed)"
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
    """(the smallest slack in radians, its bone, that bone's bound, its separation, the separation's own f64 error)
    over the predicted bones: each bound less the separation net of its own error, so negative is out of bound."""
    worst = None
    for bone, (q, error) in sorted(predictions.items()):
        replay.require(bone in recorded, f"bone {bone} is predicted but not recorded")
        turn, own = bound.separation(q, recorded[bone])
        allowed = bound.angle_bound(error)
        found = allowed - (turn - own)
        if worst is None or found < worst[0]:
            worst = (found, bone, allowed, turn, own)
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
                found, bone, allowed, turn, own = slack(predictions, recorded)
                out[j] += found < 0
                if worst[j] is None or found < worst[j][0]:
                    worst[j] = (found, case, sample, side, bone, allowed, turn, own)
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
    for key in ("format", "run", "tables", "candidates_count", "total_rows", "rigs", "interpreter"):
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


def exploration_rigs(cases):
    """{probe rig: its rig} from round 12's exploration cases (`load`), every case of a rig the same Avatar chain:
    the same axes and rests, toes, x signs and reaches."""
    rigs = {}
    for case, (rig, _, _) in sorted(cases.items()):
        first = rigs.setdefault(tables.cases()[case][0], rig)
        replay.require(all(first[key] == rig[key] for key in ("rig", "toes", "signs", "reach")),
                       f"{case}'s Avatar differs from its rig's other cases")
    return rigs


def keyed_rigs(rigs12, rigs11):
    """{rig: (its rig, where its keys come from)} for every proposed cell's rig: round 12's own built Avatar where
    exploration holds it (the compact and no-toes probe rigs), round 11's elsewhere. Round 12 builds the others
    only in its sealed partitions (selection's tall probes, confirmation's twins and clips), which the fit never
    opens."""
    found = {}
    for _, names in witness.ADMISSION:
        for name in names:
            found[name] = (rigs12[name], ROUND12) if name in rigs12 else (rigs11[name], ROUND11)
    return found


def key_differences(rigs12, rigs11):
    """The probe rigs both rounds build whose keys' inputs (toes, x signs, x reaches) differ."""
    return sorted(name for name in rigs12 if name in rigs11 and rig_summary(rigs12[name]) != rig_summary(rigs11[name]))


def cell_classes(keyed):
    """[(tuple, rig, keys' source, [class per joint candidate])] per proposed admission cell: candidates of one
    class are key-equal (`canonical`) there."""
    joint = model.candidates()
    found = []
    for name, rig_names in witness.ADMISSION:
        settings = tables.TUPLES[name]
        for rig_name in rig_names:
            rig, source = keyed[rig_name]
            ids = {}
            found.append((name, rig_name, source,
                          [ids.setdefault(model.canonical(limb, foot, settings, rig["toes"], rig["signs"],
                                                          rig["reach"]), len(ids)) for limb, foot in joint]))
    return found


def cells(feasible, classes):
    """Per proposed admission cell: the feasible candidates' equations there, each its members."""
    found = []
    for name, rig_name, source, ids in classes:
        groups = {}
        for j in feasible:
            groups.setdefault(ids[j], []).append(j)
        found.append(dict(tuple=name, rig=rig_name, keys_from=source, equations=len(groups),
                          members=sorted(groups.values()), unresolved=len(groups) > 1))
    return found


def exploration_plan():
    """anim_twist_witness's rows, in its order, of exploration's cases alone: never a selection row."""
    allowed = set(exploration_cases())
    plan = [row for row in witness.rows() if row[0] in allowed]
    replay.require(plan and all(row[5].startswith("exploration") for row in plan),
                   "the exploration plan holds a row of another partition")
    return plan


def _mask(classes, first, second):
    """The cells (a bit each, in `classes` order) whose keys separate two joint candidates."""
    return sum(1 << c for c, (_, _, _, ids) in enumerate(classes) if ids[first] != ids[second])


def witnesses(rigs12, rigs11, log=print):
    """v4 section 2's witness rule at intake, one Editor: every joint pair's witness (anim_twist_witness.search)
    among exploration's rows on round 12's own built Avatars, written out pair by pair: [first, second, the cells
    whose keys separate them (a bit each), its row (an index into `rows`, -1 for none), the bone, the margin in
    degrees]. A pair its keys separate on a cell without a witness leaves that cell to selection's rows."""
    limb_found, foot_found, joint_found, joint_open, credits = witness.search(rigs12, log, exploration_plan())
    classes = cell_classes(keyed_rigs(rigs12, rigs11))
    feet = len(model.FEET)
    rows, index, pairs = [], {}, []

    def entry(pair, found):
        (l1, l2), (f1, f2) = pair
        first, second = l1 * feet + f1, l2 * feet + f2
        if found is None:
            return [first, second, _mask(classes, first, second), -1, -1, None]
        key = tuple(found[:4])
        if key not in index:
            index[key] = len(rows)
            rows.append(list(key))
        return [first, second, _mask(classes, first, second), index[key], found[5], math.degrees(found[4])]
    for pair, found in joint_found.items():
        pairs.append(entry(pair, found))
    for pair, (kind, component) in credits.items():
        pairs.append(entry(pair, (limb_found if kind == "limb" else foot_found)[component]))
    for pair in joint_open:
        pairs.append(entry(pair, None))
    pairs.sort()
    return dict(cells=[list(c[:3]) for c in classes], key_differences=key_differences(rigs12, rigs11), rows=rows,
                pairs=pairs, limb_pairs_open=len(model.LIMBS) * (len(model.LIMBS) - 1) // 2 - len(limb_found),
                foot_pairs_open=feet * (feet - 1) // 2 - len(foot_found))


def check_witnesses(witnessed, classes, rigs12):
    """The witness result checked, never trusted: every joint pair once, in order; each pair's cells recomputed
    from the keys; each named row an exploration row, both candidates' predictions recomputed there on round 12's
    own Avatar and further apart than both bounds at the named bone by exactly the recorded margin. Returns the
    summary: pairs witnessed and open, the smallest margin's pair and row, and the cells a pair without a witness
    leaves open (from the pairs, never a supplied list)."""
    import itertools

    replay.require(witnessed["cells"] == [list(c[:3]) for c in classes], "the witness result keys other cells")
    n = len(model.candidates())
    pairs, rows = witnessed["pairs"], witnessed["rows"]
    replay.require(len(pairs) == n * (n - 1) // 2 and all(
        pair[:2] == list(expected) for pair, expected in zip(pairs, itertools.combinations(range(n), 2))),
        "the witness result does not name every joint pair once, in order")
    plan = {(case, sample): (rig, name, vector) for case, sample, rig, name, vector, _ in exploration_plan()}
    cache = {}

    def predicted(row, j):
        if (row, j) not in cache:
            case, sample, side, _ = rows[row]
            replay.require((case, sample) in plan, f"{case} sample {sample} is not an exploration row")
            rig, name, vector = plan[(case, sample)]
            limb, foot = model.candidates()[j]
            cache[(row, j)] = witness.predict(rigs12[rig], vector, tables.TUPLES[name], limb, foot, side)
        return cache[(row, j)]
    unwitnessed, smallest = set(), None
    for first, second, mask, row, bone, recorded in pairs:
        replay.require(mask == _mask(classes, first, second), f"pair {first}, {second} names other cells")
        if row < 0:
            unwitnessed |= {c for c in range(len(classes)) if mask >> c & 1}
            continue
        replay.require(0 <= row < len(rows), f"pair {first}, {second} names no row")
        a, b = predicted(row, first), predicted(row, second)
        replay.require(a is not None and b is not None and bone in a and bone in b,
                       f"pair {first}, {second}: a candidate predicts nothing at its witness")
        found = witness.margin(a[bone], b[bone])
        replay.require(found > 0 and math.degrees(found) == recorded,
                       f"pair {first}, {second}: its witness margin is not the recorded one")
        if smallest is None or recorded < smallest[0]:
            smallest = (recorded, first, second, row, bone)
    open_pairs = sum(1 for pair in pairs if pair[3] < 0)
    return dict(pairs=len(pairs), witnessed=len(pairs) - open_pairs, open=open_pairs,
                smallest_margin=None if smallest is None else dict(
                    degrees=smallest[0], pair=[describe(smallest[1]), describe(smallest[2])],
                    case=rows[smallest[3]][0], sample=rows[smallest[3]][1], side=rows[smallest[3]][2],
                    bone=smallest[4]),
                margins_under_degrees={f"{limit:g}": sum(p[3] >= 0 and p[5] < limit for p in pairs)
                                       for limit in (1e-3, 1e-2, .1, 1.)},
                unwitnessed_cells=[f"{classes[c][0]}/{classes[c][1]}" for c in sorted(unwitnessed)])


def _degrees(worst):
    """A worst slack's row with the bound and the measured separation apart, in degrees."""
    if worst is None:
        return None
    found, case, sample, side, bone, allowed, turn, own = worst
    return dict(degrees=math.degrees(found), case=case, sample=sample, side=side, bone=bone,
                bound_degrees=math.degrees(allowed), separation_degrees=math.degrees(turn),
                separation_error_degrees=math.degrees(own))


def combine(shards, rigs12, rigs11, round11_run, witnessed, running):
    """The fit of one Editor: its shards merged; every cell's equations among the feasible candidates, keyed on
    round 12's own Avatars where exploration holds them (`keyed_rigs`); the unresolved cells; the witness result
    (`witnesses`, the same run's) checked pair by pair (`check_witnesses`) and kept whole; the slacks in degrees.
    It runs under the shards' own interpreter, `running`. Selection opens where a cell is unresolved or a pair its
    keys separate has no exploration witness."""
    first = shards[0]
    replay.require(running == first["interpreter"],
                   f"combine runs under {running}, not the shards' {first['interpreter']}")
    replay.require(round11_run["editor_version"] == first["run"]["editor"],
                   f"round 11's run is Editor {round11_run['editor_version']}, not {first['run']['editor']}")
    replay.require(witnessed.get("run") == first["run"] and witnessed.get("round11_run_id") == round11_run["run_id"]
                   and witnessed.get("interpreter") == first["interpreter"],
                   "the witness result is another run's, Editor's or interpreter's")
    replay.require({name: rig_summary(rig) for name, rig in rigs12.items()} == first["rigs"],
                   "round 12's exploration Avatars differ from the shards'")
    merged = merge(shards)
    feasible = [j for j, c in enumerate(merged) if c["out"] == 0 and c["refused"] == 0]
    classes = cell_classes(keyed_rigs(rigs12, rigs11))
    found = cells(feasible, classes)
    unresolved = [f"{c['tuple']}/{c['rig']}" for c in found if c["unresolved"]]
    checked = check_witnesses(witnessed, classes, rigs12)
    unwitnessed = checked["unwitnessed_cells"]
    near = sorted((j for j, c in enumerate(merged) if j not in feasible and c["worst"] is not None),
                  key=lambda j: -merged[j]["worst"][0])[:10]
    return dict(format=FORMAT, editor=first["run"]["editor"], run_id=first["run"]["run_id"],
                round11_run_id=round11_run["run_id"], tables=first["tables"], interpreter=running,
                rows=first["total_rows"], candidates=len(merged), feasible=len(feasible),
                feasible_candidates=[dict(describe(j), out=merged[j]["out"], slack=_degrees(merged[j]["worst"]))
                                     for j in feasible],
                refusing=sum(1 for c in merged if c["refused"]),
                cells=found, unresolved_cells=unresolved, unwitnessed_cells=unwitnessed,
                key_differences=key_differences(rigs12, rigs11), witness=checked,
                resolved=bool(feasible) and not unresolved and not unwitnessed,
                selection_opens=bool(unresolved or unwitnessed),
                nearest_infeasible=[dict(describe(j), out=merged[j]["out"], refused=merged[j]["refused"],
                                         slack=_degrees(merged[j]["worst"])) for j in near],
                witness_rows=witnessed["rows"], witness_pairs=witnessed["pairs"],
                witness_cells=witnessed["cells"])


def _log(line):
    print(line, file=sys.stderr, flush=True)


def rows_main(args, running):
    identity, cases = load(args.folder, str(Path(args.unityrs).resolve()))
    result = fit_rows(cases, tuple(args.shard), log=_log)
    rigs = {name: rig_summary(rig) for name, rig in exploration_rigs(cases).items()}
    return dict(format=FORMAT, run=identity, tables=tables.digest(), interpreter=running,
                candidates_count=len(model.candidates()), rigs=rigs, **result)


def _both(args):
    """(round 12's identity, its exploration rigs, round 11's identity, its scoped rigs), one Editor's."""
    unityrs = str(Path(args.unityrs).resolve())
    identity, cases = load(args.folder, unityrs)
    round11_run, rigs11 = witness.scoped_rigs(args.round11, unityrs, log=_log)
    replay.require(round11_run["editor_version"] == identity["editor"],
                   f"round 11's run is Editor {round11_run['editor_version']}, not {identity['editor']}")
    return identity, exploration_rigs(cases), round11_run, rigs11


def witness_main(args, running):
    identity, rigs12, round11_run, rigs11 = _both(args)
    return dict(format=FORMAT + ".witness", run=identity, round11_run_id=round11_run["run_id"],
                tables=tables.digest(), interpreter=running, **witnesses(rigs12, rigs11, log=_log))


def combine_main(args, running):
    shards = [read(path) for path in args.shards]
    replay.require(shards and all(s.get("format") == FORMAT for s in shards), "not twist fit shards")
    witnessed = read(args.witness)
    replay.require(witnessed.get("format") == FORMAT + ".witness", "not a twist fit witness result")
    identity, rigs12, round11_run, rigs11 = _both(args)
    replay.require(identity == shards[0]["run"], "the round 12 run is not the shards'")
    return combine(shards, rigs12, rigs11, round11_run, witnessed, running)


def read(path):
    import gzip

    data = path.read_bytes()
    return json.loads(gzip.decompress(data) if path.suffix == ".gz" else data)


def write(path, value):
    import gzip

    data = (json.dumps(value, separators=(",", ":"), ensure_ascii=False) + "\n").encode("utf-8")
    path.write_bytes(gzip.compress(data, mtime=0) if path.suffix == ".gz" else data)


def main():
    from anim_twist_seal import interpreter

    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    rows = sub.add_parser("rows")
    rows.add_argument("--shard", nargs=2, type=int, default=(0, 1))
    found = sub.add_parser("witness")
    joined = sub.add_parser("combine")
    joined.add_argument("--witness", type=Path, required=True)
    joined.add_argument("--shards", type=Path, nargs="+", required=True)
    for each in (found, joined):
        each.add_argument("--round11", type=Path, required=True)
    for each in (rows, found, joined):
        each.add_argument("--unityrs", required=True)
        each.add_argument("--image", required=True, help="the container image's id, sha256:...")
        each.add_argument("--out", type=Path, required=True)
        each.add_argument("folder", type=Path)
    args = parser.parse_args()
    try:
        running = interpreter(args.image)
        write(args.out, dict(rows=rows_main, witness=witness_main, combine=combine_main)[args.command](args, running))
    except (ValueError, OSError) as exc:
        parser.exit(1, f"anim_twist_fit: {exc}\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
