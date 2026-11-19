"""B1's predictive adequacy: planned extra probe cases from a rig's recorded geometry.

An extra probe vector (anim_fixture_body.EXTRA_PROBES) gives each bone the
muscles of one of the rig's own recorded probe vectors and zero elsewhere.
A bone's local rotation depends only on its own muscles, so each bone of a
planned pose takes the local rotation Unity recorded for the same muscle
values in the rig's probe case, bit for bit. The hips keep the neutral
anchor's transform: a turn of the whole body turns each fitting block and
its Jacobian alike, and leaves u_s unchanged. Each planned sample's body
target and read-back stand in by the probe case's neutral sample. They set
only the magnitudes in epsilon and E_unity, at the recorded scale.

The prediction holds the design, rig geometry and schedule, to the
checkpoint's own derivation. It cannot hold Unity's outputs, which do not
enter u_s or E_unity, so a recorded run of the planned case is still the
checkpoint of record.
"""

import copy

import anim_replay as replay
from anim_fixture_body import BODY_INFO, PROBE_GROUPS, probe_schedule


def primary_probes(rig):
    return next(name for name, info in BODY_INFO.items()
                if info["kind"] == "probes" and info["rig"] == rig and not info["variant"])


def predict(name, source):
    """A pending-recording stand-in for the planned extra probe case `name`, from `source`, the rig's
    recorded probe case."""
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
    probed = {i for muscles in groups.values() for i in muscles}
    plan = probe_schedule(info["rig"], names, False, name)
    samples = []
    for t, vector in plan:
        replay.require(not any(v for i, v in enumerate(vector) if i not in probed),
                       f"{name}: a planned muscle is off the rig's probe groups")
        pose = copy.deepcopy(neutral["pose"])
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
    at = capture["samples"].index(neutral)
    editor["body_samples"] = [copy.deepcopy(editor["body_samples"][at]) for _ in plan]
    editor["body_targets"] = [copy.deepcopy(editor["body_targets"][at]) for _ in plan]
    editor["body_case"] = [dict(editor["body_case"][0], variant="extra")]
    return doc


def planned(documents):
    """Every extra probe case not recorded in `documents`, predicted from its rig's probe case."""
    out = {}
    for name, info in BODY_INFO.items():
        if info["variant"] == "extra" and name not in documents:
            source = documents.get(primary_probes(info["rig"]))
            replay.require(source is not None, f"{name}: its rig's probe case is not recorded")
            out[name] = predict(name, source)
    return out
