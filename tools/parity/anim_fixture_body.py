"""Optional body-motion fixtures for the humanoid body plan (docs/plans/humanoid-body.md section 4).

The tables mirror UnityrsBodyFixtures.cs and UnityrsBodyClips.cs. Every case
is optional: a run older than them has no rows for them and completes as
before. A run that has any body row must list every one, and each recorded
case must have its pending recording under replays/body/, which completion
checks here fail-closed. The recorder's body rows sit in the editor evidence:
the case row, F2's GetHumanPose read-back (with the Animator's root and body
rows during clip playback) per sample, each known vector's requested and
applied body target, and HumanTrait's bone table with the default masses.
"""

import struct
from math import isfinite

from anim_body_tolerance import BODY_SLOTS
from anim_fixture_contract import BODY, MUSCLES, SPINE, Fixture, compiled_curves
from anim_reference import angle, dense, euler, multiply, streamed

# name: (base rig, omitted bones, mapped extras). F6 rigs are on the compact
# rig; "noncoplanar" moves the hip and shoulder children off one plane and
# "holdout" (long arms, short legs) is never fitted.
PROBE_RIGS = {
    "compact": ("compact", (), ()), "tall": ("tall", (), ()),
    "no-chest": ("compact", ("Chest", "UpperChest"), ()), "no-upper-chest": ("compact", ("UpperChest",), ()),
    "no-neck": ("compact", ("Neck",), ()), "no-left-shoulder": ("compact", ("LeftShoulder",), ()),
    "no-right-shoulder": ("compact", ("RightShoulder",), ()),
    "no-shoulders": ("compact", ("LeftShoulder", "RightShoulder"), ()),
    "no-left-toes": ("compact", ("LeftToes",), ()), "no-right-toes": ("compact", ("RightToes",), ()),
    "no-toes": ("compact", ("LeftToes", "RightToes"), ()),
    "eyes": ("compact", (), ("LeftEye", "RightEye")), "jaw": ("compact", (), ("Jaw",)),
    "eyes-jaw": ("compact", (), ("LeftEye", "RightEye", "Jaw")),
    "noncoplanar": ("compact", (), ()), "holdout": ("compact", (), ()),
}
HOLDOUT = "holdout"
# F5: the chain from the Animator to the hips, each on the compact rig.
CHAIN_RIGS = ("scaled-animator", "large", "armature", "hips-scale")
TARGET_RIGS = ("compact", "tall")
SIDES = ("Left", "Right")
LIMBS = ("UpperLeg", "LowerLeg", "Foot", "Toes", "Shoulder", "UpperArm", "LowerArm", "Hand")
TORSO = {"compact": ("Hips", "Spine", "Chest", "UpperChest", "Neck", "Head"),
         "tall": ("Hips", "Spine", "Chest", "Neck", "Head")}

PROBE_GROUPS = (
    ("Spine", SPINE), ("Chest", ("Chest Front-Back", "Chest Left-Right", "Chest Twist Left-Right")),
    ("UpperChest", ("UpperChest Front-Back", "UpperChest Left-Right", "UpperChest Twist Left-Right")),
    ("Neck", ("Neck Nod Down-Up", "Neck Tilt Left-Right", "Neck Turn Left-Right")),
    ("Head", ("Head Nod Down-Up", "Head Tilt Left-Right", "Head Turn Left-Right")),
    *((side + "UpperLeg", (f"{side} Upper Leg Front-Back", f"{side} Upper Leg In-Out")) for side in SIDES),
    *((side + "LowerLeg", (f"{side} Lower Leg Stretch",)) for side in SIDES),
    *((side + "Foot", (f"{side} Foot Up-Down",)) for side in SIDES),
    *((side + "Toes", (f"{side} Toes Up-Down",)) for side in SIDES),
    *((side + "Shoulder", (f"{side} Shoulder Down-Up", f"{side} Shoulder Front-Back")) for side in SIDES),
    *((side + "UpperArm", (f"{side} Arm Down-Up", f"{side} Arm Front-Back")) for side in SIDES),
    *((side + "LowerArm", (f"{side} Forearm Stretch",)) for side in SIDES),
    *((side + "Hand", (f"{side} Hand Down-Up", f"{side} Hand In-Out")) for side in SIDES),
    *((side + "Eye", (f"{side} Eye Down-Up", f"{side} Eye In-Out")) for side in SIDES),
    ("Jaw", ("Jaw Close", "Jaw Left-Right")),
)
# Extra probe cases, each on its own bundle of a probe rig: vectors whose every bone takes its muscles from one of
# the rig's own probe vectors, as (bone, source) pairs for `compose`. They add independent poses where the
# adequacy checkpoint found the rig short; anim_body_design.py predicts them from the rig's recorded geometry.
EXTRA_PROBES = {
    "body-probes-tall-2": (
        (("Chest", 2), ("RightUpperLeg", 1)),
        (("LeftUpperLeg", 2), ("RightUpperLeg", 1)),
        (("Chest", -3), ("RightUpperLeg", -2)),
        (("LeftUpperLeg", 1), ("RightUpperLeg", -2)),
        (("LeftUpperLeg", 1), ("RightUpperLeg", 2)),
        (("Chest", 1), ("LeftUpperLeg", 2)),
        (("LeftUpperLeg", 2), ("LeftUpperArm", -3)),
        (("LeftUpperLeg", 2), ("RightUpperLeg", -2)),
        (("Chest", 3), ("LeftUpperLeg", 2)),
        (("LeftUpperLeg", -3), ("RightUpperLeg", 1)),
        (("Chest", -1), ("LeftUpperArm", 2)),
        (("Chest", 2), ("LeftUpperArm", 2)),
        (("LeftUpperLeg", -3), ("RightUpperLeg", 2)),
        (("Chest", -1), ("LeftUpperLeg", -3)),
        (("Chest", 2), ("LeftUpperLeg", -2)),
        (("LeftUpperLeg", -3), ("RightUpperArm", 1)),
        (("Spine", 3), ("LeftUpperLeg", 2)),
        (("LeftUpperArm", 2), ("RightUpperArm", -3)),
        (("Chest", -1), ("RightUpperLeg", 1)),
        (("Chest", -3), ("LeftUpperArm", 2)),
        (("Neck", 2), ("LeftUpperLeg", -3)),
        (("Chest", -2), ("RightUpperLeg", 1)),
        (("Chest", -3), ("RightUpperArm", 2)),
        (("Chest", 1), ("LeftUpperLeg", -1)),
        (("LeftUpperLeg", -3), ("RightUpperLeg", -2)),
        (("LeftUpperLeg", -3), ("RightUpperArm", -3)),
    ),
}
EXTRA_RIGS = {"body-probes-tall-2": "tall"}
# Twist sweeps, each on its own bundle of a training rig (never the hold-out): the shoulder-hip twist swept by
# the Spine and Chest twist muscles together (HumanTrait's default limits, +-40 degrees each, so a value of
# degrees / 80 for both), crossed with an upright body and three pitch and roll tilts of the body target taken
# from F3's tilts (Euler degrees, no yaw). They measure the read-back frame against the twist (2.2) for B2.
TWIST_RIGS = {"body-probes-twist-compact": "compact", "body-probes-twist-tall": "tall"}
TWIST_DEGREES = (0, *(sign * d for d in (2, 4, 6, 8, 10, 15, 20, 30, 45, 60) for sign in (1, -1)))
TWIST_RANGE = 80
TWIST_MUSCLES = ("Spine Twist Left-Right", "Chest Twist Left-Right")
TWIST_TILTS = ((0, 0, 0), (12, 0, 7), (-9, 0, -11), (12, 0, -11))
PROBE_VALUES = (1, -.9, .8)
COMBINED_VALUES = ((.3, -.2, .25, -.35, .15), (-.25, .4, -.15, .2, -.3), (.2, .2, -.3, -.1, .35))
TARGETS = (((.05, -.03, .04), (12, -20, 7)), ((-.04, .02, -.06), (-9, 35, -11)))
# The P2b known vectors at 0, .5 and 1: neutral, single-axis, noncommuting.
ANCHORS = {0: (0, 0, 0), .5: (.5, 0, 0), 1: (-.25, .375, -.5)}


def _flags(bits):
    return f"-o{bits >> 2 & 1}-y{bits >> 1 & 1}-xz{bits & 1}"


def _basis(bits):
    return (bool(bits & 4), bool(bits & 2), bool(bits & 1))


def _clip_variants():
    rows = {}
    neutral = dict(bake=(False,) * 3, keep=(False,) * 3, height_from_feet=False, level=0., orientation=0.,
                   start=False, motion=False, knee=False, goals=False)
    for bits in range(8):
        rows["keep" + _flags(bits)] = neutral | dict(keep=_basis(bits))
    for bits in range(1, 8):
        rows["bake" + _flags(bits)] = neutral | dict(bake=_basis(bits))
    rows["height-from-feet"] = neutral | dict(height_from_feet=True)
    rows["level"] = neutral | dict(level=.125)
    rows["orientation-offset"] = neutral | dict(orientation=30.)
    rows |= {name + "-start": row | dict(start=True) for name, row in list(rows.items())}
    rows["keep-o1-y1-xz1-motion"] = neutral | dict(keep=(True,) * 3, motion=True)
    rows["height-from-feet-knee"] = neutral | dict(height_from_feet=True, knee=True)
    rows["height-from-feet-knee-goals"] = neutral | dict(height_from_feet=True, knee=True, goals=True)
    return rows


CLIP_VARIANTS = _clip_variants()
# F5 plays the all-original clip with changing MotionT and MotionQ, so the residual body translation and tilt
# change through each chain and the original-root rule has motion to keep; completion requires its motion slots.
CHAIN_VARIANT = "keep-o1-y1-xz1-motion"
NEUTRAL_CLIP = CLIP_VARIANTS["keep-o0-y0-xz0"]
BODY_TIMES = (0, .25, .5, .75, 1)
BODY_PATH = ((0, 1, 0), (.08, .97, .1), (.2, 1.02, .12), (.26, .95, .25), (.3, 1, .3))
BODY_TURN = ((0, 0, 0), (8, 25, -4), (12, 40, -6), (6, 15, 3), (10, 30, -5))
START_OFFSET, START_YAW = (.6, 0, -.4), 50
KNEE = {"Left Upper Leg Front-Back": (0, .5, .1), "Left Lower Leg Stretch": (0, -.7, -.1),
        "Right Upper Leg Front-Back": (0, .1, .5), "Right Lower Leg Stretch": (0, -.1, -.7)}
MOTION_PATH = ((0, .04, 0), (.1, .07, .05), (.15, .02, .2), (.3, .09, .2), (.4, .05, .35))
MOTION_YAW = (0, 10, 5, 20, 12)
KNEE_TIMES = (0, .5, 1)
FOOT_GOALS = {"LeftFoot": ((-.1, .05, 0), (-.1, .2, .05), (-.1, .08, .1)),
              "RightFoot": ((.1, .05, 0), (.1, .1, .1), (.1, .25, .05))}
# Built Animator attributes: MotionT/Q 0-6, RootT/Q 7-13, LeftFootT/Q 14-20, RightFootT/Q 21-27, muscles from 42.
MOTION_ATTRIBUTES = dict(T=0, Q=3)
GOAL_ATTRIBUTES = {"LeftFoot": 14, "RightFoot": 21}
MUSCLE_ATTRIBUTE = 42
MOTION_CURVES = tuple(f"MotionT.{a}" for a in "xyz") + tuple(f"MotionQ.{a}" for a in "xyzw")
GOAL_CURVES = tuple(f"{foot}{kind}.{a}" for foot in ("LeftFoot", "RightFoot")
                    for kind, axes in (("T", "xyz"), ("Q", "xyzw")) for a in axes)
# The built m_MuscleClip fields completion copies into the evidence for B2.
SERIALIZED_ROOT = ("m_StartAtOrigin", "m_LoopBlendOrientation", "m_LoopBlendPositionY", "m_LoopBlendPositionXZ",
                   "m_KeepOriginalOrientation", "m_KeepOriginalPositionY", "m_KeepOriginalPositionXZ",
                   "m_HeightFromFeet", "m_Level", "m_OrientationOffsetY", "m_StartX", "m_StopX",
                   "m_MotionStartX", "m_MotionStopX", "m_LeftFootStartX", "m_RightFootStartX")
SPINE_PROBE = "HumanoidRig/Hips/Spine"


def _fixture(bundle, prefab, clip, kind, probe=SPINE_PROBE):
    pose = kind != "clip"
    return Fixture(bundle, bundle, prefab, clip, "human_pose" if pose else "animator", (probe, "rotation", None, None),
                   MUSCLES if pose else MUSCLES + BODY, ("spine", "muscle_only") if pose else ("spine",))


def _cases():
    cases, info = {}, {}

    def add(name, fixture, kind, rig, variant="", root="identity"):
        cases[name], info[name] = fixture, dict(kind=kind, rig=rig, variant=variant, root=root)

    for rig in PROBE_RIGS:
        bundle = "body-" + rig
        add("body-probes-" + rig, _fixture(bundle, "identity", bundle + "-muscles", "probes"), "probes", rig)
        if rig in TARGET_RIGS:
            for prefab, suffix in (("identity", ""), ("offset", "-offset")):
                add(f"body-targets-{rig}{suffix}", _fixture(bundle, prefab, bundle + "-muscles", "targets"),
                    "targets", rig, root=prefab)
    for variant, rigs in (("extra", EXTRA_RIGS), ("twist", TWIST_RIGS)):
        for name, rig in rigs.items():
            bundle = name.replace("probes-", "")
            add(name, _fixture(bundle, "identity", bundle + "-muscles", "probes"), "probes", rig, variant)
    for variant in CLIP_VARIANTS:
        for prefab, suffix in (("identity", ""), ("offset", "-offset")):
            add(f"body-clip-{variant}{suffix}", _fixture("body-clips", prefab, "body-clip-" + variant, "clip"),
                "clip", "compact", variant, prefab)
    for rig in CHAIN_RIGS:
        bundle = "body-chain-" + rig
        probe = "HumanoidRig/Armature/Hips/Spine" if rig == "armature" else SPINE_PROBE
        add(bundle + "-targets", _fixture(bundle, "identity", bundle + "-muscles", "targets", probe), "targets", rig)
        add(bundle + "-clip", _fixture(bundle, "identity", bundle + "-clip", "clip", probe), "clip", rig, CHAIN_VARIANT)
    return cases, info


BODY_CASES, BODY_INFO = _cases()
EXTRA_CASES = frozenset(name for name, info in BODY_INFO.items() if info["variant"] == "extra")
TWIST_CASES = frozenset(TWIST_RIGS)
# The body cases added after the first body run, each generation absent from older runs and required once its
# rig's own probe case records.
LATER_CASES = (EXTRA_CASES, TWIST_CASES)


def primary_probes(rig):
    """The rig's own probe case, as against its extra ones."""
    return next(name for name, info in BODY_INFO.items()
                if info["kind"] == "probes" and info["rig"] == rig and not info["variant"])
BODY_BUNDLES = {fixture.bundle for fixture in BODY_CASES.values()}


def rig_bones(rig):
    """The rig's mapped human bones, by name."""
    base, omit, extras = PROBE_RIGS.get(rig, ("compact", (), ()))
    bones = [*TORSO[base], *(side + limb for side in SIDES for limb in LIMBS), *extras]
    return {bone for bone in bones if bone not in omit}


def _probe_vectors(rig, index):
    """The rig's probe muscles by group, its single-muscle probes and its three combined vectors."""
    mapped = rig_bones(rig)
    groups, muscles, singles = {}, [], []
    for bone, group in PROBE_GROUPS:
        if bone in mapped:
            groups[bone] = [index[muscle] for muscle in group]
            for value, muscle in zip(PROBE_VALUES, group):
                muscles.append(index[muscle])
                singles.append({index[muscle]: value})
    combined = [{muscle: values[g % len(values)] for g, muscle in enumerate(muscles)} for values in COMBINED_VALUES]
    return groups, singles, combined


def _scheduled(vectors, index, length):
    """Spine anchors at 0, .5 and 1, and the vectors filling each half on a 2^-16 grid."""
    first = (len(vectors) + 1) // 2
    second = len(vectors) - first
    times = [.5 * (k + 391 / 1024) / first for k in range(first)]
    times += [.5 + .5 * (k + 391 / 1024) / second for k in range(second)]
    # On a 2^-16 grid, as the builder rounds them (C# rounds half to even, as Python).
    times = [round(t * 65536) / 65536 for t in times]
    spine = [index[muscle] for muscle in SPINE]

    def anchor(t):
        return dict(zip(spine, ANCHORS[t]))

    points = [(0, anchor(0)), *zip(times[:first], vectors[:first]), (.5, anchor(.5)),
              *zip(times[first:], vectors[first:]), (1, anchor(1))]
    return [(t, [vector.get(i, 0) for i in range(length)]) for t, vector in points]


def compose(rig, names, sources):
    """One extra probe vector: each listed bone's muscles as in one of the rig's own probe vectors, a
    single-muscle probe k (1 to 3) or a combined vector c (given as -1 - c), every other muscle zero.
    Each bone's local rotation is then one the rig's own probe case recorded."""
    index = {name: i for i, name in enumerate(names)}
    groups, singles, combined = _probe_vectors(rig, index)
    vector = {}
    if len({bone for bone, _ in sources}) != len(sources):
        raise ValueError(f"extra probe vector lists a bone twice: {sources}")
    for bone, source in sources:
        muscles = groups[bone]
        if source > 0:
            vector[muscles[source - 1]] = PROBE_VALUES[source - 1]
        else:
            vector |= {muscle: combined[-1 - source][muscle] for muscle in muscles}
    return vector


def twist_plan():
    """[(twist muscle value, body tilt)] of a twist sweep, in schedule order: each tilt, then each twist."""
    return [(degrees / TWIST_RANGE, tilt) for tilt in TWIST_TILTS for degrees in TWIST_DEGREES]


def probe_schedule(rig, names, targets, extra=None):
    """[(time, vector)] in schedule order, vectors over the captured HumanTrait muscle names; `extra`
    names an extra probe case (EXTRA_PROBES) or a twist sweep (TWIST_RIGS) of the rig."""
    index = {name: i for i, name in enumerate(names)}
    _, singles, combined = _probe_vectors(rig, index)
    if extra in TWIST_RIGS:
        vectors = [{index[muscle]: value for muscle in TWIST_MUSCLES} for value, _ in twist_plan()]
    elif extra is not None:
        vectors = [compose(rig, names, sources) for sources in EXTRA_PROBES[extra]]
    else:
        vectors = combined if targets else singles + combined
    return _scheduled(vectors, index, len(names))


def schedule_name(name):
    """The `extra` argument of probe_schedule for the body case `name`."""
    return name if BODY_INFO[name]["variant"] in ("extra", "twist") else None


def body_requests(name, count):
    """The (offset, tilt) the recorder adds to the rest body at each of the case's `count` known vectors: F3's
    alternating targets, a twist sweep's tilts (its spine anchors upright), or none."""
    info = BODY_INFO[name]
    if info["kind"] == "targets":
        return [TARGETS[i % 2] for i in range(count)]
    if info["variant"] != "twist":
        return [((0, 0, 0), (0, 0, 0))] * count
    tilts = [tilt for _, tilt in twist_plan()]
    first = (len(tilts) + 1) // 2
    upright = (0, 0, 0)
    return [((0, 0, 0), tilt) for tilt in (upright, *tilts[:first], upright, *tilts[first:], upright)]

def _f32s(values, replay, width, label="body row"):
    """A row of f32 bits the analysis reads: well formed and finite, or refused with the field named."""
    replay.require(_raw(values, width), f"malformed {label}")
    numbers = tuple(struct.unpack(">f", bytes.fromhex(v))[0] for v in values)
    replay.require(all(isfinite(v) for v in numbers), f"nonfinite {label}")
    return numbers


def _unit(q):
    return abs(sum(v * v for v in q) - 1) <= 1e-4


def expected_case_row(name, replay):
    info = BODY_INFO[name]
    clip = CLIP_VARIANTS.get(info["variant"]) if info["kind"] == "clip" else None
    clip = clip or NEUTRAL_CLIP
    return dict(kind=info["kind"], rig=info["rig"], variant=info["variant"], root=info["root"],
                level=replay.f32_bits(clip["level"]), orientation_offset=replay.f32_bits(clip["orientation"]),
                bake=list(clip["bake"]), keep=list(clip["keep"]), height_from_feet=clip["height_from_feet"],
                start_offset=clip["start"], motion_curves=clip["motion"], knee_bend=clip["knee"],
                foot_goals=clip["goals"])


def check_body(name, fixture, doc, view, clip_key, replay):
    """Check a body case's own rows; returns the serialized root rows for a clip case."""
    require = replay.require
    spec, capture = doc["spec"], doc["capture"]
    editor = spec["evidence"]["editor"]
    info = BODY_INFO[name]
    require(editor.get("body_case") == [expected_case_row(name, replay)], f"body case row mismatch for {name}")
    human = capture["humanoid"]
    bones = editor.get("human_bones")
    require(isinstance(bones, list) and all(isinstance(row, dict) for row in bones)
            and [row.get("name") for row in bones] == [row["name"] for row in human["bones"]]
            and all(type(row.get("parent")) is int and -1 <= row["parent"] < len(bones)
                    and type(row.get("required")) is bool for row in bones),
            f"body bone and mass rows mismatch for {name}")
    # The analysis folds the body slots' masses; other bones' masses are kept as recorded, even if nonfinite.
    for row in bones:
        if row["name"] in BODY_SLOTS:
            mass, = _f32s([row.get("mass")], replay, 1, f"human_bones mass of {row['name']} in {name}")
            require(mass >= 0, f"negative human_bones mass of {row['name']} in {name}")
        else:
            require(_raw([row.get("mass")], 1), f"malformed human_bones mass of {row['name']} in {name}")
    mapped = {row["name"] for row in human["bones"] if row["node"] >= 0}
    require(mapped == rig_bones(info["rig"]), f"captured Avatar topology differs from the {info['rig']} rig")
    samples = capture["samples"]
    rows = editor.get("body_samples")
    playback = info["kind"] == "clip"
    require(isinstance(rows, list) and len(rows) == len(samples) and all(isinstance(row, dict) for row in rows),
            f"body read-back rows mismatch for {name}")
    hips = next(row["node"] for row in human["bones"] if row["name"] == "Hips")
    for row in rows:
        # What the analysis reads must be finite; the rest is kept as recorded, raw bits.
        _f32s(row.get("readback_position"), replay, 3, f"readback_position in {name}")
        _f32s(row.get("world_position"), replay, 3, f"world_position in {name}")
        require(all(_unit(_f32s(row.get(field), replay, 4, f"{field} in {name}"))
                    for field in ("readback_rotation", "world_rotation")),
                f"body read-back rotation is not a unit quaternion in {name}")
        require(_raw(row.get("readback_muscles"), len(human["muscles"])), f"body read-back muscles mismatch for {name}")
        # The getter rows during playback, raw; the root-motion playback's transform, finite, and its deltas, raw.
        for field, width in (("root_position", 3), ("root_rotation", 4), ("motion_delta_position", 3),
                             ("motion_delta_rotation", 4)):
            require(_raw(row.get(field), width if playback else 0), f"body root rows mismatch for {name}")
        for field, width in (("motion_position", 3), ("motion_rotation", 4)):
            require(_raw(row.get(field), width if playback else 0), f"body root rows mismatch for {name}")
            if playback:
                _f32s(row[field], replay, width, f"{field} in {name}")
        check_nodes(name, row.get("nodes"), spec["nodes"], hips, replay)
    check_chain(info["rig"], spec, capture, replay)
    if playback:
        require(not editor.get("body_targets"), f"clip playback has known-vector targets in {name}")
        return check_clip(name, info, editor, capture, view.value(clip_key), replay)
    check_targets(name, info, spec, capture, editor, replay)
    return None


def _raw(value, width):
    return (isinstance(value, list) and len(value) == width
            and all(isinstance(v, str) and len(v) == 8 and all(c in "0123456789abcdef" for c in v) for v in value))


def check_nodes(name, rows, nodes, hips, replay):
    """Unity's own world transform of every node at a sample, and its position and rotation about the hips."""
    require = replay.require
    require(isinstance(rows, list) and len(rows) == len(nodes), f"body node rows mismatch for {name}")
    require(all(isinstance(row, dict) for row in rows), f"body node rows mismatch for {name}")
    for row, node in zip(rows, nodes):
        for field in ("position", "scale", "hips_position"):
            _f32s(row.get(field), replay, 3, f"node {field} of {node['address'] or 'the root'} in {name}")
        require(all(_unit(_f32s(row.get(field), replay, 4, f"node {field} of {node['address'] or 'the root'} in {name}"))
                    for field in ("rotation", "hips_rotation")),
                f"body node rotation is not a unit quaternion in {name}")
    at_hips = rows[hips]
    require(all(abs(v) <= 1e-5 for v in _f32s(at_hips["hips_position"], replay, 3))
            and angle(_f32s(at_hips["hips_rotation"], replay, 4), (0, 0, 0, 1)) <= 1e-5,
            f"the hips are not at the origin of their own frame in {name}")


def check_targets(name, info, spec, capture, editor, replay):
    require = replay.require
    names = [row["name"] for row in capture["humanoid"]["muscles"]]
    expected = probe_schedule(info["rig"], names, info["kind"] == "targets", schedule_name(name))
    require([point["local"] for point in spec["schedule"]] == [t for t, _ in expected]
            and all([replay.f32_bits(v) for v in point["muscles"]] == [replay.f32_bits(v) for v in vector]
                    for point, (_, vector) in zip(spec["schedule"], expected)),
            f"body known-vector schedule mismatch for {name}")
    rows = editor.get("body_targets")
    require(isinstance(rows, list) and len(rows) == len(expected) and all(isinstance(row, dict) for row in rows),
            f"body target rows mismatch for {name}")
    human = capture["humanoid"]
    rest_position = _f32s(human["rest_body_position"], replay, 3)
    rest_rotation = _f32s(human["rest_body_rotation"], replay, 4)
    zero = [replay.f32_bits(0)] * 3
    for row, (offset, tilt) in zip(rows, body_requests(name, len(rows)), strict=True):
        position = _f32s(row.get("position"), replay, 3, f"body_targets position in {name}")
        rotation = _f32s(row.get("rotation"), replay, 4, f"body_targets rotation in {name}")
        require(row.get("offset") == ([replay.f32_bits(v) for v in offset] if any(offset) else zero)
                and row.get("tilt") == ([replay.f32_bits(v) for v in tilt] if any(tilt) else zero),
                f"body target request mismatch for {name}")
        want = multiply(euler(tilt, 4), rest_rotation)
        require(all(abs(a - (b + c)) <= 1e-5 for a, b, c in zip(position, rest_position, offset))
                and angle(rotation, want) <= 1e-5, f"applied body target differs from its request in {name}")


def check_chain(rig, spec, capture, replay):
    """The F5 chain each rig claims, in the recorded rest pose."""
    nodes = {node["address"]: (i, node["name"]) for i, node in enumerate(spec["nodes"])}
    rest = capture["rest"]["transforms"]

    def scale(address):
        return _f32s(rest[nodes[address][0]]["scale"], replay, 3)

    def near(values, want):
        return all(abs(v - want) <= 1e-6 for v in values)

    hips = [address for address, (_, label) in nodes.items() if label == "Hips"]
    replay.require(len(hips) == 1, "body rig has no single Hips node")
    hips = hips[0]
    armature = rig == "armature"
    checks = {
        "scaled-animator": near(scale("0"), 1.25),
        "armature": nodes.get("0/0", (0, ""))[1] == "Armature" and near(scale("0/0"), .8) and hips == "0/0/0",
        "hips-scale": near(scale(hips), 1.2),
        "large": abs(_f32s(rest[nodes[hips][0]]["translation"], replay, 3)[1] - 1.32) <= 1e-5,
    }
    replay.require(checks.get(rig, True) and (armature or hips == "0/0")
                   and (rig == "scaled-animator" or near(scale("0"), 1))
                   and (rig == "hips-scale" or near(scale(hips), 1)), f"body rig chain differs from the {rig} rig")


def _authored(editor, replay):
    return {row["property"]: row for row in editor["bindings"]
            if (row["path"], row["type"]) == ("", "UnityEngine.Animator")}


def _built(value, want, replay):
    if type(want) is bool:
        return value is want or (type(value) is int and not isinstance(value, bool) and value == int(want))
    return type(value) in (int, float) and replay.f32_bits(value) == replay.f32_bits(want)


def check_clip(name, info, editor, capture, clip, replay):
    require = replay.require
    variant = CLIP_VARIANTS[info["variant"]]
    authored = _authored(editor, replay)
    turn = euler((0, START_YAW if variant["start"] else 0, 0), 4)
    c, s = turn[3] ** 2 - turn[1] ** 2, 2 * turn[1] * turn[3]

    def value(prop, i, field="value_f32"):
        return replay.f32_from_bits(authored[prop]["keys"][i][field])

    require(all(len(authored.get(f"Root{k}.{a}", {}).get("keys", ())) == len(BODY_TIMES)
                for k, axes in (("T", "xyz"), ("Q", "xyzw")) for a in axes), f"authored body curves missing in {name}")
    for i, t in enumerate(BODY_TIMES):
        x, y, z = BODY_PATH[i]
        point = (c * x + s * z, y, -s * x + c * z)
        if variant["start"]:
            point = tuple(a + b for a, b in zip(point, START_OFFSET))
        keyed = [value(f"RootQ.{a}", i) for a in "xyzw"]
        require(all(abs(value(f"RootT.{a}", i) - b) <= 1e-5 for a, b in zip("xyz", point))
                and angle(keyed, multiply(turn, euler(BODY_TURN[i], 4))) <= 1e-5
                and all(value(f"Root{k}.x", i, "time_f32") == t for k in "TQ"),
                f"authored body curve mismatch for {name}")
    extras = distinguishing_curves(variant, [row["name"] for row in capture["humanoid"]["muscles"]])
    for flag, curves in (("knee", KNEE), ("motion", MOTION_CURVES), ("goals", GOAL_CURVES)):
        require(all(bool(authored.get(curve, {}).get("keys")) == variant[flag] for curve in curves),
                f"authored {flag} curves mismatch for {name}")
    for group in extras:
        require(all([replay.f32_from_bits(key["time_f32"]) for key in authored[prop]["keys"]] == list(times)
                    for prop, _, times, _ in group)
                and _matches(group, lambda prop, i, t: value(prop, i)),
                f"authored {group[0][0]} curve mismatch for {name}")
    settings = capture["clip_settings"]
    flags = dict(zip(("loopBlendOrientation", "loopBlendPositionY", "loopBlendPositionXZ"), variant["bake"]))
    flags |= dict(zip(("keepOriginalOrientation", "keepOriginalPositionY", "keepOriginalPositionXZ"), variant["keep"]))
    flags |= dict(heightFromFeet=variant["height_from_feet"], loopTime=False, loopBlend=False, mirror=False)
    numbers = dict(level=variant["level"], orientationOffsetY=variant["orientation"], cycleOffset=0., startTime=0.,
                   stopTime=1.)
    require(all(settings[key] is value for key, value in flags.items())
            and all(settings[key] == replay.f32_bits(value) for key, value in numbers.items()),
            f"captured clip settings differ from the {info['variant']} variant")
    muscle = clip.get("m_MuscleClip", {})
    built = dict(zip(SERIALIZED_ROOT[1:8], (*variant["bake"], *variant["keep"], variant["height_from_feet"])))
    built |= {"m_Level": variant["level"], "m_OrientationOffsetY": variant["orientation"]}
    require(all(_built(muscle.get(key), want, replay) for key, want in built.items()),
            f"built clip root settings differ from the {info['variant']} variant")
    # The compiled body slots, Animator attributes 7 to 13, as the P2b body clip.
    bindings = clip.get("m_ClipBindingConstant", {}).get("genericBindings", [])
    attributes = sorted({row.get("attribute") for row in bindings
                         if row.get("path") == 0 and row.get("customType") == 8
                         and row.get("typeID", row.get("classID")) == 95})
    # A variant counts as covered only when the built clip keeps its distinguishing slots and values
    # where playback reads them: m_MuscleClip's data at the DOF's m_IndexArray slot. Editor curve copies do not count.
    runtime = _runtime_slots(clip, replay)
    for group in extras:
        sampled = {prop: _built_track(runtime, muscle.get("m_IndexArray") or [], attribute)
                   for prop, attribute, _, _ in group}
        require(all(sampled.values()) and _matches(group, lambda prop, i, t: sampled[prop](t)),
                f"built clip lost its {group[0][0]} slot or values in {name}")
    return dict(serialized={key: muscle.get(key) for key in SERIALIZED_ROOT}, animator_attributes=attributes)


def distinguishing_curves(variant, muscles):
    """Groups of (property, built attribute, key times, values) a variant authors beyond the body:
    each knee muscle, MotionT, MotionQ, and each foot goal's T and Q. A Q group compares by angle."""
    groups = []
    if variant["knee"]:
        groups += [[(muscle, MUSCLE_ATTRIBUTE + muscles.index(muscle), KNEE_TIMES, values)]
                   for muscle, values in KNEE.items()]

    def vector(prefix, attribute, times, rows):
        return [(f"{prefix}.{axis}", attribute + i, times, tuple(row[i] for row in rows)) for i, axis in enumerate("xyz")]

    def rotation(prefix, attribute, times, rows):
        return [(f"{prefix}.{axis}", attribute + i, times, tuple(row[i] for row in rows))
                for i, axis in enumerate("xyzw")]

    if variant["motion"]:
        groups += [vector("MotionT", 0, BODY_TIMES, MOTION_PATH),
                   rotation("MotionQ", 3, BODY_TIMES, [euler((0, yaw, 0), 4) for yaw in MOTION_YAW])]
    if variant["goals"]:
        for goal, path in FOOT_GOALS.items():
            groups += [vector(goal + "T", GOAL_ATTRIBUTES[goal], KNEE_TIMES, path),
                       rotation(goal + "Q", GOAL_ATTRIBUTES[goal] + 3, KNEE_TIMES, [(0, 0, 0, 1)] * len(KNEE_TIMES))]
    return groups


def _matches(group, sample):
    """Every key of a group within 1e-5, a quaternion group by angle (either sign)."""
    times = group[0][2]
    if len(group) == 4:
        return all(angle([sample(prop, i, t) for prop, _, _, _ in group], [values[i] for _, _, _, values in group])
                   <= 1e-4 for i, t in enumerate(times))
    return all(abs(sample(prop, i, t) - values[i]) <= 1e-5 for prop, _, _, values in group for i, t in enumerate(times))


def _runtime_slots(clip, replay):
    """m_MuscleClip's slots in storage order (streamed, dense, constant), each a (built binding key,
    sampler) pair sampled as playback does; none when two bindings share a key, since the slots
    could not then be told apart."""
    tracks = compiled_curves(clip, replay)
    if len(tracks) != len(clip.get("m_ClipBindingConstant", {}).get("genericBindings", [])):
        return []
    playback = clip.get("m_MuscleClip", {}).get("m_Clip", {})
    grid = playback.get("data", playback).get("m_DenseClip", {})

    def sampler(is_streamed, entries):
        if not entries:
            return None
        if is_streamed:
            keys = sorted(entries, key=lambda entry: entry[0])
            return lambda t: streamed(keys, t)
        values = [coefficients[3] for _, coefficients in entries]
        if len(values) == 1:
            return lambda t: values[0]
        # Dense frames interpolate linearly between frames, as playback does.
        return lambda t: dense(values, 1, 0, grid["m_BeginTime"], grid["m_SampleRate"], t)
    return [(key, sampler(*track)) for key, spans in tracks.items() for track in spans]


def _built_track(runtime, index, attribute):
    """A function of time over the runtime slot m_IndexArray gives an Animator DOF (its attribute),
    or None if the DOF is absent (-1), outside the data or bound to another attribute."""
    slot = index[attribute] if 0 <= attribute < len(index) and type(index[attribute]) is int else -1
    if not 0 <= slot < len(runtime) or runtime[slot][0] != (0, 95, attribute, 8, False):
        return None
    return runtime[slot][1]
