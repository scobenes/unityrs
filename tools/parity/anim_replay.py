"""Validate a version 1 L11 recording and derive its a priori f32 tolerances."""
import argparse
import hashlib
import json
import math
import re
import struct
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from anim_reference import BOUNDS, angular_bound, gamma, ulp32  # noqa: E402

SCHEMA = json.loads(Path(__file__).with_name("anim_replay.schema.json").read_text(encoding="utf-8"))
TRS = ("translation", "rotation", "scale")
API = {"animator": "AnimationClipPlayable", "legacy": "Animation.Sample",
       "human_pose": "HumanPoseHandler.SetHumanPose"}
BODY_DOFS = {f"RootT.{axis}" for axis in "xyz"} | {f"RootQ.{axis}" for axis in "xyzw"}
U32 = 2**-24
F64_ANGLE_FLOOR = BOUNDS["transcription"]["angular_floor_u64"] * BOUNDS["transcription"]["unit_roundoff"]


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


def _same(a, b):
    return a == b and (type(a) is bool) == (type(b) is bool)


def _shape(schema, value, path):
    """The small Draft 2020-12 subset used by this checked-in schema."""
    if "$ref" in schema:
        _shape(SCHEMA["$defs"][schema["$ref"].split("/")[-1]], value, path)
        return
    if "anyOf" in schema:
        for choice in schema["anyOf"]:
            try:
                _shape(choice, value, path)
                return
            except ValueError:
                pass
        raise ValueError(f"{path}: no schema alternative matched")
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
        if "pattern" in schema:
            require(re.fullmatch(schema["pattern"], value) is not None, f"{path}: invalid spelling")
        if schema is SCHEMA["$defs"]["f32"]:
            f32_from_bits(value)
    elif type(value) in (int, float):
        require(-math.inf < value < math.inf and schema.get("minimum", -math.inf) <= value
                <= schema.get("maximum", math.inf) and value > schema.get("exclusiveMinimum", -math.inf),
                f"{path}: number outside range")


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


def validate_spec(spec):
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
    boundaries = spec["held_boundaries"]
    require(_ordered(boundaries) and all(start <= t <= stop for t in boundaries), "invalid held boundaries")
    times = [row["local"] for row in spec["schedule"]]
    require(all(float(t) == t for t in (*boundaries, *times)), "time is not exactly representable as binary64")
    require(_ordered(times) and times[0] == 0 and times[-1] == stop - start, "invalid schedule endpoints or order")
    require(stop == start or len(times) > 2, "nonzero interval requires an off-key sample")
    require(all(("muscles" in row) == (spec["mode"] == "human_pose") for row in spec["schedule"]),
            "only known-vector playback requires muscles in the schedule")
    if spec["mode"] == "human_pose":
        require(not boundaries and "humanoid" in spec["evidence"], "invalid known-vector evidence")
    human = spec["evidence"].get("humanoid")
    paths = spec.get("editor_axis_properties", [])
    require(len(set(paths)) == len(paths) and (not paths or human is not None), "invalid Editor axis paths")
    if human is not None:
        require(spec["avatar"] is not None, "humanoid evidence requires an Avatar")
        dofs = [row["dof"] for row in human["slots"]]
        require(len(dofs) == len(set(dofs)), "duplicate humanoid DOF")
        absent_body = set()
        for row in human["slots"]:
            absent = row["storage"] == "absent"
            require(absent == (row["index_value"] == human["absent_slot"])
                    and (absent or row["index_value"] >= 0), "invalid humanoid slot sentinel")
            if row["kind"] == "body" and absent:
                absent_body.add(row["dof"])
        comparison = human.get("root_hips_comparison")
        if comparison is not None:
            require(comparison["root_node"] < len(nodes) and comparison["hips_node"] < len(nodes),
                    "unknown root or hips node")
        if human["disposition"] == "qualified_muscle_only":
            require(BODY_DOFS <= absent_body
                    and all(row["storage"] == "absent" for row in human["slots"] if row["kind"] == "body")
                    and comparison is not None and comparison["within_bounds"],
                    "qualified muscle-only playback needs absent body slots and root/hips evidence")
    if "budgets" in spec:
        budgets = spec["budgets"]
        keys = [_target_key(row) for row in budgets]
        require(len(set(keys)) == len(keys) and set(keys) == set(_targets(spec)), "invalid budget coverage")
        for row in budgets:
            rotation, weight = row["property"] == "rotation", row["property"] == "weight"
            require((row["min_norm"] > 0 if rotation else row["min_norm"] == 0)
                    and (row["output_gain"] > 0 if weight else row["output_gain"] == 1),
                    "invalid quaternion norm or output gain")


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


def validate(manifest, bundle_dir=None):
    _shape(SCHEMA, manifest, "")
    spec, capture = manifest["spec"], manifest["capture"]
    validate_spec(spec)
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
    require(human is not None or "humanoid" not in spec["evidence"], "missing humanoid capture")
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


def derive(manifest, bundle_dir=None):
    """Return per-sample bounds in target order. Observations supply only the output ulp."""
    validate(manifest, bundle_dir)
    spec = manifest["spec"]
    require("budgets" in spec, "derivation requires recorded budgets")
    keys = _targets(spec)
    budgets = {_target_key(row): row for row in spec["budgets"]}
    stages = {key: math.fsum((gamma(s["operations"], U32) * s["scale"]
                              + s["operations"] * 2**-126 + s["elementary_error"]) * s["gain"]
                             for s in budgets[key]["stages"]) for key in keys}
    result = []
    for sample in manifest["capture"]["samples"]:
        dt = abs(sample["effective_source"] - sample["source"])
        bounds = []
        for key, values in zip(keys, _pose_values(spec, sample["pose"])):
            budget = budgets[key]
            raw = (stages[key] + dt * budget["rate"]) * budget["output_gain"]
            output = .5 * ulp32(max(map(abs, values)))
            if key[0] == "rotation":
                bound = (angular_bound(raw, budget["min_norm"]) + angular_bound(output, 1.)
                         + F64_ANGLE_FLOOR)
            else:
                bound = raw + output
            bounds.append(math.nextafter(bound * (1 + 2**-45), math.inf))
        result.append({k: sample[k] for k in ("source", "effective_source", "sides")} | {"bounds": bounds})
    encoded = json.dumps(manifest, sort_keys=True, ensure_ascii=True, separators=(",", ":"), allow_nan=False)
    return {"format": "unityrs.anim_tolerances", "version": 1,
            "manifest_sha256": hashlib.sha256(encoded.encode("ascii")).hexdigest(),
            "targets": keys, "samples": result}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("validate", "validate-spec", "derive"))
    parser.add_argument("file", type=Path)
    parser.add_argument("--bundle-dir", type=Path)
    args = parser.parse_args()
    try:
        value = read_json(args.file.read_text(encoding="utf-8"))
        if args.mode == "validate-spec":
            require(args.bundle_dir is None, "bundle verification requires a capture")
            validate_spec(value)
        elif args.mode == "validate":
            validate(value, args.bundle_dir)
        else:
            print(json.dumps(derive(value, args.bundle_dir), indent=2, allow_nan=False))
    except (ValueError, OSError, OverflowError) as exc:
        parser.exit(1, f"anim_replay: {exc}\n")


if __name__ == "__main__":
    main()
