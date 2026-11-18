"""Synthetic Editor fixtures with distinct asset keys, playback and capture evidence."""

import copy
import hashlib
import json
import math
import struct

from synth import Obj, Type, serialized_file
from test_anim_replay import bits, human_capture, synthetic

ROOT_ID = -9223372036854775808
ASSETS = "assets/unityrsfixtures/"
STATIC = {
    "static-primitives": ("primitives.prefab",),
    "static-materials": ("materials.prefab", "metallic.mat", "unlit.mat", "checker.asset"),
    "static-lod": ("lod.prefab",),
    "static-skin": ("skin.prefab", "four-influence.asset"),
    "static-batch-scene": ("batch.unity",),
}
GENERIC = {
    "generic-quaternion": ("translation", 1, 3),
    "generic-euler": ("translation", 2, 5),
    "generic-constant": ("translation", 8, 8),
    "generic-offset-stepped": ("translation", 4, 12),
    "generic-weighted": ("translation", 1, 6),
    "generic-morph": ("weight", 12.5, 80),
    "generic-object-reference": ("translation", 3, 9),
    "generic-scale": ("scale", 1, 2.5),
    "generic-override-morph": ("weight", 25, 50),
    "generic-null-override": ("translation", 8, 8),
}
HUMANOID = {
    f"{rig}-{motion}{offset}": (rig, motion, offset)
    for rig in ("humanoid-compact", "humanoid-tall")
    for motion in ("muscles", "body-motion")
    for offset in ("", "-offset")
    if motion == "muscles" or (rig == "humanoid-compact" and not offset)
}
REPLAYS = ("legacy-explicit", *GENERIC, *HUMANOID)
VECTORS = tuple(f"{name}-known-vectors" for name in HUMANOID if "-muscles" in name)
DEFERRED = (
    "urp-materials", "packed-rotation-slopes", "authored-runtime-precedence", "mixed-transform-storage",
    "humanoid-twist-refusal", "humanoid-non-neutral-setting-refusal", "humanoid-slot-axis-inventory",
    "humanoid-root-hips-comparison", "integer-discrete-binding",
)
SPINE = ("Spine Front-Back", "Spine Left-Right", "Spine Twist Left-Right")
VECTOR_TIMES = (0, 391 / 2048, .5, .5 + 391 / 2048, 1)
REFERENCES = ("reference-a.asset", "reference-b.asset")


def pointer(path_id):
    return {"m_FileID": 0, "m_PathID": path_id if abs(path_id) <= 2**53 else {"t": "i64", "v": str(path_id)}}


def canonical(value):
    if isinstance(value, dict) and "t" not in value:
        return {"t": "dict", "v": [[key, canonical(item)] for key, item in value.items()]}
    if isinstance(value, list):
        return [canonical(item) for item in value]
    if type(value) is float:
        return {"t": "f32", "v": bits(value)}
    return value


def write(path, doc):
    path.write_text(json.dumps(doc), encoding="utf-8")


def unityfs(filename, rows):
    """An uncompressed UnityFS archive with the file and object IDs used by the synthetic inspection."""
    classes = sorted({kind for _, kind, _, _ in rows})
    payload = serialized_file([Type(kind) for kind in classes],
                              [Obj(path, classes.index(kind), b"\0" * 4) for path, kind, _, _ in rows],
                              enable_type_tree=False, unity_version=b"2022.3.62f3")
    size = len(payload)
    info = bytes(16) + struct.pack(">IIIHIqqI", 1, size, size, 0, 1, 0, size, 4) + filename.encode() + b"\0"
    head = b"UnityFS\0" + struct.pack(">I", 8) + b"5.x.x\x002022.3.62f3\0"
    padding = bytes(-(len(head) + 20) % 16)
    return head + struct.pack(">qIII", len(head) + 20 + len(padding) + len(info) + size,
                              len(info), len(info), 0x40) + padding + info + payload


def archive(bundles, inspect, bundle, assets, rows):
    filename = "CAB-synthetic"
    rows.append((142, 142, bundle, {"m_Container": [
        {"t": "tuple", "v": [key, canonical({"asset": pointer(path)})]} for key, path in assets.items()]}))
    data = unityfs(filename, rows)
    (bundles / bundle).write_bytes(data)
    digest = hashlib.sha256(data).hexdigest()
    headers = []
    for path, kind, name, value in rows:
        header = dict(input=0, file_index=1, file=filename, path_id=path, class_id=kind, name=name,
                      container_path=next((key for key, index in assets.items() if index == path), None))
        headers.append(header)
        write(inspect / f"{bundle}.1.{path}.show.json", dict(
            format="unityrs.show", version=1, bundle_sha256=digest, object=header, value=canonical(value),
            truncated=dict.fromkeys(("items", "depth", "bytes", "binary", "cuts"), 0)))
    write(inspect / f"{bundle}.inspect.json", dict(
        format="unityrs.inspect", version=1, bundle_sha256=digest, next_cursor=None, total_objects=len(headers),
        inputs=[dict(path=bundle, files=[dict(file_index=0, file=bundle, kind="BundleFile"),
                                         dict(file_index=1, file=filename, kind="AssetsFile")])], objects=headers))
    return digest


def static_bundles(bundles, inspect):
    assets, rows = {}, []
    kinds = {"prefab": 1, "mat": 21, "checker.asset": 28, "four-influence.asset": 43}
    for case, files in STATIC.items():
        for asset in files:
            if case == "static-batch-scene":
                archive(bundles, inspect, case, {ASSETS + "static/" + asset: 0}, [(1, 1, "Alpha", {})])
            else:
                path = len(rows) + 1
                keys = asset.split(".")
                rows.append((path, kinds.get(asset, kinds.get(keys[-1])), keys[0], {}))
                assets[ASSETS + "static/" + asset] = path
    archive(bundles, inspect, "static-objects", assets, rows)


def binding(path, prop, kind, times, first=0, last=1, *, stepped=False, weighted=False):
    keys = [dict(time_f32=bits(time), value_f32=bits(first if i == 0 else last),
                 in_tangent_f32="7f800000" if stepped else bits(0),
                 out_tangent_f32="7f800000" if stepped else bits(0),
                 in_weight_f32=bits(.25), out_weight_f32=bits(.25),
                 weighted_mode=("Out" if i == 0 else "In") if weighted else "None")
            for i, time in enumerate(times)]
    return dict(path=path, property=prop, type="UnityEngine." + kind, discrete=False,
                object_reference=False, object_keys=[], keys=keys)


def clip_bindings(case, prop, keys, first, last):
    motion = "Mötión"
    if case in HUMANOID:
        rows = [binding("", name, "Animator", keys) for name in SPINE]
        if case.endswith("body-motion"):
            rows += [binding("", family + axis, "Animator", keys) for family, axes in
                     (("RootT.", "xyz"), ("RootQ.", "xyzw")) for axis in axes]
        return rows
    if prop == "weight":
        return [binding(f"Skin/Renderer{suffix}", "blendShape.Lift", "SkinnedMeshRenderer", keys, first, last)
                for suffix in "AB"] + [binding("Skin/RootBone/Upper", "m_LocalRotation." + axis, "Transform", keys)
                                      for axis in "xyzw"]
    family = "m_LocalScale." if prop == "scale" else "m_LocalPosition."
    rows = [binding(motion, family + axis, "Transform", keys, first, last,
                    stepped=case == "generic-offset-stepped", weighted=case in ("generic-weighted", "generic-scale"))
            for axis in "xyz"]
    rotation = {"legacy-explicit": "m_LocalRotation.", "generic-quaternion": "m_LocalRotation.",
                "generic-euler": "localEulerAnglesRaw."}.get(case)
    if rotation:
        rows += [binding(motion, rotation + axis, "Transform", keys)
                 for axis in ("xyz" if case == "generic-euler" else "xyzw")]
    if case == "generic-object-reference":
        rows.append(dict(path="Reference", property="m_Mesh", type="UnityEngine.MeshFilter", discrete=False,
                         object_reference=True, keys=[], object_keys=reference_keys((0, .5))))
    return rows


def reference_keys(times):
    return [dict(time_f32=bits(time), asset=ASSETS + "generic/" + REFERENCES[int(time >= .5)],
                 name="ReferenceB" if time >= .5 else "ReferenceA") for time in times]


def rig_nodes(human, morph, reference):
    if human:
        return {"": "HumanoidFixture", "0": "HumanoidRig", "0/0": "Hips", "0/0/0": "Spine"}
    nodes = {"": "LegacyRig" if not morph else "GenericRig", "0": "Mötión"}
    if morph:
        nodes.update({"1": "Skin", "1/0": "RendererA", "1/1": "RendererB", "1/2": "RootBone", "1/2/0": "Upper"})
    if reference:
        nodes["2"] = "Reference"
    return nodes


def recording(case, bundle, prefab, clip, avatar):
    doc = synthetic()
    spec, capture = doc["spec"], doc["capture"]
    legacy, human = case == "legacy-explicit", bool(avatar)
    modes = "legacy" if legacy else "animator"
    prop, first, last = GENERIC.get(case, ("rotation", 0, .5) if human else ("translation", 2, 6))
    start = .5 if case == "generic-offset-stepped" else 0
    held = [.75] if start else []
    keys = [start, (start + .25 if held else .5), start + 1]
    times = [0, .1, .25, 1] if held else list(VECTOR_TIMES)
    nodes = rig_nodes(human, not legacy and not human, not legacy and not human)
    discovery = {"generic-override-morph": "override", "generic-null-override": "null_override"}.get(case, "")
    bindings = clip_bindings(case, prop, keys, first, last)
    editor = dict(fixture_case=case, animation_source="procedural_humanoid" if human else "procedural_clip",
                  expected_discovery=discovery, bindings=bindings)
    for key in ("prefab", "clip", "binding", "avatar"):
        del spec[key]
    spec.update(bundles=[dict(name=bundle)], prefab_asset=prefab, clip_asset=clip, avatar_asset=avatar,
                clip_name="LegacyExplicit" if legacy else clip.rsplit("/", 1)[1][:-5],
                binding_address="0" if human else "", mode=modes, start=start, stop=start + 1, sample_rate=16,
                held_boundaries=held, schedule=[dict(local=t) for t in times],
                nodes=[dict(address=address, name=name) for address, name in nodes.items()],
                renderers=[dict(address=address, component=0, mesh_group=0,
                                channels=copy.deepcopy(spec["renderers"][0]["channels"]))
                           for address in (() if legacy else ("0",) if human else ("1/0", "1/1"))])
    neutral = capture["rest"]["transforms"][0]
    rest = dict(transforms=[copy.deepcopy(neutral) for _ in nodes],
                weights=[dict(channels=[bits(0)]) for _ in spec["renderers"]])
    probe_target = 3 if human else 0 if prop == "weight" else 1
    samples = []
    for i, t in enumerate(times):
        pose = copy.deepcopy(rest)
        value = first if i == 0 or (held and t < .25) else last
        if prop == "weight":
            pose["weights"][0]["channels"][0] = bits(value)
        elif human:
            x = [0, .1, .25, .375, .5][i]
            pose["transforms"][3]["rotation"] = [bits(x), bits(0), bits(0), bits(math.sqrt(1 - x*x))]
        else:
            pose["transforms"][1][prop][0] = bits(value)
        local = struct.unpack(">f", struct.pack(">f", t))[0]
        samples.append(dict(local=t, local_f32=bits(t), source=start + t, effective_source=start + local,
                            sides=["left" if t < .25 else "right"] if held else [], muscles_f32=[], pose=pose))

    def probe(sample, value):
        return dict(sample=sample, property=prop, target=probe_target, component=0,
                    channel=0 if prop == "weight" else -1, value=bits(value))

    spec["evidence"] = dict(layout="editor_authored", editor=editor, clock=dict(
        start=dict(source=start, field="editor_interval", probe=probe(0, first)),
        stop=dict(source=start + 1, field="editor_interval", probe=probe(len(times) - 1, last)),
        key_times=keys, held=[dict(source=.75, left=probe(1, first), right=probe(2, last))] if held else []))
    capture.update(rest=rest, samples=samples)
    capture["playback"].update(api="Animation.Sample" if legacy else "AnimationClipPlayable", clip_human_motion=human,
                               wrap_mode="ClampForever" if legacy else "Once", frame_rate_f32=bits(16),
                               legacy_state="enabled_weight_one_clamp_forever" if legacy else "unused")
    capture["clip_settings"].update(startTime=bits(start), stopTime=bits(start + 1))
    if case == "generic-object-reference":
        editor["object_reference_samples"] = reference_keys(times)
    if human:
        muscles = (*SPINE, *(f"Other muscle {i}" for i in range(92)))
        capture["humanoid"] = human_capture() | dict(
            bones=[dict(name="Spine", node=3, required=True)],
            muscles=[dict(name=name, bone=0, axis=i % 3, min=bits(-40), max=bits(20))
                     for i, name in enumerate(muscles)], rest_muscles=[bits(0)] * len(muscles))
        editor.update(spine_start_f32=samples[0]["pose"]["transforms"][3]["rotation"],
                      spine_stop_f32=samples[-1]["pose"]["transforms"][3]["rotation"],
                      spine_endpoint_tolerance_degrees=.25)
    doc["format"] = "unityrs.anim_replay_pending"
    return doc


def fixture_archives(bundles, inspect):
    recordings = {}
    for bundle, cases in (("anim-legacy", ("legacy-explicit",)), ("anim-generic", tuple(GENERIC)),
                          *((rig, tuple(c for c in HUMANOID if c.startswith(rig)))
                            for rig in ("humanoid-compact", "humanoid-tall"))):
        assets, rows, clips = {}, [], []
        for index, case in enumerate(cases):
            base = 1000 * index
            legacy = case == "legacy-explicit"
            human = case in HUMANOID
            generic_dir = ASSETS + "generic/"
            directory = ASSETS + (("legacy" if legacy else bundle if human else "generic") + "/")
            stem = "-".join(HUMANOID[case][:2]) if human else "generic-constant" if case.endswith("null-override") else case
            clip = directory + stem + ".anim"
            prefab = directory + ("offset.prefab" if case.endswith("-offset") else "identity.prefab") if human else (
                directory + ("legacy.prefab" if legacy else "override.prefab" if "override" in case else "generic.prefab"))
            avatar = directory + "avatar.asset" if human else ""
            doc = recordings[case] = recording(case, bundle, prefab, clip, avatar)
            spec = doc["spec"]
            if clip not in assets:
                assets[clip] = base + 2
                clips.append((assets[clip], case))
            if prefab in assets:
                continue
            assets[prefab] = base + 1
            nodes = {node["address"]: (base + 40 + i, base + 60 + i, node["name"])
                     for i, node in enumerate(spec["nodes"])}
            nodes[""] = (ROOT_ID if not base else base + 40, base + 1, nodes[""][2])
            for address, (transform, obj, name) in nodes.items():
                parent = address.rpartition("/")[0]
                children = [pointer(key) for child, (key, _, _) in nodes.items()
                            if child and child.rpartition("/")[0] == address]
                components = [transform]
                if address == spec["binding_address"]:
                    components.append(base + 3)
                for i, renderer in enumerate(spec["renderers"]):
                    if renderer["address"] == address:
                        components.append(base + 7 + i)
                        rows.append((base + 7 + i, 137, None, {"m_Mesh": pointer(base + 9)}))
                rows.extend(((obj, 1, name, {"m_Component": [{"component": pointer(p)} for p in components]}),
                             (transform, 4, None, {"m_GameObject": pointer(obj), "m_Children": children,
                                                    "m_Father": pointer(nodes[parent][0] if address else 0)})))
            if legacy:
                rows.append((base + 3, 111, None, {"m_Animation": pointer(base + 2), "m_Animations": [pointer(base + 2)]}))
                continue
            if avatar and avatar not in assets:
                assets[avatar] = 90
                rows.append((90, 90, bundle, {}))
            rows.append((base + 9, 43, "Shared", {}))
            rows.append((base + 3, 95, None, {"m_Controller": pointer(221 if "override" in prefab else 91),
                                              "m_Avatar": pointer(90 if human else 0)}))
        for path, case in clips:
            spec = recordings[case]["spec"]
            times = spec["evidence"]["clock"]["key_times"]
            value = {"m_SampleRate": 16, "m_Legacy": case == "legacy-explicit"}
            if value["m_Legacy"]:
                value["m_PositionCurves"] = [{"curve": {"m_Curve": [{"time": t} for t in times]}}]
            else:
                # Two streamed frames and a dense grid. The held boundary is a streamed key.
                stream = [word for t in times for word in (int(bits(t), 16), 1, 0, 0, 0, 0, 0)]
                value["m_MuscleClip"] = {"m_StartTime": spec["start"], "m_StopTime": spec["stop"], "m_LoopTime": False,
                                         "m_CycleOffset": .1, "m_Clip": {"data": {
                    "m_StreamedClip": {"curveCount": 1, "data": stream},
                    "m_DenseClip": {"m_CurveCount": 1, "m_FrameCount": 4, "m_BeginTime": spec["start"],
                                    "m_SampleRate": 4, "m_SampleArray": [0., 1., 2., 3.]},
                    "m_ConstantClip": {"data": []}}}}
            rows.append((path, 74, spec["clip_name"], value))
        if bundle != "anim-legacy":
            rows.append((91, 91, "Controller", {"m_AnimationClips": [pointer(p) for p, c in clips
                                                                    if c != "generic-override-morph"]}))
        if bundle == "anim-generic":
            constant, morph, replacement = (assets[generic_dir + c + ".anim"]
                                           for c in ("generic-constant", "generic-morph", "generic-override-morph"))
            rows.append((221, 221, "Override", {"m_Controller": pointer(91), "m_Clips": [
                {"m_OriginalClip": pointer(constant), "m_OverrideClip": pointer(0)},
                {"m_OriginalClip": pointer(morph), "m_OverrideClip": pointer(replacement)}]}))
            for i, asset in enumerate(REFERENCES):
                assets[generic_dir + asset] = 80 + i
                rows.append((80 + i, 43, "Reference" + "AB"[i], {}))
        digest = archive(bundles, inspect, bundle, assets, rows)
        for case in cases:
            recordings[case]["capture"]["bundle_sha256"] = [digest]
    for name in VECTORS:
        doc = recordings[name] = copy.deepcopy(recordings[name.removesuffix("-known-vectors")])
        doc["spec"]["mode"] = "human_pose"
        doc["spec"]["evidence"]["editor"]["fixture_case"] = name
        doc["capture"]["playback"]["api"] = "HumanPoseHandler.SetHumanPose"
        for point, sample in zip(doc["spec"]["schedule"], doc["capture"]["samples"]):
            t = point["local"]
            vector = [t, 0, 0] if t <= .5 else [.5 - .75 * (2*t - 1), .375 * (2*t - 1), -.5 * (2*t - 1)]
            point["muscles"] = vector + [0] * 92
            sample["muscles_f32"] = [bits(v) for v in point["muscles"]]
    return recordings
