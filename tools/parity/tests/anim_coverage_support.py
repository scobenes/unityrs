"""Synthetic Editor recordings and bundles for the optional coverage fixtures."""

import copy
import math

from anim_fixture_support import ASSETS, ROOT_ID, RUN, archive, game_object, pointer, recording, stored_curves
from test_anim_replay import bits

from anim_fixture_coverage import BASIS_CASES, REFUSALS  # noqa: E402  (test_anim_replay puts it on the path)

INTEGER, TWIST, SETTINGS = "integer-discrete-binding", "humanoid-twist-refusal", "humanoid-non-neutral-setting-refusal"
# The integer case has no Built-in fixture: the run reports it as not covered.
COVERAGE = (TWIST, SETTINGS)
BUNDLES = {TWIST: "humanoid-twist", SETTINGS: "humanoid-settings"} | dict.fromkeys(BASIS_CASES, "humanoid-root-basis")
# Two basis matrix clips: the neutral combination and the original-Y one.
NEUTRAL_BASIS, BASIS_Y = "humanoid-root-basis-o0-y0-xz0", "humanoid-root-basis-o0-y1-xz0"
BASIS = (NEUTRAL_BASIS, BASIS_Y)
BASIS_FIELDS = ("keepOriginalOrientation", "keepOriginalPositionY", "keepOriginalPositionXZ")
TWIST_VALUES = dict(upper_arm_twist=.5, lower_arm_twist=.25, upper_leg_twist=.75, lower_leg_twist=.375)
AVATAR_TWIST = dict(m_ArmTwist=.5, m_ForeArmTwist=.25, m_UpperLegTwist=.75, m_LegTwist=.375)
# HumanTrait order puts these after the Spine muscles; the index sets the built attribute.
# name: (HumanTrait index, end value, HumanBodyBones bone)
TWIST_MUSCLES = {"Left Upper Leg Twist In-Out": (23, .5, 1), "Left Lower Leg Twist In-Out": (25, -.375, 3),
                 "Left Arm Twist In-Out": (41, .625, 13), "Left Forearm Twist In-Out": (43, -.5, 15)}
LIMBS = ("LeftUpperArm", "LeftLowerArm", "LeftHand", "LeftUpperLeg", "LeftLowerLeg", "LeftFoot")
SPINE = ("Spine Front-Back", "Spine Left-Right", "Spine Twist Left-Right")


def word(value):
    return int(bits(value), 16)


def key_rows(times, values, encode=bits):
    return [dict(time_f32=bits(t), value_f32=encode(v), in_tangent_f32=bits(0), out_tangent_f32=bits(0),
                 in_weight_f32=bits(.25), out_weight_f32=bits(.25), weighted_mode="None")
            for t, v in zip(times, values)]


def turn(degrees):
    half = math.radians(degrees) / 2
    return [bits(math.sin(half)), bits(0), bits(0), bits(math.cos(half))]


def human_recording(case, run):
    folder = ASSETS + BUNDLES[case] + "/"
    doc = recording("humanoid-compact-muscles", BUNDLES[case], folder + "identity.prefab", folder + case + ".anim",
                    folder + "avatar.asset", run)
    spec, capture = doc["spec"], doc["capture"]
    editor = spec["evidence"]["editor"]
    editor["fixture_case"] = case
    human = capture["humanoid"]
    if case in BASIS_CASES:
        editor["expected_refusal"] = REFUSALS.get(case, "")
        capture["clip_settings"].update(zip(BASIS_FIELDS, BASIS_CASES[case]))
        return doc
    if case == SETTINGS:
        editor["expected_refusal"] = "unsupported_humanoid_clip_settings"
        capture["clip_settings"].update(level=bits(.125))
        return doc
    editor["expected_refusal"] = "unsupported_humanoid_twist"
    human.update({name: bits(value) for name, value in TWIST_VALUES.items()})
    for name, (index, value, bone) in TWIST_MUSCLES.items():
        human["muscles"][index].update(name=name, bone=bone)
        editor["bindings"].append(dict(path="", property=name, type="UnityEngine.Animator", discrete=False,
                                       object_reference=False, object_keys=[], keys=key_rows((0, 1), (0, value))))
    first = len(spec["nodes"])
    spec["nodes"] += [dict(address=f"0/0/{i}", name=bone) for i, bone in enumerate(LIMBS, 1)]
    # The captured bones in HumanBodyBones order, through LeftHand.
    order = {7: "Spine", 13: "LeftUpperArm", 15: "LeftLowerArm", 17: "LeftHand",
             1: "LeftUpperLeg", 3: "LeftLowerLeg", 5: "LeftFoot"}
    nodes = {"Spine": 3} | {bone: first + i for i, bone in enumerate(LIMBS)}
    human["bones"] = [dict(name=order.get(i, f"Bone {i}"), node=nodes.get(order.get(i), -1), required=i in order)
                      for i in range(18)]
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


def human_clip(case, spec, *, ramp=True):
    """Spine slots hold .25; each twist slot ramps linearly from 0 to its end value."""
    twist = list(TWIST_MUSCLES.values()) if case == TWIST else []
    muscles = list(range(3)) + [index for index, _, _ in twist]
    frames = []
    for t in (0., .5, 1.):
        entries = [(slot, 0, 0, 0, word(.25)) for slot in range(3)]
        for slot, (_, value, _) in enumerate(twist, 3):
            slope = value if ramp and t < 1 else 0
            entries.append((slot, 0, 0, word(slope), word(value * t if ramp else value)))
        frames.append((word(t), entries))
    offset = case == SETTINGS
    basis = {}
    if case in BASIS_CASES:
        orientation, y, xz = BASIS_CASES[case]
        basis = {"m_KeepOriginalOrientation": orientation, "m_KeepOriginalPositionY": y,
                 "m_KeepOriginalPositionXZ": xz, "m_HeightFromFeet": False, "m_StartAtOrigin": not xz,
                 **dict.fromkeys(("m_LoopBlend", "m_LoopBlendOrientation", "m_LoopBlendPositionY",
                                  "m_LoopBlendPositionXZ"), False)}
    return {"m_SampleRate": 16., "m_Legacy": False, **stored_curves(spec), "m_MuscleClip": {
        "m_StartTime": 0., "m_StopTime": 1., "m_LoopTime": False, "m_CycleOffset": 0., "m_Mirror": False,
        "m_Level": .125 if offset else 0., "m_OrientationOffsetY": 0., "m_KeepOriginalPositionY": False,
        **basis, "m_IndexArray": [0, 1, -1], "m_Clip": {"data": {
            "m_StreamedClip": {"curveCount": len(muscles), "data": stream(frames)},
            "m_DenseClip": {"m_CurveCount": 0}, "m_ConstantClip": {"data": []}}}},
        "m_ClipBindingConstant": {"genericBindings": [
            dict(path=0, typeID=95, attribute=42 + index, customType=8, isPPtrCurve=0, isIntCurve=0)
            for index in muscles], "pptrCurveMapping": []}}


def coverage_archives(bundles, inspect, run=RUN, *, clips=None, avatar=None, ramp=True, cases=COVERAGE):
    """Write the coverage bundles; `clips` and `avatar` replace a built value per case.

    Cases sharing a bundle, as the basis matrix clips do, share its prefab and Avatar.
    """
    recordings = {case: human_recording(case, run) for case in cases}
    for bundle in dict.fromkeys(BUNDLES[case] for case in cases):
        members = [case for case in cases if BUNDLES[case] == bundle]
        spec = recordings[members[0]]["spec"]
        assets = {spec["prefab_asset"]: 1}
        rows = []
        prefab_rows(rows, spec)
        for i, case in enumerate(members):
            clip = human_clip(case, recordings[case]["spec"], ramp=ramp)
            if clips and case in clips:
                clip = clips[case](clip)
            assets[recordings[case]["spec"]["clip_asset"]] = 2 if i == 0 else 20 + i
            rows.append((2 if i == 0 else 20 + i, 74, recordings[case]["spec"]["clip_name"], clip))
        rows.append((91, 91, "Controller", {"m_AnimationClips": [pointer(2)] + [
            pointer(20 + i) for i in range(1, len(members))]}))
        rows.append((3, 95, None, {"m_Controller": pointer(91), "m_Avatar": pointer(90)}))
        assets[spec["avatar_asset"]] = 90
        twist = AVATAR_TWIST if bundle == BUNDLES[TWIST] else dict.fromkeys(AVATAR_TWIST, 0.)
        neutral = dict(m_ArmStretch=0., m_LegStretch=0., m_FeetSpacing=0., m_HasTDoF=False)
        value = {"m_Avatar": {"m_Human": {"data": {"m_RootX": {"t": {"x": 0.}}, **twist, **neutral}}},
                 "m_HumanDescription": dict(twist)}
        rows.append((90, 90, bundle, avatar(members[0], value) if avatar else value))
        rows.append((9, 43, "Shared", {}))
        digest = archive(bundles, inspect, bundle, assets, rows, run=run)
        for case in members:
            recordings[case]["capture"]["bundle_sha256"] = [digest]
    return recordings
