"""Round 12's witness rule (round 12 design v4 section 2), on actual built Avatars, before collection.

For every two candidates (anim_twist_model: 126 limb x 5 foot) whose keys differ on a proposed admission cell
(`canonical`), a witness is a named exploration or selection row (anim_twist_tables: a case and its capture sample
index) where some chain bone's two predictions are further apart than the sum of their derived bounds
(anim_twist_bound) and the separation's own f64 error, so no one recording can satisfy both. A pair with neither
equal keys nor a witness refuses the cell; a search that finds no witness never makes two candidates equivalent.

The search runs in passes, exactly the rule, never an approximation of it: limb pairs on the bones only the limb
candidate moves (the arm chains, the upper and lower legs) and foot pairs on the Toes; a joint pair takes a limb or
foot pair's witness only where both its candidates predict on that row; every other joint pair is searched on every
chain bone of every row.

    python3 -I tools/parity/anim_twist_witness.py --unityrs PATH --out FILE <round 11 run folder>
        One Editor's run folder. It opens only the named exploration captures (`allowlisted`: one identity-root
        in-scope clip per scoped rig, for its Avatar and rest pose; never a sweep, fingers, face or hold-out, never
        through a symlink), through round 12's allowlist (anim_fixture_round12.round11_view), and writes the result: per cell, the pairs its keys separate and those without a
        witness; every limb, foot and searched joint pair's witness row; per row group, the rows that witness some
        pair; the witnesses' margins over their bounds.
"""

import argparse
import itertools
import json
import math
import os
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
# How far a chain bone's recorded rest may turn from the Avatar's m_SkeletonPose: f32 storage, far under any
# other Avatar's rest.
REST_MATCH = 1e-5
# A recorded rest, and every chain bone's m_PreQ and m_PostQ, is a unit quaternion as Unity stores it: its f32 squared
# norm finite and normal, and its norm within REST_NORM of 1 (a few f32 ulps), so the f32 path normalizes it as the
# f64 one assumes.
REST_NORM = 1e-5
ADMISSION = (("t0", ("compact", "tall")), ("t1", ("compact", "tall")), ("m1", ("compact", "tall")),
             ("t5", SCOPED_RIGS))


def allowlisted(rig):
    """The one round 11 case a rig's Avatar comes from, by name: its scoped clip's first identity-root variant
    (anim_fixture_full.variants' order), an in-scope clip of round 11's table, never a sweep or a hold-out."""
    import anim_fixture_full as full
    import anim_fixture_round11 as r11

    scoped = rig + "-scoped"
    replay.require(rig in SCOPED_RIGS and scoped in full.SCOPED_RIGS, f"{rig} is not a scoped full-body rig")
    case = f"body-clip-{scoped}-{full.variants(scoped)[0]}"
    info = r11.ROUND11_INFO.get(case)
    replay.require(info is not None and info["kind"] == "clip" and info["rig"] == scoped
                   and info["root"] == "identity" and info.get("scope") == "in",
                   f"{case} is not an identity-root in-scope round 11 clip")
    return case


def unit_quaternion(q, label):
    """Refuse a rest, m_PreQ or m_PostQ quaternion whose f32 squared norm (Unity's sum order) is not finite and
    normal, or whose norm is not within REST_NORM of 1: (0, 0, 0, 2^-100) points at the identity but its squared norm
    underflows to 0."""
    def f32(x):
        try:
            return bound._f32(x)
        except OverflowError:
            return math.inf
    total = 0.
    for x in q:
        total = f32(total + f32(x * x))
    replay.require(len(q) == 4 and math.isfinite(total) and total >= 2. ** -126
                   and abs(math.sqrt(total) - 1) <= REST_NORM, f"{label} is not a unit quaternion")


def axes_of(record, label):
    """A chain bone's m_AxesArray record as the model reads it, its m_PreQ and m_PostQ unit quaternions."""
    from anim_humanoid_qualify import vec
    from anim_muscle_pose import f32

    axes = {key: [f32(v) for v in vec(record[name])] for key, name in
            (("pre", "m_PreQ"), ("post", "m_PostQ"), ("sign", "m_Sgn"))}
    axes |= {key: [f32(v) for v in vec(record["m_Limit"][name])] for key, name in (("min", "m_Min"), ("max", "m_Max"))}
    unit_quaternion(axes["pre"], f"{label} m_PreQ")
    unit_quaternion(axes["post"], f"{label} m_PostQ")
    return axes


def run_file(folder, *parts):
    """A file of the run itself: no component a symlink (lstat), the run folder itself included, no ancestor of it
    a symlink either (its real path its absolute one), and resolving inside the run folder."""
    replay.require(not folder.is_symlink() and folder.is_dir(), f"{folder.name} is a symlink or not a folder")
    replay.require(os.path.realpath(folder) == os.path.abspath(folder), f"{folder.name} is reached through a symlink")
    root = folder.resolve()
    path = folder
    for part in parts:
        replay.require(part not in ("", ".", "..") and "/" not in part and "\\" not in part,
                       f"{part!r} is not a plain file name")
        path = path / part
        replay.require(not path.is_symlink(), f"{path.name} is a symlink")
    replay.require(path.is_file() and path.resolve().is_relative_to(root), f"{path.name} is not a file of the run")
    return path


def load_rig(folder, rig, unityrs, views, run):
    """{bone: {"axes", "rest"}}, the toes, the x-sign products and the x reaches per side, from one allowlisted
    capture and its bundle's Avatar: the capture of this run and Editor and of its named case, at its placement,
    every chain bone's recorded rest the Avatar's own (m_SkeletonPose, by m_TOS path), and the bundle the
    capture's pinned sha256."""
    import anim_fixture_round11 as r11
    from anim_body_runs import human_of
    from anim_fixture_contract import check_identity
    from anim_humanoid_qualify import node_paths, tos_paths
    from anim_muscle_confirm import placement
    from anim_muscle_pose import avatar_node
    from anim_replay_complete import BundleView, bundle_digest, check_recording_run, read_manifest

    import anim_fixture_round12 as round12

    case = allowlisted(rig)
    # Through round 12's allowlist (anim_fixture_round12.round11_view): its exploration cases alone open.
    allow = views.setdefault("allowlist", round12.round11_view(folder))
    doc = allow.capture(case)
    spec, capture = doc["spec"], doc["capture"]
    check_recording_run(doc, run, replay, case)
    check_identity(case, r11.ROUND11_CASES[case], spec, capture, replay)
    placement(case, r11.ROUND11_INFO[case], doc)
    names = [row["name"] for row in capture["humanoid"]["muscles"]]
    replay.require(all(names[m] == name for name, m in tables.NAMES.items()),
                   f"{case}: the muscle names differ from anim_twist_tables.NAMES")
    bundle = allow.bundle(case, doc).resolve()
    digest = bundle_digest(bundle)
    replay.require(capture["bundle_sha256"] == [digest], f"{bundle.name} differs from the capture's pinned sha256")
    if (bundle, digest) not in views:
        views[(bundle, digest)] = BundleView(bundle, digest, 0, unityrs, None, replay, read_manifest(folder, replay)[1])
    view = views[(bundle, digest)]
    avatar = view.value(view.asset(spec["avatar_asset"], 90))
    human = human_of(avatar)
    skeleton = human["m_Skeleton"].get("data", human["m_Skeleton"])
    poses = human["m_SkeletonPose"].get("data", human["m_SkeletonPose"])["m_X"]
    paths, recorded_paths = tos_paths(avatar), node_paths(spec)
    bones = capture["humanoid"]["bones"]
    found, nodes = {}, {}
    for bone in CHAIN_BONES:
        node, recorded = avatar_node(human, bone), bones[bone]["node"]
        replay.require((node is None) == (recorded < 0), f"{case}: bone {bone} is mapped by only one of the capture "
                                                         "and the Avatar")
        if node is None:
            continue
        axes_id = skeleton["m_Node"][node]["m_AxesId"]
        replay.require(type(axes_id) is int and 0 <= axes_id < len(skeleton["m_AxesArray"]),
                       f"{case}: bone {bone} has no axes record (m_AxesId {axes_id!r})")
        replay.require(recorded_paths.get(recorded) == paths.get(skeleton["m_ID"][node]) is not None,
                       f"{case}: bone {bone} is recorded from another transform than the Avatar's")
        rest = [replay.f32_from_bits(v) for v in capture["rest"]["transforms"][recorded]["rotation"]]
        unit_quaternion(rest, f"{case}: bone {bone}'s rest")
        # The Avatar stores its pose in f32 off unit by an ulp or two (w 1 + 2^-22 at the identity): the same
        # rotation, to REST_MATCH.
        replay.require(bound.separation(rest, [float(poses[node]["q"][a]) for a in "xyzw"])[0] <= REST_MATCH,
                       f"{case}: bone {bone} is recorded at a rest rotation not the Avatar's")
        axes = axes_of(skeleton["m_AxesArray"][axes_id], f"{case}: bone {bone}'s")
        found[bone] = dict(axes=axes, rest=rest)
        nodes[bone] = node
    for source, receiver in PAIR_BONES + ((14, 16), (16, 18), (2, 4), (4, 6)):
        replay.require(source in nodes and receiver in nodes
                       and skeleton["m_Node"][nodes[receiver]]["m_ParentId"] == nodes[source],
                       f"{rig}: bone {receiver}'s skeleton parent is not bone {source}")
    toes = {side: TOES[side] in found for side in model.SIDES}
    signs = {side: [found[s + k]["axes"]["sign"][0] * found[r + k]["axes"]["sign"][0]
                    for s, r in PAIR_BONES] for side, k in (("Left", 0), ("Right", 1))}
    return dict(rig=found, toes=toes, signs=signs, reach=model.x_reach(found), case=case)


def rows():
    """[(case, sample, rig, tuple name, vector, group)]: every exploration and selection row, in search order: the
    core vectors of exploration's all-equal tuples, then its single-field tuples, then its full-set extras and the
    toe-absent rows, then selection. `sample` is the vector's capture sample index (anchors counted:
    anim_twist_tables.sample_index), the index a recording names it by."""
    core = {tuple(sorted(v.items())) for v in tables.core()}
    found = []
    for case, (rig, name, kind, part) in tables.cases().items():
        if part == "confirmation":
            continue
        vectors = tables.SETS[kind]()
        for index, vector in enumerate(vectors):
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
            found.append((case, tables.sample_index(index, len(vectors)), rig, name, vector, group))
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
    """How much further apart two predictions (quaternion, radius) are than their bounds' sum, in radians: their
    separation less its own f64 error and both bounds."""
    (p, ep), (q, eq) = a, b
    turn, error = bound.separation(p, q)
    return turn - error - (bound.angle_bound(ep) + bound.angle_bound(eq))


def _apart(a, b):
    """Whether two predictions are further apart than their bounds' sum: one recording cannot meet both."""
    return margin(a, b) > 0


def _best(a, b, bones):
    return max(margin(a[k], b[k]) for k in bones if k in a and k in b)


def _sides(vector):
    sides = []
    for side, (legs, arms) in tables.SIDE_OFFSETS.items():
        if any(m in vector for m in (23 + legs, 25 + legs, 27 + legs, 41 + arms, 43 + arms)):
            sides.append(side)
    return sides


def _joint(l, f):
    return l * len(model.FEET) + f


def search(rigs, log=print):
    """The witnesses: {limb pair: row}, {foot pair: row}, {joint pair: row} and the joint pairs with none, each row
    (case, sample, side, group, margin). A limb or foot witness credits a joint pair only where both of its joint
    candidates predict on that row (a pole anywhere on the side refuses the whole side); every other joint pair is
    searched on all chain bones of every row."""
    limb_open = set(itertools.combinations(range(len(model.LIMBS)), 2))
    foot_open = set(itertools.combinations(range(len(model.FEET)), 2))
    limb_found, foot_found = {}, {}
    plan = rows()
    started = time.time()
    for n, (case, sample, rig_name, name, vector, group) in enumerate(plan):
        rig, settings = rigs[model_rig(rig_name)], tables.TUPLES[name]
        for side in _sides(vector):
            if limb_open:
                preds = [predict(rig, vector, settings, limb, model.FEET[0], side) for limb in model.LIMBS]
                for pair in list(limb_open):
                    a, b = preds[pair[0]], preds[pair[1]]
                    if a is not None and b is not None:
                        best = _best(a, b, LIMB_ONLY[side])
                        if best > 0:
                            limb_open.discard(pair)
                            limb_found[pair] = (case, sample, side, group, best)
            if foot_open and rig["toes"][side]:
                preds = [predict(rig, vector, settings, model.LIMBS[0], foot, side) for foot in model.FEET]
                for pair in list(foot_open):
                    a, b = preds[pair[0]], preds[pair[1]]
                    if a is not None and b is not None and margin(a[TOES[side]], b[TOES[side]]) > 0:
                        foot_open.discard(pair)
                        foot_found[pair] = (case, sample, side, group, margin(a[TOES[side]], b[TOES[side]]))
        if n % 200 == 0:
            log(f"row {n}/{len(plan)}: {len(limb_open)} limb and {len(foot_open)} foot pairs open "
                f"({time.time() - started:.0f} s)")
    # Which joint candidates predict on each witness row and side.
    by_row = {(case, sample): (rig_name, name, vector) for case, sample, rig_name, name, vector, _ in plan}
    predicts = {}
    for case, sample, side, _, _ in itertools.chain(limb_found.values(), foot_found.values()):
        if (case, sample, side) not in predicts:
            rig_name, name, vector = by_row[(case, sample)]
            rig, settings = rigs[model_rig(rig_name)], tables.TUPLES[name]
            predicts[(case, sample, side)] = {
                _joint(l, f) for l, limb in enumerate(model.LIMBS) for f, foot in enumerate(model.FEET)
                if predict(rig, vector, settings, limb, foot, side) is not None}

    def credited(found, pair, first, second):
        row = found.get(pair)
        return row is not None and {first, second} <= predicts[row[:3]]
    joint_open, credits = set(), {}
    for (l1, f1), (l2, f2) in itertools.combinations(itertools.product(range(len(model.LIMBS)),
                                                                       range(len(model.FEET))), 2):
        first, second = _joint(l1, f1), _joint(l2, f2)
        limbs, feet = (min(l1, l2), max(l1, l2)), (min(f1, f2), max(f1, f2))
        if l1 != l2 and credited(limb_found, limbs, first, second):
            credits[((l1, l2), (f1, f2))] = ("limb", limbs)
        elif f1 != f2 and credited(foot_found, feet, first, second):
            credits[((l1, l2), (f1, f2))] = ("foot", feet)
        else:
            joint_open.add(((l1, l2), (f1, f2)))
    joint_found = {}
    log(f"{len(credits)} joint pairs credited, {len(joint_open)} left for every chain bone")
    for case, sample, rig_name, name, vector, group in plan:
        if not joint_open:
            break
        rig, settings = rigs[model_rig(rig_name)], tables.TUPLES[name]
        for side in _sides(vector):
            cache = {}

            def at(limb, foot):
                if (limb, foot) not in cache:
                    cache[(limb, foot)] = predict(rig, vector, settings, model.LIMBS[limb], model.FEET[foot], side)
                return cache[(limb, foot)]
            for pair in list(joint_open):
                (l1, l2), (f1, f2) = pair
                a, b = at(l1, f1), at(l2, f2)
                if a is not None and b is not None:
                    best = _best(a, b, CHAIN_BONES)
                    if best > 0:
                        joint_open.discard(pair)
                        joint_found[pair] = (case, sample, side, group, best)
    return limb_found, foot_found, joint_found, joint_open, credits


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
            keys = [model.canonical(limb, foot, settings, rig["toes"], rig["signs"], rig["reach"])
                    for limb, foot in joint]
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


def _row(found):
    case, sample, side, group, best = found
    return dict(case=case, sample=sample, side=side, group=group, margin_degrees=math.degrees(best))


def run(folder, unityrs, log=print):
    from anim_replay_complete import read_manifest

    run_file(folder, "run.json")
    _, identity = read_manifest(folder, replay)
    views = {}
    rigs = {}
    for rig in SCOPED_RIGS:
        rigs[rig] = load_rig(folder, rig, unityrs, views, identity)
        log(f"read {rig}")
    limb_found, foot_found, joint_found, joint_open, credits = search(rigs, log)
    groups = {}
    for found in (limb_found, foot_found, joint_found):
        for case, sample, _, group, _ in found.values():
            groups.setdefault(group, set()).add((case, sample))
    summary = {group: dict(rows=len(rows_), cases=sorted({c for c, _ in rows_})) for group, rows_ in groups.items()}
    # Each witness's margin at its row, in degrees: how far a recording must stray from the f32 equation to meet
    # both candidates.
    margins = sorted(math.degrees(found[4]) for table in (limb_found, foot_found, joint_found)
                     for found in table.values())
    spread = dict(pairs=len(margins), smallest_degrees=margins[0] if margins else None,
                  under={f"{limit:g}": sum(m < limit for m in margins) for limit in (1e-3, 1e-2, .1, 1.)})
    return dict(format=FORMAT, editor=identity["editor_version"], run_id=identity["run_id"],
                tables=tables.digest(), limb_candidates=len(model.LIMBS), foot_candidates=len(model.FEET),
                limb_pairs_unwitnessed=len(model.LIMBS) * (len(model.LIMBS) - 1) // 2 - len(limb_found),
                foot_pairs_unwitnessed=10 - len(foot_found), joint_pairs_unwitnessed=len(joint_open),
                witness_rows=summary, witness_margins=spread, cells=cells(rigs, joint_open),
                unwitnessed=[describe(((l1, f1), (l2, f2))) for (l1, l2), (f1, f2) in sorted(joint_open)],
                limb_witnesses=[dict(limbs=list(pair), **_row(found)) for pair, found in sorted(limb_found.items())],
                foot_witnesses=[dict(feet=list(pair), **_row(found)) for pair, found in sorted(foot_found.items())],
                joint_witnesses=[dict(joint=[[l1, f1], [l2, f2]], **_row(found))
                                 for ((l1, l2), (f1, f2)), found in sorted(joint_found.items())],
                # Every joint pair a limb or foot witness credits, with that witness's row: [limb 1, foot 1, limb 2,
                # foot 2, "limb" or "foot", case, capture sample index, side].
                joint_credits=[[l1, f1, l2, f2, kind,
                                *(limb_found if kind == "limb" else foot_found)[component][:3]]
                               for ((l1, l2), (f1, f2)), (kind, component) in sorted(credits.items())])


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--unityrs", required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("folder", type=Path)
    args = parser.parse_args()
    try:
        result = run(args.folder, str(Path(args.unityrs).resolve()))
        args.out.write_text(json.dumps(result, separators=(",", ":"), ensure_ascii=False) + "\n", encoding="utf-8")
    except (ValueError, OSError) as exc:
        parser.exit(1, f"anim_twist_witness: {exc}\n")


if __name__ == "__main__":
    sys.exit(main())
