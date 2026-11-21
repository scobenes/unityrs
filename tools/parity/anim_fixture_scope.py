"""Each round 11 clip's declared scope, checked against freeze v8's frozen limits (docs/plans/humanoid-body.md, B5b).

B5b needs complete replays that pass with the frozen refusals enforced, and replays it must refuse. Every round
11 clip declares which it is (anim_fixture_round11.ROUND11_INFO's `scope`):

- `in`: every frame inside the frozen scope. Each spine Left-Right curve's authored keys are within the freeze's
  `scope.lateral_limit`, and so is its compiled value, as playback reads it (the built clip's slot at the
  muscle's Animator attribute), at every recorded sample's time; and every recorded sample's shoulder-hip twist
  (anim_body_model.shoulder_hip_twist on Unity's recorded pose) is within its `bounds.twist_limit`. The twist is
  a completion check on what Unity recorded, never a prediction.
- `refusal`: refusal evidence. Some spine Left-Right key is over the lateral limit, and so is some compiled
  value at a recorded sample's time, so Unity played frames outside the frozen scope.

A muscle without a compiled slot plays as 0 (E5's Spine-only clips curve the Spine's alone).

The limits are read from the freeze artifact itself (anim_body_freeze_v8.json), never restated here.
"""

import json
from pathlib import Path

from anim_body_model import LATERAL_MUSCLES, Refused, shoulder_hip_twist
from anim_fixture_body import MUSCLE_ATTRIBUTE, _runtime_slots
from anim_body_tolerance import FRAME_BONES
from anim_reference import multiply

FREEZE = json.loads((Path(__file__).resolve().parent / "anim_body_freeze_v8.json").read_text(encoding="utf-8"))
LATERAL_LIMIT = FREEZE["scope"]["lateral_limit"]
TWIST_LIMIT = FREEZE["bounds"]["twist_limit"]
SCOPES = ("in", "refusal")


def _rotate(q, v):
    x, y, z, w = q
    return multiply(multiply(q, (*v, 0.)), (-x, -y, -z, w))[:3]


def world_positions(nodes, transforms, replay):
    """Each node's world position from its recorded local transforms, parents by address."""
    index = {node["address"]: k for k, node in enumerate(nodes)}
    replay.require(len(transforms) == len(nodes), "recorded pose does not cover the nodes")
    world = []
    for k, node in enumerate(nodes):
        row = transforms[k]
        t, q, s = ([replay.f32_from_bits(v) for v in row[key]] for key in ("translation", "rotation", "scale"))
        address = node["address"]
        if address == "":
            world.append((t, q, s))
            continue
        parent = index.get(address.rpartition("/")[0] if "/" in address else "")
        replay.require(parent is not None and parent < k, f"node {address!r} has no earlier parent")
        pt, pq, ps = world[parent]
        moved = _rotate(pq, [a * b for a, b in zip(ps, t)])
        world.append(([a + b for a, b in zip(pt, moved)], multiply(pq, q), [a * b for a, b in zip(ps, s)]))
    return [position for position, _, _ in world]


def lateral_keys(authored, replay):
    """Every spine Left-Right curve's authored key values; a muscle without a curve stays at 0 (E5's Spine-only
    clips curve the Spine's alone)."""
    return [0., *(replay.f32_from_bits(key["value_f32"]) for prop in LATERAL_MUSCLES
                  for key in authored.get(prop, {}).get("keys") or [])]


def lateral_track(runtime, index, attribute, replay):
    """A spine Left-Right muscle's compiled track, or None for a muscle the clip does not curve (`m_IndexArray`
    -1). A malformed slot refuses: no index entry, a slot outside the runtime table or bound to another
    attribute, or a table whose slots cannot be told apart."""
    replay.require(isinstance(index, list) and 0 <= attribute < len(index) and type(index[attribute]) is int,
                   f"the built clip's m_IndexArray has no entry for Animator attribute {attribute}")
    slot = index[attribute]
    if slot == -1:
        return None
    replay.require(0 <= slot < len(runtime) and runtime[slot][0] == (0, 95, attribute, 8, False)
                   and runtime[slot][1] is not None,
                   f"the built clip's spine Left-Right slot {slot} for Animator attribute {attribute} is malformed")
    return runtime[slot][1]


def compiled_lateral(doc, clip, replay):
    """The largest compiled spine Left-Right magnitude at each recorded sample's time, on its source clock."""
    capture = doc["capture"]
    names = [row["name"] for row in capture["humanoid"]["muscles"]]
    replay.require(all(name in names for name in LATERAL_MUSCLES), "the capture lacks a spine Left-Right muscle")
    runtime = _runtime_slots(clip, replay)
    index = clip.get("m_MuscleClip", {}).get("m_IndexArray")
    tracks = [lateral_track(runtime, index, MUSCLE_ATTRIBUTE + names.index(name), replay) for name in LATERAL_MUSCLES]
    rows = []
    for sample in capture["samples"]:
        time = sample.get("effective_source", sample["local"])
        replay.require(type(time) in (int, float), "a sample without a source time")
        rows.append((time, max(abs(track(time)) if track is not None else 0. for track in tracks)))
    return rows


def check_scope(name, declared, doc, authored, clip, replay):
    """A clip's declared scope against the frozen limits; refuses a clip whose curves, compiled values or
    recorded poses disagree with its declaration."""
    replay.require(declared in SCOPES, f"{name} declares no scope")
    lateral = max(abs(v) for v in lateral_keys(authored, replay))
    compiled = compiled_lateral(doc, clip, replay)
    if declared == "refusal":
        replay.require(lateral > LATERAL_LIMIT, f"{name} is declared refusal evidence but its spine Left-Right "
                                                f"curves stay within the frozen limit {LATERAL_LIMIT}")
        replay.require(any(value > LATERAL_LIMIT for _, value in compiled),
                       f"{name} is declared refusal evidence but its compiled spine Left-Right values stay within "
                       f"the frozen limit {LATERAL_LIMIT} at every recorded sample")
        return
    replay.require(lateral <= LATERAL_LIMIT,
                   f"{name} is declared in scope but a spine Left-Right key is {lateral} over {LATERAL_LIMIT}")
    for time, value in compiled:
        replay.require(value <= LATERAL_LIMIT, f"{name} is declared in scope but its compiled spine Left-Right "
                                               f"value at {time} is {value}, over {LATERAL_LIMIT}")
    capture = doc["capture"]
    nodes = {row["name"]: row["node"] for row in capture["humanoid"]["bones"]}
    replay.require(all(nodes.get(bone, -1) >= 0 for bone in FRAME_BONES), f"{name} lacks a frame bone")
    for sample in capture["samples"]:
        positions = world_positions(doc["spec"]["nodes"], sample["pose"]["transforms"], replay)
        try:
            twist = shoulder_hip_twist({bone: tuple(positions[nodes[bone]]) for bone in FRAME_BONES})
        except Refused as error:
            replay.require(False, f"{name} at {sample['local']}: {error}")
        replay.require(twist <= TWIST_LIMIT, f"{name} is declared in scope but its shoulder-hip twist at "
                                             f"{sample['local']} is {twist} rad, over {TWIST_LIMIT}")
