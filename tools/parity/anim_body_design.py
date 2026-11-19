"""B1's predictive adequacy: planned extra probe cases from a rig's recorded geometry.

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
"""

import copy

import anim_replay as replay
from anim_fixture_body import BODY_INFO, HOLDOUT, PROBE_GROUPS, primary_probes, probe_schedule


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
    samples = []
    for t, vector in plan:
        replay.require(not any(v for i, v in enumerate(vector) if i not in probed),
                       f"{name}: a planned muscle is off the rig's probe groups")
        pose = copy.deepcopy(neutral["pose"])
        pose["transforms"][bones["Hips"]] = copy.deepcopy(hips_transform)
        for bone, muscles in groups.items():
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
    editor["body_case"] = [dict(editor["body_case"][0], variant="extra")]
    return doc


def planned(documents):
    """Every extra probe case not recorded in `documents`: one predicted recording per envelope pair."""
    out = {}
    for name, info in BODY_INFO.items():
        if info["variant"] == "extra" and name not in documents:
            source = documents.get(primary_probes(info["rig"]))
            replay.require(source is not None, f"{name}: its rig's probe case is not recorded")
            out[name] = [predict(name, source, hips) for hips in envelope(documents, info["rig"])]
    return out


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
