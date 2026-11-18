"""Synthetic Editor recordings and bundles for the optional coverage fixtures."""

import copy
import math
from zlib import crc32

from anim_fixture_support import ASSETS, ROOT_ID, RUN, archive, game_object, pointer, recording, stored_curves
from test_anim_replay import bits

INTEGER, TWIST, SETTINGS = "integer-discrete-binding", "humanoid-twist-refusal", "humanoid-non-neutral-setting-refusal"
COVERAGE = (INTEGER, TWIST, SETTINGS)
BUNDLES = {INTEGER: "anim-discrete", TWIST: "humanoid-twist", SETTINGS: "humanoid-settings"}
TWIST_VALUES = dict(upper_arm_twist=.5, lower_arm_twist=.25, upper_leg_twist=.75, lower_leg_twist=.375)
AVATAR_TWIST = dict(m_ArmTwist=.5, m_ForeArmTwist=.25, m_UpperLegTwist=.75, m_LegTwist=.375)
# HumanTrait order puts these after the Spine muscles; the index sets the built attribute.
TWIST_MUSCLES = {"Left Upper Leg Twist In-Out": (23, .5), "Left Lower Leg Twist In-Out": (25, -.375),
                 "Left Arm Twist In-Out": (41, .625), "Left Forearm Twist In-Out": (43, -.5)}
LIMBS = ("LeftUpperArm", "LeftLowerArm", "LeftHand", "LeftUpperLeg", "LeftLowerLeg", "LeftFoot")
SPINE = ("Spine Front-Back", "Spine Left-Right", "Spine Twist Left-Right")


def word(value):
    return int(bits(value), 16)


def key_rows(times, values):
    return [dict(time_f32=bits(t), value_f32=bits(v), in_tangent_f32=bits(0), out_tangent_f32=bits(0),
                 in_weight_f32=bits(.25), out_weight_f32=bits(.25), weighted_mode="None")
            for t, v in zip(times, values)]


def turn(degrees):
    half = math.radians(degrees) / 2
    return [bits(math.sin(half)), bits(0), bits(0), bits(math.cos(half))]


def integer_recording(run):
    folder = ASSETS + "discrete/"
    doc = recording("generic-object-reference", BUNDLES[INTEGER], folder + "discrete.prefab",
                    folder + INTEGER + ".anim", "", run)
    spec, capture = doc["spec"], doc["capture"]
    editor = spec["evidence"]["editor"]
    spec["nodes"] = [dict(address="", name="DiscreteRig"), dict(address="0", name="Mötión"),
                     dict(address="1", name="Discrete")]
    spec["renderers"] = []
    for pose in (capture["rest"], *(sample["pose"] for sample in capture["samples"])):
        pose["transforms"] = pose["transforms"][:2] + [copy.deepcopy(capture["rest"]["transforms"][0])]
        pose["weights"] = []
    editor["fixture_case"] = INTEGER
    editor["bindings"] = [row for row in editor["bindings"] if not row["object_reference"]] + [dict(
        path="Discrete", property="m_SortingOrder", type="UnityEngine.MeshRenderer", discrete=True,
        object_reference=False, object_keys=[], keys=key_rows((0, .5), (3, 7)))]
    del editor["object_reference_samples"]
    editor["discrete_samples"] = [dict(time_f32=s["local_f32"], value=3 if s["local"] < .5 else 7)
                                  for s in capture["samples"]]
    return doc


def human_recording(case, run):
    folder = ASSETS + BUNDLES[case] + "/"
    doc = recording("humanoid-compact-muscles", BUNDLES[case], folder + "identity.prefab", folder + case + ".anim",
                    folder + "avatar.asset", run)
    spec, capture = doc["spec"], doc["capture"]
    editor = spec["evidence"]["editor"]
    editor["fixture_case"] = case
    human = capture["humanoid"]
    if case == SETTINGS:
        editor["expected_refusal"] = "unsupported_humanoid_clip_settings"
        capture["clip_settings"].update(level=bits(.125), orientationOffsetY=bits(20))
        return doc
    editor["expected_refusal"] = "unsupported_humanoid_twist"
    human.update({name: bits(value) for name, value in TWIST_VALUES.items()})
    for name, (index, value) in TWIST_MUSCLES.items():
        human["muscles"][index]["name"] = name
        editor["bindings"].append(dict(path="", property=name, type="UnityEngine.Animator", discrete=False,
                                       object_reference=False, object_keys=[], keys=key_rows((0, 1), (0, value))))
    first = len(spec["nodes"])
    spec["nodes"] += [dict(address=f"0/0/{i}", name=bone) for i, bone in enumerate(LIMBS, 1)]
    human["bones"] += [dict(name=bone, node=first + i, required=True) for i, bone in enumerate(LIMBS)]
    rest = capture["rest"]["transforms"][0]
    for pose in (capture["rest"], *(sample["pose"] for sample in capture["samples"])):
        pose["transforms"] += [copy.deepcopy(rest) for _ in LIMBS]
    last = capture["samples"][-1]["pose"]["transforms"]
    for i in range(len(LIMBS)):
        last[first + i]["rotation"] = turn(10)
    editor["twist_effects"] = [dict(bone=bone, node=first + i, authored_f32=turn(10), neutral_f32=turn(0))
                               for i, bone in enumerate(LIMBS)]
    return doc


def prefab_rows(rows, spec, extra=None):
    """The prefab hierarchy, its Animator at the binding address and any skinned renderer."""
    nodes = {node["address"]: (40 + i, 60 + i, node["name"]) for i, node in enumerate(spec["nodes"])}
    nodes[""] = (ROOT_ID, 1, nodes[""][2])
    for address, (transform, obj, name) in nodes.items():
        children = [key for child, (key, _, _) in nodes.items() if child and child.rpartition("/")[0] == address]
        components = [3] if address == spec["binding_address"] else []
        if any(renderer["address"] == address for renderer in spec["renderers"]):
            components.append(7)
            rows.append((7, 137, None, {"m_Mesh": pointer(9)}))
        if extra and name in extra:
            components.append(extra[name][0])
            rows.append(extra[name])
        game_object(rows, obj, transform, name, parent=nodes[address.rpartition("/")[0]][0] if address else 0,
                    children=children, components=components)


def stream(frames):
    data = []
    for time, entries in frames:
        data += [time, len(entries)]
        for slot, *coefficients in entries:
            data += [slot, *coefficients]
    return data + [0x7f800000, 0]


def integer_clip(spec, *, encode=lambda value: value):
    """The 2022.3 layout: three streamed translation slots, then the discrete integer slot."""
    seed = [(0, 0, 0, 0, word(3.)), (1, 0, 0, 0, 0), (2, 0, 0, 0, 0), (3, 0, 0, 0, encode(3))]
    frames = [(0xff7fffff, seed), (0, [(0, 0, 0, word(6.), word(3.)), (1, 0, 0, 0, 0), (2, 0, 0, 0, 0)]),
              (word(.5), [(3, 0, 0, 0, encode(7))]),
              (word(1.), [(0, 0, 0, 0, word(9.)), (1, 0, 0, 0, 0), (2, 0, 0, 0, 0)])]
    return {"m_SampleRate": 16., "m_Legacy": False, "m_MuscleClip": {
        "m_StartTime": 0., "m_StopTime": 1., "m_LoopTime": False, "m_IndexArray": [], "m_Clip": {"data": {
            "m_StreamedClip": {"curveCount": 3, "discreteCurveCount": 1, "data": stream(frames)},
            "m_DenseClip": {"m_CurveCount": 0, "m_FrameCount": 0, "m_BeginTime": 0., "m_SampleRate": 16.,
                            "m_SampleArray": []},
            "m_ConstantClip": {"data": []}}}},
        "m_ClipBindingConstant": {"genericBindings": [
            dict(path=crc32("Mötión".encode()), typeID=4, attribute=1, customType=0, isPPtrCurve=0, isIntCurve=0),
            dict(path=crc32(b"Discrete"), typeID=23, attribute=crc32(b"m_SortingOrder"), customType=0,
                 isPPtrCurve=0, isIntCurve=1)], "pptrCurveMapping": []}}


def human_clip(case, spec):
    muscles = list(range(3)) + ([index for index, _ in TWIST_MUSCLES.values()] if case == TWIST else [])
    entries = [(slot, 0, 0, 0, word(.25)) for slot in range(len(muscles))]
    frames = [(word(t), entries) for t in (0., .5, 1.)]
    offset = case == SETTINGS
    return {"m_SampleRate": 16., "m_Legacy": False, **stored_curves(spec), "m_MuscleClip": {
        "m_StartTime": 0., "m_StopTime": 1., "m_LoopTime": False, "m_CycleOffset": 0., "m_Mirror": False,
        "m_Level": .125 if offset else 0., "m_OrientationOffsetY": 20. if offset else 0.,
        "m_IndexArray": [0, 1, -1], "m_Clip": {"data": {
            "m_StreamedClip": {"curveCount": len(muscles), "data": stream(frames)},
            "m_DenseClip": {"m_CurveCount": 0}, "m_ConstantClip": {"data": []}}}},
        "m_ClipBindingConstant": {"genericBindings": [
            dict(path=0, typeID=95, attribute=42 + index, customType=8, isPPtrCurve=0, isIntCurve=0)
            for index in muscles], "pptrCurveMapping": []}}


def coverage_archives(bundles, inspect, run=RUN, *, clips=None, avatar=None):
    """Write the three coverage bundles; `clips` and `avatar` replace a built value per case."""
    recordings = {INTEGER: integer_recording(run), TWIST: human_recording(TWIST, run),
                  SETTINGS: human_recording(SETTINGS, run)}
    for case, doc in recordings.items():
        spec = doc["spec"]
        assets = {spec["prefab_asset"]: 1, spec["clip_asset"]: 2}
        rows = []
        human = case != INTEGER
        prefab_rows(rows, spec, None if human else {"Discrete": (23, 23, None, {"m_Materials": []})})
        clip = integer_clip(spec) if not human else human_clip(case, spec)
        if clips and case in clips:
            clip = clips[case](clip)
        rows.append((2, 74, spec["clip_name"], clip))
        rows.append((91, 91, "Controller", {"m_AnimationClips": [pointer(2)]}))
        rows.append((3, 95, None, {"m_Controller": pointer(91), "m_Avatar": pointer(90 if human else 0)}))
        if human:
            assets[spec["avatar_asset"]] = 90
            twist = AVATAR_TWIST if case == TWIST else dict.fromkeys(AVATAR_TWIST, 0.)
            value = {"m_Avatar": {"m_Human": {"data": {"m_RootX": {"t": {"x": 0.}}, **twist}}},
                     "m_HumanDescription": dict(twist)}
            rows.append((90, 90, BUNDLES[case], avatar(case, value) if avatar else value))
            rows.append((9, 43, "Shared", {}))
        digest = archive(bundles, inspect, BUNDLES[case], assets, rows, run=run)
        doc["capture"]["bundle_sha256"] = [digest]
    return recordings
