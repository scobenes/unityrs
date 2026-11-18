"""Optional coverage fixtures: an integer discrete binding and two humanoid refusal probes.

Each case is optional in run.json. A case recorded there must have its pending
recording, and completion checks the evidence fail-closed. A case reported as
not covered must not leave a recording, so an older run without these fixtures
completes unchanged.
"""

from math import radians
import struct
from zlib import crc32

from anim_fixture_contract import (MOTION, MUSCLES, POSITION, SPINE_PATH, Fixture, compiled_curves, floats,
                                   matches_reference_time)

SPINE_PROBE = (SPINE_PATH, "rotation", None, None)
COVERAGE_CASES = {
    "integer-discrete-binding": Fixture("anim-discrete", "discrete", "discrete", "integer-discrete-binding",
                                        "animator", (MOTION, "translation", 3, 9), POSITION, ("integer",)),
    "humanoid-twist-refusal": Fixture("humanoid-twist", "humanoid-twist", "identity", "humanoid-twist-refusal",
                                      "animator", SPINE_PROBE, MUSCLES, ("spine", "muscle_only", "twist")),
    "humanoid-non-neutral-setting-refusal": Fixture(
        "humanoid-settings", "humanoid-settings", "identity", "humanoid-non-neutral-setting-refusal",
        "animator", SPINE_PROBE, MUSCLES, ("spine", "muscle_only", "settings")),
}
REFUSALS = {"humanoid-twist-refusal": "unsupported_humanoid_twist",
            "humanoid-non-neutral-setting-refusal": "unsupported_humanoid_clip_settings"}

# A MeshRenderer's sorting order, keyed 3 then 7 at .5 s on a "Discrete" node.
DISCRETE = ("Discrete", "MeshRenderer", "m_SortingOrder")
DISCRETE_CLASS = 23
DISCRETE_KEYS = ((0, 3), (.5, 7))

# Distinct values catch a swapped field. The capture names come from
# HumanDescription; the runtime and serialized Avatar fields follow Unity's names.
TWIST = {"upper_arm_twist": .5, "lower_arm_twist": .25, "upper_leg_twist": .75, "lower_leg_twist": .375}
AVATAR_TWIST = {"m_ArmTwist": "upper_arm_twist", "m_ForeArmTwist": "lower_arm_twist",
                "m_UpperLegTwist": "upper_leg_twist", "m_LegTwist": "lower_leg_twist"}
TWIST_MUSCLES = {"Left Arm Twist In-Out": .625, "Left Forearm Twist In-Out": -.5,
                 "Left Upper Leg Twist In-Out": .5, "Left Lower Leg Twist In-Out": -.375}
ARM_BONES = ("LeftUpperArm", "LeftLowerArm", "LeftHand")
LEG_BONES = ("LeftUpperLeg", "LeftLowerLeg", "LeftFoot")
# AnimatorMuscle attributes put the muscles after the 42 body and goal slots.
MUSCLE_ATTRIBUTE_BASE = 42

SETTINGS = {"level": .125, "orientationOffsetY": 20}
NEUTRAL_OFFSETS = ("cycleOffset", "additiveReferencePoseTime")
NEUTRAL_FLAGS = ("mirror", "loopTime", "loopBlend", "hasAdditiveReferencePose")


def f32(word):
    return struct.unpack(">f", struct.pack(">I", word))[0]


def same_f32(value, expected, replay):
    return type(value) in (int, float) and replay.f32_bits(value) == replay.f32_bits(expected)


def check_coverage(name, fixture, doc, view, clip_key, avatar_key, replay):
    """Check a coverage case's own evidence, and that other cases claim none."""
    spec, capture = doc["spec"], doc["capture"]
    editor = spec["evidence"]["editor"]
    refusal = REFUSALS.get(name, "")
    checks = fixture.checks
    replay.require(editor.get("expected_refusal", "") == refusal, f"fixture refusal label mismatch for {name}")
    replay.require(bool(editor.get("discrete_samples")) == ("integer" in checks)
                   and bool(editor.get("twist_effects")) == ("twist" in checks),
                   f"fixture coverage evidence mismatch for {name}")
    if refusal:
        spec["evidence"]["expected_refusal"] = refusal
    if "integer" in checks:
        spec["evidence"]["integer_discrete"] = check_integer(editor, capture, view.value(clip_key), replay)
    if "twist" in checks:
        check_twist(editor, capture, view.value(clip_key), view.value(avatar_key), replay)
    if {"twist", "settings"} & set(checks):
        check_settings(spec, capture, "settings" in checks, replay)


def check_integer(editor, capture, clip, replay):
    require = replay.require
    rows = [row for row in editor["bindings"] if (row["path"], row["type"].removeprefix("UnityEngine."),
                                                   row["property"]) == DISCRETE]
    require(len(rows) == 1 and {(row["path"], row["type"].removeprefix("UnityEngine."), row["property"])
                                for row in editor["bindings"]} == set(POSITION) | {DISCRETE},
            "integer fixture must author translation and one discrete sorting-order binding")
    row = rows[0]
    require(row["discrete"] is True and row["object_reference"] is False and not row["object_keys"]
            and [(key["time_f32"], key["value_f32"]) for key in row["keys"]]
            == [(replay.f32_bits(t), replay.f32_bits(v)) for t, v in DISCRETE_KEYS],
            "integer fixture binding is not the authored discrete step")
    samples = capture["samples"]
    observed = editor["discrete_samples"]
    require(len(observed) == len(samples) and all(
        set(item) == {"time_f32", "value"} and item["time_f32"] == sample["local_f32"] and type(item["value"]) is int
        and item["value"] == (DISCRETE_KEYS[0][1] if sample["local"] < DISCRETE_KEYS[1][0] else DISCRETE_KEYS[1][1])
        for item, sample in zip(observed, samples)), "integer fixture playback evidence mismatch")
    return built_integer(clip, replay)


def built_integer(clip, replay):
    """Find the integer slot in the streamed range and read its stored step."""
    require = replay.require
    bindings = clip.get("m_ClipBindingConstant", {}).get("genericBindings", [])
    playback = clip.get("m_MuscleClip", {}).get("m_Clip", {})
    playback = playback.get("data", playback)
    stream = playback.get("m_StreamedClip", {})
    count, discrete = stream.get("curveCount", 0), stream.get("discreteCurveCount", 0)
    dense = playback.get("m_DenseClip", {}).get("m_CurveCount", 0)
    constants = floats(playback.get("m_ConstantClip", {}).get("data", []), replay)
    path, _, prop = DISCRETE
    position = (crc32(MOTION.encode()), 4, 1, False, False)
    integer = (crc32(path.encode()), DISCRETE_CLASS, crc32(prop.encode()), False, True)
    rows = [(row["path"], row.get("typeID", row.get("classID")), row["attribute"], bool(row.get("isPPtrCurve")),
             bool(row.get("isIntCurve"))) for row in bindings]
    require(sorted(rows) == sorted([position, integer]), "built integer fixture bindings mismatch")
    slot = sum(3 if row == position else 1 for row in rows[:rows.index(integer)])
    # 2022.3 counts discrete slots apart and serializes them after the others.
    require(dense == 0 and not constants and count + discrete == 4 and discrete in (0, 1)
            and (discrete == 0 or slot == count), "built integer slot outside a confirmed streamed layout")
    words, keys, cursor = stream.get("data", []), [], 0
    while cursor + 2 <= len(words) and words[cursor] != 0x7f800000:
        stop = cursor + 2 + 5 * words[cursor + 1]
        keys.extend((f32(words[cursor]), words[i + 4]) for i in range(cursor + 2, stop, 5) if words[i] == slot)
        cursor = stop
    encodings = {"int32_bits": lambda word: struct.unpack(">i", struct.pack(">I", word))[0], "f32": f32}
    for encoding, decode in encodings.items():
        changes = []
        for time, word in keys:
            value = decode(word)
            if not changes or value != changes[-1][1]:
                changes.append((time, value))
        if ([value for _, value in changes] == [value for _, value in DISCRETE_KEYS] and changes[0][0] <= 0
                and matches_reference_time(changes[1][0], clip["m_SampleRate"])):
            custom = next(row["customType"] for row in bindings if row["path"] == integer[0])
            return dict(encoding=encoding, slot=slot, counted_discrete=bool(discrete), custom_type=custom)
    raise ValueError("built integer discrete slot does not hold the authored step")


def check_twist(editor, capture, clip, avatar, replay):
    require = replay.require
    human = capture["humanoid"]
    require(all(human[name] == replay.f32_bits(value) for name, value in TWIST.items()),
            "captured Avatar twist differs from the authored twist")
    runtime = avatar.get("m_Avatar", {}).get("m_Human", {})
    runtime = runtime.get("data", runtime)
    described = avatar.get("m_HumanDescription", {})
    require(all(same_f32(source.get(field), TWIST[name], replay) for source in (runtime, described)
                for field, name in AVATAR_TWIST.items()), "built Avatar twist differs from the authored twist")
    muscles = [row["name"] for row in human["muscles"]]
    curves = compiled_curves(clip, replay)
    for muscle, value in TWIST_MUSCLES.items():
        rows = [row for row in editor["bindings"] if (row["path"], row["type"], row["property"])
                == ("", "UnityEngine.Animator", muscle)]
        require(len(rows) == 1 and not rows[0]["discrete"] and rows[0]["keys"]
                and rows[0]["keys"][-1]["value_f32"] == replay.f32_bits(value)
                and rows[0]["keys"][0]["value_f32"] == replay.f32_bits(0), f"missing fixture twist curve: {muscle}")
        require(muscle in muscles, f"captured muscles omit {muscle}")
        track = curves.get((0, 95, MUSCLE_ATTRIBUTE_BASE + muscles.index(muscle), 8, False))
        require(track and track[0][1], f"missing built twist muscle slot: {muscle}")
    bones = {row["name"]: row["node"] for row in human["bones"]}
    effects = editor["twist_effects"]
    require([row.get("bone") for row in effects] == [*ARM_BONES, *LEG_BONES],
            "fixture twist evidence must cover the left arm and leg")
    last = capture["samples"][-1]
    moved = {}
    for row in effects:
        node = row["node"]
        require(type(node) is int and bones.get(row["bone"]) == node >= 0, "fixture twist bone mapping mismatch")
        built, authored, neutral = (tuple(map(replay.f32_from_bits, values)) for values in (
            last["pose"]["transforms"][node]["rotation"], row["authored_f32"], row["neutral_f32"]))
        require(replay.angle(built, authored) <= radians(.25),
                f"built twist pose differs from the authored Avatar at {row['bone']}")
        moved[row["bone"]] = replay.angle(authored, neutral)
    require(max(moved[bone] for bone in ARM_BONES) > radians(1)
            and max(moved[bone] for bone in LEG_BONES) > radians(1),
            "fixture twist evidence does not show redistribution in both limbs")


def check_settings(spec, capture, offset, replay):
    """The settings probe sets only height and orientation offsets; the twist probe none."""
    require = replay.require
    rows = {row["property"]: row for row in spec["evidence"]["import_settings"]}
    settings = capture["clip_settings"]
    for name, value in SETTINGS.items():
        value = value if offset else 0
        require(name in rows and same_f32(rows[name]["effective"], value, replay)
                and same_f32(rows[name]["serialized"], value, replay) and settings[name] == replay.f32_bits(value),
                f"fixture clip setting mismatch: {name}")
    require(all(settings[name] == replay.f32_bits(0) for name in NEUTRAL_OFFSETS)
            and not any(settings[name] for name in NEUTRAL_FLAGS), "fixture refusal probe has other clip settings")
