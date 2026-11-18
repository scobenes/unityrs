"""Optional coverage fixtures: an integer discrete binding and two humanoid refusal probes.

Each case is optional in run.json unless REQUIRED_COVERAGE lists it for the
run's Editor version. A case recorded there must have its pending recording,
and completion checks the evidence fail-closed. A case reported as not covered
must not leave a recording, so an older run without these fixtures completes
unchanged.
"""

from math import isfinite, radians
import struct
from zlib import crc32

from anim_fixture_contract import (MOTION, MUSCLES, POSITION, SPINE_PATH, Fixture, compiled_curves, floats,
                                   matches_reference_time)
from anim_reference import dense, streamed

# The coverage cases a run must record, by Editor version ("2022.3" matches
# 2022.3.x). Add a case once a real run of that version has recorded it and
# `complete` has accepted it; from then on a not_covered status fails that run.
REQUIRED_COVERAGE = {
    "2022.3": (),
    "6000": (),
}

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
# Unity keeps a discrete integer key as the integer's bits in the f32 value.
DISCRETE = ("Discrete", "MeshRenderer", "m_SortingOrder")
DISCRETE_CLASS = 23
DISCRETE_KEYS = ((0, 3), (.5, 7))

# Distinct values catch a swapped field. The capture names come from
# HumanDescription; the runtime and serialized Avatar fields follow Unity's names.
TWIST = {"upper_arm_twist": .5, "lower_arm_twist": .25, "upper_leg_twist": .75, "lower_leg_twist": .375}
AVATAR_TWIST = {"m_ArmTwist": "upper_arm_twist", "m_ForeArmTwist": "lower_arm_twist",
                "m_UpperLegTwist": "upper_leg_twist", "m_LegTwist": "lower_leg_twist"}
AVATAR_NEUTRAL = ("m_ArmStretch", "m_LegStretch", "m_FeetSpacing")
# Each twist muscle's end value and the bone HumanTrait maps it to. The clip
# ramps each from 0. The captured bones are in HumanBodyBones order.
TWIST_MUSCLES = {"Left Arm Twist In-Out": (.625, "LeftUpperArm"), "Left Forearm Twist In-Out": (-.5, "LeftLowerArm"),
                 "Left Upper Leg Twist In-Out": (.5, "LeftUpperLeg"),
                 "Left Lower Leg Twist In-Out": (-.375, "LeftLowerLeg")}
RAMP_TIMES = (0, .5, 1)
ARM_BONES = ("LeftUpperArm", "LeftLowerArm", "LeftHand")
LEG_BONES = ("LeftUpperLeg", "LeftLowerLeg", "LeftFoot")
# AnimatorMuscle attributes put the muscles after the 42 body and goal slots.
MUSCLE_ATTRIBUTE_BASE = 42

# Every captured clip setting at neutral; the settings probe changes only level.
NEUTRAL_CLIP = dict.fromkeys(("startTime", "orientationOffsetY", "level", "cycleOffset",
                              "additiveReferencePoseTime"), 0.) | {"stopTime": 1.} | dict.fromkeys((
    "loopTime", "loopBlend", "loopBlendOrientation", "loopBlendPositionY", "loopBlendPositionXZ",
    "keepOriginalOrientation", "keepOriginalPositionY", "keepOriginalPositionXZ", "heightFromFeet", "mirror",
    "hasAdditiveReferencePose"), False) | {"additiveReferencePoseClipName": ""}
SETTINGS_TRIGGER = {"level": .125}


def required_coverage(editor_version):
    return {name for prefix, names in REQUIRED_COVERAGE.items()
            if editor_version.startswith(prefix + ".") for name in names}


def f32(word):
    return struct.unpack(">f", struct.pack(">I", word))[0]


def int32(word):
    return struct.unpack(">i", struct.pack(">I", word))[0]


def same_f32(value, expected, replay):
    return type(value) in (int, float) and replay.f32_bits(value) == replay.f32_bits(expected)


def check_coverage(name, fixture, doc, view, clip_key, avatar_key, replay):
    """Check a coverage case's own evidence, and that other cases claim none."""
    spec, capture = doc["spec"], doc["capture"]
    editor = spec["evidence"]["editor"]
    refusal = REFUSALS.get(name, "")
    checks = fixture.checks
    replay.require(editor.get("expected_refusal", "") == refusal, f"fixture refusal label mismatch for {name}")
    replay.require(bool(editor.get("discrete_samples")) == bool(editor.get("discrete_keys")) == ("integer" in checks)
                   and bool(editor.get("twist_effects")) == ("twist" in checks),
                   f"fixture coverage evidence mismatch for {name}")
    if refusal:
        spec["evidence"]["expected_refusal"] = refusal
    if "integer" in checks:
        spec["evidence"]["integer_discrete"] = check_integer(editor, capture, view.value(clip_key), replay)
    if "twist" in checks:
        check_twist(editor, capture, view.value(clip_key), replay)
    if refusal:
        check_isolated(spec, capture, view.value(avatar_key), "twist" in checks, replay)


def check_integer(editor, capture, clip, replay):
    require = replay.require
    rows = [row for row in editor["bindings"] if (row["path"], row["type"].removeprefix("UnityEngine."),
                                                   row["property"]) == DISCRETE]
    require(len(rows) == 1 and {(row["path"], row["type"].removeprefix("UnityEngine."), row["property"])
                                for row in editor["bindings"]} == set(POSITION) | {DISCRETE},
            "integer fixture must author translation and one discrete sorting-order binding")
    row = rows[0]
    expected = [(replay.f32_bits(t), v) for t, v in DISCRETE_KEYS]
    require(row["discrete"] is True and row["object_reference"] is False and not row["object_keys"]
            and [(key["time_f32"], int32(int(key["value_f32"], 16))) for key in row["keys"]] == expected,
            "integer fixture binding is not the authored bit-cast discrete step")
    require([(key.get("time_f32"), key.get("value")) for key in editor["discrete_keys"]] == expected
            and all(set(key) == {"time_f32", "value"} and type(key["value"]) is int for key in editor["discrete_keys"]),
            "integer fixture decoded keys mismatch")
    samples = capture["samples"]
    observed = editor["discrete_samples"]
    require(len(observed) == len(samples) and all(
        set(item) == {"time_f32", "value"} and item["time_f32"] == sample["local_f32"] and type(item["value"]) is int
        and item["value"] == (DISCRETE_KEYS[0][1] if sample["local"] < DISCRETE_KEYS[1][0] else DISCRETE_KEYS[1][1])
        for item, sample in zip(observed, samples)), "integer fixture playback evidence mismatch")
    return built_integer(clip, replay)


def built_integer(clip, replay):
    """Find the integer slot in the streamed range and read its stored step as int32 bits."""
    require = replay.require
    bindings = clip.get("m_ClipBindingConstant", {}).get("genericBindings", [])
    playback = clip.get("m_MuscleClip", {}).get("m_Clip", {})
    playback = playback.get("data", playback)
    stream = playback.get("m_StreamedClip", {})
    count, discrete = stream.get("curveCount", 0), stream.get("discreteCurveCount", 0)
    dense_count = playback.get("m_DenseClip", {}).get("m_CurveCount", 0)
    constants = floats(playback.get("m_ConstantClip", {}).get("data", []), replay)
    path, _, prop = DISCRETE
    position = (crc32(MOTION.encode()), 4, 1, False, False)
    integer = (crc32(path.encode()), DISCRETE_CLASS, crc32(prop.encode()), False, True)
    rows = [(row["path"], row.get("typeID", row.get("classID")), row["attribute"], bool(row.get("isPPtrCurve")),
             bool(row.get("isIntCurve"))) for row in bindings]
    require(sorted(rows) == sorted([position, integer]), "built integer fixture bindings mismatch")
    slot = sum(3 if row == position else 1 for row in rows[:rows.index(integer)])
    # 2022.3 counts discrete slots apart and serializes them after the others.
    require(dense_count == 0 and not constants and count + discrete == 4 and discrete in (0, 1)
            and (discrete == 0 or slot == count), "built integer slot outside a confirmed streamed layout")
    words, keys, cursor = stream.get("data", []), [], 0
    while cursor + 2 <= len(words) and words[cursor] != 0x7f800000:
        stop = cursor + 2 + 5 * words[cursor + 1]
        keys.extend((f32(words[cursor]), int32(words[i + 4])) for i in range(cursor + 2, stop, 5) if words[i] == slot)
        cursor = stop
    changes = []
    for time, value in keys:
        if not changes or value != changes[-1][1]:
            changes.append((time, value))
    require([value for _, value in changes] == [value for _, value in DISCRETE_KEYS] and changes[0][0] <= 0
            and matches_reference_time(changes[1][0], clip["m_SampleRate"]),
            "built integer discrete slot does not hold the authored step")
    custom = next(row["customType"] for row in bindings if row["path"] == integer[0])
    return dict(encoding="int32_bits", slot=slot, counted_discrete=bool(discrete), custom_type=custom)


def sample_track(track, time, clip):
    is_streamed, keys = track
    if is_streamed:
        return streamed(keys, time)
    values = [coefficients[3] for _, coefficients in keys]
    if len(values) == 1:
        return values[0]
    playback = clip["m_MuscleClip"]["m_Clip"]
    grid = playback.get("data", playback)["m_DenseClip"]
    return dense(values, 1, 0, grid["m_BeginTime"], grid["m_SampleRate"], time)


def check_twist(editor, capture, clip, replay):
    require = replay.require
    human = capture["humanoid"]
    muscles = {row["name"]: (index, row) for index, row in enumerate(human["muscles"])}
    curves = compiled_curves(clip, replay)
    attributes = [row["attribute"] for row in clip.get("m_ClipBindingConstant", {}).get("genericBindings", [])
                  if (row["path"], row.get("typeID", row.get("classID")), row["customType"]) == (0, 95, 8)]
    for muscle, (value, bone) in TWIST_MUSCLES.items():
        rows = [row for row in editor["bindings"] if (row["path"], row["type"], row["property"])
                == ("", "UnityEngine.Animator", muscle)]
        require(len(rows) == 1 and not rows[0]["discrete"]
                and [key["value_f32"] for key in rows[0]["keys"]] == [replay.f32_bits(0), replay.f32_bits(value)],
                f"missing fixture twist curve: {muscle}")
        mapped = muscles[muscle][1]["bone"] if muscle in muscles else -1
        require(0 <= mapped < len(human["bones"]) and human["bones"][mapped]["name"] == bone,
                f"captured muscle mapping mismatch: {muscle}")
        attribute = MUSCLE_ATTRIBUTE_BASE + muscles[muscle][0]
        track = curves.get((0, 95, attribute, 8, False))
        require(attributes.count(attribute) == 1 and track and len(track) == 1 and track[0][1],
                f"missing built twist muscle slot: {muscle}")
        ramp = [sample_track(track[0], t, clip) for t in RAMP_TIMES]
        require(all(isfinite(v) and abs(v - value * t) <= 1e-5 for v, t in zip(ramp, RAMP_TIMES)),
                f"built twist muscle does not ramp to its authored value: {muscle}")
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


def check_isolated(spec, capture, avatar, twist, replay):
    """A refusal probe carries exactly one trigger: Avatar twist, or the clip's height offset."""
    require = replay.require
    expected = NEUTRAL_CLIP | ({} if twist else SETTINGS_TRIGGER)
    settings = capture["clip_settings"]

    def matches(value, want):
        if type(want) is bool:
            return value is want or (type(value) is int and value == int(want))
        if type(want) is str:
            return value == want
        return same_f32(value, want, replay)

    require(settings.keys() == expected.keys() and all(
        matches(settings[name], want) if type(want) is not float else settings[name] == replay.f32_bits(want)
        for name, want in expected.items()), "fixture refusal probe has another clip setting trigger")
    rows = {row["property"]: row for row in spec["evidence"]["import_settings"]}
    require(rows.keys() <= expected.keys() and all(
        matches(row["effective"], expected[name]) and (row["serialized"] is None or matches(row["serialized"], expected[name]))
        for name, row in rows.items()), "fixture refusal probe has another serialized clip setting trigger")
    require("level" in rows and rows["level"]["serialized"] is not None,
            "fixture refusal probe is missing the serialized height offset")
    human = capture["humanoid"]
    twist_values = TWIST if twist else dict.fromkeys(TWIST, 0)
    require(all(human[name] == replay.f32_bits(value) for name, value in twist_values.items())
            and all(human[name] == replay.f32_bits(0) for name in ("arm_stretch", "leg_stretch", "feet_spacing"))
            and human["translation_dof"] is False, "captured Avatar settings differ from the probe's one trigger")
    runtime = avatar.get("m_Avatar", {}).get("m_Human", {})
    runtime = runtime.get("data", runtime)
    described = avatar.get("m_HumanDescription", {})
    require(all(same_f32(source.get(field), twist_values[name], replay) for source in (runtime, described)
                for field, name in AVATAR_TWIST.items())
            and all(same_f32(runtime.get(field), 0, replay) for field in AVATAR_NEUTRAL)
            and not runtime.get("m_HasTDoF"), "built Avatar settings differ from the probe's one trigger")
