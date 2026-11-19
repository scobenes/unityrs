"""Resolve the serialized identities in a pending Unity fixture replay."""

import hashlib
import json
import math
import os
import re
import struct
from uuid import uuid4
from zlib import crc32

import child
from anim_fixture_body import (BAND_CASES, BODY_BUNDLES, BODY_CASES, BODY_INFO, HOLDOUT, LATER_CASES, check_body,
                               primary_probes)
from anim_fixture_coverage import BASIS_CASES, COVERAGE_CASES, check_coverage, required_coverage
from anim_fixture_contract import (ASSETS, MOTION, REPLAY_CASES, STATIC_CASES, VECTOR_CASES, VECTOR_SUFFIX,
                                   check_built_clip, check_built_static, check_discovery, check_evidence,
                                   check_identity, check_vectors)

PENDING_FORMAT = "unityrs.anim_replay_pending"
OPTIONAL_CASES = {
    "urp-materials", "packed-rotation-slopes", "authored-runtime-precedence", "mixed-transform-storage",
    "humanoid-twist-refusal", "humanoid-non-neutral-setting-refusal", "humanoid-slot-axis-inventory",
    "humanoid-root-hips-comparison", "integer-discrete-binding", *BASIS_CASES, *BODY_CASES,
}


def pending_pose(name, doc):
    """A known-vector capture keeps the pending human_pose schema until its slot inventory is added."""
    pose = name in VECTOR_CASES or name in BODY_CASES and BODY_INFO[name]["kind"] != "clip"
    return pose and "humanoid" not in doc["spec"]["evidence"]


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

    def __init__(self, path, digest, index, unityrs, inspect_dir, replay, run):
        self.path, self.index, self.unityrs, self.inspect_dir = path, index, unityrs, inspect_dir
        self.digest = digest
        self.require, self.read_json, self.replay = replay.require, replay.read_json, replay
        with path.open("rb") as source:
            self.require(source.read(8) == b"UnityFS\0", f"invalid UnityFS signature: {path.name}")
        self.headers = {}
        self.values = {}
        self.container = None
        cursor = None
        seen = set()
        while True:
            options = ["--limit", "10000", "--max-bytes", "1048576", "--external-paths"]
            if cursor is not None:
                options += ["--cursor", cursor]
            doc = self.document("inspect", options, path.name + ".inspect.json")
            self.require(len(doc["inputs"]) == 1 and "error" not in doc["inputs"][0],
                         f"cannot inspect {path.name}: {doc['inputs']}")
            files = {row["file_index"]: row for row in doc["inputs"][0]["files"]}
            self.require(files.get(0, {}).get("kind") == "BundleFile"
                         and files[0]["file"] == path.name, f"expected inspected Unity bundle: {path.name}")
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
        self.files = files
        self.require(files[0]["unity_version"] == run["editor_version"]
                     and all((row["unity_version"], row["target_platform"])
                             == (run["editor_version"], run["build_target_id"])
                             for row in files.values() if row["kind"] == "AssetsFile"),
                     f"fixture run identity mismatch in bundle platform or Editor version: {path.name}")
        self.check_run_stamp(run_stamp(run))

    def check_run_stamp(self, stamp):
        message = f"fixture run identity mismatch in built bundle stamp: {self.path.name}"
        if self.path.name == "static-batch-scene":
            markers = [key for key, row in self.headers.items() if row["class_id"] == 1
                       and isinstance(row["name"], str) and row["name"].startswith("unityrs fixture run ")]
            self.require(len(markers) == 1 and self.value(markers[0])["m_Name"] == stamp, message)
        else:
            key = self.asset(f"{ASSETS}run/{self.path.name}.txt", 49, ignore_case=True)
            self.require(self.value(key)["m_Script"] == stamp, message)

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

    def value(self, key, *, full_bytes=False):
        cache_key = key, full_bytes
        if cache_key not in self.values:
            file, path = key
            options = ["--file-index", f"0:{file}", "--path-id", str(path),
                       "--max-items", "4096", "--max-bytes", "4194304"]
            # Canonical show summarizes byte arrays longer than 64 bytes.
            options += ["--value", "natural", "--bytes", "full"] if full_bytes else ["--value", "canonical"]
            doc = self.document("show", options, f"{self.path.name}.{file}.{path}.show.json")
            self.require(doc["object"] == self.headers[key], f"show identity disagrees with inspect: {key}")
            truncated = doc.get("truncated")
            self.require(isinstance(truncated, dict) and set(truncated) == {"cuts", "items", "depth", "bytes", "binary"}
                         and all(type(count) is int and count == 0 for count in truncated.values()),
                         f"truncated show or missing truncation counters: {key}")
            value = wire_value(doc["value"], self.replay)
            self.require(isinstance(value, dict) and "$unityrs" not in value, f"unreadable show value: {key}")
            self.values[cache_key] = value
        return self.values[cache_key]

    def identity(self, key):
        if key is None:
            return None
        file_index, path_id = key
        return dict(bundle=self.index, file_index=file_index, file=self.headers[key]["file"], path_id=str(path_id))

    def pointer(self, owner, ptr, classes=None, *, within_bundle=False):
        self.require(type(ptr["m_PathID"]) is int and type(ptr["m_FileID"]) is int,
                     f"invalid PPtr in {self.path.name}: {owner}")
        if ptr["m_PathID"] == 0:
            return None
        # The generator puts every replay dependency in the same bundle.
        # An external file ID cannot be resolved from inspect headers.
        file_index, file_id = owner[0], ptr["m_FileID"]
        if file_id:
            self.require(within_bundle, f"external replay PPtr in {self.path.name}: {owner}")
            externals = self.files[file_index].get("external_paths", [])
            self.require(0 < file_id <= len(externals), f"missing external file for {owner}: {file_id}")
            name = externals[file_id - 1].replace("\\", "/").rsplit("/", 1)[-1]
            candidates = [idx for idx, row in self.files.items() if row["kind"] == "AssetsFile"
                          and row["file"].rsplit("/", 1)[-1].lower() == name.lower()]
            self.require(len(candidates) == 1, f"unresolved external file in {self.path.name}: {name}")
            file_index = candidates[0]
        key = (file_index, ptr["m_PathID"])
        self.require(key in self.headers and (classes is None or self.headers[key]["class_id"] in classes),
                     f"unresolved or wrong-class PPtr in {self.path.name}: {owner} -> {key}")
        return key

    def asset(self, name, class_id, *, ignore_case=False):
        if self.container is None:
            self.container = []
            for key, header in self.headers.items():
                if header["class_id"] == 142:
                    container = self.value(key)["m_Container"]
                    rows = container.items() if isinstance(container, dict) else container
                    self.container.extend((asset_key, key, info["asset"]) for asset_key, info in rows)
        match = name.lower() if ignore_case else name
        candidates = []
        for asset_key, key, ptr in self.container:
            if (asset_key.lower() if ignore_case else asset_key) == match:
                candidates.append(self.pointer(key, ptr, None if class_id is None else {class_id}))
        # Scene bundles list a .unity address with a null PPtr.
        self.require(len(candidates) == 1 and (candidates[0] is None) == (class_id is None),
                     f"expected one AssetBundle container entry for {name} in {self.path.name}")
        return candidates[0]

    def components(self, game_object):
        return [key for row in self.value(game_object)["m_Component"]
                if (key := self.pointer(game_object, row[1] if isinstance(row, list) else row["component"]))
                is not None]

    def hierarchy(self, prefab):
        """Index Transform addresses and stored component order from the prefab root."""
        nodes, visited = {}, set()
        roots = [key for key in self.components(prefab) if self.headers[key]["class_id"] == 4]
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
            ordered = self.components(game_object)
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


def streamed_times(words, width, require, *, require_real_keys=False):
    """Read frame times from the CLI's uint32 view of m_StreamedClip.data."""
    cursor, previous, occupied = 0, -math.inf, set()
    while cursor < len(words):
        require(cursor + 2 <= len(words), "incomplete built streamed clip frame")
        bits, count = words[cursor:cursor + 2]
        require(type(bits) is int and 0 <= bits <= 0xffffffff and type(count) is int and count >= 0,
                "invalid built streamed clip frame")
        time, = struct.unpack(">f", struct.pack(">I", bits))
        seed = cursor == 0 and bits == 0xff7fffff
        start = cursor + 2
        cursor = start + 5 * count
        require(cursor <= len(words), "incomplete built streamed clip frame")
        if time == math.inf:
            require(count == 0 and cursor == len(words), "invalid built streamed clip terminal frame")
            break
        require(math.isfinite(time) and time > previous, "invalid built streamed clip time")
        previous = time
        slots = words[start:cursor:5]
        require(all(type(slot) is int and 0 <= slot < width for slot in slots)
                and len(set(slots)) == len(slots), "invalid built streamed clip slot")
        if not seed or not require_real_keys:
            occupied.update(slots)
        if count and not seed:
            yield time
    require(len(occupied) == width, "missing built streamed clip frame data")


def packed_times(times, require):
    width, count = times["m_BitSize"], times["m_NumItems"]
    require(type(width) is int and type(count) is int and 0 <= width <= 32 and 0 < count <= 65536,
            "invalid built packed rotation time layout")
    raw = times["m_Data"]
    if isinstance(raw, dict):
        require("len" not in raw and "sha256" not in raw,
                "incomplete built packed rotation times: canonical show elided the bytes")
        require(raw.get("t") in ("bytes", "u8s") and isinstance(raw.get("hex"), str),
                "invalid built packed rotation time bytes")
        data = bytes.fromhex(raw["hex"])
    else:
        require(isinstance(raw, list) and all(type(byte) is int and 0 <= byte <= 255 for byte in raw),
                "invalid built packed rotation time bytes")
        data = bytes(raw)
    require(count * width <= 8 * len(data) and (width > 0 or count == 1),
            "incomplete built packed rotation times")
    total, mask = 0, (1 << width) - 1
    for i in range(count):
        byte, shift = divmod(i * width, 8)
        delta = (int.from_bytes(data[byte:byte + 5], "little") >> shift) & mask
        require(i == 0 or delta > 0, "unordered built packed rotation times")
        total += delta
        yield total / 100


def built_key_times(value, playback, require, *, require_real_stream_keys=False):
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
        count, discrete, words = stream["curveCount"], stream.get("discreteCurveCount", 0), stream.get("data")
        require(type(count) is int and count >= 0 and type(discrete) is int and discrete >= 0,
                "invalid built streamed clip curve count")
        # 2022.3 counts discrete (PPtr) streamed slots apart, after the others.
        count += discrete
        require(isinstance(words, list) and (count == 0 or words), "missing built streamed clip frame data")
        times.extend(streamed_times(words, count, require, require_real_keys=require_real_stream_keys))
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
    is_scale_fixture = evidence.get("editor", {}).get("fixture_case") == "generic-scale"
    muscle = value.get("m_MuscleClip", {})
    playback = muscle.get("m_Clip", {})
    playback = playback.get("data", playback)
    streamed = playback.get("m_StreamedClip", {})
    stream = streamed.get("curveCount", 0) + streamed.get("discreteCurveCount", 0)
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
    built_times = built_key_times(value, playback if compiled else {}, replay.require,
                                require_real_stream_keys=is_scale_fixture)
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
    if is_scale_fixture:
        binding = bindings[0] if len(bindings) == 1 else {}
        evidence["mixed_transform_storage"] = (compiled and len(layouts) > 1 and stream + dense + constant == 3
                                                and binding.get("typeID", binding.get("classID")) == 4
                                                and binding.get("attribute") == 3 and binding.get("customType") == 0
                                                and binding.get("path") == crc32(MOTION.encode())
                                                and not binding.get("isPPtrCurve"))
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


def check_recording_run(doc, run, replay, label):
    spec, capture = doc["spec"], doc["capture"]
    replay.require(spec["evidence"]["editor"].get("run_id") == run["run_id"]
                   and capture["editor_version"] == run["editor_version"]
                   and capture["build_target"] == spec["build_target"] == run["build_target"],
                   f"fixture run identity mismatch in {label}")


def complete_recording(name, fixture, doc, bundle_dir, views, unityrs, inspect_dir, replay, run):
    replay.require(set(doc) == {"format", "version", "spec", "capture"}
                   and doc["format"] == PENDING_FORMAT and type(doc["version"]) is int and doc["version"] == 1,
                   "expected unityrs.anim_replay_pending version 1")
    spec, capture = doc["spec"], doc["capture"]
    check_recording_run(doc, run, replay, "recording")
    replay.require(not {"prefab", "clip", "binding", "avatar"}.intersection(spec),
                   "pending replay already contains built identities")
    check_identity(name, fixture, spec, capture, replay)
    path, digest = pending_bundle(doc, bundle_dir, replay)
    spec["bundles"][0]["sha256"] = digest
    view_key = path.name, digest
    if view_key not in views:
        views[view_key] = BundleView(path, digest, 0, unityrs, inspect_dir, replay, run)
    view = views[view_key]
    prefab = view.asset(spec["prefab_asset"], 1)
    clip = view.asset(spec["clip_asset"], 74)
    replay.require(view.headers[clip]["name"] == spec["clip_name"], "built clip name mismatch")
    nodes = view.hierarchy(prefab)
    replay.require(set(nodes) == {node["address"] for node in spec["nodes"]}, "built node coverage mismatch")
    for node in spec["nodes"]:
        key, node_name, _ = nodes[node["address"]]
        replay.require(node_name == node["name"] and "id" not in node, "built node name or pending identity mismatch")
        node["id"] = view.identity(key)

    binding_class = 111 if spec["mode"] == "legacy" else 95
    binding = view.component(nodes, spec["binding_address"], spec["binding_component"], binding_class, "binding")
    avatar = view.asset(spec["avatar_asset"], 90) if spec["avatar_asset"] else None
    if binding_class == 95:
        animator = view.value(binding)
        replay.require(view.pointer(binding, animator["m_Avatar"], {90}) == avatar, "built Animator Avatar mismatch")
        controller_class = 221 if fixture.playback in ("override", "null_override") else 91
        replay.require(view.pointer(binding, animator["m_Controller"], {controller_class}) is not None,
                       "missing built fixture Animator controller")
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
    check_built_clip(name, fixture, view, clip, nodes, avatar, spec["evidence"]["editor"]["bindings"])
    discovery, effective = view.discovery(binding, binding_class)
    replay.require(clip in effective, "selected clip is absent from built playback component discovery")
    check_discovery(fixture, view, discovery, clip)
    spec["evidence"]["discovery"] = discovery
    doc["format"] = "unityrs.anim_replay"
    restore_parsed_clock(spec, capture, replay)
    if pending_pose(name, doc):
        replay._shape(replay.SCHEMA["$defs"]["capture"], capture, "/capture")
        replay._validate_recording(spec, capture, pending_humanoid=True)
    else:
        replay.validate(doc)
    check_evidence(fixture, doc, view, replay)
    check_coverage(name, fixture, doc, view, clip, avatar, replay)
    if name in BODY_CASES:
        settings = check_body(name, fixture, doc, view, clip, replay)
        if settings is not None:
            spec["evidence"]["body_settings"] = settings
    elif spec["evidence"]["editor"].get("body_case"):
        replay.require(False, f"{name} claims body fixture evidence")
    return doc


def restore_parsed_clock(spec, capture, replay):
    """Undo JsonUtility's misparse of a schedule time in a recording made before the recorder kept its clock.

    The recorder captured from a JSON round trip of its own spec, and 2022.3's JsonUtility read some 16-digit
    times on the 2^-16 grid one ulp off. A sample whose `local` (and so `source`) is the schedule's time one
    double ulp away, at exactly the schedule's f32 sample time and effective source, was sampled at the
    scheduled time: its double labels take the schedule's value. Any other difference stays and is refused."""
    start, stop = float(spec["start"]), float(spec["stop"])
    for point, sample in zip(spec["schedule"], capture["samples"]):
        local, recorded = point["local"], sample.get("local")
        if (type(local) is not float or type(recorded) is not float or recorded == local
                or abs(recorded - local) > math.ulp(local)):
            continue
        local_f32, source, effective = replay.clocks(spec, local, replay.f32_from_bits(replay.f32_bits(stop - start)))
        if (sample.get("local_f32"), sample.get("effective_source")) == (local_f32, effective) \
                and sample.get("source") in (start + recorded, stop if recorded == stop - start else None):
            sample["local"], sample["source"] = local, source


def run_stamp(run):
    return "unityrs fixture run " + " ".join(run[key] for key in ("run_id", "editor_version", "build_target"))


def read_manifest(folder, replay):
    """Read run.json's format and run identity. Runs before the variant field omit it (Built-in)."""
    manifest = replay.read_json((folder / "run.json").read_text(encoding="utf-8"))
    replay.require(isinstance(manifest, dict) and set(manifest) == {"format", "version", "run", "cases"}
                   and manifest["format"] == "unityrs.fixture_run" and type(manifest["version"]) is int
                   and manifest["version"] == 1 and isinstance(manifest["cases"], list),
                   "expected unityrs.fixture_run version 1 in run.json")
    run = manifest["run"]
    identity = {"run_id", "editor_version", "build_target", "build_target_id"}
    replay.require(isinstance(run, dict) and set(run) - {"variant"} == identity
                   and all(isinstance(run[k], str) and re.fullmatch(pattern, run[k]) for k, pattern in (
                       ("run_id", r"[0-9a-f]{32}"), ("editor_version", r"[A-Za-z0-9.]+"),
                       ("build_target", r"[A-Za-z0-9_]+")))
                   and type(run["build_target_id"]) is int and isinstance(run.get("variant", "builtin"), str),
                   "invalid fixture run identity in run.json")
    return manifest, run


def check_handoff_stamps(folder, run, replay):
    with (folder / "build.log").open(encoding="utf-8-sig") as log:
        replay.require(log.readline().rstrip("\r\n") == run_stamp(run), "fixture run identity mismatch in build.log header")
    marker = (folder / "unityrs-generated.txt").read_text(encoding="utf-8-sig").splitlines()
    replay.require(marker == ["Generated by the unityrs fixture builder.", run_stamp(run)],
                   "fixture run identity mismatch in generated folder marker")


def recorded_paths(folder, replay, manifest, run):
    """Check the fixed v1 inventory before trusting any pending recording."""
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
            path = paths[name] if name in paths else folder / "bundles" / STATIC_CASES[name][0]
            if not path.is_file() or path.stat().st_size == 0:
                errors.append(f"{name}: missing {path.relative_to(folder)}")
        if name in BODY_CASES and status == "recorded":
            paths[name] = replay_dir / "body" / f"{name}.pending.json"
            if not paths[name].is_file() or paths[name].stat().st_size == 0:
                errors.append(f"{name}: missing {paths[name].relative_to(folder)}")
        elif name in COVERAGE_CASES and status == "recorded":
            # A recorded coverage case is checked like a required one. Any
            # other status, as in a run that predates it, needs no recording.
            paths[name] = replay_dir / f"{name}.pending.json"
            if not paths[name].is_file() or paths[name].stat().st_size == 0:
                errors.append(f"{name}: missing {paths[name].relative_to(folder)}")
        elif name in OPTIONAL_CASES and status == "recorded" and name != "mixed-transform-storage":
            errors.append(f"{name}: no recording defined for this optional case")
    # A run older than the basis matrix has no rows for it; one older than the
    # body fixtures has none of theirs, and a newer one has them all. The extra
    # probe cases and then the twist sweeps came later again, each a generation
    # of its own.
    body = BODY_CASES.keys() - set().union(*LATER_CASES)
    older = BASIS_CASES.keys() | (set() if statuses.keys() & body else body)
    for generation in LATER_CASES:
        older |= set() if statuses.keys() & generation else generation
    errors.extend(f"missing run.json case: {name}" for name in sorted(expected - statuses.keys() - older))
    # A later case is required once its rig's own probe case records; a later rig's own probe case is required
    # whenever the run has its generation, so an Avatar Unity refuses is an error, not lost coverage.
    errors.extend(f"{name}: required once {primary} records, but {statuses[name]}"
                  for name in sorted(set().union(*LATER_CASES))
                  for primary in [primary_probes(BODY_INFO[name]["rig"])]
                  if primary != name and statuses.get(primary) == "recorded"
                  and statuses.get(name, "recorded") != "recorded")
    for generation in LATER_CASES:
        rigs = sorted(name for name in generation if name == primary_probes(BODY_INFO[name]["rig"]))
        missing = [f"{name} ({statuses[name]})" for name in rigs
                   if statuses.keys() & generation and statuses.get(name, "recorded") != "recorded"]
        if missing:
            errors.append("training rigs not recorded: " + ", ".join(missing))
    # The band sweeps validate B2's lateral domain: once the run has them, each is required whatever its rig's
    # own probe case did.
    missing = [f"{name} ({statuses[name]})" for name in sorted(BAND_CASES)
               if statuses.keys() & BAND_CASES and statuses.get(name, "recorded") != "recorded"]
    if missing:
        errors.append("band sweeps not recorded: " + ", ".join(missing))
    # Coverage confirmed on this Editor version is no longer optional.
    errors.extend(f"{name}: required for Unity {run['editor_version']} but {statuses[name]}"
                  for name in sorted(required_coverage(run["editor_version"]))
                  if statuses.get(name, "recorded") != "recorded")
    undocumented = set(replay_dir.rglob("*.pending.json")) - set(paths.values())
    errors.extend(f"unexpected pending recording: {path.relative_to(folder)}" for path in sorted(undocumented))
    replay.require(not errors, "incomplete fixture run:\n" + "\n".join(errors))
    expected_bundles = {fixture.bundle for fixture in REPLAY_CASES.values()} | {name for name, _ in STATIC_CASES.values()}
    # Unity builds a coverage bundle before its recording can still be refused.
    recorded = {fixture.bundle for name, fixture in (COVERAGE_CASES | BODY_CASES).items() if name in paths}
    optional = {fixture.bundle for fixture in COVERAGE_CASES.values()} | BODY_BUNDLES
    built = {path.name for path in (folder / "bundles").iterdir()}
    replay.require(expected_bundles | recorded <= built <= expected_bundles | optional,
                   "unexpected fixture bundle inventory")
    check_handoff_stamps(folder, run, replay)
    uncovered = [f"{row['name']}: {row['status']} ({row['reason']})" for row in manifest["cases"]
                 if row["name"] in OPTIONAL_CASES and row["status"] != "recorded"]
    return sorted(paths.values()), statuses["mixed-transform-storage"] == "recorded", run, sorted(uncovered)


def pending_bundle(doc, bundle_dir, replay):
    spec, capture = doc["spec"], doc["capture"]
    replay.require(len(spec["bundles"]) == 1 and set(spec["bundles"][0]) == {"name"},
                   "pending replay must select one self-contained bundle by name")
    name = spec["bundles"][0]["name"]
    replay._shape(replay.SCHEMA["$defs"]["bundle"]["properties"]["name"], name, "/spec/bundles/0/name")
    path = bundle_dir / name
    digest = bundle_digest(path)
    replay.require(capture["bundle_sha256"] == [digest], f"bundle digest mismatch: {name}")
    return path, digest


def bundle_digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def check_static(bundle_dir, views, unityrs, inspect_dir, replay, run):
    bundles = {}
    for case, (bundle, assets) in STATIC_CASES.items():
        try:
            if bundle not in bundles:
                path = bundle_dir / bundle
                digest = bundle_digest(path)
                bundles[bundle] = views[bundle, digest] = BundleView(path, digest, 0, unityrs, inspect_dir, replay, run)
            check_built_static(case, bundles[bundle], assets)
        except (KeyError, TypeError, IndexError, AttributeError, ValueError, OSError) as exc:
            raise ValueError(f"{case}: {exc}") from exc


def complete_folder(folder, unityrs, inspect_dir, replay, unseal=None):
    """Validate every result before writing any completed recordings; returns (count, uncovered, sealed).

    The hold-out rig's body cases (by the builder's case table) stay sealed: neither their recordings nor
    their bundles are opened, and they are listed as sealed, until `unseal` names a freeze artifact that
    anim_body_freeze.verify_unseal accepts for this run."""
    manifest, run = read_manifest(folder, replay)
    if run.get("variant", "builtin") != "builtin":
        from anim_fixture_variants import complete_variant

        return complete_variant(folder, manifest, run, unityrs, inspect_dir, replay)
    bundle_dir = folder / "bundles"
    results, views, recordings = [], {}, {}
    paths, recorded_mixed, run, uncovered = recorded_paths(folder, replay, manifest, run)
    holdout = {folder / "replays" / "body" / f"{name}.pending.json": name for name, info in BODY_INFO.items()
               if info["rig"] == HOLDOUT}
    sealed = sorted(holdout[path] for path in paths if path in holdout)
    if sealed and unseal is not None:
        from anim_body_freeze import verify_unseal

        verify_unseal(folder, unseal)
    elif sealed:
        paths = [path for path in paths if path not in holdout]
    check_static(bundle_dir, views, unityrs, inspect_dir, replay, run)
    fixtures = REPLAY_CASES | VECTOR_CASES | COVERAGE_CASES | BODY_CASES
    for path in paths:
        try:
            document = replay.read_json(path.read_text(encoding="utf-8"))
            name = path.name.removesuffix(".pending.json")
            result = complete_recording(name, fixtures[name], document, bundle_dir, views, unityrs, inspect_dir, replay, run)
            recordings[name] = result
            output = path.with_name(name + ".json")
            if pending_pose(name, result):
                if output.exists():
                    existing = replay.read_json(output.read_text(encoding="utf-8"))
                    replay.require(existing.get("format") == "unityrs.anim_replay"
                                   and type(existing.get("version")) is int and existing["version"] == 1,
                                   f"expected completed replay version 1 in {output.name}")
                    check_recording_run(existing, run, replay, "existing known-vector recording")
                continue
            if name == "generic-scale" and recorded_mixed:
                replay.require(result["spec"]["evidence"]["mixed_transform_storage"],
                               "mixed Transform storage reported by Unity is absent from the built scale clip")
            data = json.dumps(result, indent=2, ensure_ascii=False, allow_nan=False) + "\n"
            replay.require(not output.exists() or output.read_text(encoding="utf-8") == data,
                           f"{output.name} already exists with different content")
            results.append((output, data))
        except (KeyError, TypeError, IndexError, AttributeError, ValueError, OSError) as exc:
            raise ValueError(f"{path.name}: {exc}") from exc
    for name in VECTOR_CASES:
        try:
            check_vectors(recordings[name], recordings[name.removesuffix(VECTOR_SUFFIX)], replay)
        except ValueError as exc:
            raise ValueError(f"{name}.pending.json: {exc}") from exc
    for view in views.values():
        replay.require(bundle_digest(view.path) == view.digest, f"bundle changed during inspection: {view.path.name}")
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
    return len(results), uncovered, [] if unseal is not None else sealed
