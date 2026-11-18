"""Synthetic Editor fixtures with distinct asset keys, playback and capture evidence."""

import copy
import hashlib
import json
import math
import struct
from zlib import crc32

from synth import External, Node, Obj, Type, s32, serialized_file, vec
from test_anim_replay import bits, human_capture, synthetic

ROOT_ID = -9223372036854775808
ASSETS = "assets/unityrsfixtures/"
RUN = dict(run_id="1" * 32, editor_version="2022.3.62f3", build_target="StandaloneWindows64", build_target_id=19)
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


def pointer(path_id, file_id=0):
    return {"m_FileID": file_id, "m_PathID": path_id if abs(path_id) <= 2**53 else {"t": "i64", "v": str(path_id)}}


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


def untag(value):
    if isinstance(value, list):
        return [untag(item) for item in value]
    if isinstance(value, dict):
        if value.get("t") == "dict":
            return {key: untag(item) for key, item in value["v"]}
        return {key: untag(item) for key, item in value.items()}
    return value


def rebuild_bundle(bundles, inspect, bundle, recordings):
    """Repack edited object values and bind the inspection and captures to the new bytes."""
    index_path = inspect / (bundle + ".inspect.json")
    index = json.loads(index_path.read_text())
    metadata = index["inputs"][0]["files"]
    files = []
    shows = {}
    for file in metadata[1:]:
        number = file["file_index"]
        rows = []
        for row in index["objects"]:
            if row["file_index"] == number:
                path = inspect / f"{bundle}.{number}.{row['path_id']}.show.json"
                shows[path] = doc = json.loads(path.read_text())
                rows.append((row["path_id"], row["class_id"], row["name"], untag(doc["value"])))
        files.append((file["file"], rows, file["external_paths"]))
    data = unityfs(files, RUN | {"editor_version": metadata[0]["unity_version"],
                                "build_target_id": metadata[1]["target_platform"]})
    (bundles / bundle).write_bytes(data)
    digest = hashlib.sha256(data).hexdigest()
    for path, doc in (shows | {index_path: index}).items():
        doc["bundle_sha256"] = digest
        write(path, doc)
    for doc in recordings.values():
        if doc["spec"]["bundles"] == [{"name": bundle}]:
            doc["capture"]["bundle_sha256"] = [digest]


def stamp(run):
    return "unityrs fixture run {run_id} {editor_version} {build_target}".format(**run)


def serialized_value(name, value):
    """Encode the fixture's minimal type tree and the same value shown in the sidecar."""
    if isinstance(value, dict):
        tag = value.get("t")
        if tag == "i64":
            return Node("SInt64", name), struct.pack("<q", int(value["v"]))
        if tag == "f32":
            return Node("float", name), bytes.fromhex(value["v"])[::-1]
        if tag == "bytes":
            data = bytes.fromhex(value["hex"])
            return Node("TypelessData", name), struct.pack("<i", len(data)) + data
        if tag == "tuple":
            fields = [serialized_value(key, item) for key, item in zip(("first", "second"), value["v"])]
        else:
            fields = [serialized_value(key, item) for key, item in value.items()]
        kind = "pair" if tag == "tuple" else "PPtr<Object>" if set(value) == {"m_FileID", "m_PathID"} else "FixtureRecord"
        return Node(kind, name, [node for node, _ in fields]), b"".join(
            data for _, data in fields)
    if isinstance(value, list):
        items = [serialized_value("data", item) for item in value]
        elem = items[0][0] if items else Node("int", "data")
        return vec(name, elem), struct.pack("<i", len(value)) + b"".join(data for _, data in items)
    if isinstance(value, str):
        return Node("string", name), s32(value)
    if isinstance(value, float):
        return Node("float", name), struct.pack("<f", value)
    if isinstance(value, bool):
        return Node("bool", name, meta=0x4000), struct.pack("<i", value)
    if name == "m_PathID":
        return Node("SInt64", name), struct.pack("<q", value)
    return Node("int" if value < 0 or name == "m_FileID" else "UInt32", name), struct.pack("<I", value & 0xffffffff)


def unityfs(files, run):
    """Uncompressed UnityFS with real object payloads and embedded type trees."""
    payload, directory = b"", b""
    for filename, rows, externals in files:
        types, objects = [], []
        for path, kind, name, value in rows:
            if name is not None:
                value = {"m_Name": name, **value}
            tree, data = serialized_value("Base", value)
            types.append(Type(kind, tree))
            objects.append(Obj(path, len(types) - 1, data))
        serialized = serialized_file(types, objects, unity_version=run["editor_version"],
                                     platform=run["build_target_id"], externals=[External(p) for p in externals])
        directory += struct.pack(">qqI", len(payload), len(serialized), 4) + filename.encode() + b"\0"
        payload += serialized
    size = len(payload)
    info = bytes(16) + struct.pack(">IIIHI", 1, size, size, 0, len(files)) + directory
    head = b"UnityFS\0" + struct.pack(">I", 8) + b"5.x.x\0" + run["editor_version"].encode() + b"\0"
    padding = bytes(-(len(head) + 20) % 16)
    return head + struct.pack(">qIII", len(head) + 20 + len(padding) + len(info) + size,
                              len(info), len(info), 0x40) + padding + info + payload


def archive(bundles, inspect, bundle, assets, rows, run=RUN, shared=()):
    if bundle == "static-batch-scene":
        filename = "PlayerBuild-batch"
        rows.append((999, 1, stamp(run), {}))
    else:
        filename = "CAB-synthetic"
        rows.append((999, 49, bundle, {"m_Script": stamp(run)}))
        assets[ASSETS + "run/" + bundle + ".txt"] = 999
    rows.append((142, 142, bundle, {"m_PreloadTable": [pointer(path) for path in assets.values()], "m_Container": [
        {"t": "tuple", "v": [key, {"preloadIndex": i, "preloadSize": 1, "asset": pointer(path)}]}
        for i, (key, path) in enumerate(assets.items())]}))
    external = filename + ".sharedasset"
    files = [(filename, rows, ["archive:/" + filename + "/" + external] if shared else [])]
    if shared:
        files.append((external, shared, []))
    data = unityfs(files, run)
    (bundles / bundle).write_bytes(data)
    digest = hashlib.sha256(data).hexdigest()
    headers = []
    metadata = [dict(file_index=0, file=bundle, kind="BundleFile", unity_version=run["editor_version"])]
    for file_index, (filename, rows, externals) in enumerate(files, 1):
        metadata.append(dict(file_index=file_index, file=filename, kind="AssetsFile", external_paths=externals,
                             unity_version=run["editor_version"], target_platform=run["build_target_id"]))
        for path, kind, name, value in rows:
            header = dict(input=0, file_index=file_index, file=filename, path_id=path, class_id=kind, name=name,
                          container_path=next((key for key, index in assets.items() if index == path), None))
            headers.append(header)
            if name is not None:
                value = {"m_Name": name, **value}
            write(inspect / f"{bundle}.{file_index}.{path}.show.json", dict(
                format="unityrs.show", version=1, bundle_sha256=digest, object=header, value=canonical(value),
                truncated=dict.fromkeys(("items", "depth", "bytes", "binary", "cuts"), 0)))
    write(inspect / f"{bundle}.inspect.json", dict(
        format="unityrs.inspect", version=1, bundle_sha256=digest, next_cursor=None, total_objects=len(headers),
        inputs=[dict(path=bundle, files=metadata)], objects=headers))
    return digest


def game_object(rows, obj, transform, name, *, parent=0, children=(), components=(), **trs):
    rows.extend(((obj, 1, name, {"m_Component": [{"component": pointer(p)} for p in (transform, *components)]}),
                 (transform, 4, None, {"m_GameObject": pointer(obj), "m_Children": [pointer(p) for p in children],
                                       "m_Father": pointer(parent), **trs})))


def mesh_parts(rows, obj, mesh_filter, renderer, mesh, material, **fields):
    rows.extend(((mesh_filter, 33, None, {"m_GameObject": pointer(obj), "m_Mesh": mesh}),
                 (renderer, 23, None, {"m_GameObject": pointer(obj), "m_Materials": [material], **fields})))


def skin_mesh():
    channels = [dict(stream=0, offset=0, format=0, dimension=0)] * 12
    channels += [dict(stream=0, offset=0, format=0, dimension=4), dict(stream=0, offset=16, format=6, dimension=4)]
    vertices = b"".join(struct.pack("<4f4B", .5, .25, .1875, .0625, i, i ^ 1, i ^ 2, i ^ 3) for i in range(4))
    return {"m_BindPose": [{"e00": 1.}] * 4, "m_VertexData": {
        "m_VertexCount": 4, "m_Channels": channels, "m_DataSize": {"t": "bytes", "hex": vertices.hex()},
    }, "m_Shapes": {
        "channels": [{"name": "Lift", "frameIndex": 0, "frameCount": 2}], "fullWeights": [25., 80.],
        "shapes": [{"firstVertex": 0, "vertexCount": 1}, {"firstVertex": 1, "vertexCount": 1}],
        "vertices": [{"index": 2, "vertex": {"x": 0., "y": 0., "z": weight}} for weight in (.25, 1.)],
    }}


def skin_renderer(mesh, bones):
    return {"m_Mesh": pointer(mesh), "m_Bones": [pointer(bone) for bone in bones], "m_RootBone": pointer(bones[0])}


def static_bundles(bundles, inspect):
    assets = {ASSETS + "static/" + name: i for i, name in enumerate((
        "primitives.prefab", "materials.prefab", "metallic.mat", "unlit.mat", "checker.asset", "lod.prefab",
        "four-influence.asset", "skin.prefab"), 1)}
    rows = [(5, 28, "Checker", {"m_Width": 2, "m_Height": 2}), (7, 43, "FourInfluencePanel", skin_mesh())]
    specs = (
        (1, "Primitives", ("Cube", "Cylinder", "Sphere", "Capsule", "Plane", "Quad"), ()),
        (2, "BuiltInMaterials", ("Metallic", "Unlit"), ()),
        # As built, the saved prefab root takes the asset file's name.
        (6, "lod", ("Near", "Far"), (205,)),
    )
    for root, name, children, components in specs:
        base = 1000 * root
        transforms = [base + 4 * i + 2 for i in range(len(children))]
        game_object(rows, root, base, name, children=transforms, components=components)
        for i, (label, transform) in enumerate(zip(children, transforms)):
            obj, mesh, renderer = (transform + offset for offset in (-1, 1, 2))
            game_object(rows, obj, transform, label, parent=base, components=(mesh, renderer))
            mesh_parts(rows, obj, mesh, renderer, pointer(10500 + i), pointer(3 + i if root == 2 else 10550))
        if root == 6:
            rows.append((205, 205, None, {"m_LODs": [
                {"screenRelativeHeight": h, "renderers": [{"renderer": pointer(t + 2)}]}
                for t, h in zip(transforms, (.6, .2))]}))
    for i, kind in enumerate(("Metallic", "Unlit"), 3):
        tex = {"m_Texture": pointer(5), "m_Scale": {"x": 2., "y": 3.}, "m_Offset": {"x": .25, "y": .5}}
        entry = {"t": "tuple", "v": ["_MainTex", tex]}
        rows.append((i, 21, kind, {"m_Shader": pointer(100 + i), "m_SavedProperties": {
            "m_TexEnvs": [entry], "m_Floats": {"_Metallic": .625, "_Glossiness": .375},
            "m_Colors": {"_EmissionColor": {"r": .125, "g": .25, "b": .5, "a": 1.}},
        }}))
    rows.extend((10500 + i, 43, name, {}) for i, name in enumerate(specs[0][2]))
    rows.extend(((103, 48, "Standard", {}), (104, 48, "Unlit/Texture", {}), (10550, 21, "Default", {})))
    game_object(rows, 8, 8000, "SkinnedPanel", children=range(8002, 8014, 2))
    for i, name in enumerate(("RendererA", "RendererB", "RootBone", "Upper", "Right", "Left")):
        transform = 8002 + 2 * i
        components = (137 + i,) if i < 2 else ()
        game_object(rows, transform - 1, transform, name, parent=8000, components=components)
        if components:
            rows.append((components[0], 137, None, skin_renderer(7, (8006, 8008, 8010, 8012))))
    archive(bundles, inspect, "static-objects", assets, rows)

    scene = []
    for i, name in enumerate(("Alpha", "Zulu")):
        game_object(scene, i + 1, i + 4, name, components=(i + 23, i + 33))
        mesh_parts(scene, i + 1, i + 33, i + 23, pointer(43, 1), pointer(21, 1),
                   m_StaticBatchInfo={"firstSubMesh": i, "subMeshCount": 1})
    shared = [(43, 43, "CombinedBatchPanels", {"m_SubMeshes": [{"indexCount": 6}] * 2}),
              (21, 21, "BatchMaterial", {})]
    archive(bundles, inspect, "static-batch-scene", {ASSETS + "static/batch.unity": 0}, scene, shared=shared)


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
        nodes.update({"1": "Skin", "1/0": "RendererA", "1/1": "RendererB", "1/2": "RootBone",
                      "1/2/0": "Upper", "1/2/1": "Right", "1/2/2": "Left"})
    if reference:
        nodes["2"] = "Reference"
    return nodes


def recording(case, bundle, prefab, clip, avatar, run):
    doc = synthetic()
    spec, capture = doc["spec"], doc["capture"]
    legacy, human = case == "legacy-explicit", bool(avatar)
    modes = "legacy" if legacy else "animator"
    prop, first, last = GENERIC.get(case, ("rotation", 0, .5) if human else ("translation", 2, 6))
    start = .5 if case == "generic-offset-stepped" else 0
    held = [.75] if start else []
    # As in the Editor run, the late-start clip's length and playable time
    # are measured from zero, holding the start pose up to the interval.
    base = 0 if held else start
    keys = [start, (start + .25 if held else .5), start + 1]
    # The recorder's zero-origin schedule: keys, off-key samples, and the
    # f32 predecessor and a 1/4096 left sample of the held boundary.
    before = struct.unpack(">f", struct.pack(">I", struct.unpack(">I", struct.pack(">f", .75))[0] - 1))[0]
    times = ([0, .5, .5 + .25 * 391 / 1024, .75 - 1 / 4096, before, .75, .75 + .75 * 391 / 1024, 1.5]
             if held else list(VECTOR_TIMES))
    nodes = rig_nodes(human, not legacy and not human, not legacy and not human)
    discovery = {"generic-override-morph": "override", "generic-null-override": "null_override"}.get(case, "")
    bindings = clip_bindings(case, prop, keys, first, last)
    editor = dict(fixture_case=case, run_id=run["run_id"], animation_source="procedural_humanoid" if human else "procedural_clip",
                  expected_discovery=discovery, bindings=bindings)
    for key in ("prefab", "clip", "binding", "avatar"):
        del spec[key]
    spec.update(bundles=[dict(name=bundle)], prefab_asset=prefab, clip_asset=clip, avatar_asset=avatar,
                clip_name="LegacyExplicit" if legacy else clip.rsplit("/", 1)[1][:-5],
                binding_address="0" if human else "", mode=modes, start=start, stop=start + 1, sample_rate=16,
                build_target=run["build_target"], source_clock="zero_origin" if held else "widened",
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
        value = first if i == 0 or (held and t < .75) else last
        if prop == "weight":
            pose["weights"][0]["channels"][0] = bits(value)
        elif human:
            x = [0, .1, .25, .375, .5][i]
            pose["transforms"][3]["rotation"] = [bits(x), bits(0), bits(0), bits(math.sqrt(1 - x*x))]
        else:
            pose["transforms"][1][prop][0] = bits(value)
        local = struct.unpack(">f", struct.pack(">f", t))[0]
        samples.append(dict(local=t, local_f32=bits(t), source=max(start, base + t),
                            effective_source=max(start, base + local),
                            sides=["left" if t < .75 else "right"] if held else [], muscles_f32=[], pose=pose))

    def probe(sample, value):
        return dict(sample=sample, property=prop, target=probe_target, component=0,
                    channel=0 if prop == "weight" else -1, value=bits(value))

    spec["evidence"] = dict(layout="editor_authored", editor=editor, clock=dict(
        start=dict(source=start, field="editor_interval", probe=probe(0, first)),
        stop=dict(source=start + 1, field="editor_interval", probe=probe(len(times) - 1, last)),
        key_times=keys, held=[dict(source=.75, left=probe(4, first), right=probe(5, last))] if held else []))
    capture.update(rest=rest, samples=samples, editor_version=run["editor_version"], build_target=run["build_target"])
    capture["playback"].update(api="Animation.Sample" if legacy else "AnimationClipPlayable", clip_human_motion=human,
                               wrap_mode="ClampForever" if legacy else "Once", frame_rate_f32=bits(16),
                               clip_length_f32=bits(start + 1 - base),
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


def stored_curves(spec):
    rows = spec["evidence"]["editor"]["bindings"]
    fields = {"m_LocalPosition": "m_PositionCurves", "m_LocalScale": "m_ScaleCurves",
              "m_LocalRotation": "m_RotationCurves", "localEulerAnglesRaw": "m_EulerCurves"}
    vectors, result = {}, {}
    for row in rows:
        prop, path = row["property"], row["path"]
        kind = row["type"].removeprefix("UnityEngine.")
        if row["object_reference"]:
            result["m_PPtrCurves"] = [{"path": path, "classID": 33, "attribute": prop,
                                       "curve": [{"time": t, "value": pointer(80 + i)} for i, t in enumerate((0., .5))]}]
            continue
        keys = []
        for i, key in enumerate(row["keys"]):
            read = lambda field: struct.unpack(">f", bytes.fromhex(key[field]))[0]
            weighted = {"None": 0, "In": 1, "Out": 2}[key["weighted_mode"]]
            slope = 0. if key["out_tangent_f32"] != "7f800000" else {"t": "f32", "v": "7f800000"}
            value = read("value_f32")
            if prop.startswith("m_LocalRotation."):
                value = (math.sin(i / 4) if prop.endswith("y") else math.cos(i / 4) if prop.endswith("w") else 0.)
            keys.append(dict(time=read("time_f32"), value=value, inSlope=slope, outSlope=slope, weightedMode=weighted,
                             inWeight=.375, outWeight=.25))
        if kind == "Transform":
            family, _, axis = prop.rpartition(".")
            vectors.setdefault((fields[family], path), {})[axis] = keys
        else:
            result.setdefault("m_FloatCurves", []).append({"path": path, "classID": 95 if kind == "Animator" else 137,
                                                          "attribute": prop, "curve": {"m_Curve": keys}})
    for (field, path), axes in vectors.items():
        curve = []
        for points in zip(*axes.values()):
            first = points[0]
            curve.append({"time": first["time"], **{key: dict(zip(axes, (p[key] for p in points))) for key in first
                                                    if key != "time"}})
        result.setdefault(field, []).append({"path": path, "curve": {"m_Curve": curve}})
    return result


def fixture_archives(bundles, inspect, run=RUN, legacy_only=False):
    recordings = {}
    inventory = [("anim-legacy", ("legacy-explicit",))]
    if not legacy_only:
        inventory.extend([("anim-generic", tuple(GENERIC)), *((rig, tuple(c for c in HUMANOID if c.startswith(rig)))
                                                             for rig in ("humanoid-compact", "humanoid-tall"))])
    for bundle, cases in inventory:
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
            doc = recordings[case] = recording(case, bundle, prefab, clip, avatar, run)
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
                children = [key for child, (key, _, _) in nodes.items()
                            if child and child.rpartition("/")[0] == address]
                components = [transform]
                if address == spec["binding_address"]:
                    components.append(base + 3)
                for i, renderer in enumerate(spec["renderers"]):
                    if renderer["address"] == address:
                        components.append(base + 7 + i)
                        value = {"m_Mesh": pointer(base + 9)} if human else skin_renderer(
                            base + 9, [nodes[key][0] for key in ("1/2", "1/2/0", "1/2/1", "1/2/2")])
                        rows.append((base + 7 + i, 137, None, value))
                if name == "Reference":
                    components.append(base + 33)
                    rows.append((base + 33, 33, None, {"m_Mesh": pointer(80)}))
                trs = {}
                if human and address == "0":
                    offset = case.endswith("-offset")
                    trs = dict(m_LocalPosition=dict(x=2. if offset else 0., y=.25 if offset else 0., z=-1. if offset else 0.),
                               m_LocalRotation=dict(x=0., y=.3 if offset else 0., z=0., w=.95 if offset else 1.))
                game_object(rows, obj, transform, name, parent=nodes[parent][0] if address else 0,
                            children=children, components=components[1:], **trs)
            if legacy:
                rows.append((base + 3, 111, None, {"m_Animation": pointer(base + 2), "m_Animations": [pointer(base + 2)]}))
                continue
            if avatar and avatar not in assets:
                assets[avatar] = 90
                rows.append((90, 90, bundle, {"m_Avatar": {"m_Human": {"m_RootX": {"t": {"x": 0.}}}}}))
            rows.append((base + 9, 43, "Shared", {} if human else skin_mesh()))
            rows.append((base + 3, 95, None, {"m_Controller": pointer(221 if "override" in prefab else 91),
                                              "m_Avatar": pointer(90 if human else 0)}))
        for path, case in clips:
            spec = recordings[case]["spec"]
            times = spec["evidence"]["clock"]["key_times"]
            value = {"m_SampleRate": 16., "m_Legacy": case == "legacy-explicit", **stored_curves(spec)}
            if not value["m_Legacy"]:
                # Two streamed frames and a dense grid. The held boundary is a streamed key.
                stream = [word for t in times for word in (int(bits(t), 16), 1, 0, 0, 0, 0, 0)]
                value["m_MuscleClip"] = {"m_StartTime": spec["start"], "m_StopTime": spec["stop"], "m_LoopTime": False,
                                         "m_CycleOffset": .1, "m_Clip": {"data": {
                    "m_StreamedClip": {"curveCount": 1, "data": stream},
                    "m_DenseClip": {"m_CurveCount": 1, "m_FrameCount": 4, "m_BeginTime": spec["start"],
                                    "m_SampleRate": 4, "m_SampleArray": [0., 1., 2., 3.]},
                    "m_ConstantClip": {"data": []}}}}
                if case in HUMANOID:
                    value["m_MuscleClip"]["m_IndexArray"] = [0, 1, -1]
                if case == "generic-object-reference":
                    value["m_ClipBindingConstant"] = {
                        "genericBindings": [
                            dict(path=crc32(b"Reference"), typeID=33, attribute=crc32(b"m_Mesh"),
                                 customType=0, isPPtrCurve=True),
                            dict(path=crc32("Mötión".encode()), typeID=4, attribute=1,
                                 customType=0, isPPtrCurve=False),
                        ],
                        "pptrCurveMapping": [pointer(80), pointer(81)],
                    }
                    data = value["m_MuscleClip"]["m_Clip"]["data"]
                    data["m_StreamedClip"]["data"] = [
                        0xff7fffff, 1, 0, 0, 0, 0, 0,
                        int(bits(.5), 16), 1, 0, 0, 0, 0, int(bits(1.), 16),
                        0x7f800000, 0,
                    ]
                    data["m_ConstantClip"]["data"] = [0., 0.]
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
        digest = archive(bundles, inspect, bundle, assets, rows, run=run)
        for case in cases:
            recordings[case]["capture"]["bundle_sha256"] = [digest]
    for name in () if legacy_only else VECTORS:
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
