"""Complete and validate L11 recordings and derive their a priori f32 tolerances."""
import argparse
import hashlib
import json
import math
import re
import struct
import sys
from bisect import bisect_left
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from anim_reference import BOUNDS, angle, angular_bound, ulp32  # noqa: E402
from anim_replay_bounds import budget_model, roundoff  # noqa: E402

SCHEMA = json.loads(Path(__file__).with_name("anim_replay.schema.json").read_text(encoding="utf-8"))
TRS = ("translation", "rotation", "scale")
API = {"animator": "AnimationClipPlayable", "legacy": "Animation.Sample",
       "human_pose": "HumanPoseHandler.SetHumanPose"}
BODY_DOFS = {f"RootT.{axis}" for axis in "xyz"} | {f"RootQ.{axis}" for axis in "xyzw"}
MUSCLE_COUNT = 95  # HumanTrait order for the supported fixture versions
F64_ANGLE_FLOOR = BOUNDS["transcription"]["angular_floor_u64"] * BOUNDS["transcription"]["unit_roundoff"]
NEUTRAL_SETTINGS = dict.fromkeys((
    "loop", "loopTime", "loopPose", "loopBlend", "loopBlendOrientation", "loopBlendPositionY",
    "loopBlendPositionXZ", "lockRootRotation", "lockRootHeightY", "lockRootPositionXZ",
    "keepOriginalOrientation", "keepOriginalPositionY", "keepOriginalPositionXZ",
    "heightFromFeet", "startAtOrigin", "mirror", "hasAdditiveReferencePose", "translation_dof",
), False) | dict.fromkeys((
    "orientationOffsetY", "level", "rotationOffset", "heightOffset", "cycleOffset",
    "additiveReferencePoseTime", "upper_arm_twist", "lower_arm_twist", "upper_leg_twist",
    "lower_leg_twist",
), 0) | {"additiveReferencePoseClipName": ""}


def require(ok, message):
    if not ok:
        raise ValueError(message)


def read_json(text):
    def unique(pairs):
        result = {}
        for key, value in pairs:
            require(key not in result, f"duplicate JSON member {key}")
            result[key] = value
        return result

    def nonfinite(token):
        raise ValueError(f"invalid JSON number {token}")

    return json.loads(text, object_pairs_hook=unique, parse_constant=nonfinite)


def _digest(value):
    encoded = json.dumps(value, sort_keys=True, ensure_ascii=False, separators=(",", ":"), allow_nan=False)
    return hashlib.sha256(encoded.encode("utf-8")).hexdigest()


def _same(a, b):
    return a == b and (type(a) is bool) == (type(b) is bool)


def _matches(schema, value, path):
    try:
        _shape(schema, value, path)
        return True
    except ValueError:
        return False


def _shape(schema, value, path):
    """The small Draft 2020-12 subset used by this checked-in schema."""
    if "$ref" in schema:
        _shape(SCHEMA["$defs"][schema["$ref"].split("/")[-1]], value, path)
        return
    if "anyOf" in schema:
        require(any(_matches(choice, value, path) for choice in schema["anyOf"]),
                f"{path}: no schema alternative matched")
    types = {"null": (type(None),), "object": (dict,), "array": (list,), "string": (str,),
             "boolean": (bool,), "number": (int, float), "integer": (int,)}
    if "type" in schema:
        require(type(value) in types[schema["type"]], f"{path}: expected {schema['type']}")
    if "const" in schema:
        require(_same(value, schema["const"]), f"{path}: expected {schema['const']!r}")
    if "enum" in schema:
        require(any(_same(value, v) for v in schema["enum"]), f"{path}: unknown value")
    if type(value) is dict:
        fields = schema.get("properties", {})
        require(value.keys() >= set(schema.get("required", ())), f"{path}: missing required member")
        for key, item in value.items():
            require(schema.get("additionalProperties", True) or key in fields,
                    f"{path}: unknown member {key}")
            _shape(fields.get(key, {}), item, f"{path}/{key}")
    elif type(value) in (list, str):
        suffix = "Items" if type(value) is list else "Length"
        require(schema.get("min" + suffix, 0) <= len(value) <= schema.get("max" + suffix, math.inf),
                f"{path}: wrong length")
        if type(value) is list:
            for index, item in enumerate(value):
                _shape(schema.get("items", {}), item, f"{path}/{index}")
            if "contains" in schema:
                require(any(_matches(schema["contains"], item, path) for item in value),
                        f"{path}: missing {schema['title']}")
        if "pattern" in schema:
            require(re.fullmatch(schema["pattern"], value) is not None, f"{path}: invalid spelling")
        if schema is SCHEMA["$defs"]["f32"]:
            f32_from_bits(value)
    elif type(value) in (int, float):
        require(-math.inf < value < math.inf and schema.get("minimum", -math.inf) <= value
                <= schema.get("maximum", math.inf) and value > schema.get("exclusiveMinimum", -math.inf),
                f"{path}: number outside range")
    for constraint in schema.get("allOf", ()):
        _shape(constraint, value, path)


def f32_bits(value):
    try:
        bits = struct.pack(">f", value).hex()
    except (OverflowError, struct.error) as exc:
        raise ValueError("f32 overflow") from exc
    f32_from_bits(bits)
    return bits


def f32_from_bits(bits):
    number, = struct.unpack(">f", bytes.fromhex(bits))
    require(math.isfinite(number), "expected finite f32 bits")
    return number


def _ordered(values):
    return all(a < b for a, b in zip(values, values[1:]))


def _target_key(row):
    return row["property"], row["target"], row["channel"]


def _targets(spec):
    return ([(prop, i, -1) for i in range(len(spec["nodes"])) for prop in TRS]
            + [("weight", i, c["index"]) for i, r in enumerate(spec["renderers"]) for c in r["channels"]])


def _validate_clock(spec, start, stop, length):
    evidence = spec["evidence"]["clock"]
    times = [row["local"] for row in spec["schedule"]]
    require(_ordered(times) and times[0] == 0 and times[-1] == stop - start, "invalid schedule endpoints or order")
    require((evidence["start"]["source"], evidence["stop"]["source"]) == (start, stop),
            "source origin disagrees with independently inspected endpoints")
    boundaries = spec["held_boundaries"]
    require(_ordered(boundaries) and all(start <= t <= stop for t in boundaries), "invalid held boundaries")
    require([row["source"] for row in evidence["held"]] == boundaries, "missing or mismatched held observations")
    keys = evidence["key_times"]
    require(_ordered(keys) and keys[0] == start and keys[-1] == stop and set(boundaries) <= set(keys),
            "invalid clock evidence key times")
    authored_keys = evidence.get("authored_key_times", [])
    if authored_keys:
        require(_ordered(authored_keys) and authored_keys[0] == start and authored_keys[-1] == stop,
                "invalid authored key times")
    require(all(float(t) == t for t in (*boundaries, *keys, *authored_keys, *times)),
            "time is not exactly representable as binary64")
    effective = [clocks(spec, time, length) for time in times]
    if stop > start:
        runtime_source_bits = {f32_bits(t) for t in keys} if authored_keys else set()
        keys = sorted(set(keys) | set(authored_keys))
        key_locals = sorted({f32_from_bits(f32_bits(t - start)) for t in keys})
        key_sources = (sorted({f32_from_bits(f32_bits(t)) for t in keys})
                       if spec["source_clock"] == "f32_add" else keys)

        def between(keys, time):
            i = bisect_left(keys, time)
            return 0 < i < len(keys) and time < keys[i]

        require(any(start < source < stop and between(key_locals, f32_from_bits(bits)) and between(key_sources, source)
                    and (not authored_keys or between(keys, nominal) and f32_bits(source) not in runtime_source_bits)
                    for bits, nominal, source in effective[1:-1]), "nonzero interval requires an effective f32 off-key sample")
        require(_ordered([f32_from_bits(bits) for bits, _, _ in effective]), "duplicate effective f32 sample times")

    targets = set(_targets(spec))

    def probe_index(probe):
        key = _target_key(probe)
        widths = {"translation": 3, "rotation": 4, "scale": 3, "weight": 1}
        require(key in targets and probe["component"] < widths[key[0]] and probe["sample"] < len(times),
                "invalid clock probe sample or target")
        return probe["sample"]

    for endpoint, index in (("start", 0), ("stop", len(times) - 1)):
        require(probe_index(evidence[endpoint]["probe"]) == index, "clock probe must observe the endpoint sample")
    effective_sources = [source for _, _, source in effective]
    for row in evidence["held"]:
        left, right = row.get("left"), row["right"]
        first_right = bisect_left(effective_sources, row["source"])
        require(probe_index(right) == first_right, "held probe must observe the first effective right sample")
        if row["source"] == start:
            require(left is None, "held boundary at start has only a right-side probe")
        else:
            require(left is not None and probe_index(left) == first_right - 1,
                    "held probe must observe the last effective left sample")
            require((_target_key(left), left["component"]) == (_target_key(right), right["component"])
                    and f32_from_bits(left["value"]) != f32_from_bits(right["value"]),
                    "held probes must distinguish both sides of the same component")
    return _clock_discriminated(spec, times, length, effective_sources)


def _clock_discriminated(spec, times, length, selected_sources):
    boundaries = spec["held_boundaries"]
    if not boundaries:
        return False
    other = "f32_add" if spec["source_clock"] == "widened" else "widened"
    try:
        if other == "f32_add" and f32_from_bits(f32_bits(spec["start"])) != spec["start"]:
            return False
        alternative = spec | {"source_clock": other}
        other_sources = [clocks(alternative, time, length)[2] for time in times]
    except ValueError:
        # There is no f32-add prediction when source time overflows f32.
        return False
    # Different splits put at least one of the validated adjacent probes
    # on the opposite side under the other model.
    return any(bisect_left(selected_sources, jump) != bisect_left(other_sources, jump)
               for jump in boundaries)


def _validate_humanoid_spec(spec, human):
    require(spec["avatar"] is not None, "humanoid evidence requires an Avatar")
    slots = human["slots"]
    require(len({row["dof"] for row in slots}) == len(slots), "duplicate humanoid DOF")
    for row in slots:
        absent = row["storage"] == "absent"
        require(absent == (row["index_value"] == human["absent_slot"])
                and (absent or row["index_value"] >= 0), "invalid humanoid slot sentinel")
    comparison = human.get("root_hips_comparison")
    nodes = spec["nodes"]
    if comparison is not None:
        require(comparison["root_node"] < len(nodes) and comparison["hips_node"] < len(nodes),
                "unknown root or hips node")
    if human["disposition"] != "qualified_muscle_only":
        return

    body = {row["dof"]: row for row in slots if row["kind"] == "body"}
    require(body.keys() == BODY_DOFS and all(row["storage"] == "absent" for row in body.values()),
            "qualification requires the complete absent body slot inventory")
    muscles = [row for row in slots if row["kind"] == "muscle"]
    mapped = [row for row in slots if row["storage"] != "absent"]
    require(len(muscles) == MUSCLE_COUNT and mapped and all(row["kind"] == "muscle" for row in mapped),
            "qualification requires the complete muscle slot inventory and only muscle writers")
    counts = human.get("storage_counts")
    require(counts is not None and len({row["index_value"] for row in mapped}) == len(mapped),
            "qualification requires unambiguous humanoid storage ranges")
    offset = 0
    for storage in ("streamed", "dense", "constant"):
        end = offset + counts[storage]
        require(all(offset <= row["index_value"] < end for row in mapped if row["storage"] == storage),
                "humanoid slot lies outside its storage range")
        offset = end
    require(human["axes"], "qualification requires runtime axes")
    settings = {row["property"]: row for row in spec["evidence"]["import_settings"]}
    require(settings.keys() >= NEUTRAL_SETTINGS.keys(), "qualification requires complete effective import settings")
    for name, neutral in NEUTRAL_SETTINGS.items():
        row = settings[name]
        require(_same(row["effective"], neutral), f"non-neutral qualified import setting: {name}")
        serialized = row["serialized"]
        if type(neutral) is bool and type(serialized) is int and serialized in (0, 1):
            serialized = bool(serialized)
        require(_same(serialized, neutral), f"missing or non-neutral serialized import setting: {name}")

    require(spec["mode"] == "animator" and comparison is not None and "budgets" in spec,
            "qualification requires clip playback, root and hips-local baseline and frozen budgets")
    root, hips = (nodes[comparison[name]]["address"] for name in ("root_node", "hips_node"))
    require(root == spec["binding_address"] and hips != root and (not root or hips.startswith(root + "/")),
            "hips must be a distinct descendant of the Animator root")


def validate_spec(spec, *, pending_humanoid=False):
    _shape(SCHEMA["$defs"]["spec"], spec, "/spec")
    bundles = spec["bundles"]
    require(len({bundle["name"] for bundle in bundles}) == len(bundles), "duplicate bundle name")
    require(len({bundle["sha256"] for bundle in bundles}) == len(bundles), "duplicate bundle digest")
    files = {}

    def identity(obj):
        bundle, index, path = obj["bundle"], obj["file_index"], int(obj["path_id"])
        require(bundle < len(bundles) and -(2**63) <= path < 2**63, "out-of-range serialized identity")
        require(files.setdefault((bundle, index), obj["file"]) == obj["file"], "inconsistent serialized file name")
        return bundle, index, path

    objects = [identity(spec[k]) for k in ("prefab", "clip", "binding")]
    if spec["avatar"] is not None:
        _shape(SCHEMA["$defs"]["asset"], spec["avatar_asset"], "/spec/avatar_asset")
        objects.append(identity(spec["avatar"]))
        require(spec["mode"] != "legacy", "legacy binding cannot have an Avatar")
    else:
        require(spec["avatar_asset"] == "" and spec["mode"] != "human_pose", "missing Avatar identity")
    for asset in ("prefab_asset", "clip_asset", "avatar_asset"):
        require(all(p not in (".", "..") for p in spec[asset].split("/")), "expected a bundle asset key")
    nodes = spec["nodes"]
    addresses = {node["address"] for node in nodes}
    require(len(addresses) == len(nodes), "duplicate node address")
    require(nodes[0]["address"] == "" and spec["binding_address"] in addresses
            and all(p == "" or p.rpartition("/")[0] in addresses for p in addresses),
            "invalid prefab node or binding address")
    objects.extend(identity(node["id"]) for node in nodes)
    renderers = spec["renderers"]
    addresses_seen = set()
    meshes = {}
    for renderer in renderers:
        key = renderer["address"], renderer["component"]
        require(key not in addresses_seen, "duplicate renderer component address")
        addresses_seen.add(key)
        require(key[0] in addresses, "renderer node is absent")
        objects.append(identity(renderer["id"]))
        mesh = identity(renderer["mesh"])
        channels = renderer["channels"]
        require([c["index"] for c in channels] == list(range(len(channels))), "invalid channel indices")
        require(meshes.setdefault(mesh, channels) == channels, "inconsistent shared mesh channels")
        require(all(_ordered(c["full_weights"]) for c in channels), "invalid mesh frame weight order")
        require(all(f32_from_bits(f32_bits(v)) == v for c in channels for v in c["full_weights"]),
                "mesh full weight is not an exact f32")
    require(len(set(objects)) == len(objects), "duplicate serialized object identity")
    require(not set(objects).intersection(meshes), "mesh identity aliases a different object")
    discovery = spec["evidence"]["discovery"]
    slots = [(identity(r["owner"]), r["field"], r["ordinal"]) for r in discovery]
    require(len(set(slots)) == len(slots), "duplicate discovery slot")
    for row in discovery:
        for role in ("original", "resolved"):
            if row[role] is not None:
                identity(row[role])

    start, stop = float(spec["start"]), float(spec["stop"])
    # These are decoded source timings, not asset-import frame numbers.
    require(start == spec["start"] and stop == spec["stop"] and start <= stop
            and math.isfinite(stop - start), "invalid binary64 source interval")
    length = f32_from_bits(f32_bits(stop - start))
    require(length > 0 or stop == start, "collapsed playback interval")
    require(f32_from_bits(f32_bits(spec["sample_rate"])) == spec["sample_rate"], "invalid serialized f32 rate")
    if spec["source_clock"] == "f32_add":
        require(f32_from_bits(f32_bits(start)) == start, "f32 source clock needs an exact f32 start")
    clock_discriminated = _validate_clock(spec, start, stop, length)
    require(all(("muscles" in row) == (spec["mode"] == "human_pose") for row in spec["schedule"]),
            "only known-vector playback requires muscles in the schedule")
    if spec["mode"] == "human_pose":
        require(not spec["held_boundaries"] and (pending_humanoid or "humanoid" in spec["evidence"]),
                "invalid known-vector evidence")
    settings = spec["evidence"]["import_settings"]
    require(len({row["property"] for row in settings}) == len(settings), "duplicate effective import setting")
    human = spec["evidence"].get("humanoid")
    paths = spec.get("editor_axis_properties", [])
    require(len(set(paths)) == len(paths) and (not paths or human is not None), "invalid Editor axis paths")
    if human is not None:
        _validate_humanoid_spec(spec, human)
    if "budgets" in spec:
        budgets = spec["budgets"]
        keys = [_target_key(row) for row in budgets]
        require(len(set(keys)) == len(keys) and set(keys) == set(_targets(spec)), "invalid budget coverage")
        for row in budgets:
            budget_model(row)
    return clock_discriminated


def clocks(spec, local, length):
    local_f32 = f32_bits(local)
    effective_local = f32_from_bits(local_f32)
    start, stop = float(spec["start"]), float(spec["stop"])
    source = stop if local == stop - start else start + local
    if effective_local == length:
        effective = stop
    else:
        effective = start + effective_local
        if spec["source_clock"] == "f32_add":
            effective = f32_from_bits(f32_bits(effective))
        effective = max(start, min(stop, effective))
    return local_f32, source, effective


def _pose_values(spec, pose):
    require(len(pose["transforms"]) == len(spec["nodes"]) and len(pose["weights"]) == len(spec["renderers"]),
            "incomplete pose coverage")
    for node in pose["transforms"]:
        for prop in TRS:
            values = tuple(map(f32_from_bits, node[prop]))
            require(prop != "rotation" or any(values), "invalid captured quaternion")
            yield values
    for renderer, weights in zip(spec["renderers"], pose["weights"]):
        channels = weights["channels"]
        require(len(channels) == len(renderer["channels"]), "incomplete mesh channel coverage")
        for value in channels:
            yield (f32_from_bits(value),)


def _validate_probes(spec, samples):
    clock = spec["evidence"]["clock"]
    probes = [clock[side]["probe"] for side in ("start", "stop")]
    probes.extend(row[side] for row in clock["held"] for side in ("left", "right") if side in row)
    for probe in probes:
        pose = samples[probe["sample"]]["pose"]
        prop, target, channel = _target_key(probe)
        if prop == "weight":
            actual = pose["weights"][target]["channels"][channel]
        else:
            actual = pose["transforms"][target][prop][probe["component"]]
        require(actual == probe["value"], "captured pose disagrees with independent clock probe")


def _baseline_samples(spec, capture, comparison, baselines):
    recording = baselines.get(comparison["baseline_capture_id"])
    require(recording is not None, "unknown baseline capture id")
    baseline_spec, baseline = recording["spec"], recording["capture"]
    require(all(baseline[key] == capture[key] for key in ("bundle_sha256", "editor_version", "build_target"))
            and all(baseline_spec[key] == spec[key] for key in
                    ("prefab", "binding", "avatar", "binding_address", "binding_component", "nodes")),
            "root and hips-local baseline must use the same built rig and Editor")
    require([row["local_f32"] for row in baseline["samples"]] == [row["local_f32"] for row in capture["samples"]],
            "root and hips-local baseline schedule mismatch")
    root, hips = comparison["root_node"], comparison["hips_node"]
    require([bone["node"] for bone in baseline["humanoid"]["bones"] if bone["name"] == "Hips"] == [hips],
            "hips node disagrees with the known-vector capture")

    def root_hips(samples):
        return [(row["pose"]["transforms"][root], row["pose"]["transforms"][hips]) for row in samples]

    # A muscle-only clip may leave root and hips-local unchanged, so samples
    # equal to the clip's are a valid zero residual. Distinctness comes from
    # the separate known-vector capture checked above, not from the values.
    return root_hips(baseline["samples"])


def _validate_qualification(spec, capture, baselines):
    human = spec["evidence"].get("humanoid")
    if human is None:
        return
    comparison = human.get("root_hips_comparison")
    baseline_poses = _baseline_samples(spec, capture, comparison, baselines) if comparison is not None else None
    if human["disposition"] != "qualified_muscle_only":
        return
    require(capture["playback"]["clip_human_motion"], "qualified playback requires an observed human-motion clip")
    captured = capture["humanoid"]
    muscles = [row for row in human["slots"] if row["kind"] == "muscle"]
    require([row["dof"] for row in muscles] == [row["name"] for row in captured["muscles"]]
            and len({(row["bone"], row["axis"]) for row in captured["muscles"]}) == MUSCLE_COUNT,
            "qualified muscle slots disagree with the independent HumanTrait inventory")
    bones = captured["bones"]
    axes = {row["bone"]: row for row in human["axes"]}
    require(len(axes) == len(human["axes"]) and all(bone < len(bones) for bone in axes),
            "invalid qualified runtime axis mapping")
    for slot, muscle in zip(muscles, captured["muscles"]):
        if slot["storage"] != "absent":
            bone, axis = muscle["bone"], muscle["axis"]
            require(bones[bone]["node"] >= 0 and bone in axes, "mapped muscle needs a bone and runtime axes")
            record = axes[bone]
            require(all(any(map(f32_from_bits, record[name])) for name in ("pre", "post"))
                    and f32_from_bits(record["sign"][axis]) in (-1, 1)
                    and f32_from_bits(record["min"][axis]) <= 0 <= f32_from_bits(record["max"][axis]),
                    "invalid runtime axes for mapped muscle")

    hips = comparison["hips_node"]
    require([bone["node"] for bone in bones if bone["name"] == "Hips"] == [hips],
            "hips node disagrees with the independent HumanTrait map")
    settings = {row["property"]: row["effective"] for row in spec["evidence"]["import_settings"]}
    for name in NEUTRAL_SETTINGS.keys() & captured.keys():
        observed = captured[name]
        if isinstance(observed, str):
            observed = f32_from_bits(observed)
        require(_same(observed, settings[name]), f"effective Avatar setting mismatch: {name}")

    for sample, baseline, bounds in zip(capture["samples"], baseline_poses, _sample_bounds(spec, capture)):
        targets = (("root", comparison["root_node"]), ("hips_local", hips))
        for (name, target), expected_pose in zip(targets, baseline):
            actual = sample["pose"]["transforms"][target]
            for axis, prop in enumerate(TRS):
                measured, expected = (tuple(map(f32_from_bits, pose[prop])) for pose in (actual, expected_pose))
                residual = (angle(measured, expected) if prop == "rotation"
                            else max(abs(a - b) for a, b in zip(measured, expected)))
                require(residual <= bounds[3 * target + axis],
                        f"{name} {prop} residual exceeds frozen replay bound at {sample['local']}")


def _validate_recording(spec, capture, bundle_dir=None, *, pending_humanoid=False):
    clock_discriminated = validate_spec(spec, pending_humanoid=pending_humanoid)
    require(spec["build_target"] == capture["build_target"], "build target mismatch")
    require([b["sha256"] for b in spec["bundles"]] == capture["bundle_sha256"], "bundle digest mismatch")
    if bundle_dir is not None:
        for bundle in spec["bundles"]:
            with (bundle_dir / bundle["name"]).open("rb") as source:
                digest = hashlib.file_digest(source, "sha256").hexdigest()
            require(digest == bundle["sha256"], f"bundle digest mismatch: {bundle['name']}")
    mode, playback = spec["mode"], capture["playback"]
    require(playback["api"] == API[mode], "playback mode/API mismatch")
    settings = capture["clip_settings"]
    for entry in spec["evidence"]["import_settings"]:
        prop = entry["property"]
        if prop in settings:
            observed = settings[prop]
            if SCHEMA["$defs"]["clip_settings"]["properties"][prop].get("$ref") == "#/$defs/f32":
                observed = f32_from_bits(observed)
            require(_same(observed, entry["effective"]), f"effective clip setting mismatch: {prop}")
    require(playback["legacy_state"] == ("enabled_weight_one_clamp_forever" if mode == "legacy" else "unused"),
            "legacy state mismatch")
    require(mode != "animator" or not playback["clip_looping"], "looping Animator endpoint is unconfirmed")
    require(mode != "legacy" or playback["wrap_mode"] == "ClampForever", "legacy endpoint must be clamped")
    length = f32_from_bits(playback["clip_length_f32"])
    require(playback["clip_length_f32"] == f32_bits(float(spec["stop"]) - float(spec["start"]))
            and f32_from_bits(playback["frame_rate_f32"]) == spec["sample_rate"], "clip length or sample rate mismatch")
    human = capture.get("humanoid")
    require(human is not None or ("humanoid" not in spec["evidence"] and not playback["clip_human_motion"]),
            "missing humanoid capture")
    if human is not None:
        require(spec["avatar"] is not None, "humanoid capture requires an Avatar identity")
        require([row["path"] for row in human["axis_properties"]] == spec.get("editor_axis_properties", []),
                "Editor axis property coverage mismatch")
        axis_shapes = {"Float": "f32", "Vector3": "vec3", "Quaternion": "vec4"}
        for row in human["axis_properties"]:
            kind, values = row["kind"], row["value"]
            if kind == "Integer":
                require(len(values) == 1 and re.fullmatch(r"0|-?[1-9][0-9]{0,18}", values[0])
                        and -(2**63) <= int(values[0]) < 2**63, "invalid Editor axis integer")
            else:
                value = values[0] if kind == "Float" and len(values) == 1 else values
                _shape(SCHEMA["$defs"][axis_shapes[kind]], value, "/capture/humanoid/axis_properties")
        require(all(-1 <= b["node"] < len(spec["nodes"]) and (not b["required"] or b["node"] >= 0)
                    for b in human["bones"]), "invalid recorded bone mapping")
        require(all(m["bone"] < len(human["bones"]) for m in human["muscles"])
                and len(human["muscles"]) == len(human["rest_muscles"]), "invalid recorded muscle mapping")
        require(f32_from_bits(human["human_scale"]) > 0, "invalid captured human scale")
        if mode == "human_pose":
            require(all(len(t["muscles"]) == len(human["muscles"]) for t in spec["schedule"]),
                    "known-vector muscle count mismatch")
    list(_pose_values(spec, capture["rest"]))
    require(len(capture["samples"]) == len(spec["schedule"]), "sample coverage differs from schedule")
    for point, sample in zip(spec["schedule"], capture["samples"]):
        local_f32, source, effective = clocks(spec, point["local"], length)
        require((point["local"], local_f32, source, effective)
                == (sample["local"], sample["local_f32"], sample["source"], sample["effective_source"]),
                "sample clock mismatch")
        require(sample["sides"] == ["left" if effective < t else "right" for t in spec["held_boundaries"]],
                "incorrect held side")
        require(sample["muscles_f32"] == [f32_bits(value) for value in point.get("muscles", ())],
                "applied known-vector bits disagree with schedule")
        list(_pose_values(spec, sample["pose"]))
    _validate_probes(spec, capture["samples"])
    return clock_discriminated


def _known_vector_id(recording):
    spec, capture = recording["spec"], recording["capture"]
    require(spec["mode"] == "human_pose" and capture["playback"]["api"] == API["human_pose"]
            and "root_hips_comparison" not in spec["evidence"].get("humanoid", {}),
            "baseline must be a standalone known-vector SetHumanPose capture")
    return _digest(recording)


def validate(manifest, bundle_dir=None):
    """Check the recording and return whether a held probe discriminates the clock."""
    require(not (isinstance(manifest, dict) and manifest.get("format") == "unityrs.anim_replay_pending"),
            "pending replay needs the complete command")
    _shape(SCHEMA, manifest, "")
    spec, capture = manifest["spec"], manifest["capture"]
    clock_discriminated = _validate_recording(spec, capture, bundle_dir)
    baselines = {}
    for baseline in manifest.get("baseline_captures", ()):
        capture_id = _known_vector_id(baseline)
        require(capture_id not in baselines, "duplicate known-vector baseline capture")
        _validate_recording(baseline["spec"], baseline["capture"])
        baselines[capture_id] = baseline
    _validate_qualification(spec, capture, baselines)
    return clock_discriminated


def _sample_bounds(spec, capture):
    keys = _targets(spec)
    budgets = {_target_key(row): row for row in spec["budgets"]}
    models = [budget_model(budgets[key]) for key in keys]
    for sample in capture["samples"]:
        dt = abs(sample["effective_source"] - sample["source"])
        bounds = []
        for model, values in zip(models, _pose_values(spec, sample["pose"])):
            raw = model.error(dt)
            output = .5 * ulp32(max(map(abs, values)))
            if model.min_norm is not None:
                unit_error = output + roundoff(32, 1)
                bound = (angular_bound(raw, model.min_norm) + angular_bound(unit_error, 1.)
                         + F64_ANGLE_FLOOR)
            else:
                bound = raw + output
            bound = math.nextafter(bound * (1 + 2**-45), math.inf)
            require(math.isfinite(bound) and (model.min_norm is None or bound < math.pi),
                    "replay tolerance is nonfinite or reaches pi")
            bounds.append(bound)
        yield bounds


def derive(manifest, bundle_dir=None):
    """Return per-sample bounds in target order. Observations supply only the output ulp."""
    clock_discriminated = validate(manifest, bundle_dir)
    spec, capture = manifest["spec"], manifest["capture"]
    require("budgets" in spec, "derivation requires recorded budgets")
    result = [{k: sample[k] for k in ("source", "effective_source", "sides")} | {"bounds": bounds}
              for sample, bounds in zip(capture["samples"], _sample_bounds(spec, capture))]
    return {"format": "unityrs.anim_tolerances", "version": 1,
            "manifest_sha256": _digest(manifest),
            "clock_discriminated": clock_discriminated,
            "targets": _targets(spec), "samples": result}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("complete", "validate", "validate-spec", "derive", "capture-id"))
    parser.add_argument("file", type=Path, help="replay file or output folder for complete")
    parser.add_argument("--bundle-dir", type=Path)
    inspection = parser.add_mutually_exclusive_group()
    inspection.add_argument("--unityrs", help="unityrs executable for complete (default: unityrs on PATH)")
    inspection.add_argument("--inspect-dir", type=Path, help="saved inspect and show JSON for complete")
    args = parser.parse_args()
    try:
        if args.mode == "complete":
            from anim_replay_complete import complete_folder

            require(args.bundle_dir is None, "complete reads bundles/ inside the output folder")
            executable = args.unityrs or "unityrs"
            if args.unityrs:
                executable = str(Path(executable).resolve())
            count = complete_folder(args.file, executable, args.inspect_dir, sys.modules[__name__])
            print(f"Completed and validated {count} replay(s).")
            return
        require(args.unityrs is None and args.inspect_dir is None, "inspection options require complete")
        value = read_json(args.file.read_text(encoding="utf-8"))
        if args.mode == "validate-spec":
            require(args.bundle_dir is None, "bundle verification requires a capture")
            discriminated = validate_spec(value)
        elif args.mode == "derive":
            print(json.dumps(derive(value, args.bundle_dir), indent=2, allow_nan=False))
            return
        else:
            discriminated = validate(value, args.bundle_dir)
            if args.mode == "capture-id":
                require("baseline_captures" not in value, "capture-id requires a standalone recording")
                print(_known_vector_id(value))
                return
        spec = value if args.mode == "validate-spec" else value["spec"]
        if spec["mode"] != "human_pose" and not discriminated:
            print("anim_replay: clock not discriminated", file=sys.stderr)
    except (ValueError, OSError, OverflowError) as exc:
        parser.exit(1, f"anim_replay: {exc}\n")


if __name__ == "__main__":
    main()
