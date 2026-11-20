"""Each round 11 clip's declared scope, checked against freeze v8's frozen limits (docs/plans/humanoid-body.md, B5b).

B5b needs complete replays that pass with the frozen refusals enforced, and replays it must refuse. Every round
11 clip declares which it is (anim_fixture_round11.ROUND11_INFO's `scope`):

- `in`: every frame inside the frozen scope. Each spine Left-Right curve's authored keys are within the freeze's
  `scope.lateral_limit` (the curves are polylines, so the keys bound the whole curve), and every recorded
  sample's shoulder-hip twist (anim_body_model.shoulder_hip_twist on the recorded pose) is within its
  `bounds.twist_limit`.
- `refusal`: refusal evidence. Some spine Left-Right key is over the lateral limit, so the clip's frames there
  are outside the frozen scope.

The limits are read from the freeze artifact itself (anim_body_freeze_v8.json), never restated here.
"""

import json
from pathlib import Path

from anim_body_model import LATERAL_MUSCLES, Refused, shoulder_hip_twist
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


def check_scope(name, declared, doc, authored, replay):
    """A clip's declared scope against the frozen limits; refuses a clip whose curves or recorded poses
    disagree with its declaration."""
    replay.require(declared in SCOPES, f"{name} declares no scope")
    lateral = max(abs(v) for v in lateral_keys(authored, replay))
    if declared == "refusal":
        replay.require(lateral > LATERAL_LIMIT, f"{name} is declared refusal evidence but its spine Left-Right "
                                                f"curves stay within the frozen limit {LATERAL_LIMIT}")
        return
    replay.require(lateral <= LATERAL_LIMIT,
                   f"{name} is declared in scope but a spine Left-Right key is {lateral} over {LATERAL_LIMIT}")
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
