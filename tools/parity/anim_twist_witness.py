"""Round 12's witness rule (round 12 design v4 section 2), on actual built Avatars, before collection.

For every two candidates (anim_twist_model: 126 limb x 5 foot) whose keys differ on a proposed admission cell
(`canonical`), a witness is a named exploration or selection row (anim_twist_tables: a case and a vector index)
where some chain bone's two predictions are further apart than the sum of their derived bounds
(anim_twist_bound), so no one recording can satisfy both. A pair with neither equal keys nor a witness refuses the
cell; a search that finds no witness never makes two candidates equivalent.

The search runs in two passes, exactly the rule, never an approximation of it: limb pairs on the bones only the
limb candidate moves (the arm chains, the upper and lower legs) and foot pairs on the Toes, then every joint pair
neither settles on the Foot, which both move.

    python3 -I tools/parity/anim_twist_witness.py --unityrs PATH --out FILE <round 11 run folder>
        One Editor's run folder. It opens only the allowlisted exploration captures (one identity-root
        full-body clip per scoped rig, for its Avatar and rest pose; never a sweep, fingers, face or hold-out),
        and writes the result: per cell, the pairs its keys separate and those without a witness; per row group,
        the rows that first witness some pair; the witnesses' margins over their bounds.
"""

import argparse
import itertools
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

FORMAT = "unityrs.twist_witness"
# The scoped full-body rigs (anim_fixture_full.SCOPED_RIGS) whose first identity-root clip gives each Avatar.
SCOPED_RIGS = ("compact", "large", "small", "tall", "armature", "armature-large", "scaled-animator",
               "scaled-animator-large", "scaled-animator-small", "hips-scale", "hips-scale-large",
               "hips-scale-small", "compact-no-face", "compact-eyes", "compact-jaw", "tall-no-face", "tall-eyes",
               "tall-jaw", "no-upper-chest", "no-shoulders", "no-left-toes", "no-right-toes", "no-toes",
               "no-upper-chest-toes", "no-upper-chest-left-toes", "no-upper-chest-right-toes", "noncoplanar")
# The probe rigs stand on these scoped rigs' Avatars until round 12 builds its own (intake reruns this).
PROBE_RIGS = {"compact": "compact", "tall": "tall", "no-toes": "no-toes"}
CHAIN_BONES = (1, 2, 3, 4, 5, 6, 13, 14, 15, 16, 17, 18, 19, 20)
LIMB_ONLY = {"Left": (13, 15, 17, 1, 3), "Right": (14, 16, 18, 2, 4)}
FOOT, TOES = {"Left": 5, "Right": 6}, {"Left": 19, "Right": 20}
PAIR_BONES = ((13, 15), (15, 17), (1, 3), (3, 5))
ADMISSION = (("t0", ("compact", "tall")), ("t1", ("compact", "tall")), ("m1", ("compact", "tall")),
             ("t5", SCOPED_RIGS))


def allowlisted(folder, rig):
    """The one capture a rig's Avatar comes from: its scoped clip's first identity-root variant."""
    body = folder / "replays" / "body"
    found = sorted(p for p in body.glob(f"body-clip-{rig}-scoped-*.pending.json")
                   if not p.name.endswith("-offset.pending.json"))
    replay.require(found, f"no identity-root scoped clip for {rig} in {folder}")
    return found[0]


def load_rig(folder, rig, unityrs, views):
    """{bone: {"axes", "rest"}}, the toes per side and the x-sign products per side and pair, from one allowlisted
    capture and its bundle's Avatar (the bundle checked against the capture's pinned sha256)."""
    from anim_body_runs import human_of
    from anim_humanoid_qualify import vec
    from anim_muscle_pose import avatar_node, f32
    from anim_replay_complete import BundleView, bundle_digest, read_manifest

    path = allowlisted(folder, rig)
    doc = replay.read_json(path.read_text(encoding="utf-8"))
    capture = doc["capture"]
    names = [row["name"] for row in capture["humanoid"]["muscles"]]
    replay.require(all(names[m] == name for name, m in tables.NAMES.items()),
                   f"{path.name}: the muscle names differ from anim_twist_tables.NAMES")
    bundle = (folder / "bundles" / doc["spec"]["bundles"][0]["name"]).resolve()
    digest = bundle_digest(bundle)
    replay.require(capture["bundle_sha256"] == [digest], f"{bundle.name} differs from the capture's pinned sha256")
    if (bundle, digest) not in views:
        views[(bundle, digest)] = BundleView(bundle, digest, 0, unityrs, None, replay, read_manifest(folder, replay)[1])
    view = views[(bundle, digest)]
    human = human_of(view.value(view.asset(doc["spec"]["avatar_asset"], 90)))
    skeleton = human["m_Skeleton"].get("data", human["m_Skeleton"])
    bones = capture["humanoid"]["bones"]
    found, nodes = {}, {}
    for bone in CHAIN_BONES:
        node = avatar_node(human, bone)
        if bones[bone]["node"] < 0 or node is None:
            continue
        record = skeleton["m_AxesArray"][skeleton["m_Node"][node]["m_AxesId"]]
        axes = {key: [f32(v) for v in vec(record[name])] for key, name in
                (("pre", "m_PreQ"), ("post", "m_PostQ"), ("sign", "m_Sgn"))}
        axes |= {key: [f32(v) for v in vec(record["m_Limit"][name])] for key, name in
                 (("min", "m_Min"), ("max", "m_Max"))}
        rest = [replay.f32_from_bits(v) for v in capture["rest"]["transforms"][bones[bone]["node"]]["rotation"]]
        found[bone], nodes[bone] = dict(axes=axes, rest=rest), node
    for source, receiver in PAIR_BONES + ((14, 16), (16, 18), (2, 4), (4, 6)):
        replay.require(skeleton["m_Node"][nodes[receiver]]["m_ParentId"] == nodes[source],
                       f"{rig}: bone {receiver}'s skeleton parent is not bone {source}")
    toes = {side: TOES[side] in found for side in model.SIDES}
    signs = {side: [found[s + k]["axes"]["sign"][0] * found[r + k]["axes"]["sign"][0]
                    for s, r in PAIR_BONES] for side, k in (("Left", 0), ("Right", 1))}
    return dict(rig=found, toes=toes, signs=signs, case=path.name[:-len(".pending.json")])


def rows():
    """[(case, index, rig, tuple name, vector, group)]: every exploration and selection row, in search order: the
    core vectors of exploration's all-equal tuples, then its single-field tuples, then its full-set extras and the
    toe-absent rows, then selection."""
    core = {tuple(sorted(v.items())) for v in tables.core()}
    found = []
    for case, (rig, name, kind, part) in tables.cases().items():
        if part == "confirmation":
            continue
        for index, vector in enumerate(tables.SETS[kind]()):
            if part == "selection":
                group = "selection"
            elif kind == "toe-absent":
                group = "exploration toe-absent"
            elif tuple(sorted(vector.items())) not in core:
                group = "exploration full-set extras"
            elif name in tables.ALL_EQUAL:
                group = "exploration all-equal core"
            else:
                group = "exploration single-field core"
            found.append((case, index, rig, name, vector, group))
    order = ("exploration all-equal core", "exploration single-field core", "exploration full-set extras",
             "exploration toe-absent", "selection")
    return sorted(found, key=lambda row: order.index(row[5]))


def predict(rig, vector, settings, limb, foot, side):
    values = [0.] * 95
    for m, v in vector.items():
        values[m] = v
    try:
        return model.chains(rig["rig"], values, [0.] * 95, settings, limb, foot, side)
    except bound.Pole:
        return None


def margin(a, b):
    """How much further apart two predictions (unit quaternion, radius) are than their bounds' sum, in radians."""
    (p, ep), (q, eq) = a, b
    dot = abs(sum(x * y for x, y in zip(p, q)))
    turn = 2 * math.acos(min(1., dot / (bound.norm(p) * bound.norm(q))))
    return turn - (bound.angle_bound(ep) + bound.angle_bound(eq))


def _apart(a, b):
    """Whether two predictions are further apart than their bounds' sum: one recording cannot meet both."""
    return margin(a, b) > 0


def _sides(vector):
    sides = []
    for side, (legs, arms) in tables.SIDE_OFFSETS.items():
        if any(m in vector for m in (23 + legs, 25 + legs, 27 + legs, 41 + arms, 43 + arms)):
            sides.append(side)
    return sides


def search(rigs, log=print):
    """The witnesses: {limb pair: row}, {foot pair: row}, {joint pair: row}, and the joint pairs with none."""
    limb_open = set(itertools.combinations(range(len(model.LIMBS)), 2))
    foot_open = set(itertools.combinations(range(len(model.FEET)), 2))
    limb_found, foot_found = {}, {}
    plan = rows()
    started = time.time()
    for n, (case, index, rig_name, name, vector, group) in enumerate(plan):
        rig, settings = rigs[model_rig(rig_name)], tables.TUPLES[name]
        for side in _sides(vector):
            if limb_open:
                preds = [predict(rig, vector, settings, limb, model.FEET[0], side) for limb in model.LIMBS]
                for pair in list(limb_open):
                    a, b = preds[pair[0]], preds[pair[1]]
                    if a is not None and b is not None:
                        best = max(margin(a[k], b[k]) for k in LIMB_ONLY[side])
                        if best > 0:
                            limb_open.discard(pair)
                            limb_found[pair] = (case, index, group, best)
            if foot_open and rig["toes"][side]:
                preds = [predict(rig, vector, settings, model.LIMBS[0], foot, side) for foot in model.FEET]
                for pair in list(foot_open):
                    a, b = preds[pair[0]], preds[pair[1]]
                    if a is not None and b is not None and margin(a[TOES[side]], b[TOES[side]]) > 0:
                        foot_open.discard(pair)
                        foot_found[pair] = (case, index, group, margin(a[TOES[side]], b[TOES[side]]))
        if n % 200 == 0:
            log(f"row {n}/{len(plan)}: {len(limb_open)} limb and {len(foot_open)} foot pairs open "
                f"({time.time() - started:.0f} s)")
    # Joint pairs neither pass settles: the limbs equal or unwitnessed, the feet equal or unwitnessed.
    # Two different limbs meet a foot pair in either orientation; one limb needs the foot pair once.
    same_limbs = [(i, i) for i in range(len(model.LIMBS))]
    same_feet = [(f, f) for f in range(len(model.FEET))]
    feet_both = sorted(foot_open) + [(f2, f1) for f1, f2 in sorted(foot_open)]
    joint_open = {(li, fi) for li in same_limbs for fi in sorted(foot_open)}
    joint_open |= {(li, fi) for li in sorted(limb_open) for fi in same_feet + feet_both}
    joint_found = {}
    log(f"{len(joint_open)} joint pairs left for the Foot")
    for case, index, rig_name, name, vector, group in plan:
        if not joint_open:
            break
        rig, settings = rigs[model_rig(rig_name)], tables.TUPLES[name]
        for side in _sides(vector):
            cache = {}

            def at(limb, foot):
                if (limb, foot) not in cache:
                    found = predict(rig, vector, settings, model.LIMBS[limb], model.FEET[foot], side)
                    cache[(limb, foot)] = None if found is None else found[FOOT[side]]
                return cache[(limb, foot)]
            for pair in list(joint_open):
                (l1, l2), (f1, f2) = pair
                a, b = at(l1, f1), at(l2, f2)
                if a is not None and b is not None and margin(a, b) > 0:
                    joint_open.discard(pair)
                    joint_found[pair] = (case, index, group, margin(a, b))
    return limb_found, foot_found, joint_found, joint_open


def model_rig(name):
    return PROBE_RIGS[name]


def cells(rigs, joint_open):
    """Per proposed admission cell: the joint pairs its keys separate, and those of them without a witness."""
    found = []
    joint = model.candidates()
    for name, rig_names in ADMISSION:
        settings = tables.TUPLES[name]
        for rig_name in rig_names:
            rig = rigs[rig_name]
            keys = [model.canonical(limb, foot, settings, rig["toes"], rig["signs"]) for limb, foot in joint]
            groups = {}
            for key in keys:
                groups[key] = groups.get(key, 0) + 1
            total = len(keys) * (len(keys) - 1) // 2 - sum(n * (n - 1) // 2 for n in groups.values())
            index = {(model.LIMBS.index(limb), model.FEET.index(foot)): k for k, (limb, foot) in enumerate(joint)}
            unwitnessed = [((l1, f1), (l2, f2)) for (l1, l2), (f1, f2) in joint_open
                           if keys[index[(l1, f1)]] != keys[index[(l2, f2)]]]
            found.append(dict(tuple=name, rig=rig_name, case=rig["case"], toes=rig["toes"], signs=rig["signs"],
                              equations=len(groups), separated_pairs=total, unwitnessed=len(unwitnessed),
                              refused=bool(unwitnessed), examples=[describe(p) for p in sorted(unwitnessed)[:20]]))
    return found


def describe(pair):
    return [dict(limb=model.LIMBS[l]._asdict(), foot=model.FEET[f]._asdict()) for l, f in pair]


def run(folder, unityrs, log=print):
    views = {}
    rigs = {}
    for rig in SCOPED_RIGS:
        rigs[rig] = load_rig(folder, rig, unityrs, views)
        log(f"read {rig}")
    limb_found, foot_found, joint_found, joint_open = search(rigs, log)
    groups = {}
    for found in (limb_found, foot_found, joint_found):
        for case, index, group, _ in found.values():
            groups.setdefault(group, set()).add((case, index))
    summary = {group: dict(rows=len(rows_), cases=sorted({c for c, _ in rows_})) for group, rows_ in groups.items()}
    # Each witness's margin at its first row, in degrees: how far a recording must stray from the f32 equation to
    # meet both candidates.
    margins = sorted(math.degrees(found[3]) for table in (limb_found, foot_found, joint_found)
                     for found in table.values())
    spread = dict(pairs=len(margins), smallest_degrees=margins[0] if margins else None,
                  under={f"{limit:g}": sum(m < limit for m in margins) for limit in (1e-3, 1e-2, .1, 1.)})
    manifest = replay.read_json((folder / "run.json").read_text(encoding="utf-8"))
    return dict(format=FORMAT, editor=manifest["run"]["editor_version"], run_id=manifest["run"]["run_id"],
                tables=tables.digest(), limb_candidates=len(model.LIMBS), foot_candidates=len(model.FEET),
                limb_pairs_unwitnessed=len(model.LIMBS) * (len(model.LIMBS) - 1) // 2 - len(limb_found),
                foot_pairs_unwitnessed=10 - len(foot_found), joint_pairs_unwitnessed=len(joint_open),
                witness_rows=summary, witness_margins=spread, cells=cells(rigs, joint_open),
                unwitnessed=[describe(((l1, f1), (l2, f2))) for (l1, l2), (f1, f2) in sorted(joint_open)])


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--unityrs", required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("folder", type=Path)
    args = parser.parse_args()
    try:
        result = run(args.folder, str(Path(args.unityrs).resolve()))
        args.out.write_text(json.dumps(result, indent=1, ensure_ascii=False) + "\n", encoding="utf-8")
    except (ValueError, OSError) as exc:
        parser.exit(1, f"anim_twist_witness: {exc}\n")


if __name__ == "__main__":
    sys.exit(main())
