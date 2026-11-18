"""Humanoid qualification evidence for plan section 2.4.

Point this at a completed Editor fixture run, the ``<version>/<target>/``
folder ``anim_replay.py complete`` accepted. For each humanoid clip it reads
the built clip and Avatar through ``unityrs show`` and records:

- the slot inventory from ``m_IndexArray`` and its absent sentinel, with the
  storage ranges, crosswalked against the built bindings, the Editor curve
  bindings and the recorder's ``HumanTrait`` muscle and bone order;
- the runtime axes of each mapped muscle, against the recorded limits;
- the body-bearing test, the effective settings and Avatar twist;
- the root and hips-local comparison with the reset rig and with the
  same-schedule known-vector ``SetHumanPose`` baseline;
- the muscle-to-pose residual of the candidate joint-axis equation;
- whether the clip qualifies and the typed refusal the Rust reader must give.

``python3 -I tools/parity/anim_humanoid_qualify.py RUN_DIR --unityrs CLI -o OUT``
writes ``unityrs.anim_humanoid_qualification`` version 1. The Rust replay
test ``recorded_humanoid_refusals`` reads it with the same bundles.
"""

import argparse
import hashlib
import json
import math
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import anim_replay as replay  # noqa: E402
from anim_fixture_contract import REPLAY_CASES, VECTOR_SUFFIX  # noqa: E402
from anim_fixture_coverage import COVERAGE_CASES, REFUSALS  # noqa: E402
from anim_replay_complete import BundleView, read_manifest  # noqa: E402

FORMAT = "unityrs.anim_humanoid_qualification"
ABSENT_SLOT = -1
MUSCLE_COUNT = 95
REQUIRED_BONES = (0, 1, 2, 3, 4, 5, 6, 7, 10, 13, 14, 15, 16, 17, 18)
GOALS = ("LeftFoot", "RightFoot", "LeftHand", "RightHand")
TQ = ("T.x", "T.y", "T.z", "Q.x", "Q.y", "Q.z", "Q.w")
# Refusal order, as in crates/unityrs-anim/src/humanoid/clip.rs.
SETTINGS = (
    "m_Mirror", "m_CycleOffset", "m_Level", "m_OrientationOffsetY", "m_LoopTime", "m_LoopBlend",
    "m_LoopBlendOrientation", "m_LoopBlendPositionY", "m_LoopBlendPositionXZ", "m_KeepOriginalOrientation",
    "m_KeepOriginalPositionY", "m_KeepOriginalPositionXZ", "m_HeightFromFeet", "m_StartAtOrigin",
)
TWIST = ("m_ArmTwist", "m_ForeArmTwist", "m_UpperLegTwist", "m_LegTwist")
AVATAR_SETTINGS = ("m_ArmStretch", "m_LegStretch", "m_FeetSpacing")
# Residuals at or below these bounds count as equal: a few f32 units at the
# rig's metre scale, far below the decimetre and degree effects measured.
TRANSLATION_BOUND = 1e-5
ROTATION_BOUND_DEGREES = 1e-3
require = replay.require


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
    actual, expected = trs(actual), trs(expected)
    return {
        "translation": max(abs(a - b) for a, b in zip(actual["translation"], expected["translation"])),
        "rotation_degrees": angle_degrees(actual["rotation"], expected["rotation"]),
        "scale": max(abs(a - b) for a, b in zip(actual["scale"], expected["scale"])),
    }


def worst(rows):
    return {key: max(row[key] for row in rows) for key in rows[0]}


def within(row):
    return row["translation"] <= TRANSLATION_BOUND and row["scale"] <= TRANSLATION_BOUND \
        and row["rotation_degrees"] <= ROTATION_BOUND_DEGREES


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


def crosswalk(clip, avatar, spec, capture, slots):
    """Check bindings, bones, muscles and limits against the recorder's order."""
    by_dof = {row["dof"]: row for row in slots}
    bindings = clip["m_ClipBindingConstant"]["genericBindings"]
    require(len(bindings) == sum(row["storage"] != "absent" for row in slots), "binding and slot counts differ")
    for slot, row in enumerate(bindings):
        require(row["typeID"] == 95 and row["customType"] == 8 and by_dof[row["attribute"]]["index_value"] == slot,
                f"built binding {slot} disagrees with m_IndexArray")
    editor = {row["property"] for row in spec["evidence"]["editor"]["bindings"]}
    built = {by_dof[row["attribute"]]["name"] for row in bindings}
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
        built = None if node is None else paths.get(skeleton["m_ID"][node])
        require((built is None) == (recorded is None) and (built is None or nodes.get(recorded) == built),
                f"human bone {row['name']} maps to a different node")
        require(row["required"] == (bone in REQUIRED_BONES), f"required flag differs for {row['name']}")
    axes = []
    limit_rows = {row["human"]: row for row in capture["humanoid"]["limits"]}
    for ordinal, row in enumerate(capture["humanoid"]["muscles"]):
        bone = muscle_bone(ordinal)
        require(row["bone"] == bone, f"muscle {row['name']} rotates another bone")
        node = bone_node(human, bone)
        if node is None:
            continue
        record_id = skeleton["m_Node"][node]["m_AxesId"]
        require(record_id >= 0, f"bone of {row['name']} has no runtime axes")
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
        if by_dof[42 + ordinal]["storage"] != "absent":
            axes.append(dict(muscle=row["name"], bone=bone, node=node, axis=row["axis"],
                             pre=vec(record["m_PreQ"]), post=vec(record["m_PostQ"]), sign=vec(record["m_Sgn"]),
                             min=vec(record["m_Limit"]["m_Min"]), max=vec(record["m_Limit"]["m_Max"]),
                             type=record["m_Type"]))
    return axes


def comparison(doc, baseline, avatar, spec):
    """Root and hips-local TRS against the reset rig and the known-vector capture."""
    capture = doc["capture"]
    root = next(i for i, row in enumerate(spec["nodes"]) if row["address"] == spec["binding_address"])
    hips = next(row["node"] for row in capture["humanoid"]["bones"] if row["name"] == "Hips")
    hips_address = spec["nodes"][hips]["address"]
    require(hips_address.rsplit("/", 1)[0] == spec["binding_address"], "hips parent is not the Animator node")
    rest = capture["rest"]["transforms"]
    samples = capture["samples"]
    result = {"root_node": root, "hips_node": hips}
    result["reset"] = {name: worst([residual(row["pose"]["transforms"][node], rest[node]) for row in samples])
                       for name, node in (("root", root), ("hips_local", hips))}
    if baseline is None:
        return result
    other = baseline["capture"]["samples"]
    require([row["local_f32"] for row in other] == [row["local_f32"] for row in samples],
            "known-vector schedule differs")
    require(baseline["capture"]["bundle_sha256"] == capture["bundle_sha256"], "known-vector bundle differs")
    # The raw recorder output, not a completed replay: its digest names the input.
    result["baseline_recording_id"] = replay._known_vector_id(baseline)
    result["baseline"] = {name: worst([residual(a["pose"]["transforms"][node], b["pose"]["transforms"][node])
                                       for a, b in zip(samples, other)])
                          for name, node in (("root", root), ("hips_local", hips))}
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


def refusal(entry, avatar, clip, axes):
    """The first typed refusal in the Rust reader's order (``humanoid_refusal``)."""
    mapped = {row["kind"] for row in entry["slots"] if row["storage"] != "absent"}
    if "motion" in mapped or avatar["m_Avatar"]["m_RootMotionBoneIndex"] >= 0 \
            or clip.get("m_HasGenericRootTransform") or clip.get("m_HasMotionFloatCurves"):
        return "unsupported_locomotion"
    if "goal" in mapped:
        return "unsupported_humanoid_ik"
    if "translation" in mapped or entry["avatar_settings"]["m_HasTDoF"]:
        return "unsupported_translation_dof"
    if float_count(clip["m_MuscleClip"]["m_ValueArrayReferencePose"]):
        return "unsupported_additive_pose"
    if "body" in mapped:
        return "unsupported_body_motion(body_slots)"
    for name in TWIST:
        if entry["avatar_settings"][name] != 0:
            return f"unsupported_humanoid_twist({name})"
    for name in AVATAR_SETTINGS:
        if entry["avatar_settings"][name] != 0:
            return f"unsupported_humanoid_avatar_setting({name})"
    for row in entry["settings"]:
        if not neutral(row["serialized"]):
            return f"unsupported_humanoid_clip_settings({row['field']})"
    return "unsupported_body_motion(hips_correction)"


def reasons(entry):
    """Every qualification criterion of plan section 2.4 the clip misses."""
    result = [f"mapped {row['name']}" for row in entry["slots"]
              if row["storage"] != "absent" and row["kind"] != "muscle"]
    result += [f"Avatar {name} = {value}" for name, value in entry["avatar_settings"].items()
               if name != "m_HasTDoF" and value != 0]
    result += [f"{row['field']} = {row['serialized']}" for row in entry["settings"] if not neutral(row["serialized"])]
    compared = entry["root_hips"]
    for scope in ("reset", "baseline"):
        for name, row in compared.get(scope, {}).items():
            if not within(row):
                result.append(f"{name} differs from the {scope} pose: {row}")
    return result


def spec_identity(spec):
    keys = ("bundles", "prefab_asset", "clip_asset", "avatar_asset", "binding_address", "binding_component",
            "prefab", "clip", "binding", "avatar")
    return {key: spec[key] for key in keys}


def qualify_clip(name, doc, baseline, view, run):
    spec, capture = doc["spec"], doc["capture"]
    require(doc["format"] == "unityrs.anim_replay" and spec["mode"] == "animator"
            and capture["playback"]["clip_human_motion"], f"{name}: expected a completed humanoid clip replay")
    clip = view.value((spec["clip"]["file_index"], int(spec["clip"]["path_id"])))
    avatar = view.value((spec["avatar"]["file_index"], int(spec["avatar"]["path_id"])))
    slots, counts = inventory(clip, capture)
    axes = crosswalk(clip, avatar, spec, capture, slots)
    clip_settings, avatar_settings = settings(clip, avatar)
    entry = dict(case=name, editor_version=capture["editor_version"], spec=spec_identity(spec),
                 absent_slot=ABSENT_SLOT, storage_counts=counts, slots=slots, axes=axes,
                 settings=clip_settings, avatar_settings=avatar_settings,
                 body_bearing=any(row["kind"] == "body" and row["storage"] != "absent" for row in slots),
                 root_hips=comparison(doc, baseline, avatar, spec))
    entry["muscle_pose"] = {"known_vectors": None}
    if baseline is not None:
        entry["muscle_pose"]["known_vectors"] = muscle_pose(baseline, axes)
        spine = next(row["node"] for row in capture["humanoid"]["bones"] if row["name"] == "Spine")
        entry["muscle_pose"]["clip_vs_known_vectors_spine_degrees"] = max(
            residual(a["pose"]["transforms"][spine], b["pose"]["transforms"][spine])["rotation_degrees"]
            for a, b in zip(capture["samples"], baseline["capture"]["samples"]))
    entry["reasons"] = reasons(entry)
    entry["qualifies"] = False
    entry["refusal"] = refusal(entry, avatar, clip, axes)
    expected = REFUSALS.get(name)
    require(expected is None or entry["refusal"].split("(")[0] == expected,
            f"{name}: refusal {entry['refusal']} differs from the probe's {expected}")
    return entry


def humanoid_cases():
    cases = {name: fixture for name, fixture in REPLAY_CASES.items() if fixture.bundle.startswith("humanoid-")}
    cases.update({name: fixture for name, fixture in COVERAGE_CASES.items() if name in REFUSALS})
    return dict(sorted(cases.items()))


def qualify_folder(folder, unityrs):
    manifest, run = read_manifest(folder, replay)
    del manifest
    views, entries = {}, []
    for name, fixture in humanoid_cases().items():
        path = folder / "replays" / f"{name}.json"
        if not path.exists():
            continue
        doc = replay.read_json(path.read_text(encoding="utf-8"))
        vector = folder / "replays" / "known-vectors" / f"{name}{VECTOR_SUFFIX}.pending.json"
        baseline = replay.read_json(vector.read_text(encoding="utf-8")) if vector.exists() else None
        bundle = folder / "bundles" / fixture.bundle
        digest = hashlib.sha256(bundle.read_bytes()).hexdigest()
        require([row["sha256"] for row in doc["spec"]["bundles"]] == [digest], f"{name}: bundle digest differs")
        if fixture.bundle not in views:
            views[fixture.bundle] = BundleView(bundle, digest, 0, unityrs, None, replay, run)
        try:
            entries.append(qualify_clip(name, doc, baseline, views[fixture.bundle], run))
        except (KeyError, TypeError, IndexError, ValueError, StopIteration) as exc:
            raise ValueError(f"{name}: {exc!r}") from exc
    require(entries, "no completed humanoid replays")
    return dict(format=FORMAT, version=1, run_id=run["run_id"], editor_version=run["editor_version"],
                build_target=run["build_target"], bounds=dict(translation=TRANSLATION_BOUND,
                                                              rotation_degrees=ROTATION_BOUND_DEGREES),
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
