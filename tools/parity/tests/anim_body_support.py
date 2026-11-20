"""Synthetic body-fixture recordings: the builder's rigs, their bone table and read-back rows.

Every value is synthetic. The read-back follows the centre-of-mass hypothesis
of docs/plans/humanoid-body.md 2.1 with one chosen coefficient per edge, so
the adequacy checkpoint sees a consistent fit.
"""

import copy
import struct

from test_anim_replay import bits

import anim_body_tolerance as tolerance  # noqa: E402  (test_anim_replay puts it on the path)
from anim_fixture_body import (BODY_INFO, BODY_PATH, BODY_TIMES, BODY_TURN, CLIP_VARIANTS,  # noqa: E402
                               body_requests, distinguishing_curves, PROBE_GROUPS, SPINE,
                               START_OFFSET, START_YAW, expected_case_row, probe_schedule, schedule_name)
from anim_fixture_rigs import rig_layout  # noqa: E402
from anim_reference import euler, multiply  # noqa: E402
import anim_replay as replay  # noqa: E402

# HumanBodyBones 0-23, 30 finger slots, then UpperChest, with HumanTrait's parents.
BODY = tolerance.NAMES[:24]
PARENT = {"Hips": None, "LeftUpperLeg": "Hips", "RightUpperLeg": "Hips", "LeftLowerLeg": "LeftUpperLeg",
          "RightLowerLeg": "RightUpperLeg", "LeftFoot": "LeftLowerLeg", "RightFoot": "RightLowerLeg",
          "Spine": "Hips", "Chest": "Spine", "UpperChest": "Chest", "Neck": "UpperChest", "Head": "Neck",
          "LeftShoulder": "UpperChest", "RightShoulder": "UpperChest", "LeftUpperArm": "LeftShoulder",
          "RightUpperArm": "RightShoulder", "LeftLowerArm": "LeftUpperArm", "RightLowerArm": "RightUpperArm",
          "LeftHand": "LeftLowerArm", "RightHand": "RightLowerArm", "LeftToes": "LeftFoot", "RightToes": "RightFoot",
          "LeftEye": "Head", "RightEye": "Head", "Jaw": "Head"}
BONES = (*BODY, *(f"Finger {i}" for i in range(30)), "UpperChest")
# Each body slot's own default mass, exact in f32 so its hierarchy sums are too.
# The Eyes and the Jaw weigh nothing, as in every recorded Avatar.
MASSES = {name: 0. if name in ("LeftEye", "RightEye", "Jaw") else (2 + i % 7) / 128
          for i, name in enumerate(tolerance.NAMES)}


def hierarchy_masses(parents):
    """HumanTrait.GetBoneDefaultHierarchyMass, as the recorder writes it: own mass plus the descendants'."""
    total = dict.fromkeys(parents, 0.)
    for name in parents:
        bone = name
        while bone is not None:
            total[bone] += MASSES.get(name, 0.)
            bone = parents[bone]
    return total
PROBE_MUSCLES = tuple(muscle for _, group in PROBE_GROUPS for muscle in group)
MUSCLES = (*PROBE_MUSCLES, *(f"Other muscle {i}" for i in range(95 - len(PROBE_MUSCLES))))
COEFFICIENT = .4
# A stand-in for the muscle-to-rotation map: each probe muscle turns its bone
# about one local axis, by its value times its default limit in degrees
# (HumanTrait.GetMuscleDefaultMin below zero, GetMuscleDefaultMax above).
MUSCLE_LIMITS = {
    "Spine": (-40, 40), "Chest": (-40, 40), "UpperChest": (-20, 20), "Neck": (-40, 40), "Head": (-40, 40),
    "Upper Leg Front-Back": (-90, 50), "Upper Leg In-Out": (-60, 60), "Lower Leg Stretch": (-80, 80),
    "Foot Up-Down": (-50, 50), "Toes Up-Down": (-50, 50), "Shoulder Down-Up": (-15, 30),
    "Shoulder Front-Back": (-15, 15), "Arm Down-Up": (-60, 100), "Arm Front-Back": (-100, 100),
    "Forearm Stretch": (-80, 80), "Hand Down-Up": (-80, 80), "Hand In-Out": (-40, 40),
    "Eye Down-Up": (-10, 15), "Eye In-Out": (-20, 20), "Jaw": (-10, 10),
}
GROUP_AXES = {3: "XZY", 2: "XZ", 1: "X"}
ARM_AXES = {"Shoulder": "ZY", "UpperArm": "ZY", "LowerArm": "Y", "Hand": "ZY", "Eye": "XY", "Jaw": "XY"}
# The builder's T-pose (UnityrsBodyFixtures.cs BodyLayout) as (name, parent, offset).
layout = rig_layout


def transform(t=(0, 0, 0), q=(0, 0, 0, 1), s=1.):
    return dict(translation=[bits(v) for v in t], rotation=[bits(v) for v in q], scale=[bits(s)] * 3)


ANIMATOR_SCALE = {"scaled-animator": 1.25}


def rig_nodes(rig):
    """Spec nodes and rest transforms, with F5's Animator, armature and hips scales."""
    nodes, rest = [("", "HumanoidFixture"), ("0", "HumanoidRig")], [transform(), transform(
        s=ANIMATOR_SCALE.get(rig, 1.))]
    top, addresses, counts = "0", {"": "0"}, {}
    if rig == "armature":
        nodes.append(("0/0", "Armature"))
        rest.append(transform((0, .05, .1), euler((-90, 0, 0), 4), .8))
        top = addresses[""] = "0/0"
    for name, parent, offset in layout(rig):
        base = addresses[parent]
        address = f"{base}/{counts.setdefault(base, 0)}"
        counts[base] += 1
        addresses[name] = address
        nodes.append((address, name))
        if name == "Hips" and rig == "armature":
            rest.append(transform((0, .125, .8375), euler((90, 0, 0), 4)))
        elif name == "Hips" and rig == "hips-scale":
            rest.append(transform(tuple(1.2 * v for v in offset), s=1.2))
        else:
            rest.append(transform(offset))
    return [dict(address=a, name=n) for a, n in nodes], rest, top


def muscle_axes():
    """Each probe muscle's bone and axis."""
    axes = {}
    for bone, group in PROBE_GROUPS:
        part = next((key for key in ARM_AXES if bone.endswith(key)), None)
        letters = ARM_AXES[part] if part else GROUP_AXES[len(group)]
        axes.update({muscle: (bone, "XYZ".index(axis)) for muscle, axis in zip(group, letters)})
    return axes


AXES = muscle_axes()
# The recorder's axis of each three-muscle group's muscles: Front-Back z, Left-Right y, Twist x.
MUSCLE_SLOTS = {muscle: slot for _, group in PROBE_GROUPS if len(group) == 3 for muscle, slot in zip(group, (2, 1, 0))}


def limits(muscle):
    """A muscle's default (min, max) in degrees."""
    name = muscle.removeprefix("Left ").removeprefix("Right ")
    return next(value for key, value in MUSCLE_LIMITS.items() if name.startswith(key + " ") or name == key)


def spine_vector(t):
    """The P2b known vectors' spine muscles along a clip, as an Editor clip plays them."""
    alpha = 2 * t - 1
    return (t, 0, 0) if t <= .5 else (.5 - .75 * alpha, .375 * alpha, -.5 * alpha)


def pose_rotations(vector):
    """Local rotations by bone from a muscle vector over MUSCLES."""
    angles = {}
    for muscle, value in zip(MUSCLES, vector):
        if value and muscle in AXES:
            bone, axis = AXES[muscle]
            turn = angles.setdefault(bone, [0., 0., 0.])
            turn[axis] += value * abs(limits(muscle)[value > 0])
    return {bone: euler(turn, 4) for bone, turn in angles.items()}


def world_nodes(nodes, pose, human_bones):
    """Unity's world transform of every node, and its position and rotation about the hips."""
    frames = tolerance.kinematics(nodes, pose, replay.f32_from_bits, "", include_base=True)
    hips = frames[nodes[next(row["node"] for row in human_bones if row["name"] == "Hips")]["address"]]
    inverse = (*(-v for v in hips.rotation[:3]), hips.rotation[3])
    rows = []
    for node in nodes:
        frame = frames[node["address"]]
        about = tolerance.rotate(inverse, tolerance.sub(frame.position, hips.position))
        rows.append(dict(position=[bits(v) for v in frame.position], rotation=[bits(v) for v in frame.rotation],
                         scale=[bits(frame.scale)] * 3, hips_position=[bits(v / hips.scale) for v in about],
                         hips_rotation=[bits(v) for v in multiply(inverse, frame.rotation)]))
    return rows


def body_doc(name, run, *, samples=None):
    """A pending body recording posed by its muscle vectors, with consistent read-back rows.

    `samples` limits the count."""
    info = BODY_INFO[name]
    rig, kind = info["rig"], info["kind"]
    nodes, rest, _ = rig_nodes(rig)
    index = {node["name"]: i for i, node in enumerate(nodes)}
    if kind == "clip":
        spine = [MUSCLES.index(muscle) for muscle in SPINE]
        schedule = [(t, None) for t in (0, .25 * 391 / 1024, .25, .5, .75, 1)]
    else:
        schedule = probe_schedule(rig, MUSCLES, kind == "targets", schedule_name(name))
    requests = body_requests(name, len(schedule))
    if samples:
        schedule = schedule[:samples]
    human_bones = [dict(name=b, node=index.get(b, -1), required=b in ("Hips", "Spine")) for b in BONES]
    parents = {b: PARENT.get(b) for b in BONES}
    hierarchy = hierarchy_masses(parents)
    table = [dict(name=b, parent=BONES.index(parents[b]) if parents[b] else -1, mass=bits(hierarchy[b]),
                  required=row["required"]) for b, row in zip(BONES, human_bones)]
    masses = {b: MASSES[b] for b in tolerance.BODY_SLOTS}
    rows, samples_out, targets = [], [], []

    def frame(transforms):
        bones = tolerance.bone_frames(human_bones, nodes, tolerance.kinematics(nodes, transforms, replay.f32_from_bits))
        return tolerance.body_frame({b: bone.position for b, bone in bones.items()})

    rest_frame = frame(rest)
    for t, vector in schedule:
        pose = copy.deepcopy(rest)
        muscles = vector
        if muscles is None:
            muscles = [0.] * len(MUSCLES)
            for i, value in zip(spine, spine_vector(t)):
                muscles[i] = value
        for bone, rotation in pose_rotations(muscles).items():
            if bone in index:
                pose[index[bone]]["rotation"] = [bits(v) for v in rotation]

        def centre():
            """c in A, the Animator's frame without its scale."""
            frames = tolerance.kinematics(nodes, pose, replay.f32_from_bits, "0", include_base=False)
            bones = tolerance.bone_frames(human_bones, nodes, frames)
            weights = tolerance.folded_masses(masses, parents, set(bones))
            c = [sum(weights[b] * bones[b].position[k] for b in tolerance.ordered(bones)) for k in range(3)]
            for i, j in tolerance.edges(parents, set(bones)):
                for k in range(3):
                    c[k] += COEFFICIENT * weights[i] * (bones[j].position[k] - bones[i].position[k])
            return c

        # The model's body rotation, F(pose) F(T)^-1 (2.2), as Unity's read-back. Unity's world body
        # position is free of the Animator's scale (the Animator sits at the origin here), and the
        # recorder's conversion to the Animator divides by that scale again.
        turn = tolerance.matrix_quaternion(tolerance.product(frame(pose), rest_frame, transpose=True))
        body = centre()
        rows.append(dict(readback_position=[bits(v / ANIMATOR_SCALE.get(rig, 1.)) for v in body],
                         readback_rotation=[bits(v) for v in turn],
                         readback_muscles=[bits(v) for v in muscles],
                         world_position=[bits(v) for v in body], world_rotation=[bits(0)] * 3 + [bits(1)],
                         nodes=world_nodes(nodes, pose, human_bones),
                         **dict.fromkeys(("root_position", "root_rotation",
                                          "motion_position", "motion_rotation", "motion_delta_position",
                                          "motion_delta_rotation"), [bits(0)] * 3 if kind == "clip" else [])))
        if kind == "clip":
            for field in ("root_rotation", "motion_rotation", "motion_delta_rotation"):
                rows[-1][field] = [bits(0)] * 3 + [bits(1)]
        else:
            offset, tilt = requests[len(targets)]
            targets.append(dict(offset=[bits(v) for v in offset], tilt=[bits(v) for v in tilt],
                                position=[bits(v + (k == 1)) for k, v in enumerate(offset)],
                                rotation=[bits(v) for v in euler(tilt, 4)]))
        samples_out.append(dict(local=t, local_f32=bits(t), source=t, effective_source=t, sides=[],
                                muscles_f32=[bits(v) for v in vector] if vector else [],
                                pose=dict(transforms=pose, weights=[])))
    muscle_rows = [dict(name=m, bone=0, axis=MUSCLE_SLOTS.get(m, 0),
                        **dict(zip(("min", "max"), map(bits, limits(m) if m in AXES else (-40, 40)))))
                   for m in MUSCLES]
    human = dict(bones=human_bones, muscles=muscle_rows, limits=[],
                 human_scale=bits(1), rest_body_position=[bits(0), bits(1), bits(0)],
                 rest_body_rotation=[bits(0)] * 3 + [bits(1)])
    editor = dict(fixture_case=name, run_id=run["run_id"], body_case=[expected_case_row(name, replay)],
                  body_samples=rows, body_targets=targets, human_bones=table, bindings=[])
    spec = dict(nodes=nodes, binding_address="0", mode="animator" if kind == "clip" else "human_pose",
                schedule=[dict(local=t, **({"muscles": list(v)} if v else {})) for t, v in schedule],
                evidence=dict(editor=editor))
    capture = dict(humanoid=human, rest=dict(transforms=rest, weights=[]), samples=samples_out,
                   clip_settings={})
    return dict(format="unityrs.anim_replay_pending", version=1, spec=spec, capture=capture)


def keys(times, values):
    return [dict(time_f32=bits(t), value_f32=bits(v), in_tangent_f32=bits(0), out_tangent_f32=bits(0),
                 in_weight_f32=bits(0), out_weight_f32=bits(0), weighted_mode="None") for t, v in zip(times, values)]


def animator(prop, rows):
    return dict(path="", property=prop, type="UnityEngine.Animator", discrete=False, object_reference=False,
                object_keys=[], keys=rows)


def clip_case(doc, variant_name):
    """Give a clip case the authored curves, captured settings and built clip of its variant."""
    variant = CLIP_VARIANTS[variant_name]
    turn = euler((0, START_YAW if variant["start"] else 0, 0), 4)
    path, rotations = [], []
    for point, angles in zip(BODY_PATH, BODY_TURN):
        x, y, z = point
        c, s = turn[3] ** 2 - turn[1] ** 2, 2 * turn[1] * turn[3]
        moved = (c * x + s * z, y, -s * x + c * z)
        path.append(tuple(a + b for a, b in zip(moved, START_OFFSET)) if variant["start"] else moved)
        rotations.append(multiply(turn, euler(angles, 4)))
    bindings = [animator(f"RootT.{a}", keys(BODY_TIMES, [p[i] for p in path])) for i, a in enumerate("xyz")]
    bindings += [animator(f"RootQ.{a}", keys(BODY_TIMES, [q[i] for q in rotations])) for i, a in enumerate("xyzw")]
    extras = distinguishing_curves(variant, list(MUSCLES))
    bindings += [animator(prop, keys(times, values)) for group in extras for prop, _, times, values in group]
    doc["spec"]["evidence"]["editor"]["bindings"] = bindings
    flags = dict(zip(("loopBlendOrientation", "loopBlendPositionY", "loopBlendPositionXZ"), variant["bake"]))
    flags |= dict(zip(("keepOriginalOrientation", "keepOriginalPositionY", "keepOriginalPositionXZ"), variant["keep"]))
    doc["capture"]["clip_settings"] = flags | dict(
        heightFromFeet=variant["height_from_feet"], loopTime=False, loopBlend=False, mirror=False,
        level=bits(variant["level"]), orientationOffsetY=bits(variant["orientation"]), cycleOffset=bits(0),
        startTime=bits(0), stopTime=bits(1))
    built = dict(zip(("m_LoopBlendOrientation", "m_LoopBlendPositionY", "m_LoopBlendPositionXZ"), variant["bake"]))
    built |= dict(zip(("m_KeepOriginalOrientation", "m_KeepOriginalPositionY", "m_KeepOriginalPositionXZ"),
                      map(int, variant["keep"])))
    body = [(7 + i, BODY_TIMES, [p[i] for p in path]) for i in range(3)]
    body += [(10 + i, BODY_TIMES, [q[i] for q in rotations]) for i in range(4)]
    stream = compiled(extras, body)
    start = dict(t=dict(zip("xyz", (f32(v) for v in path[0]))), q=dict(zip("xyzw", (f32(v) for v in rotations[0]))),
                 s=dict(x=1., y=1., z=1.))
    return {"m_MuscleClip": built | dict(m_HeightFromFeet=variant["height_from_feet"], m_Level=variant["level"],
                                         m_OrientationOffsetY=variant["orientation"], m_StartAtOrigin=True,
                                         m_StartX=start, m_Clip=stream["m_Clip"], m_IndexArray=stream["m_IndexArray"]),
            "m_ClipBindingConstant": stream["m_ClipBindingConstant"]}


def f32(value):
    return struct.unpack(">f", struct.pack(">f", value))[0]


def hermite(times, values):
    """Streamed keys (time, (c3, c2, c1, c0)) of a curve through `values` with zero tangents, as f32."""
    keys = []
    for k, (t, v) in enumerate(zip(times, values)):
        if k + 1 < len(times):
            dt, dv = times[k + 1] - t, values[k + 1] - v
            keys.append((t, (f32(-2 * dv / dt ** 3), f32(3 * dv / dt ** 2), 0., f32(v))))
        else:
            keys.append((t, (0., 0., 0., f32(v))))
    return keys

def word(value):
    return int(bits(value), 16)


def compiled(extras, body=None):
    """The built clip's streamed slots: the body's seven (`body`'s curves, or held at zero), then each
    distinguishing curve, each a Hermite cubic through its keys with zero tangents."""
    slots = body or [(a, (0, 1), (0, 0)) for a in range(7, 14)]
    slots += [(attribute, times, values) for group in extras for _, attribute, times, values in group]
    frames = {}
    for slot, (_, times, values) in enumerate(slots):
        for t, coefficients in hermite(times, values):
            frames.setdefault(t, []).append((slot, *(word(c) for c in coefficients)))
    data = []
    for t in sorted(frames):
        data += [word(t), len(frames[t])]
        for entry in frames[t]:
            data += list(entry)
    index = [-1] * 200
    for slot, (attribute, _, _) in enumerate(slots):
        index[attribute] = slot
    return {"m_ClipBindingConstant": {"genericBindings": [
        dict(path=0, typeID=95, attribute=attribute, customType=8, isPPtrCurve=0, isIntCurve=0)
        for attribute, _, _ in slots]}, "m_IndexArray": index, "m_Clip": {"data": {
            "m_StreamedClip": {"curveCount": len(slots), "data": data + [0x7f800000, 0]},
            "m_DenseClip": {"m_CurveCount": 0}, "m_ConstantClip": {"data": []}}}}
