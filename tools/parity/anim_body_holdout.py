"""The hold-out's adequacy predicted before the freeze (docs/plans/humanoid-body.md 6 and 7), from its spec alone.

Section 7's criterion, u_s <= E_unity,s, depends only on the frozen design
and the hold-out's planned geometry and poses, never on what Unity records
for them, so the freeze can require it at every planned hold-out sample
before E3b opens anything. No hold-out recording or bundle is read here.

The stand-in. The hold-out rig shares its base rig's topology and bone
directions (anim_fixture_body.PROBE_RIGS) and differs only in its limbs'
lengths (anim_fixture_rigs.rig_layout), and every hold-out vector gives
each bone one of the base rig's own probe vectors. So each bone's local
rotation is the one Unity recorded for the same muscles in the base rig's
probe case (anim_body_design.predict), and its local translations are the
builder's layout, as are the Avatar's m_SkeletonPose translations. The
human scale is the rest pose's centre-of-mass height and the known body
target the rest body position (as Unity defines both), and each sample's
hips are where the model places them (2.3, with the given coefficients).
The rows are then the checkpoint's (anim_body_adequacy.samples): J_s is
exact, and E_unity,s is the hold-out's own up to the model's hips error.
A sample E3b would not compare is no comparison here either: one whose
hips the frame scope refuses, one past the lateral limit (E3's `outside`)
or out of topology scope. Each version needs at least E3b's minimum of
in-scope comparisons among the rest.

The freeze requires E_unity,s / u_s >= HOLDOUT_MARGIN at every planned
in-frame sample of every Editor version, so a prediction off by a few
percent still cannot let E3b fail on the criterion. Against round 9's
consumed hold-out, recomputed at every sample, the predicted ratio was
within 0.26% (docs/plans/humanoid-body.md, E3b on freeze v7). The margin is not larger because the criterion is block-l1: a
training row of a given leverage cannot lower u_s below what that leverage
allows, however often it repeats, so added training poses lift the worst
planned sample only a little past it.
"""

import copy

import anim_body_adequacy as adequacy
import anim_replay as replay
from anim_body_design import predict
from anim_body_model import Refused, centre, solve_hips
from anim_body_tolerance import U64
from anim_fixture_body import BODY_INFO, HOLDOUT, PROBE_RIGS, primary_probes, rig_bones
from anim_fixture_rigs import rig_layout

HOLDOUT_MARGIN = 1.05
F32, BITS = replay.f32_from_bits, replay.f32_bits


def source_rig(rig):
    """The training rig whose recorded probe case gives the hold-out rig's rotations: its base rig."""
    base = PROBE_RIGS[rig][0]
    replay.require(rig_bones(rig) == rig_bones(base), f"the {rig} rig maps other bones than its base rig")
    return base


def standin(name, source, avatar=None):
    """(recording, Avatar) stand-ins for the planned hold-out case `name`: the source probe case's rotations
    for its schedule, on the hold-out rig's layout, with no read-back."""
    offsets = {bone: offset for bone, _, offset in rig_layout(BODY_INFO[name]["rig"])}
    doc = predict(name, source)
    for i, node in enumerate(doc["spec"]["nodes"]):
        if node["name"] in offsets:
            bits = [BITS(v) for v in offsets[node["name"]]]
            doc["capture"]["rest"]["transforms"][i]["translation"] = bits
            for sample in doc["capture"]["samples"]:
                sample["pose"]["transforms"][i]["translation"] = list(bits)
    doc["spec"]["evidence"]["editor"].pop("body_samples", None)
    if avatar is not None:
        avatar = copy.deepcopy(avatar)
        human = avatar["m_Avatar"]["m_Human"]
        human = human.get("data", human)
        skeleton = human["m_Skeleton"].get("data", human["m_Skeleton"])
        pose = human["m_SkeletonPose"].get("data", human["m_SkeletonPose"])["m_X"]
        paths = dict(map(tuple, avatar.get("m_TOS") or []))
        for k, node_id in enumerate(skeleton.get("m_ID") or []):
            leaf = (paths.get(node_id) or "").rpartition("/")[2]
            if leaf in offsets and k < len(pose):
                pose[k]["t"] = {axis: float(F32(BITS(v))) for axis, v in zip("xyz", offsets[leaf])}
    return doc, avatar


def planned(name, source, coefficients, rows_of, avatar=None):
    """The planned hold-out case's rows (module docstring), each with `in_frame`. `rows_of(doc, avatar)` gives
    a recording's checkpoint rows as the caller's context reads them (its masses and Avatar)."""
    doc, avatar = standin(name, source, avatar)
    editor = doc["spec"]["evidence"]["editor"]
    rest = next(row for row in rows_of(doc, avatar) if not any(F32(b) for b in row["state"]["sample"]["muscles_f32"]))
    state = rest["state"]
    c = centre({bone: frame.position for bone, frame in state["bones"].items()}, state["weights"], state["links"],
               coefficients)
    scale = F32(BITS(c[1]))
    doc["capture"]["humanoid"]["human_scale"] = BITS(scale)
    for target in editor["body_targets"]:
        replay.require(not any(F32(b) for b in target["offset"] + target["tilt"]), f"{name}: a hold-out target moves")
        target["position"] = [BITS(c[0] / scale), BITS(1.), BITS(c[2] / scale)]
    hips = next(row["node"] for row in doc["capture"]["humanoid"]["bones"] if row["name"] == "Hips")
    framed = []
    for sample, row in zip(doc["capture"]["samples"], rows_of(doc, avatar), strict=True):
        state = row["state"]
        local = {bone: frame.position for bone, frame in state["local"][U64][0].items()}
        try:
            t, q = solve_hips(state["target"], state["body_rotation"], state["scale"], local, state["weights"],
                              state["links"], coefficients, state["rest_frame"], state["chain"][U64],
                              state["hips_scale"])
        except Refused:
            framed.append(False)
            continue
        framed.append(True)
        sample["pose"]["transforms"][hips]["translation"] = [BITS(v) for v in t]
        sample["pose"]["transforms"][hips]["rotation"] = [BITS(v) for v in q]
    rows = rows_of(doc, avatar)
    for row, in_frame in zip(rows, framed, strict=True):
        row["in_frame"] = in_frame
    return rows


def margins(rows, uncertainty, label, margin=HOLDOUT_MARGIN, limit=None, minimum=0):
    """The predicted criterion over the rows E3b would compare (in frame, in topology scope, within the lateral
    `limit`): E_unity,s / u_s at each, the worst, and a failure for each under `margin` and for fewer than
    `minimum` of them."""
    from anim_body_check import outside

    table, failures = [], []
    for row in rows:
        if not row["in_frame"] or row.get("refused") or outside(row, limit):
            continue
        u = uncertainty(row)
        ratio = row["unity"] / u if u > 0 else float("inf")
        table.append(dict(local=row["local"], unity=row["unity"], uncertainty=u, margin=ratio))
        if not ratio >= margin:
            failures.append(f"{label}@{row['local']}: predicted u_s {u:.4e} against E_unity,s {row['unity']:.4e}, "
                            f"margin {ratio:.4f} under {margin}")
    if len(table) < minimum:
        failures.append(f"{label}: {len(table)} predicted in-scope comparison(s), under the minimum {minimum}")
    worst = min((entry["margin"] for entry in table), default=None)
    return dict(worst_margin=worst, in_frame=len(table), refused=len(rows) - len(table), failures=failures,
                samples=table)


def predicted(runs, design, coefficients, avatar_of=None, rig=HOLDOUT, limit=None, minimum=0):
    """Each run's predicted hold-out margins under `design` (anim_body_runs.Design) and the coefficients, with
    the run's Avatars when `avatar_of` reads them, the lateral `limit` and the `minimum` in-scope count E3b
    applies: {version: margins(...)}, never reading a hold-out file."""
    from anim_body_runs import avatar_masses, avatar_scope, rest_translations

    name, source = primary_probes(rig), primary_probes(source_rig(rig))
    out = {}
    for run in runs:
        doc = run.documents.get(source)
        replay.require(doc is not None, f"{run.version}: the hold-out prediction needs {source}")
        parents, masses = adequacy.bone_table(run.documents.values())

        def rows_of(recording, avatar):
            if avatar is None:
                return list(adequacy.samples(name, recording, parents, masses))
            skeleton, refused = avatar_scope(avatar, recording, parents)
            replay.require(refused is None, f"{name}: the planned Avatar is refused: {refused}")
            return list(adequacy.samples(name, recording, parents, avatar_masses(avatar), skeleton,
                                         rest_translations(avatar, recording)))

        rows = planned(name, doc, coefficients, rows_of, avatar_of(run, doc) if avatar_of else None)
        out[run.version] = margins(rows, design.hips_uncertainty, f"{run.version}/{name}", limit=limit,
                                   minimum=minimum)
    return out
