"""B1's predictive adequacy: planned extra probe cases and range sweeps from a rig's recorded geometry.

An extra probe vector (anim_fixture_body.EXTRA_PROBES) gives each bone the
muscles of one of the rig's own recorded probe vectors and zero elsewhere.
A bone's local rotation depends only on its own muscles, so each bone of a
planned pose takes the local rotation Unity recorded for the same muscle
values in the rig's probe case, bit for bit. That fixes the pose's
hips-local forward kinematics, so its columns in A (up to a turn of the
whole block) and its Jacobian J_s are exact. The body target is the
recorder's own known vector, the same in every probe sample.

Unity's solved hips transform and world read-back are not known before a
run, and they set epsilon (the block's weight) and E_unity,s (the bound on
the right). They are bounded by an envelope: every recorded sample of the
rig's identity-root probe and target cases (never the hold-out) lends its
hips transform and world read-back to the planned pose, and the pose takes
the largest epsilon and the smallest E_unity,s and the largest E_eval,s
over the envelope (`conservative`). A larger epsilon weights the block
less, so it can only raise u_s elsewhere; a smaller E_unity,s lowers the
pose's own bound. The margin is then a conservative prediction. A
recorded run of the planned case stays the checkpoint of record.

A range sweep (anim_fixture_body.RANGE_RIGS) sets each mapped spine bone's
three muscles to values its probe case never recorded, so its spine bones'
local rotations come from Unity's muscle map instead, rebuilt from the probe
case alone: a bone's local rotation is q_0 S(t_y, t_z) T(t_x), q_0 its
recorded rest rotation, S = normalize(t_y e_y + t_z e_z, 1) the swing and
T = normalize(t_x e_x, 1) the twist, each t = tan(theta / 2) with theta the
muscle's value times its upper limit (or minus its lower one below zero) in
degrees: the bone's own limit row when it has one, else HumanTrait's
default. Each axis e is its single-muscle probe's turn from q_0 as vector
over scalar part, divided by that probe's own t (q_0^-1 q = (t e, 1)
normalized), so Unity's PreQ, PostQ and sign are inside e. A probe whose turn
angle is not its value's theta, within ANGLE_SLACK, refuses: the map would
not hold. Every other bone keeps its neutral rotation, as the vector sets no
other muscle. The envelope and the merge are the extra cases'.
"""

import copy
import math

import anim_replay as replay
from anim_fixture_body import (BODY_INFO, HOLDOUT, PROBE_GROUPS, PROBE_VALUES, RANGE_BONES, primary_probes,
                               probe_schedule)
from anim_reference import multiply, normalize

# Degrees by which a probe's recorded turn may miss its value's limit angle: f32 rotations hold some 1e-5.
ANGLE_SLACK = 1e-3


def envelope(documents, rig):
    """The recorded (hips transform, read-back) pairs that bound a planned pose of `rig`: every sample with
    read-back of the rig's identity-root probe and target cases."""
    replay.require(rig != HOLDOUT, "the hold-out rig plans no poses")
    pairs = []
    for name, info in BODY_INFO.items():
        doc = documents.get(name)
        if (doc is None or info["rig"] != rig or info["root"] != "identity" or info["variant"]
                or info["kind"] not in ("probes", "targets")):
            continue
        hips = next(row["node"] for row in doc["capture"]["humanoid"]["bones"] if row["name"] == "Hips")
        readbacks = doc["spec"]["evidence"]["editor"].get("body_samples") or []
        pairs += [(sample["pose"]["transforms"][hips], readback)
                  for sample, readback in zip(doc["capture"]["samples"], readbacks)]
    replay.require(pairs, f"no recorded {rig} pose with read-back bounds the planned hips")
    return pairs


def muscle_map(capture, bones):
    """{spine bone: (q_0, [e_x, e_y, e_z], [(lower, upper) limit per axis], [muscle index per axis])} from a probe
    case's capture and its single-muscle probes, for anim_body_design's range prediction."""
    rows = capture["humanoid"]["muscles"]
    names = [row["name"] for row in rows]
    custom = {row["human"]: row for row in capture["humanoid"]["limits"] if not row["defaults"]}
    recorded = [(sample, [replay.f32_from_bits(bits) for bits in sample["muscles_f32"]])
                for sample in capture["samples"]]
    neutral = next(sample for sample, values in recorded if not any(values))
    out = {}
    for bone, group in PROBE_GROUPS:
        if bone not in RANGE_BONES or bone not in bones:
            continue
        rest = decode(neutral["pose"]["transforms"][bones[bone]]["rotation"])
        axes, limits, muscles = [None] * 3, [None] * 3, [None] * 3
        for value, muscle in zip(PROBE_VALUES, group):
            i = names.index(muscle)
            axis = rows[i]["axis"]
            own = custom.get(bone)
            limit = tuple(replay.f32_from_bits(own[key][axis] if own else rows[i][key]) for key in ("min", "max"))
            probe = replay.f32_from_bits(replay.f32_bits(value))
            sample = next((sample for sample, values in recorded
                           if values[i] == probe and sum(1 for v in values if v) == 1), None)
            replay.require(sample is not None, f"no single-muscle probe of {muscle} recorded")
            turn = multiply((-rest[0], -rest[1], -rest[2], rest[3]),
                            decode(sample["pose"]["transforms"][bones[bone]]["rotation"]))
            turn = turn if turn[3] > 0 else tuple(-v for v in turn)
            want = angle(probe, limit)
            replay.require(abs(2 * math.degrees(math.atan2(math.sqrt(sum(v * v for v in turn[:3])), turn[3]))
                               - abs(math.degrees(want))) <= ANGLE_SLACK,
                           f"the probe of {muscle} does not turn by its limit angle")
            axes[axis] = tuple(v / turn[3] / math.tan(want / 2) for v in turn[:3])
            limits[axis], muscles[axis] = limit, i
        out[bone] = (rest, axes, limits, muscles)
    return out


def angle(value, limit):
    """A muscle value's joint angle (radians): times the upper limit, or minus the lower one below zero."""
    return math.radians(value * limit[1] if value >= 0 else -value * limit[0])


def decode(rotation):
    return tuple(replay.f32_from_bits(bits) for bits in rotation)


def spine_rotation(entry, vector):
    """A spine bone's local rotation for the muscle vector, q_0 S T (the module docstring)."""
    rest, axes, limits, muscles = entry
    tx, ty, tz = (math.tan(angle(vector[muscles[k]], limits[k]) / 2) for k in range(3))
    swing = normalize((*(ty * axes[1][k] + tz * axes[2][k] for k in range(3)), 1.))
    twist = normalize((*(tx * axes[0][k] for k in range(3)), 1.))
    return normalize(multiply(multiply(rest, swing), twist))


def predict(name, source, hips=None):
    """A pending-recording stand-in for the planned extra probe case `name`, from `source`, the rig's
    recorded probe case, with `hips` (a recorded hips transform and read-back) on every sample, or the
    neutral sample's."""
    info = BODY_INFO[name]
    capture = source["capture"]
    names = [row["name"] for row in capture["humanoid"]["muscles"]]
    bones = {row["name"]: row["node"] for row in capture["humanoid"]["bones"] if row["node"] >= 0}
    index = {muscle: i for i, muscle in enumerate(names)}
    groups = {bone: [index[muscle] for muscle in group] for bone, group in PROBE_GROUPS if bone in bones}
    recorded = [(sample, [replay.f32_from_bits(bits) for bits in sample["muscles_f32"]]) for sample in capture["samples"]]
    replay.require(all(len(values) == len(names) for _, values in recorded), f"{name}: the probe case lacks muscles")
    neutral = next((sample for sample, values in recorded if not any(values)), None)
    replay.require(neutral is not None, f"{name}: the probe case has no neutral sample")
    editor = source["spec"]["evidence"]["editor"]
    at = capture["samples"].index(neutral)
    hips_transform, readback = hips or (neutral["pose"]["transforms"][bones["Hips"]], editor["body_samples"][at])
    probed = {i for muscles in groups.values() for i in muscles}
    plan = probe_schedule(info["rig"], names, False, name)
    spine = muscle_map(capture, bones) if info["variant"] == "range" else {}
    samples = []
    for t, vector in plan:
        replay.require(not any(v for i, v in enumerate(vector) if i not in probed),
                       f"{name}: a planned muscle is off the rig's probe groups")
        pose = copy.deepcopy(neutral["pose"])
        pose["transforms"][bones["Hips"]] = copy.deepcopy(hips_transform)
        for bone, entry in spine.items():
            pose["transforms"][bones[bone]]["rotation"] = [replay.f32_bits(v) for v in spine_rotation(entry, vector)]
        for bone, muscles in groups.items():
            if bone in spine:
                continue
            want = [replay.f32_bits(vector[i]) for i in muscles]
            match = next((sample for sample, _ in recorded
                          if [sample["muscles_f32"][i] for i in muscles] == want), None)
            replay.require(match is not None, f"{name}: no recorded pose of {bone} for the planned muscles at {t}")
            if bone != "Hips":
                pose["transforms"][bones[bone]]["rotation"] = list(match["pose"]["transforms"][bones[bone]]["rotation"])
        samples.append(dict(local=t, local_f32=replay.f32_bits(t), source=t, effective_source=t, sides=[],
                            muscles_f32=[replay.f32_bits(v) for v in vector], pose=pose))
    doc = copy.deepcopy(source)
    doc["spec"]["schedule"] = [dict(local=t, muscles=list(vector)) for t, vector in plan]
    doc["capture"]["samples"] = samples
    editor = doc["spec"]["evidence"]["editor"]
    editor["body_samples"] = [copy.deepcopy(readback) for _ in plan]
    editor["body_targets"] = [copy.deepcopy(editor["body_targets"][at]) for _ in plan]
    editor["body_case"] = [dict(editor["body_case"][0], variant=info["variant"])]
    return doc


def planned(documents):
    """Every extra probe case or range sweep not recorded in `documents` whose rig's probe case is, every
    scheduled sample with read-back: one predicted recording per envelope pair. Any other stays unrecorded,
    and the checkpoint names it missing."""
    out = {}
    for name, info in BODY_INFO.items():
        if info["variant"] in ("extra", "range") and name not in documents:
            source = documents.get(primary_probes(info["rig"]))
            if source is None or not whole(source, info["rig"]):
                continue
            out[name] = [predict(name, source, hips) for hips in envelope(documents, info["rig"])]
    return out


def whole(doc, rig):
    """Whether a probe case holds its rig's every scheduled sample, each with read-back."""
    names = [row["name"] for row in doc["capture"]["humanoid"]["muscles"]]
    readbacks = doc["spec"]["evidence"]["editor"].get("body_samples") or []
    have = {sample["local"] for sample, _ in zip(doc["capture"]["samples"], readbacks)}
    return all(t in have for t, _ in probe_schedule(rig, names, False))


def conservative(variants):
    """One row per planned sample from its rows under every envelope pair: the first pair's columns and
    Jacobian (exact up to a turn of the block), the largest epsilon and E_eval,s, the smallest E_unity,s."""
    out = []
    for rows in zip(*variants, strict=True):
        replay.require(len({row["local"] for row in rows}) == 1 and all(row["epsilon"] is not None for row in rows),
                       "planned envelope rows disagree")
        out.append(dict(rows[0], epsilon=max(row["epsilon"] for row in rows),
                        unity=min(row["unity"] for row in rows),
                        evaluation=max(row["evaluation"] for row in rows)))
    return out
