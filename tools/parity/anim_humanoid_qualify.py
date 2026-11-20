"""Humanoid qualification evidence for plan section 2.4.

Point this at a completed Editor fixture run, the ``<version>/<target>/``
folder ``anim_replay.py complete`` accepted. For each humanoid clip the run's
``run.json`` lists as recorded it reads the built clip and Avatar through
``unityrs show`` and records:

- the slot inventory from ``m_IndexArray`` and its absent sentinel, with the
  storage ranges, crosswalked against the built bindings (each row at its
  generic width), the Editor curve bindings and the recorder's
  ``HumanTrait`` muscle and bone order;
- the runtime axes of each mapped muscle, against the recorded limits, and
  the muscles of absent optional bones, which playback drops;
- the root and hips-local comparison with the reset rig, against the bound
  ``anim_replay.py`` derives for a rest-valued track, and with the
  same-schedule known-vector ``SetHumanPose`` baseline;
- the muscle-to-pose residual of the candidate joint-axis equation;
- three independent verdicts (clip settings, root basis, residual) and
  every typed cause the clip has, unordered. The Rust reader returns one of
  them in its own order; ``recorded_humanoid_refusals`` checks it is listed.

A clip qualifies when it has no cause and a baseline within its bound: the
known-vector ``SetHumanPose`` recording of the same prefab, Animator, clip and
Avatar, whose root stays at its rest and which the clip's root and hips match
within the rest-track bounds. A root basis matrix clip has no known-vector
recording; its baseline is its own prefab's reset rig, which the residual
verdict already compares. A qualifying clip's verdict is
``qualified_pending_playback``: positive playback is deferred, so Rust still
refuses it as ``unsupported_humanoid_playback``.

``python3 -I tools/parity/anim_humanoid_qualify.py RUN_DIR --unityrs CLI -o OUT``
writes ``unityrs.anim_humanoid_qualification`` version 2.
"""

import argparse
import hashlib
import json
import math
import sys
from dataclasses import replace
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import anim_replay as replay  # noqa: E402
import anim_replay_budgets as budgets  # noqa: E402
from anim_fixture_contract import REPLAY_CASES, VECTOR_SUFFIX  # noqa: E402
from anim_fixture_coverage import BASIS_CASES, COVERAGE_CASES, REFUSALS  # noqa: E402
from anim_replay_complete import BundleView, read_manifest  # noqa: E402

FORMAT = "unityrs.anim_humanoid_qualification"
VERSION = 2
ABSENT_SLOT = -1
MUSCLE_COUNT = 95
REQUIRED_BONES = (0, 1, 2, 3, 4, 5, 6, 7, 10, 13, 14, 15, 16, 17, 18)
GOALS = ("LeftFoot", "RightFoot", "LeftHand", "RightHand")
TQ = ("T.x", "T.y", "T.z", "Q.x", "Q.y", "Q.z", "Q.w")
SETTINGS = (
    "m_Mirror", "m_CycleOffset", "m_Level", "m_OrientationOffsetY", "m_LoopTime", "m_LoopBlend",
    "m_LoopBlendOrientation", "m_LoopBlendPositionY", "m_LoopBlendPositionXZ", "m_KeepOriginalOrientation",
    "m_KeepOriginalPositionY", "m_KeepOriginalPositionXZ", "m_HeightFromFeet", "m_StartAtOrigin",
)
# The root basis flags. Their neutral profile: the first four off and
# start-at-origin on, as built clips store it whatever their XZ flag.
ROOT_BASIS = SETTINGS[-5:]
TWIST = ("m_ArmTwist", "m_ForeArmTwist", "m_UpperLegTwist", "m_LegTwist")
AVATAR_SETTINGS = ("m_ArmStretch", "m_LegStretch", "m_FeetSpacing")
# The HumanBodyBones each Avatar setting acts on (HumanDescription): a twist
# redistributes between a limb's two joints, a stretch lengthens a limb.
LIMBS = {"m_ArmTwist": (13, 14, 15, 16), "m_ForeArmTwist": (15, 16, 17, 18), "m_UpperLegTwist": (1, 2, 3, 4),
         "m_LegTwist": (3, 4, 5, 6), "m_ArmStretch": (13, 14, 15, 16, 17, 18),
         "m_LegStretch": (1, 2, 3, 4, 5, 6), "m_FeetSpacing": (1, 2, 3, 4, 5, 6)}
# The clips every run records; recorded coverage cases join them.
REQUIRED_CASES = tuple(sorted(name for name, fixture in REPLAY_CASES.items()
                              if fixture.bundle.startswith("humanoid-")))
# The neutral twin: a clip on the fixtures' neutral Avatar, the one whose
# twist and stretch the recorder's neutral-twist twin evaluates with.
NEUTRAL_TWINS = ("humanoid-non-neutral-setting-refusal", "humanoid-compact-muscles")
require = replay.require
# What a known-vector baseline shares with its clip's replay.
BASELINE_IDENTITY = ("prefab_asset", "clip_asset", "avatar_asset", "binding_address", "binding_component")


def dof_kind(dof):
    """The kind, ordinal and label of a DOF in the built 200-entry layout."""
    if dof < 7:
        return "motion", dof, "Motion" + TQ[dof]
    if dof < 14:
        return "body", dof - 7, "Root" + TQ[dof - 7]
    if dof < 42:
        goal, axis = divmod(dof - 14, 7)
        return "goal", dof - 14, GOALS[goal] + TQ[axis]
    if dof < 42 + MUSCLE_COUNT:
        return "muscle", dof - 42, None
    if dof < 200:
        bone, axis = divmod(dof - 137, 3)
        return "translation", dof - 137, f"TDoF{bone}.{'xyz'[axis]}"
    raise ValueError(f"unknown humanoid DOF {dof}")


def muscle_bone(muscle):
    """The HumanBodyBones index a HumanTrait muscle rotates (Rust ``muscle_bone``)."""
    table = ((0, 2, 7), (3, 5, 8), (6, 8, 54), (9, 11, 9), (12, 14, 10), (15, 16, 21), (17, 18, 22),
             (19, 20, 23), (21, 23, 1), (24, 25, 3), (26, 27, 5), (28, 28, 19), (29, 31, 2), (32, 33, 4),
             (34, 35, 6), (36, 36, 20), (37, 38, 11), (39, 41, 13), (42, 43, 15), (44, 45, 17),
             (46, 47, 12), (48, 50, 14), (51, 52, 16), (53, 54, 18))
    for low, high, bone in table:
        if low <= muscle <= high:
            return bone
    require(55 <= muscle < MUSCLE_COUNT, f"unknown muscle {muscle}")
    finger = muscle - 55
    return 24 + finger // 4 * 3 + (0, 0, 1, 2)[finger % 4]


def bone_node(human, bone):
    """The human skeleton node of a HumanBodyBones index, or None."""
    if bone <= 8:
        raw = human["m_HumanBoneIndex"][bone]
    elif bone == 54:
        raw = human["m_HumanBoneIndex"][9]
    elif bone <= 23:
        raw = human["m_HumanBoneIndex"][bone + 1]
    else:
        hand, finger = divmod(bone - 24, 15)
        raw = human[("m_LeftHand", "m_RightHand")[hand]]["data"]["m_HandBoneIndex"][finger]
    return None if raw == ABSENT_SLOT else raw


def data(value):
    return value.get("data", value)


def vec(value, axes="xyzw"):
    return [value[axis] for axis in axes if axis in value]


def multiply(a, b):
    ax, ay, az, aw = a
    bx, by, bz, bw = b
    return (aw * bx + ax * bw + ay * bz - az * by, aw * by - ax * bz + ay * bw + az * bx,
            aw * bz + ax * by - ay * bx + az * bw, aw * bw - ax * bx - ay * by - az * bz)


def normalize(q):
    norm = math.sqrt(sum(x * x for x in q))
    require(norm > 0, "zero quaternion")
    return tuple(x / norm for x in q)


def angle_degrees(q, r):
    q, r = normalize(q), normalize(r)
    if sum(a * b for a, b in zip(q, r)) < 0:
        r = tuple(-x for x in r)
    lo = math.sqrt(sum((a - b) ** 2 for a, b in zip(q, r)))
    hi = math.sqrt(sum((a + b) ** 2 for a, b in zip(q, r)))
    return math.degrees(4 * math.atan2(lo, hi))


def muscle_rotation(axes, values):
    """Candidate local rotation of one bone from (muscle value, axis) pairs.

    Each axis angle is ``d * max`` for ``d = sign * m >= 0`` and ``d * -min``
    otherwise, in radians. The twist (x) and swing (y, z) use half-angle
    tangents, ``normalize((0, tan y/2, tan z/2, 1)(tan x/2, 0, 0, 1))``, between
    the pre rotation and the conjugate post rotation. It is a hypothesis the
    known-vector recordings test; Unity does not publish the equation.
    """
    angles = [0.0, 0.0, 0.0]
    for value, axis in values:
        d = axes["m_Sgn"][axis] * value
        angles[axis] = d * axes["m_Limit"]["m_Max"][axis] if d >= 0 else d * -axes["m_Limit"]["m_Min"][axis]
    tx, ty, tz = (math.tan(a / 2) for a in angles)
    joint = normalize(multiply((0.0, ty, tz, 1.0), (tx, 0.0, 0.0, 1.0)))
    pre, post = axes["m_PreQ"], axes["m_PostQ"]
    return normalize(multiply(multiply(pre, joint), (-post[0], -post[1], -post[2], post[3])))


def trs(pose):
    return {key: [replay.f32_from_bits(bits) for bits in pose[key]] for key in ("translation", "rotation", "scale")}


def residual(actual, expected):
    """Componentwise translation and scale differences, and the rotation angle in radians."""
    actual, expected = trs(actual), trs(expected)
    return {
        "translation": [abs(a - b) for a, b in zip(actual["translation"], expected["translation"])],
        "rotation": math.radians(angle_degrees(actual["rotation"], expected["rotation"])),
        "scale": [abs(a - b) for a, b in zip(actual["scale"], expected["scale"])],
    }


def rest_bounds(rest, node, times):
    """The bounds ``anim_replay.py derive`` gives an unanimated (rest) track of one node.

    The rows are the ones ``anim_replay_budgets.py`` authors for a Transform
    property no curve reads; the bound is the final f32 rounding at the rest
    value's magnitude, widened as every derived bound is.
    """
    values = trs(rest[node])
    bounds = {}
    for prop in ("translation", "rotation", "scale"):
        row = budgets.budget(prop, node, -1, values[prop], [], None, times)
        model = replay.budget_model(row)
        if prop == "rotation":
            [(raw, _)] = model.errors(0.)
            bounds[prop] = replay._bound(model, raw, 1.)
        else:
            bounds[prop] = [replay._bound(model, raw, reach) for raw, reach in model.errors(0.)]
    return bounds


def judge(pairs, bounds):
    """The worst residual over sample pairs, and whether every component stays within its bound."""
    rows = [residual(actual, expected) for actual, expected in pairs]
    within = all(all(d <= b for d, b in zip(row[key], bounds[key])) for row in rows for key in ("translation", "scale")) \
        and all(row["rotation"] <= bounds["rotation"] for row in rows)
    return dict(translation=max(max(row["translation"]) for row in rows),
                rotation_degrees=math.degrees(max(row["rotation"] for row in rows)),
                scale=max(max(row["scale"]) for row in rows), within=within)


def tos_paths(avatar):
    paths = {}
    for hash_value, path in avatar["m_TOS"]:
        require(paths.setdefault(hash_value, path) == path, "conflicting m_TOS entry")
    return paths


def node_paths(spec):
    """Paths of the recorded nodes relative to the Animator node, by node index."""
    names = {row["address"]: row["name"] for row in spec["nodes"]}
    root = spec["binding_address"]
    result = {}
    for index, row in enumerate(spec["nodes"]):
        address = row["address"]
        if address == root:
            result[index] = ""
        elif address.startswith(root + "/" if root else ""):
            parts = address.split("/")
            depth = len(root.split("/")) if root else 0
            result[index] = "/".join(names["/".join(parts[:i])] for i in range(depth + 1, len(parts) + 1))
    return result


def float_count(value):
    """Length of a float array, which canonical show may pack as ``f32s`` hex."""
    if isinstance(value, dict):
        require(value.get("t") == "f32s" and len(value["v"]) % 8 == 0, "unexpected float array")
        return len(value["v"]) // 8
    return len(value)


def binding_rows(bindings):
    """Each binding with its first slot, width and role, at the generic widths.

    A humanoid DOF row (Animator class 95, ``customType`` 8) is one slot; a
    local position, rotation, scale or Euler row is 3, 4, 3 or 3; any other row one.
    """
    rows, first = [], 0
    for index, row in enumerate(bindings):
        class_id, custom, attribute = row.get("typeID", row.get("classID")), row["customType"], row["attribute"]
        if (class_id, custom) == (95, 8):
            role, width = "human", 1
        elif class_id == 4 or (class_id == 224 and 1 <= attribute <= 4):
            require(custom == 0 or attribute == 4, f"unknown width of binding {index}")
            role, width = "transform", {1: 3, 2: 4, 3: 3, 4: 3}[attribute]
        else:
            role, width = "other", 1
        rows.append(dict(index=index, first=first, width=width, role=role, row=row))
        first += width
    return rows, first


def inventory(clip, capture):
    muscle = clip["m_MuscleClip"]
    index = muscle["m_IndexArray"]
    require(len(index) == 200, f"unsupported m_IndexArray length {len(index)}")
    stored = data(muscle["m_Clip"])
    streamed = stored["m_StreamedClip"]
    counts = {"streamed": streamed["curveCount"] + streamed.get("discreteCurveCount", 0),
              "dense": stored["m_DenseClip"]["m_CurveCount"], "constant": float_count(stored["m_ConstantClip"]["data"])}
    names = [row["name"] for row in capture["humanoid"]["muscles"]]
    require(len(names) == MUSCLE_COUNT, "recorder muscle list is not HumanTrait's 95")
    slots = []
    for dof, value in enumerate(index):
        kind, ordinal, label = dof_kind(dof)
        storage, offset = "absent", value
        for name in ("streamed", "dense", "constant"):
            if 0 <= offset < counts[name] and value != ABSENT_SLOT:
                storage = name
                break
            offset -= counts[name]
        require((value == ABSENT_SLOT) == (storage == "absent"), f"slot {value} of DOF {dof} outside storage")
        slots.append(dict(dof=dof, kind=kind, name=names[ordinal] if kind == "muscle" else label,
                          index_value=value, storage=storage))
    mapped = [row["index_value"] for row in slots if row["storage"] != "absent"]
    require(len(set(mapped)) == len(mapped), "two DOFs share a slot")
    return slots, counts


def crosswalk(clip, avatar, spec, capture, slots, counts):
    """Check bindings, bones, muscles and limits against the recorder's order.

    Returns the mapped muscles' axes, the mapped muscles of absent optional
    bones, and the generic Transform bindings that write a human skeleton node.
    """
    by_dof = {row["dof"]: row for row in slots}
    rows, width = binding_rows(clip["m_ClipBindingConstant"]["genericBindings"])
    require(width == sum(counts.values()), "binding widths and slot counts differ")
    human_rows = [row for row in rows if row["role"] == "human"]
    require(len(human_rows) == sum(row["storage"] != "absent" for row in slots), "binding and slot counts differ")
    for row in human_rows:
        attribute = row["row"]["attribute"]
        require(attribute in by_dof and by_dof[attribute]["index_value"] == row["first"],
                f"built binding {row['index']} disagrees with m_IndexArray")
    editor = {row["property"] for row in spec["evidence"]["editor"]["bindings"]
              if row.get("type", "UnityEngine.Animator") == "UnityEngine.Animator"}
    built = {by_dof[row["row"]["attribute"]]["name"] for row in human_rows}
    require(editor == built, f"Editor curve bindings {sorted(editor)} differ from built DOFs {sorted(built)}")

    human = data(avatar["m_Avatar"]["m_Human"])
    skeleton = data(human["m_Skeleton"])
    paths, nodes = tos_paths(avatar), node_paths(spec)
    bones = capture["humanoid"]["bones"]
    require(len(bones) == 55 and bones[0]["name"] == "Hips" and bones[54]["name"] == "UpperChest",
            "recorder bones are not in HumanBodyBones order")
    for bone, row in enumerate(bones):
        node = bone_node(human, bone)
        recorded = row["node"] if row["node"] >= 0 else None
        found = None if node is None else paths.get(skeleton["m_ID"][node])
        require((found is None) == (recorded is None) and (found is None or nodes.get(recorded) == found),
                f"human bone {row['name']} maps to a different node")
        require(row["required"] == (bone in REQUIRED_BONES), f"required flag differs for {row['name']}")
    axes, dropped, missing_axes = [], [], []
    limit_rows = {row["human"]: row for row in capture["humanoid"]["limits"]}
    for ordinal, row in enumerate(capture["humanoid"]["muscles"]):
        bone = muscle_bone(ordinal)
        require(row["bone"] == bone, f"muscle {row['name']} rotates another bone")
        node = bone_node(human, bone)
        mapped = by_dof[42 + ordinal]["storage"] != "absent"
        if node is None:
            if mapped and bone not in REQUIRED_BONES:
                dropped.append(row["name"])
            continue
        record_id = skeleton["m_Node"][node]["m_AxesId"]
        if record_id < 0:
            if mapped:
                missing_axes.append(bone)
            continue
        record = skeleton["m_AxesArray"][record_id]
        limits = [math.degrees(record["m_Limit"][key]["xyz"[row["axis"]]]) for key in ("m_Min", "m_Max")]
        # The recorder's muscle rows hold HumanTrait's defaults; an Avatar's
        # own limits are in its per-bone rows, in degrees on the same axes.
        custom = limit_rows.get(bones[bone]["name"])
        if custom is not None and not custom["defaults"]:
            recorded = [replay.f32_from_bits(custom[key][row["axis"]]) for key in ("min", "max")]
        else:
            recorded = [replay.f32_from_bits(row[key]) for key in ("min", "max")]
        require(all(abs(a - b) <= 1e-4 * max(1, abs(b)) for a, b in zip(limits, recorded)),
                f"runtime limits of {row['name']} differ from the recorder's")
        if mapped:
            axes.append(dict(muscle=row["name"], bone=bone, node=node, axis=row["axis"],
                             pre=vec(record["m_PreQ"]), post=vec(record["m_PostQ"]), sign=vec(record["m_Sgn"]),
                             min=vec(record["m_Limit"]["m_Min"]), max=vec(record["m_Limit"]["m_Max"]),
                             type=record["m_Type"]))
    ids = {value for value in skeleton["m_ID"] if value}
    writers = [row["index"] for row in rows if row["role"] == "transform" and row["row"]["path"] in ids]
    return axes, dropped, missing_axes, writers


def identity_root(spec, capture):
    """Whether the Animator node and every recorded ancestor rest at the identity."""
    root = spec["binding_address"]
    chain = [i for i, row in enumerate(spec["nodes"])
             if row["address"] == root or root.startswith(row["address"] + "/") or row["address"] == ""]
    for node in chain:
        values = trs(capture["rest"]["transforms"][node])
        if values["translation"] != [0, 0, 0] or values["scale"] != [1, 1, 1] \
                or values["rotation"][:3] != [0, 0, 0] or abs(values["rotation"][3]) != 1:
            return False
    return True


def comparison(doc, baseline, avatar, spec):
    """Root and hips-local TRS against the reset rig, within the rest-track bounds, and the known vectors."""
    capture = doc["capture"]
    root = next(i for i, row in enumerate(spec["nodes"]) if row["address"] == spec["binding_address"])
    hips = next(row["node"] for row in capture["humanoid"]["bones"] if row["name"] == "Hips")
    hips_address = spec["nodes"][hips]["address"]
    require(hips_address.rsplit("/", 1)[0] == spec["binding_address"], "hips parent is not the Animator node")
    rest = capture["rest"]["transforms"]
    samples = capture["samples"]
    times = sorted({row["local"] for row in samples})
    result = {"root_node": root, "hips_node": hips, "bounds": {}, "reset": {}}
    for name, node in (("root", root), ("hips_local", hips)):
        bounds = rest_bounds(rest, node, times)
        result["bounds"][name] = bounds
        result["reset"][name] = judge([(row["pose"]["transforms"][node], rest[node]) for row in samples], bounds)
    if baseline is None:
        return result
    other = baseline["capture"]["samples"]
    require([row["local_f32"] for row in other] == [row["local_f32"] for row in samples],
            "known-vector schedule differs")
    require(baseline["capture"]["bundle_sha256"] == capture["bundle_sha256"], "known-vector bundle differs")
    other_spec = baseline["spec"]
    require(all(other_spec[key] == spec[key] for key in BASELINE_IDENTITY)
            and [row["name"] for row in other_spec["bundles"]] == [row["name"] for row in spec["bundles"]],
            "known-vector baseline is another prefab, clip, Animator or Avatar")
    require([(row["address"], row["name"]) for row in other_spec["nodes"]]
            == [(row["address"], row["name"]) for row in spec["nodes"]]
            and baseline["capture"]["rest"]["transforms"] == rest, "known-vector baseline has another reset rig")
    # The raw recorder output, not a completed replay: its digest names the input.
    result["baseline_recording_id"] = replay._known_vector_id(baseline)
    result["baseline"] = {name: judge([(a["pose"]["transforms"][node], b["pose"]["transforms"][node])
                                       for a, b in zip(samples, other)], result["bounds"][name])
                          for name, node in (("root", root), ("hips_local", hips))}
    # The baseline poses the muscles alone: its own root stays at its rest.
    result["baseline_root_at_rest"] = judge([(row["pose"]["transforms"][root], rest[root]) for row in other],
                                            result["bounds"]["root"])["within"]
    result["baseline_within"] = result["baseline_root_at_rest"] and all(
        row["within"] for row in result["baseline"].values())
    # Absent body slots leave the body at the Animator root origin: the
    # clip's hips translation is the baseline's less the rest body offset.
    offset = vec(data(avatar["m_Avatar"]["m_Human"])["m_RootX"]["t"])
    result["body_origin_translation"] = max(
        abs(t - (u - o)) for a, b in zip(samples, other)
        for t, u, o in zip(trs(a["pose"]["transforms"][hips])["translation"],
                           trs(b["pose"]["transforms"][hips])["translation"], offset))
    result["m_RootX_t"] = offset
    return result


def muscle_pose(document, axes):
    """Worst angle between each mapped bone's captured rotation and the candidate."""
    capture = document["capture"]
    if not capture["samples"] or not capture["samples"][0].get("muscles_f32"):
        return None
    bones = {row["name"]: row["node"] for row in capture["humanoid"]["bones"]}
    names = [row["name"] for row in capture["humanoid"]["bones"]]
    result = {}
    for bone in sorted({row["bone"] for row in axes}):
        rows = [row for row in axes if row["bone"] == bone]
        record = {"m_PreQ": rows[0]["pre"], "m_PostQ": rows[0]["post"], "m_Sgn": rows[0]["sign"],
                  "m_Limit": {"m_Min": rows[0]["min"], "m_Max": rows[0]["max"]}}
        ordinals = [i for i, row in enumerate(capture["humanoid"]["muscles"]) if row["bone"] == bone]
        node = bones[names[bone]]
        rest = trs(capture["rest"]["transforms"][node])["rotation"]
        worst_angle = 0.0
        for sample in capture["samples"]:
            values = [replay.f32_from_bits(bits) for bits in sample["muscles_f32"]]
            joint = muscle_rotation(record, [(values[i], capture["humanoid"]["muscles"][i]["axis"]) for i in ordinals])
            predicted = normalize(multiply(rest, joint))
            actual = trs(sample["pose"]["transforms"][node])["rotation"]
            worst_angle = max(worst_angle, angle_degrees(predicted, actual))
        result[names[bone]] = worst_angle
    return result


def settings(clip, avatar):
    muscle = clip["m_MuscleClip"]
    human = data(avatar["m_Avatar"]["m_Human"])
    return ([dict(field=name, serialized=muscle.get(name)) for name in SETTINGS],
            {name: human[name] for name in TWIST + AVATAR_SETTINGS + ("m_HasTDoF",)})


def neutral(value):
    return value is False or (type(value) is float and value == 0.0)


def basis_off(values):
    """The root basis flags off their neutral profile."""
    off = [name for name in ROOT_BASIS[:4] if values.get(name) is not False]
    if values.get("m_StartAtOrigin") is not True:
        off.append("m_StartAtOrigin")
    return off


def causes(entry, avatar, clip, neutral_avatar):
    """Every typed cause the clip has, in no order (the Rust reader picks one)."""
    mapped = {row["kind"] for row in entry["slots"] if row["storage"] != "absent"}
    human = data(avatar["m_Avatar"]["m_Human"])
    found = [f"missing_human_bone({bone})" for bone in REQUIRED_BONES if bone_node(human, bone) is None]
    if not entry["identity_root"]:
        found.append("unsupported_animator_root")
    found += [f"conflicting_humanoid_writer({index})" for index in entry["generic_writers"]]
    if "motion" in mapped or avatar["m_Avatar"]["m_RootMotionBoneIndex"] >= 0 \
            or clip.get("m_HasGenericRootTransform") or clip.get("m_HasMotionFloatCurves"):
        found.append("unsupported_locomotion")
    if "goal" in mapped:
        found.append("unsupported_humanoid_ik")
    if "translation" in mapped or entry["avatar_settings"]["m_HasTDoF"]:
        found.append("unsupported_translation_dof")
    if float_count(clip["m_MuscleClip"]["m_ValueArrayReferencePose"]):
        found.append("unsupported_additive_pose")
    if "body" in mapped:
        found.append("unsupported_body_motion(body_slots)")
    found += [f"unsupported_humanoid_axes({bone})" for bone in entry["missing_axes"]]
    animated = {row["bone"] for row in entry["axes"]}
    for name in TWIST + AVATAR_SETTINGS:
        if entry["avatar_settings"][name] != neutral_avatar[name] and animated & set(LIMBS[name]):
            kind = "twist" if name in TWIST else "avatar_setting"
            found.append(f"unsupported_humanoid_{kind}({name})")
    found += [f"unsupported_humanoid_clip_settings({field})" for field in entry["verdicts"]["clip_settings"]]
    found += [f"unsupported_humanoid_root_basis({field})" for field in entry["verdicts"]["root_basis"]]
    residual = entry["verdicts"]["residual"]
    if not residual["hips_local"]:
        found.append("unsupported_body_motion(hips_correction)")
    if not residual["root"]:
        found.append("unsupported_body_motion(root_projection)")
    return sorted(set(found))


def reasons(entry):
    """Every qualification criterion of plan section 2.4 the clip misses, in words."""
    result = [f"mapped {row['name']}" for row in entry["slots"]
              if row["storage"] != "absent" and row["kind"] != "muscle"]
    result += [f"Avatar {name} = {value}" for name, value in entry["avatar_settings"].items()
               if name != "m_HasTDoF" and value != entry["neutral_avatar"].get(name, value)]
    result += [f"{row['field']} = {row['serialized']}" for row in entry["settings"]
               if row["field"] in entry["verdicts"]["clip_settings"] + entry["verdicts"]["root_basis"]]
    for name, row in entry["root_hips"]["reset"].items():
        if not row["within"]:
            result.append(f"{name} differs from the reset pose beyond its rest bound: {row}")
    root_hips = entry["root_hips"]
    if root_hips["baseline_kind"] is None:
        result.append("no known-vector baseline")
    elif root_hips["baseline_kind"] == "known_vectors" and not root_hips["baseline_within"]:
        result.append(f"the known-vector baseline differs beyond its bound: {root_hips['baseline']}"
                      + ("" if root_hips["baseline_root_at_rest"] else ", and moves its root"))
    return result


def spec_identity(spec):
    keys = ("bundles", "prefab_asset", "clip_asset", "avatar_asset", "binding_address", "binding_component",
            "prefab", "clip", "binding", "avatar")
    return {key: spec[key] for key in keys}


def assets(doc, view):
    spec = doc["spec"]
    return (view.value((spec["clip"]["file_index"], int(spec["clip"]["path_id"]))),
            view.value((spec["avatar"]["file_index"], int(spec["avatar"]["path_id"]))))


def qualify_clip(name, doc, baseline, view, neutral_avatar):
    spec, capture = doc["spec"], doc["capture"]
    require(doc["format"] == "unityrs.anim_replay" and spec["mode"] == "animator"
            and capture["playback"]["clip_human_motion"], f"{name}: expected a completed humanoid clip replay")
    clip, avatar = assets(doc, view)
    slots, counts = inventory(clip, capture)
    axes, dropped, missing_axes, writers = crosswalk(clip, avatar, spec, capture, slots, counts)
    clip_settings, avatar_settings = settings(clip, avatar)
    values = {row["field"]: row["serialized"] for row in clip_settings}
    entry = dict(case=name, editor_version=capture["editor_version"], spec=spec_identity(spec),
                 absent_slot=ABSENT_SLOT, storage_counts=counts, slots=slots, axes=axes,
                 dropped_muscles=dict(count=len(dropped), names=dropped), missing_axes=missing_axes,
                 generic_writers=writers, identity_root=identity_root(spec, capture),
                 settings=clip_settings, avatar_settings=avatar_settings, neutral_avatar=neutral_avatar,
                 root_basis_relation=dict(start_at_origin=values.get("m_StartAtOrigin"),
                                          keep_original_position_xz=values.get("m_KeepOriginalPositionXZ")),
                 body_bearing=any(row["kind"] == "body" and row["storage"] != "absent" for row in slots),
                 root_hips=comparison(doc, baseline, avatar, spec))
    reset = entry["root_hips"]["reset"]
    entry["verdicts"] = dict(
        clip_settings=[row["field"] for row in clip_settings
                       if row["field"] not in ROOT_BASIS and not neutral(row["serialized"])],
        root_basis=basis_off(values),
        residual={key: row["within"] for key, row in reset.items()})
    entry["muscle_pose"] = {"known_vectors": None}
    if baseline is not None:
        entry["muscle_pose"]["known_vectors"] = muscle_pose(baseline, axes)
        spine = next(row["node"] for row in capture["humanoid"]["bones"] if row["name"] == "Spine")
        entry["muscle_pose"]["clip_vs_known_vectors_spine_degrees"] = max(
            angle_degrees(trs(a["pose"]["transforms"][spine])["rotation"], trs(b["pose"]["transforms"][spine])["rotation"])
            for a, b in zip(capture["samples"], baseline["capture"]["samples"]))
    entry["causes"] = causes(entry, avatar, clip, neutral_avatar)
    if baseline is not None:
        entry["root_hips"]["baseline_kind"] = "known_vectors"
    else:
        # A basis matrix clip's baseline is its own prefab's reset rig.
        entry["root_hips"]["baseline_kind"] = "reset" if name in BASIS_CASES else None
        entry["root_hips"]["baseline_within"] = name in BASIS_CASES
    entry["reasons"] = reasons(entry)
    entry["qualifies"] = not entry["causes"] and entry["root_hips"]["baseline_within"]
    entry["verdict"] = "qualified_pending_playback" if entry["qualifies"] else "refused"
    expected = entry["expected_refusal"] = REFUSALS.get(name, "")
    require(not expected or expected in {cause.split("(")[0] for cause in entry["causes"]},
            f"{name}: causes {entry['causes']} lack the probe's {expected}")
    return entry


def humanoid_cases():
    """Every humanoid clip case: the required ones and the optional coverage cases."""
    cases = {name: fixture for name, fixture in REPLAY_CASES.items() if fixture.bundle.startswith("humanoid-")}
    cases.update({name: fixture for name, fixture in COVERAGE_CASES.items() if "spine" in fixture.checks})
    return dict(sorted(cases.items()))


def expected_cases(manifest):
    """The required clips and every optional humanoid clip ``run.json`` marks recorded."""
    statuses = {row["name"]: row["status"] for row in manifest["cases"]}
    optional = sorted(set(humanoid_cases()) - set(REQUIRED_CASES))
    return [*REQUIRED_CASES, *(name for name in optional if statuses.get(name) == "recorded")]


def qualify_folder(folder, unityrs):
    manifest, run = read_manifest(folder, replay)
    cases = humanoid_cases()
    expected = sorted(expected_cases(manifest))
    views, docs = {}, {}
    for name in expected:
        fixture = cases[name]
        path = folder / "replays" / f"{name}.json"
        require(path.is_file(), f"{name}: missing completed replay {path.name}")
        doc = replay.read_json(path.read_text(encoding="utf-8"))
        bundle = folder / "bundles" / fixture.bundle
        digest = hashlib.sha256(bundle.read_bytes()).hexdigest()
        require([row["sha256"] for row in doc["spec"]["bundles"]] == [digest], f"{name}: bundle digest differs")
        if fixture.bundle not in views:
            views[fixture.bundle] = BundleView(bundle, digest, 0, unityrs, None, replay, run)
        docs[name] = (doc, views[fixture.bundle])
    twin = next((name for name in NEUTRAL_TWINS if name in docs), None)
    require(twin is not None, "no neutral twin clip recorded")
    _, twin_avatar = assets(*docs[twin])
    twin_values = settings({"m_MuscleClip": {}}, twin_avatar)[1]
    neutral_avatar = {name: twin_values[name] for name in TWIST + AVATAR_SETTINGS}
    entries = []
    for name, (doc, view) in docs.items():
        vector = folder / "replays" / "known-vectors" / f"{name}{VECTOR_SUFFIX}.pending.json"
        baseline = replay.read_json(vector.read_text(encoding="utf-8")) if vector.exists() else None
        try:
            entries.append(qualify_clip(name, doc, baseline, view, neutral_avatar))
        except (KeyError, TypeError, IndexError, ValueError, StopIteration) as exc:
            raise ValueError(f"{name}: {exc!r}") from exc
    for entry in entries:
        # The identity-root twin, by the case table's prefab, not by the case name.
        fixture = cases[entry["case"]]
        twin_case = next((name for name, other in cases.items() if fixture.prefab == "offset"
                          and other == replace(fixture, prefab="identity")), None)
        if twin_case in docs:
            # The same clip on an identity Animator root, sampled on the same schedule.
            doc, other = docs[entry["case"]][0], docs[twin_case][0]
            hips = entry["root_hips"]["hips_node"]
            samples, twins = doc["capture"]["samples"], other["capture"]["samples"]
            require([row["local_f32"] for row in samples] == [row["local_f32"] for row in twins],
                    f"{entry['case']}: identity twin schedule differs")
            entry["root_hips"]["offset_vs_identity"] = judge(
                [(a["pose"]["transforms"][hips], b["pose"]["transforms"][hips]) for a, b in zip(samples, twins)],
                entry["root_hips"]["bounds"]["hips_local"])
    require([entry["case"] for entry in entries] == expected and entries, "incomplete humanoid clip set")
    return dict(format=FORMAT, version=VERSION, run_id=run["run_id"], editor_version=run["editor_version"],
                build_target=run["build_target"], verified_editor_versions=[run["editor_version"]],
                expected_cases=expected, neutral_twin=dict(case=twin, values=neutral_avatar),
                qualified=[row["case"] for row in entries if row["qualifies"]], clips=entries)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("folder", type=Path, help="completed <version>/<target>/ fixture folder")
    parser.add_argument("--unityrs", default="unityrs", help="unityrs CLI used for show and inspect")
    parser.add_argument("-o", "--output", type=Path, help="write the evidence here instead of stdout")
    args = parser.parse_args(argv)
    try:
        document = qualify_folder(args.folder, args.unityrs)
    except (ValueError, OSError) as exc:
        print(f"anim_humanoid_qualify: {exc}", file=sys.stderr)
        return 1
    text = json.dumps(document, indent=2, ensure_ascii=False, allow_nan=False) + "\n"
    if args.output is None:
        sys.stdout.write(text)
    else:
        args.output.write_text(text, encoding="utf-8", newline="\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
