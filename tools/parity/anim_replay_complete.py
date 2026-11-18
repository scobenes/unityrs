"""Resolve the serialized identities in a pending Unity fixture replay."""

import hashlib
import json
import math
import os
import struct
from uuid import uuid4

import child

PENDING_FORMAT = "unityrs.anim_replay_pending"
STATIC_CASES = dict.fromkeys(("static-primitives", "static-materials", "static-lod", "static-skin"), "static-objects")
STATIC_CASES["static-batch-scene"] = "static-batch-scene"
HUMANOID_CASES = tuple(f"humanoid-{rig}-muscles{offset}" for rig in ("compact", "tall") for offset in ("", "-offset"))
REPLAY_CASES = (
    "legacy-explicit", "generic-quaternion", "generic-euler", "generic-constant", "generic-offset-stepped",
    "generic-weighted", "generic-morph", "generic-object-reference", "generic-override-morph",
    "generic-null-override", "generic-scale", "humanoid-compact-body-motion", *HUMANOID_CASES,
)
VECTOR_CASES = tuple(f"{name}-known-vectors" for name in HUMANOID_CASES)
OPTIONAL_CASES = {
    "urp-materials", "packed-rotation-slopes", "authored-runtime-precedence", "mixed-transform-storage",
    "humanoid-twist-refusal", "humanoid-non-neutral-setting-refusal", "humanoid-slot-axis-inventory",
    "humanoid-root-hips-comparison", "integer-discrete-binding",
}


def wire_value(value, replay):
    """Read the show model, widening canonical float bits exactly."""
    if isinstance(value, list):
        return [wire_value(item, replay) for item in value]
    if not isinstance(value, dict):
        return value
    replay.require(value.get("$unityrs") != "cut" and "cut" not in value, "cut show value")
    tag = value.get("t")
    if tag == "dict":
        return {key: wire_value(item, replay) for key, item in value["v"]}
    if tag in ("tuple", "map"):
        return wire_value(value["v"], replay)
    if tag == "f32":
        # An unneeded nonfinite value, e.g. a stepped curve's tangent,
        # must survive the read without becoming a JSON number.
        try:
            return replay.f32_from_bits(value["v"])
        except ValueError:
            return value
    if tag in ("i64", "u64"):
        return int(value["v"])
    return {key: wire_value(item, replay) for key, item in value.items()}


class BundleView:
    """One bundle's inspect headers and lazily visited show values."""

    def __init__(self, path, digest, index, unityrs, inspect_dir, replay):
        self.path, self.index, self.unityrs, self.inspect_dir = path, index, unityrs, inspect_dir
        self.digest = digest
        self.require, self.read_json, self.replay = replay.require, replay.read_json, replay
        self.headers = {}
        self.values = {}
        cursor = None
        seen = set()
        while True:
            options = ["--limit", "10000", "--max-bytes", "1048576"]
            if cursor is not None:
                options += ["--cursor", cursor]
            doc = self.document("inspect", options, path.name + ".inspect.json")
            self.require(len(doc["inputs"]) == 1 and "error" not in doc["inputs"][0],
                         f"cannot inspect {path.name}: {doc['inputs']}")
            files = {row["file_index"]: row for row in doc["inputs"][0]["files"]}
            for header in doc["objects"]:
                key = (header["file_index"], header["path_id"])
                self.require(header["input"] == 0 and key not in self.headers
                             and files[key[0]]["kind"] == "AssetsFile"
                             and files[key[0]]["file"] == header["file"],
                             f"inconsistent inspect object header in {path.name}")
                self.headers[key] = header
            cursor = doc["next_cursor"]
            if cursor is None:
                self.require(len(self.headers) == doc["total_objects"], f"incomplete inspect for {path.name}")
                break
            self.require(inspect_dir is None and cursor not in seen, f"expected complete inspect for {path.name}")
            seen.add(cursor)

    def document(self, command, options, json_name):
        if self.inspect_dir is None:
            result = child.run([self.unityrs, command, self.path.name, *options],
                               cwd=self.path.parent, capture_output=True, text=True)
            self.require(result.returncode == 0,
                         f"unityrs {command} {self.path.name}: {(result.stderr or result.stdout).strip()}")
            data = result.stdout
        else:
            data = (self.inspect_dir / json_name).read_text(encoding="utf-8")
        doc = self.read_json(data)
        self.require(doc.get("format") == f"unityrs.{command}" and doc.get("version") == 1,
                     f"expected unityrs {command} v1 for {self.path.name}")
        if self.inspect_dir is not None:
            self.require(doc.get("bundle_sha256") == self.digest, f"inspection bundle digest mismatch: {json_name}")
        return doc

    def value(self, key):
        if key not in self.values:
            file, path = key
            options = ["--file-index", f"0:{file}", "--path-id", str(path), "--value", "canonical",
                       "--max-items", "4096", "--max-bytes", "4194304"]
            doc = self.document("show", options, f"{self.path.name}.{file}.{path}.show.json")
            self.require(doc["object"] == self.headers[key], f"show identity disagrees with inspect: {key}")
            truncated = doc.get("truncated")
            self.require(isinstance(truncated, dict) and set(truncated) == {"cuts", "items", "depth", "bytes", "binary"}
                         and all(type(count) is int and count == 0 for count in truncated.values()),
                         f"truncated show or missing truncation counters: {key}")
            value = wire_value(doc["value"], self.replay)
            self.require(isinstance(value, dict) and "$unityrs" not in value, f"unreadable show value: {key}")
            self.values[key] = value
        return self.values[key]

    def identity(self, key):
        if key is None:
            return None
        file_index, path_id = key
        return dict(bundle=self.index, file_index=file_index, file=self.headers[key]["file"], path_id=str(path_id))

    def pointer(self, owner, ptr, classes=None):
        self.require(type(ptr["m_PathID"]) is int and type(ptr["m_FileID"]) is int,
                     f"invalid PPtr in {self.path.name}: {owner}")
        if ptr["m_PathID"] == 0:
            return None
        # The generator puts every replay dependency in the same bundle.
        # An external file ID cannot be resolved from inspect headers.
        self.require(ptr["m_FileID"] == 0, f"external replay PPtr in {self.path.name}: {owner}")
        key = (owner[0], ptr["m_PathID"])
        self.require(key in self.headers and (classes is None or self.headers[key]["class_id"] in classes),
                     f"unresolved or wrong-class PPtr in {self.path.name}: {owner} -> {key}")
        return key

    def asset(self, name, class_id):
        candidates = []
        for key, header in self.headers.items():
            if header["class_id"] != 142:
                continue
            container = self.value(key)["m_Container"]
            rows = container.items() if isinstance(container, dict) else container
            for asset_key, info in rows:
                if asset_key == name:
                    candidates.append(self.pointer(key, info["asset"], {class_id}))
        self.require(len(candidates) == 1 and candidates[0] is not None,
                     f"expected one AssetBundle container entry for {name} in {self.path.name}")
        return candidates[0]

    def hierarchy(self, prefab):
        """Index Transform addresses and stored component order from the prefab root."""
        nodes = {}
        visited = set()

        def components(game_object):
            return [key for row in self.value(game_object)["m_Component"]
                    if (key := self.pointer(game_object, row[1] if isinstance(row, list) else row["component"]))
                    is not None]

        roots = [key for key in components(prefab) if self.headers[key]["class_id"] == 4]
        self.require(len(roots) == 1, "prefab must have one root Transform")
        stack = [(roots[0], "", None)]
        while stack:
            key, address, parent = stack.pop()
            self.require(key not in visited, f"repeated Transform at address {address!r}")
            visited.add(key)
            value = self.value(key)
            game_object = self.pointer(key, value["m_GameObject"], {1})
            self.require(game_object is not None and self.pointer(key, value["m_Father"], {4}) == parent,
                         f"inconsistent Transform links at address {address!r}")
            ordered = components(game_object)
            self.require(key in ordered and (address != "" or game_object == prefab),
                         f"inconsistent GameObject at address {address!r}")
            nodes[address] = (key, self.headers[game_object]["name"], ordered)
            for ordinal, ptr in enumerate(value["m_Children"]):
                child_key = self.pointer(key, ptr, {4})
                self.require(child_key is not None, f"null child at address {address!r}")
                stack.append((child_key, f"{address}/{ordinal}" if address else str(ordinal), key))
        return nodes

    def component(self, nodes, address, index, class_id, label):
        self.require(address in nodes, f"unknown {label} node at address {address!r}")
        matches = [key for key in nodes[address][2] if self.headers[key]["class_id"] == class_id]
        self.require(type(index) is int and 0 <= index < len(matches),
                     f"missing {label} component {index} at address {address!r}")
        return matches[index]

    def discovery(self, binding, binding_class):
        rows = []

        def emit(owner, field, ordinal, original, resolved, outcome):
            rows.append(dict(owner=self.identity(owner), field=field, ordinal=ordinal,
                             original=self.identity(original), resolved=self.identity(resolved), outcome=outcome))

        def clips(owner, field, pointers):
            keys = []
            for ordinal, ptr in enumerate(pointers):
                clip = self.pointer(owner, ptr, {74})
                emit(owner, field, ordinal, clip, clip, "null" if clip is None else "resolved")
                keys.append(clip)
            return keys

        value = self.value(binding)
        if binding_class == 111:
            direct = clips(binding, "m_Animation", [value["m_Animation"]])
            return rows, direct + clips(binding, "m_Animations", value["m_Animations"])

        controller = self.pointer(binding, value["m_Controller"], {91, 221})
        if controller is None:
            return rows, []
        overrides = None
        if self.headers[controller]["class_id"] == 221:
            overrides = controller
            controller = self.pointer(overrides, self.value(overrides)["m_Controller"], {91})
            self.require(controller is not None, "override controller has no base controller")
        direct = clips(controller, "m_AnimationClips", self.value(controller)["m_AnimationClips"])
        if overrides is not None:
            substitutions = {}
            for ordinal, row in enumerate(self.value(overrides)["m_Clips"]):
                original = self.pointer(overrides, row["m_OriginalClip"], {74})
                replacement = self.pointer(overrides, row["m_OverrideClip"], {74})
                self.require(original in direct and original is not None and original not in substitutions,
                             "invalid or duplicate original override clip")
                resolved = replacement if replacement is not None else original
                substitutions[original] = resolved
                emit(overrides, "m_OverrideClip", ordinal, original, resolved,
                     "override" if replacement is not None else "null_override")
            direct = [substitutions.get(clip, clip) for clip in direct]
        return rows, direct


def streamed_times(words, require):
    """Read frame times from the CLI's uint32 view of m_StreamedClip.data."""
    cursor, previous = 0, -math.inf
    while cursor < len(words):
        bits, count = words[cursor:cursor + 2]
        require(type(bits) is int and 0 <= bits <= 0xffffffff and type(count) is int and count >= 0,
                "invalid built streamed clip frame")
        time, = struct.unpack(">f", struct.pack(">I", bits))
        seed = cursor == 0 and bits == 0xff7fffff
        cursor += 2 + 5 * count
        require(cursor <= len(words), "incomplete built streamed clip frame")
        if time == math.inf:
            require(count == 0 and cursor == len(words), "invalid built streamed clip terminal frame")
            break
        require(math.isfinite(time) and time > previous, "invalid built streamed clip time")
        previous = time
        if count and not seed:
            yield time


def packed_times(times, require):
    width, count = times["m_BitSize"], times["m_NumItems"]
    require(type(width) is int and type(count) is int and 0 <= width <= 32 and 0 < count <= 65536,
            "invalid built packed rotation time layout")
    raw = times["m_Data"]
    data = bytes.fromhex(raw["hex"]) if isinstance(raw, dict) else bytes(raw)
    require(count * width <= 8 * len(data) and (width > 0 or count == 1),
            "incomplete built packed rotation times")
    total, mask = 0, (1 << width) - 1
    for i in range(count):
        byte, shift = divmod(i * width, 8)
        delta = (int.from_bytes(data[byte:byte + 5], "little") >> shift) & mask
        require(i == 0 or delta > 0, "unordered built packed rotation times")
        total += delta
        yield total / 100


def built_key_times(value, playback, require):
    times = []
    for field in ("m_PositionCurves", "m_RotationCurves", "m_EulerCurves", "m_ScaleCurves",
                  "m_FloatCurves", "m_PPtrCurves"):
        for row in value.get(field, []):
            curve = row["curve"]
            if field != "m_PPtrCurves":
                curve = curve["m_Curve"]
            times.extend(key["time"] for key in curve)
    for row in value.get("m_CompressedRotationCurves", []):
        times.extend(packed_times(row["m_Times"], require))
    stream = playback.get("m_StreamedClip", {})
    if stream:
        times.extend(streamed_times(stream["data"], require))
    dense = playback.get("m_DenseClip", {})
    if dense.get("m_CurveCount", 0):
        curves, count, rate, begin = (dense[k] for k in ("m_CurveCount", "m_FrameCount", "m_SampleRate", "m_BeginTime"))
        array = dense["m_SampleArray"]
        if isinstance(array, dict):
            require(array.get("t") == "f32s" and len(array["v"]) % 8 == 0, "incomplete built dense samples")
            size = len(array["v"]) // 8
        else:
            size = len(array)
        require(type(curves) is int and type(count) is int and curves > 0 and count > 0
                and size == curves * count and math.isfinite(begin) and math.isfinite(rate) and rate > 0,
                "invalid built dense clip grid")
        times.extend(begin + i / rate for i in range(count))
    require(all(type(time) in (int, float) and math.isfinite(time) for time in times), "invalid built clip key time")
    return sorted(set(times))


def clip_evidence(spec, capture, value, replay):
    evidence = spec["evidence"]
    muscle = value.get("m_MuscleClip", {})
    playback = muscle.get("m_Clip", {})
    playback = playback.get("data", playback)
    stream = playback.get("m_StreamedClip", {}).get("curveCount", 0)
    dense = playback.get("m_DenseClip", {}).get("m_CurveCount", 0)
    constants = playback.get("m_ConstantClip", {}).get("data", [])
    if isinstance(constants, dict):
        replay.require(constants.get("t") == "f32s" and len(constants["v"]) % 8 == 0,
                       "incomplete built constant clip data")
        constant = len(constants["v"]) // 8
    else:
        constant = len(constants)
    bindings = value.get("m_ClipBindingConstant", {}).get("genericBindings", [])
    compiled = spec["mode"] != "legacy" and bool(stream or dense or constant or bindings or capture.get("humanoid"))
    fields = "m_MuscleClip" if compiled else "m_AnimationClipSettings"
    serialized = value.get(fields, {})
    replay.require("$unityrs" not in serialized and value["m_SampleRate"] == spec["sample_rate"]
                   and bool(value["m_Legacy"]) == (spec["mode"] == "legacy"), "built clip rate or mode mismatch")
    for side, field in (("start", "m_StartTime"), ("stop", "m_StopTime")):
        if compiled or fields in value:
            replay.require(serialized.get(field) == spec[side], f"built clip {side} differs from Editor evidence")
            evidence["clock"][side]["field"] = f"{fields}.{field}"
    built_times = built_key_times(value, playback if compiled else {}, replay.require)
    if not compiled and fields not in value:
        replay.require(built_times and (built_times[0], built_times[-1]) == (spec["start"], spec["stop"]),
                       "built clip key bounds differ from Editor interval")
        for side in ("start", "stop"):
            evidence["clock"][side]["field"] = "key_bounds"
    clock = evidence["clock"]
    clock["authored_key_times"] = clock["key_times"]
    clock["key_times"] = sorted({spec["start"], spec["stop"],
                                 *(time for time in built_times if spec["start"] <= time <= spec["stop"])})
    layouts = [label for label, count in (("streamed", stream), ("dense", dense), ("constant", constant))
               if count]
    evidence["layout"] = ("_".join(layouts) or "compiled") if compiled else "explicit"
    if evidence.get("editor", {}).get("fixture_case") == "generic-scale":
        binding = bindings[0] if len(bindings) == 1 else {}
        evidence["mixed_transform_storage"] = (compiled and len(layouts) > 1 and stream + dense + constant == 3
                                                and binding.get("typeID", binding.get("classID")) == 4
                                                and binding.get("attribute") == 3 and binding.get("customType") == 0)
    f32_settings = {"startTime", "stopTime", "orientationOffsetY", "level", "cycleOffset",
                    "additiveReferencePoseTime"}
    evidence["import_settings"] = []
    for name, effective in capture["clip_settings"].items():
        if name == "additiveReferencePoseClipName":
            # This effective name is not itself stored (the field is a PPtr).
            continue
        field = "m_" + name[0].upper() + name[1:]
        stored = serialized.get(field)
        replay.require(stored is None or type(stored) in (int, float, str, bool),
                       f"unsupported serialized clip setting: {fields}.{field}")
        evidence["import_settings"].append(dict(
            property=name, effective=replay.f32_from_bits(effective) if name in f32_settings else effective,
            serialized_path=f"{fields}.{field}", serialized=stored))


def complete_recording(doc, bundle_dir, views, unityrs, inspect_dir, replay):
    replay.require(set(doc) == {"format", "version", "spec", "capture"}
                   and doc["format"] == PENDING_FORMAT and type(doc["version"]) is int and doc["version"] == 1,
                   "expected unityrs.anim_replay_pending version 1")
    spec, capture = doc["spec"], doc["capture"]
    replay.require(not {"prefab", "clip", "binding", "avatar"}.intersection(spec),
                   "pending replay already contains built identities")
    path, digest = pending_bundle(doc, bundle_dir, replay)
    spec["bundles"][0]["sha256"] = digest
    view_key = path.name, digest
    if view_key not in views:
        views[view_key] = BundleView(path, digest, 0, unityrs, inspect_dir, replay)
    view = views[view_key]
    prefab = view.asset(spec["prefab_asset"], 1)
    clip = view.asset(spec["clip_asset"], 74)
    replay.require(view.headers[clip]["name"] == spec["clip_name"], "built clip name mismatch")
    nodes = view.hierarchy(prefab)
    replay.require(set(nodes) == {node["address"] for node in spec["nodes"]}, "built node coverage mismatch")
    for node in spec["nodes"]:
        key, name, _ = nodes[node["address"]]
        replay.require(name == node["name"] and "id" not in node, "built node name or pending identity mismatch")
        node["id"] = view.identity(key)

    binding_class = 111 if spec["mode"] == "legacy" else 95
    binding = view.component(nodes, spec["binding_address"], spec["binding_component"], binding_class, "binding")
    avatar = view.asset(spec["avatar_asset"], 90) if spec["avatar_asset"] else None
    if binding_class == 95:
        pointer = view.value(binding)["m_Avatar"]
        replay.require(view.pointer(binding, pointer, {90}) == avatar, "built Animator Avatar mismatch")
    spec.update(prefab=view.identity(prefab), clip=view.identity(clip),
                binding=view.identity(binding), avatar=view.identity(avatar))

    meshes, groups, renderer_ids = {}, {}, set()
    for renderer in spec["renderers"]:
        replay.require(not {"id", "mesh"}.intersection(renderer), "pending renderer already has built IDs")
        key = view.component(nodes, renderer["address"], renderer["component"], 137, "renderer")
        mesh = view.pointer(key, view.value(key)["m_Mesh"], {43})
        group = renderer.pop("mesh_group")
        replay.require(mesh is not None and type(group) is int and group >= 0
                       and meshes.setdefault(group, mesh) == mesh and groups.setdefault(mesh, group) == group,
                       "built shared mesh relationship mismatch")
        renderer_ids.add(key)
        renderer.update(id=view.identity(key), mesh=view.identity(mesh))
    replay.require(renderer_ids == {key for _, _, keys in nodes.values() for key in keys
                                   if view.headers[key]["class_id"] == 137}, "built renderer coverage mismatch")

    clip_evidence(spec, capture, view.value(clip), replay)
    discovery, effective = view.discovery(binding, binding_class)
    replay.require(clip in effective, "selected clip is absent from built playback component discovery")
    expected = spec["evidence"].get("editor", {}).get("expected_discovery")
    if expected:
        replay.require(any(row["outcome"] == expected and row["resolved"] == view.identity(clip)
                           for row in discovery), f"missing expected built discovery outcome: {expected}")
    spec["evidence"]["discovery"] = discovery
    doc["format"] = "unityrs.anim_replay"
    replay.validate(doc, bundle_dir)
    return doc


def recorded_paths(folder, replay):
    """Check the fixed v1 inventory before trusting any pending recording."""
    manifest = replay.read_json((folder / "run.json").read_text(encoding="utf-8"))
    replay.require(isinstance(manifest, dict) and set(manifest) == {"format", "version", "cases"}
                   and manifest["format"] == "unityrs.fixture_run" and type(manifest["version"]) is int
                   and manifest["version"] == 1 and isinstance(manifest["cases"], list),
                   "expected unityrs.fixture_run version 1 in run.json")
    replay_dir = folder / "replays"
    paths = {name: replay_dir / f"{name}.pending.json" for name in REPLAY_CASES}
    paths.update({name: replay_dir / "known-vectors" / f"{name}.pending.json" for name in VECTOR_CASES})
    required = set(STATIC_CASES) | paths.keys()
    expected = required | OPTIONAL_CASES
    errors, statuses = [], {}
    for row in manifest["cases"]:
        replay.require(isinstance(row, dict) and set(row) == {"name", "status", "reason"}
                       and all(isinstance(value, str) for value in row.values()), "invalid run.json case row")
        name, status, reason = row["name"], row["status"], row["reason"]
        replay.require(name in expected and name not in statuses, f"unknown or duplicate run.json case: {name}")
        replay.require(status in {"recorded", "failed", "skipped", "not_covered"}
                       and (status == "recorded" or reason.strip()), f"invalid run.json case status or reason: {name}")
        statuses[name] = status
        if status == "failed" or (name in required and status != "recorded"):
            errors.append(f"{name}: {status} ({reason})")
        if name in required and status == "recorded":
            path = paths[name] if name in paths else folder / "bundles" / STATIC_CASES[name]
            if not path.is_file() or path.stat().st_size == 0:
                errors.append(f"{name}: missing {path.relative_to(folder)}")
        if name in OPTIONAL_CASES and status == "recorded" and name != "mixed-transform-storage":
            errors.append(f"{name}: no recording defined for this optional case")
    errors.extend(f"missing run.json case: {name}" for name in sorted(expected - statuses.keys()))
    undocumented = set(replay_dir.rglob("*.pending.json")) - set(paths.values())
    errors.extend(f"unexpected pending recording: {path.relative_to(folder)}" for path in sorted(undocumented))
    replay.require(not errors, "incomplete fixture run:\n" + "\n".join(errors))
    return sorted(paths.values()), statuses["mixed-transform-storage"] == "recorded"


def pending_bundle(doc, bundle_dir, replay):
    spec, capture = doc["spec"], doc["capture"]
    replay.require(len(spec["bundles"]) == 1 and set(spec["bundles"][0]) == {"name"},
                   "pending replay must select one self-contained bundle by name")
    name = spec["bundles"][0]["name"]
    replay._shape(replay.SCHEMA["$defs"]["bundle"]["properties"]["name"], name, "/spec/bundles/0/name")
    path = bundle_dir / name
    with path.open("rb") as source:
        digest = hashlib.file_digest(source, "sha256").hexdigest()
    replay.require(capture["bundle_sha256"] == [digest], f"bundle digest mismatch: {name}")
    return path, digest


def check_raw_vectors(doc, replay):
    spec, capture = doc["spec"], doc["capture"]
    replay._shape(replay.SCHEMA["$defs"]["capture"], capture, "/capture")
    replay.require(spec["mode"] == "human_pose" and capture["playback"]["api"] == replay.API["human_pose"]
                   and capture["playback"]["clip_human_motion"] and "humanoid" in capture,
                   "expected a pending known-vector recording")
    schedule, samples = spec["schedule"], capture["samples"]
    width = len(capture["humanoid"]["muscles"])
    replay.require(len(schedule) == len(samples) and all(
        len(point["muscles"]) == width and point["local"] == sample["local"]
        and [replay.f32_bits(value) for value in point["muscles"]] == sample["muscles_f32"]
        for point, sample in zip(schedule, samples)), "incomplete known-vector recording")


def complete_folder(folder, unityrs, inspect_dir, replay):
    """Validate every result before writing any completed recordings."""
    bundle_dir = folder / "bundles"
    results, views = [], {}
    paths, recorded_mixed = recorded_paths(folder, replay)
    for path in paths:
        try:
            document = replay.read_json(path.read_text(encoding="utf-8"))
            spec = document["spec"]
            name = path.name.removesuffix(".pending.json")
            replay.require(spec["evidence"]["editor"].get("fixture_case") == name,
                           f"pending recording does not identify manifest case {name}")
            if path.parent.name == "known-vectors":
                replay.require(document["format"] == PENDING_FORMAT and type(document["version"]) is int
                               and document["version"] == 1, "expected a pending known-vector recording")
                check_raw_vectors(document, replay)
                if "humanoid" not in spec["evidence"]:
                    pending_bundle(document, bundle_dir, replay)
                    continue
            result = complete_recording(document, bundle_dir, views, unityrs, inspect_dir, replay)
            if name == "generic-scale" and recorded_mixed:
                replay.require(result["spec"]["evidence"]["mixed_transform_storage"],
                               "mixed Transform storage reported by Unity is absent from the built scale clip")
            output = path.with_name(path.name.removesuffix(".pending.json") + ".json")
            data = json.dumps(result, indent=2, ensure_ascii=False, allow_nan=False) + "\n"
            replay.require(not output.exists() or output.read_text(encoding="utf-8") == data,
                           f"{output.name} already exists with different content")
            results.append((output, data))
        except (KeyError, TypeError, IndexError, AttributeError, ValueError, OSError) as exc:
            raise ValueError(f"{path.name}: {exc}") from exc
    for output, data in results:
        if output.exists():
            continue
        staged = output.with_name(f".{output.name}.{uuid4().hex}.tmp")
        try:
            staged.write_text(data, encoding="utf-8", newline="\n")
            try:
                os.link(staged, output)
            except FileExistsError:
                replay.require(output.read_text(encoding="utf-8") == data,
                               f"{output.name} already exists with different content")
        finally:
            staged.unlink(missing_ok=True)
    return len(results)
