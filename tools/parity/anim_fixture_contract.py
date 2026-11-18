"""Required case identities and evidence for the Windows Editor fixture run."""

from dataclasses import dataclass
from math import isfinite, radians
import struct
from zlib import crc32

from anim_reference import dense, streamed, weighted

ASSETS = "assets/unityrsfixtures/"
STATIC_CASES = {
    "static-primitives": ("static-objects", {"primitives.prefab": 1}),
    "static-materials": ("static-objects", {"materials.prefab": 1, "metallic.mat": 21,
                                           "unlit.mat": 21, "checker.asset": 28}),
    "static-lod": ("static-objects", {"lod.prefab": 1}),
    "static-skin": ("static-objects", {"skin.prefab": 1, "four-influence.asset": 43}),
    "static-batch-scene": ("static-batch-scene", {"batch.unity": None}),
}
MOTION = "Mötión"
SPINE_PATH = "HumanoidRig/Hips/Spine"
SPINE = ("Spine Front-Back", "Spine Left-Right", "Spine Twist Left-Right")
VECTOR_SUFFIX = "-known-vectors"
VECTOR_TIMES = (0, 391 / 2048, .5, .5 + 391 / 2048, 1)
REFERENCE_ASSETS = tuple(ASSETS + "generic/reference-" + suffix + ".asset" for suffix in "ab")


@dataclass(frozen=True)
class Fixture:
    bundle: str
    folder: str
    prefab: str
    clip: str
    playback: str
    probe: tuple
    bindings: tuple
    checks: tuple = ()

    @property
    def avatar(self):
        return ASSETS + self.folder + "/avatar.asset" if "spine" in self.checks else ""


def channels(path, kind, prefix, axes):
    return tuple((path, kind, prefix + axis) for axis in axes)


POSITION = channels(MOTION, "Transform", "m_LocalPosition.", "xyz")
ROTATION = channels(MOTION, "Transform", "m_LocalRotation.", "xyzw")
EULER = channels(MOTION, "Transform", "localEulerAnglesRaw.", "xyz")
SCALE = channels(MOTION, "Transform", "m_LocalScale.", "xyz")
MORPH = tuple((f"Skin/Renderer{suffix}", "SkinnedMeshRenderer", "blendShape.Lift") for suffix in "AB")
MORPH += channels("Skin/RootBone/Upper", "Transform", "m_LocalRotation.", "xyzw")
REFERENCE = ("Reference", "MeshFilter", "m_Mesh")
MUSCLES = tuple(("", "Animator", muscle) for muscle in SPINE)
BODY = channels("", "Animator", "RootT.", "xyz") + channels("", "Animator", "RootQ.", "xyzw")


def generic(clip, first, last, bindings=POSITION, *, playback="animator", prop="translation", checks=()):
    path = "Skin/RendererA" if prop == "weight" else MOTION
    return Fixture("anim-generic", "generic", "override" if playback.endswith("override") else "generic",
                   clip, playback, (path, prop, first, last), bindings, checks)


REPLAY_CASES = {
    "legacy-explicit": Fixture("anim-legacy", "legacy", "legacy", "legacy-explicit", "legacy",
                               (MOTION, "translation", 2, 6), POSITION + ROTATION),
    "generic-quaternion": generic("generic-quaternion", 1, 3, POSITION + ROTATION),
    "generic-euler": generic("generic-euler", 2, 5, POSITION + EULER),
    "generic-constant": generic("generic-constant", 8, 8, checks=("constant",)),
    "generic-offset-stepped": generic("generic-offset-stepped", 4, 12, checks=("held",)),
    "generic-weighted": generic("generic-weighted", 1, 6, checks=("weighted",)),
    "generic-morph": generic("generic-morph", 12.5, 80, MORPH, prop="weight"),
    "generic-object-reference": generic("generic-object-reference", 3, 9, POSITION + (REFERENCE,),
                                        checks=("references",)),
    "generic-override-morph": generic("generic-override-morph", 25, 50, MORPH, playback="override", prop="weight"),
    "generic-null-override": generic("generic-constant", 8, 8, playback="null_override", checks=("constant",)),
    "generic-scale": generic("generic-scale", 1, 2.5, SCALE, prop="scale", checks=("weighted",)),
}
VECTOR_CASES = {}
for rig in ("humanoid-compact", "humanoid-tall"):
    for prefab, suffix in (("identity", ""), ("offset", "-offset")):
        clip = rig + "-muscles"
        case = clip + suffix
        probe = (SPINE_PATH, "rotation", None, None)
        REPLAY_CASES[case] = Fixture(rig, rig, prefab, clip, "animator", probe, MUSCLES, ("spine", "muscle_only"))
        VECTOR_CASES[case + VECTOR_SUFFIX] = Fixture(rig, rig, prefab, clip, "human_pose", probe,
                                                     MUSCLES, ("spine", "muscle_only"))
REPLAY_CASES["humanoid-compact-body-motion"] = Fixture(
    "humanoid-compact", "humanoid-compact", "identity", "humanoid-compact-body-motion", "animator",
    (SPINE_PATH, "rotation", None, None), MUSCLES + BODY, ("spine",))


TRANSFORM_CURVES = {
    "m_LocalPosition": ("m_PositionCurves", 1, "xyz"),
    "m_LocalRotation": ("m_RotationCurves", 2, "xyzw"),
    "m_LocalScale": ("m_ScaleCurves", 3, "xyz"),
    "localEulerAnglesRaw": ("m_EulerCurves", 4, "xyz"),
}
# AnimatorMuscle bindings (custom type 8) reserve seven slots each for
# Motion, Root and four IK goals before the three Spine muscle attributes.
HUMANOID_ATTRIBUTES = dict(zip(BODY + MUSCLES, (*range(7, 14), *range(42, 45))))


def built_transform_curves(clip):
    curves = {}
    for name, (field, _, axes) in TRANSFORM_CURVES.items():
        for row in clip.get(field, []):
            keys = row["curve"]["m_Curve"]
            if keys and all(isinstance(key.get("value"), dict) and set(axes) <= key["value"].keys() for key in keys):
                for axis in axes:
                    curves[row["path"], "Transform", name + "." + axis] = keys, axis
    return curves


def unit_quaternion(values):
    return all(type(value) in (int, float) and isfinite(value) for value in values) and abs(
        sum(value * value for value in values) - 1) <= .002


def built_float_curves(clip, field):
    classes = {4: "Transform", 95: "Animator", 137: "SkinnedMeshRenderer"}
    return {(row["path"], classes[row["classID"]], row["attribute"]): (keys, None)
            for row in clip.get(field, []) if row["classID"] in classes
            if (keys := row["curve"]["m_Curve"]) and all(type(key.get("value")) in (float, int) for key in keys)}


def curve_member(key, field, axis):
    value = key.get(field)
    return value.get(axis) if axis is not None and isinstance(value, dict) else value


def floats(values, replay):
    if isinstance(values, dict):
        replay.require(values.get("t") == "f32s" and len(values["v"]) % 8 == 0, "incomplete built float array")
        return [replay.f32_from_bits(values["v"][i:i + 8]) for i in range(0, len(values["v"]), 8)]
    return values


def compiled_curves(clip, replay):
    bindings = clip.get("m_ClipBindingConstant", {}).get("genericBindings", [])
    if not bindings:
        return {}
    playback = clip.get("m_MuscleClip", {}).get("m_Clip", {})
    playback = playback.get("data", playback)
    stream = playback.get("m_StreamedClip", {})
    dense = playback.get("m_DenseClip", {})
    stream_count, dense_count = stream.get("curveCount", 0), dense.get("m_CurveCount", 0)
    constants = floats(playback.get("m_ConstantClip", {}).get("data", []), replay)
    slot_count = stream_count + dense_count + len(constants)
    curves, slots = {}, []
    widths = {attribute: len(axes) for _, attribute, axes in TRANSFORM_CURVES.values()}
    for row in bindings:
        path, class_id, attribute, custom = row["path"], row.get("typeID", row.get("classID")), row["attribute"], row["customType"]
        width = widths.get(attribute, 1) if class_id == 4 else 1
        span = tuple(range(len(slots), len(slots) + width))
        slots.extend(span)
        curves[path, class_id, attribute, custom, bool(row.get("isPPtrCurve"))] = span
    if len(slots) != slot_count:
        return {}

    def f32(word):
        return struct.unpack(">f", struct.pack(">I", word))[0]

    tracks = [[] for _ in range(stream_count)]
    seeds = {}
    words = stream.get("data", [])
    cursor = 0
    while cursor < len(words):
        bits, count = words[cursor:cursor + 2]
        stop = cursor + 2 + 5 * count
        if cursor == 0 and bits == 0xff7fffff:
            seeds = {words[i]: (f32(bits), tuple(map(f32, words[i+1:i+5]))) for i in range(cursor + 2, stop, 5)}
        elif bits != 0x7f800000:
            for i in range(cursor + 2, stop, 5):
                slot, *coefficients = words[i:i+5]
                tracks[slot].append((f32(bits), tuple(map(f32, coefficients))))
        cursor = stop
    data = floats(dense.get("m_SampleArray", []), replay)
    dense_tracks = [[(dense["m_BeginTime"] + i / dense["m_SampleRate"], (0, 0, 0, p))
                     for i, p in enumerate(data[slot::dense_count])] for slot in range(dense_count)]
    tracks += dense_tracks + [[(0, (0, 0, 0, value))] for value in constants]
    return {key: [(slot < stream_count, [seeds[slot], *tracks[slot]] if key[-1] and slot in seeds else tracks[slot])
                  for slot in span] for key, span in curves.items()}


def check_compiled_quaternion(tracks, timing, authored_times):
    if not all(keys and all(type(value) in (int, float) and isfinite(value)
                            for _, coefficients in keys for value in coefficients)
               for _, keys in tracks):
        return False
    playback = timing["m_Clip"]
    grid = playback.get("data", playback).get("m_DenseClip", {})

    def sampler(track):
        is_streamed, keys = track
        if is_streamed:
            return lambda time: streamed(keys, time)
        values = [coefficients[3] for _, coefficients in keys]
        if len(values) == 1:
            return lambda _: values[0]
        return lambda time: dense(values, 1, 0, grid["m_BeginTime"], grid["m_SampleRate"], time)

    # Unity normalizes interpolated rotations during playback.
    samplers = [sampler(track) for track in tracks]
    times = authored_times | {time for _, keys in tracks for time, _ in keys}
    for time in times:
        values = [sample(time) for sample in samplers]
        if time in authored_times:
            if not unit_quaternion(values):
                return False
        elif not any(values) or not all(map(isfinite, values)):
            return False
    return True


def mesh_reference(view, clip_key, pointer):
    try:
        return view.pointer(clip_key, pointer, {43})
    except (KeyError, TypeError, ValueError):
        return None


def matches_reference_time(time, rate):
    def f32(value):
        return struct.unpack(">f", struct.pack(">f", value))[0]

    # Compare the decoded grid and the inclusive half-frame window as f32.
    if not isfinite(rate) or rate <= 0:
        return False
    half_interval = .5 / rate
    return f32(.5 - half_interval) <= f32(time) <= f32(.5 + half_interval)


def explicit_references_match(view, clip_key, clip, expected):
    path, _, prop = REFERENCE
    rows = [row["curve"] for row in clip.get("m_PPtrCurves", [])
            if (row["path"], row["classID"], row["attribute"]) == (path, 33, prop)]
    if len(rows) != 1 or len(rows[0]) != 2:
        return False
    first, second = rows[0]
    return (first["time"] == 0 < second["time"]
            and matches_reference_time(second["time"], clip["m_SampleRate"])
            and [mesh_reference(view, clip_key, point["value"]) for point in rows[0]] == expected)


def compiled_references_match(view, clip_key, clip, curves, expected):
    path, _, prop = REFERENCE
    tracks = [track for (path_hash, class_id, attribute, _, pptr), track in curves.items()
              if (path_hash, class_id, attribute, pptr) == (crc32(path.encode()), 33, crc32(prop.encode()), True)]
    if len(tracks) != 1 or len(tracks[0]) != 1:
        return False
    is_streamed, keys = tracks[0][0]
    mapping = clip["m_ClipBindingConstant"].get("pptrCurveMapping", [])
    changes = []
    for time, coefficients in keys:
        index = coefficients[3]
        if type(index) not in (int, float) or not isfinite(index) or not 0 <= index < len(mapping) or index != int(index):
            return False
        target = mesh_reference(view, clip_key, mapping[int(index)])
        if not changes or target != changes[-1][1]:
            changes.append((time, target))
    if len(changes) != 2 or changes[0][0] > 0 or [target for _, target in changes] != expected:
        return False
    playback = clip["m_MuscleClip"]["m_Clip"]
    rate = clip["m_SampleRate"] if is_streamed else playback.get("data", playback)["m_DenseClip"]["m_SampleRate"]
    return matches_reference_time(changes[1][0], rate)


def check_compiled_weight(track, scale):
    if track is None:
        return False
    is_streamed, keys = track
    stem = (0, 1, 1, .5, 3, -2, .25, .375) if scale else (0, 1, 1, 6, 5, -2, .25, .375)
    if is_streamed:
        samples = [(t, streamed(keys, t)) for t in (.25, .5, .75)] if len(keys) > 1 else []
    else:
        samples = [(t, co[3]) for t, co in keys if .125 <= t <= .875]
    return len(samples) >= 2 and all(abs(value - weighted(*stem, t)) < .002 for t, value in samples)


def check_built_clip(name, fixture, view, clip_key, nodes, avatar, editor_bindings):
    """Qualify required content from the stored clip and its actual targets."""
    clip = view.value(clip_key)
    runtime = built_transform_curves(clip) | built_float_curves(clip, "m_FloatCurves")
    authored = built_float_curves(clip, "m_EditorCurves")
    by_hash = compiled_curves(clip, view.replay) if fixture.playback != "legacy" else {}
    for row in clip.get("m_RotationCurves", []):
        view.require(all(unit_quaternion([point["value"].get(axis) for axis in "xyzw"])
                         for point in row["curve"]["m_Curve"]), "invalid built quaternion key")
    compiled = {}
    for path, kind, prop in fixture.bindings:
        key = path, kind, prop
        if key == REFERENCE:
            expected = [view.asset(asset, 43, ignore_case=True) for asset in REFERENCE_ASSETS]
            view.require(explicit_references_match(view, clip_key, clip, expected)
                         or compiled_references_match(view, clip_key, clip, by_hash, expected),
                         "invalid built MeshFilter.m_Mesh PPtr transition")
            reference = [keys for _, node_name, keys in nodes.values() if node_name == path]
            filters = [key for keys in reference for key in keys if view.headers[key]["class_id"] == 33]
            view.require(len(filters) == 1 and view.pointer(filters[0], view.value(filters[0])["m_Mesh"], {43})
                         == expected[0], "built object-reference curve has no matching MeshFilter target")
            continue
        track = []
        if kind == "Transform":
            family, _, axis = prop.rpartition(".")
            _, attribute, axes = TRANSFORM_CURVES[family]
            family_tracks = by_hash.get((crc32(path.encode()), 4, attribute, 0, False))
            if family_tracks:
                if attribute == 2 and axis == "x":
                    properties = {family + "." + channel for channel in axes}
                    authored_times = {view.replay.f32_from_bits(key["time_f32"])
                                      for binding in editor_bindings
                                      if (binding["path"], binding["type"]) == (path, "UnityEngine.Transform")
                                      and binding["property"] in properties for key in binding["keys"]}
                    view.require(check_compiled_quaternion(family_tracks, clip["m_MuscleClip"], authored_times),
                                 "invalid built quaternion in compiled slots")
                track = family_tracks[axes.index(axis):axes.index(axis) + 1]
        elif kind == "SkinnedMeshRenderer":
            track = by_hash.get((crc32(path.encode()), 137, crc32(prop.encode()), 20, False), [])
        elif kind == "Animator":
            track = by_hash.get((0, 95, HUMANOID_ATTRIBUTES[key], 8, False), [])
        if track and track[0][1] and all(isfinite(v) for _, co in track[0][1] for v in co):
            compiled[key] = track[0]
        view.require(key in runtime or key in compiled or (kind == "Animator" and key in authored),
                     f"missing built fixture curve for {key}")

    if fixture.playback == "legacy":
        view.require(all(key in built_transform_curves(clip) for key in ROTATION), "missing built legacy quaternion curves")
    if "constant" in fixture.checks:
        keys, axis = runtime.get(POSITION[0], ([], None))
        _, stored = compiled.get(POSITION[0], (False, []))
        view.require((stored and all(co == (0, 0, 0, 8) for _, co in stored))
                     or (keys and all(curve_member(key, "value", axis) == 8
                                      and curve_member(key, "inSlope", axis) == curve_member(key, "outSlope", axis) == 0
                                      for key in keys)),
                     "missing built fixture curve holding translation at 8")
    if "held" in fixture.checks:
        keys, axis = runtime.get(POSITION[0], ([], None))
        is_streamed, stored = compiled.get(POSITION[0], (False, []))
        held = is_streamed and stored[:2] == [(.5, (0, 0, 0, 4)), (.75, (0, 0, 0, 12))]
        view.require(held or ([key["time"] for key in keys] == [.5, .75, 1.5]
                     and [curve_member(key, "value", axis) for key in keys] == [4, 12, 12]
                     and all(curve_member(key, "outSlope", axis) == {"t": "f32", "v": "7f800000"} for key in keys[:-1])),
                     "missing built fixture curve for the stepped translation")
    if "weighted" in fixture.checks:
        scale = fixture.probe[1] == "scale"
        key = SCALE[1] if scale else POSITION[0]
        keys, axis = authored.get(key, runtime.get(key, ([], None)))
        view.require((keys and curve_member(keys[0], "weightedMode", axis) in (2, 3)
                      and curve_member(keys[-1], "weightedMode", axis) in (1, 3)
                      and curve_member(keys[0], "outWeight", axis) == .25
                      and curve_member(keys[-1], "inWeight", axis) == .375)
                     or check_compiled_weight(compiled.get(key), scale),
                     "missing built fixture curve with weighted keys")
    if fixture.probe[1] == "weight":
        renderers = [key for _, label, keys in nodes.values() for key in keys
                     if label in ("RendererA", "RendererB") and view.headers[key]["class_id"] == 137]
        check_skin(view, renderers, "built morph", expected_mesh=None)
    if avatar is not None:
        muscle = clip.get("m_MuscleClip", {})
        view.require(muscle.get("m_IndexArray") and any(slot >= 0 for slot in muscle["m_IndexArray"]),
                     "missing built fixture curve slots for humanoid playback")
        view.require(view.value(avatar).get("m_Avatar", {}).get("m_Human"), "missing built humanoid Avatar")
        has_body = any(key in runtime or key in authored or (0, 95, HUMANOID_ATTRIBUTES[key], 8, False) in by_hash
                       for key in BODY)
        view.require(has_body == ("muscle_only" not in fixture.checks), "built humanoid body motion mismatch")
        if name.removesuffix(VECTOR_SUFFIX).endswith("-offset"):
            transform = view.value(nodes["0"][0])
            view.require(transform.get("m_LocalPosition") == {"x": 2, "y": .25, "z": -1}
                         and transform.get("m_LocalRotation", {}).get("y", 0) != 0,
                         "missing built humanoid rig offset")


def prefab_components(view, root):
    return {label: {view.headers[key]["class_id"]: key for key in components}
            for _, label, components in view.hierarchy(root).values()}


def mesh_parts(view, node):
    view.require({23, 33} <= node.keys(), "missing built MeshFilter or MeshRenderer")
    return node[33], view.value(node[33]), view.value(node[23])


def skin_vertex_influences(vertex):
    """Read BlendWeight and BlendIndices from the little-endian Windows mesh."""
    channels = vertex.get("m_Channels", [])
    data = vertex.get("m_DataSize", {})
    if len(channels) < 14 or not isinstance(data, dict) or data.get("t") not in ("bytes", "u8s") or "hex" not in data:
        return
    blob = bytes.fromhex(data["hex"])
    weights, indices = channels[12:14]
    if any(channel["dimension"] & 15 != 4 for channel in (weights, indices)):
        return
    # Unity 2019+ vertex format IDs, including the channels that precede skinning.
    formats = "feBbHhBbHhIi"
    scales = {0: 1, 1: 1, 2: 255, 4: 65535}
    if weights["format"] not in scales or indices["format"] < 6:
        return
    readers = [struct.Struct("<4" + formats[channel["format"]]) for channel in (weights, indices)]
    count = vertex["m_VertexCount"]
    strides = [0] * (max(channel["stream"] for channel in channels) + 1)
    for channel in channels:
        dimension = channel["dimension"] & 15
        if dimension:
            strides[channel["stream"]] += dimension * struct.calcsize("<" + formats[channel["format"]])
    starts, offset = [], 0
    for stride in strides:
        starts.append(offset)
        offset = (offset + count * stride + 15) & ~15
    for index in range(count):
        row = []
        for channel, reader in zip((weights, indices), readers):
            stream = channel["stream"]
            start = starts[stream] + index * strides[stream] + channel["offset"]
            if channel["offset"] + reader.size > strides[stream] or start + reader.size > len(blob):
                return
            row.append(reader.unpack_from(blob, start))
        yield tuple(value / scales[weights["format"]] for value in row[0]), row[1]


def check_skin(view, renderers, label, expected_mesh):
    view.require(len(renderers) == 2, f"{label} renderer count")
    meshes, bone_sets = set(), set()
    for key in renderers:
        value = view.value(key)
        mesh = view.pointer(key, value["m_Mesh"], {43})
        view.require(mesh is not None, f"{label} missing skin mesh")
        meshes.add(mesh)
        bones = tuple(view.pointer(key, ptr, {4}) for ptr in value.get("m_Bones", []))
        view.require(len(bones) == len(set(bones)) == 4 and None not in bones
                     and view.pointer(key, value["m_RootBone"], {4}) == bones[0],
                     f"{label} bone count or root bone")
        bone_sets.add(bones)
    view.require(len(meshes) == len(bone_sets) == 1 and (expected_mesh is None or meshes == {expected_mesh}),
                 f"{label} shared mesh or bones mismatch")
    mesh = view.value(next(iter(meshes)), full_bytes=True)
    shapes = mesh.get("m_Shapes", {})
    weights = floats(shapes.get("fullWeights", []), view.replay)
    frames = shapes.get("shapes", [])
    channels = shapes.get("channels", [])
    view.require(weights == [25, 80] and len(frames) == 2 and len(channels) == 1
                 and (channels[0]["name"], channels[0]["frameIndex"], channels[0]["frameCount"]) == ("Lift", 0, 2)
                 and all(0 <= row["firstVertex"] < row["firstVertex"] + row["vertexCount"]
                         <= len(shapes.get("vertices", [])) for row in frames), "missing built Lift blend-shape frames")
    bind_count = len(mesh.get("m_BindPose", []))
    view.require(bind_count == 4, f"{label} bind pose count")
    vertex = mesh.get("m_VertexData", {})
    influences = list(skin_vertex_influences(vertex))
    view.require(len(influences) == vertex.get("m_VertexCount") and influences and
                 all(set(indices) == set(range(bind_count)) and all(isfinite(w) and w > 0 for w in weights)
                     and abs(sum(weights) - 1) <= .01 for weights, indices in influences),
                 f"{label} per-vertex bone weights")


def entries(value):
    return value if isinstance(value, dict) else dict(value)


def check_built_static(case, view, assets):
    built = {asset: view.asset(ASSETS + "static/" + asset, class_id, ignore_case=True)
             for asset, class_id in assets.items()}
    if case == "static-batch-scene":
        check_batch_scene(view)
        return
    root = next(key for asset, key in built.items() if asset.endswith(".prefab"))
    nodes = prefab_components(view, root)
    if case == "static-primitives":
        names = {"Cube", "Cylinder", "Sphere", "Capsule", "Plane", "Quad"}
        view.require(names <= nodes.keys(), "missing built primitive nodes")
        parts = [mesh_parts(view, nodes[name]) for name in names]
        view.require(len({tuple(sorted(mesh["m_Mesh"].items())) for _, mesh, _ in parts}) == len(names)
                     and all(mesh["m_Mesh"]["m_PathID"] and any(p["m_PathID"] for p in renderer["m_Materials"])
                             for _, mesh, renderer in parts), "missing distinct built primitive meshes or materials")
    elif case == "static-materials":
        texture = built["checker.asset"]
        image = view.value(texture)
        view.require((image["m_Width"], image["m_Height"]) == (2, 2), "built checker texture size mismatch")
        shaders = set()
        for name in ("Metallic", "Unlit"):
            material = built[name.lower() + ".mat"]
            renderer = nodes[name][23]
            value = view.value(material)
            view.require([view.pointer(renderer, ptr, {21}) for ptr in view.value(renderer)["m_Materials"]] == [material],
                         "built renderer material mismatch")
            props = value["m_SavedProperties"]
            main = entries(props["m_TexEnvs"])["_MainTex"]
            view.require(view.pointer(material, main["m_Texture"], {28}) == texture and value["m_Shader"]["m_PathID"],
                         "missing built material shader or checker reference")
            shaders.add(tuple(sorted(value["m_Shader"].items())))
            if name == "Metallic":
                floats = entries(props["m_Floats"])
                view.require((floats.get("_Metallic"), floats.get("_Glossiness")) == (.625, .375)
                             and main["m_Scale"] == {"x": 2, "y": 3} and main["m_Offset"] == {"x": .25, "y": .5}
                             and "_EmissionColor" in entries(props["m_Colors"]), "built metallic material properties mismatch")
        view.require(len(shaders) == 2, "built fixture materials share one shader")
    elif case == "static-lod":
        # Unity names a saved prefab's root after the asset file, so find
        # the group on the root node rather than by its authored name.
        group = {view.headers[key]["class_id"]: key for key in view.hierarchy(root)[""][2]}.get(205)
        lods = view.value(group).get("m_LODs", []) if group is not None else []
        view.require(len(lods) == 2, "built LOD count must be two")
        for lod, name, height in zip(lods, ("Near", "Far"), (.6, .2)):
            view.require(view.replay.f32_bits(lod["screenRelativeHeight"]) == view.replay.f32_bits(height)
                         and [view.pointer(group, row["renderer"], {23}) for row in lod["renderers"]] == [nodes[name][23]],
                         "built LOD renderer or threshold mismatch")
    elif case == "static-skin":
        check_skin(view, [nodes[name][137] for name in ("RendererA", "RendererB")], "built skin", built["four-influence.asset"])


def check_batch_scene(view):
    meshes, ranges, scene_files = set(), set(), set()
    for name in ("Alpha", "Zulu"):
        objects = [key for key, row in view.headers.items() if row["class_id"] == 1 and row["name"] == name]
        view.require(len(objects) == 1, f"missing built static batch object {name}")
        obj = objects[0]
        scene_files.add(obj[0])
        components = {view.headers[key]["class_id"]: key for key in view.components(obj)}
        owner, mesh, renderer = mesh_parts(view, components)
        view.require(view.value(components[4])["m_Father"]["m_PathID"] == 0, "built static batch object is not a scene root")
        ptr = mesh.get("m_Mesh")
        view.require(ptr is not None and ptr["m_PathID"], "missing built static batch shared mesh")
        meshes.add(view.pointer(owner, ptr, {43}, within_bundle=True))
        batch = renderer.get("m_StaticBatchInfo", {})
        ranges.add((batch.get("firstSubMesh"), batch.get("subMeshCount")))
    view.require(len(scene_files) == len(meshes) == 1, "built static batch shared mesh mismatch")
    view.require(ranges == {(0, 1), (1, 1)}, "built static batch submesh ranges mismatch")
    mesh = view.value(next(iter(meshes)))
    view.require(len(mesh.get("m_SubMeshes", [])) == 2 and all(row["indexCount"] == 6 for row in mesh["m_SubMeshes"]),
                 "built static batch submesh ranges exceed the combined mesh")


def check_identity(name, fixture, spec, capture, replay):
    require = replay.require
    prefix = ASSETS + fixture.folder + "/"
    editor = spec["evidence"]["editor"]
    human = bool(fixture.avatar)
    overrides = {"override", "null_override"}
    outcome = fixture.playback if fixture.playback in overrides else ""
    mode = "animator" if outcome else fixture.playback
    require(editor.get("fixture_case") == name, f"pending recording does not identify manifest case {name}")
    require(spec["bundles"] == [{"name": fixture.bundle}]
            and [spec[key].lower() for key in ("prefab_asset", "clip_asset", "avatar_asset")]
            == [prefix + fixture.prefab + ".prefab", prefix + fixture.clip + ".anim", fixture.avatar],
            f"fixture identity mismatch for {name}")
    require(spec["mode"] == mode and capture["playback"]["api"] == replay.API[mode]
            and editor.get("expected_discovery", "") == outcome
            and spec["binding_address"] == ("0" if human else "") and spec["binding_component"] == 0,
            f"fixture playback mode mismatch for {name}")
    require(capture["playback"]["clip_human_motion"] is human and ("humanoid" in capture) == human
            and editor.get("animation_source") == ("procedural_humanoid" if human else "procedural_clip"),
            f"fixture animation source mismatch for {name}")


def check_discovery(fixture, view, discovery, selected):
    """The selected clip must use the expected controller route and override source."""
    outcome = fixture.playback
    rows = [row for row in discovery if row["field"] == "m_OverrideClip"]
    if outcome == "override":
        original = view.asset(ASSETS + "generic/generic-morph.anim", 74, ignore_case=True)
    elif outcome == "null_override":
        original = selected
    else:
        view.require(not rows, "unexpected built override controller for fixture")
        return
    view.require(any(row["outcome"] == outcome and row["resolved"] == view.identity(selected)
                     and row["original"] == view.identity(original) for row in rows),
                 f"missing expected built discovery outcome: {outcome}")


def check_bindings(fixture, editor, replay):
    require = replay.require
    rows = {}
    for binding in editor["bindings"]:
        kind = binding["type"].removeprefix("UnityEngine.")
        key = binding["path"], kind, binding["property"]
        require(key not in rows, f"duplicate Editor fixture binding: {key}")
        rows[key] = binding
    for key in fixture.bindings:
        require(key in rows, f"missing fixture binding: {key}")
        row = rows[key]
        require(row["object_reference"] is (key == REFERENCE) and row["discrete"] is False,
                f"fixture binding flags mismatch: {key}")
        if key != REFERENCE:
            require(row["keys"], f"missing fixture curve keys: {key}")
    if "muscle_only" in fixture.checks:
        require(rows.keys().isdisjoint(BODY), "muscle-only fixture has authored body motion")
    if "constant" in fixture.checks:
        require(all(replay.f32_from_bits(key["value_f32"]) == 8 for key in rows[POSITION[0]]["keys"]),
                "fixture constant curve must hold at 8")
    if "held" in fixture.checks:
        keys = rows[POSITION[0]]["keys"]
        require([key["time_f32"] for key in keys] == [replay.f32_bits(t) for t in (.5, .75, 1.5)]
                and all(key["out_tangent_f32"] == "7f800000" for key in keys[:-1]),
                "missing fixture stepped curve evidence")
    if "weighted" in fixture.checks:
        keys = rows[SCALE[1] if fixture.probe[1] == "scale" else POSITION[0]]["keys"]
        require(keys[0]["weighted_mode"] in ("Out", "Both") and keys[-1]["weighted_mode"] in ("In", "Both"),
                "missing fixture weighted curve evidence")
    return rows


def check_references(editor, row, samples, view, replay):
    names = [view.headers[view.asset(key, 43, ignore_case=True)]["name"] for key in REFERENCE_ASSETS]

    def check(observed, times):
        expected = [(replay.f32_bits(t), REFERENCE_ASSETS[t >= .5], names[t >= .5]) for t in times]
        actual = [(item["time_f32"], item["asset"].lower(), item["name"]) for item in observed]
        replay.require(actual == expected, "fixture object-reference evidence mismatch")

    check(row["object_keys"], (0, .5))
    check(editor["object_reference_samples"], [replay.f32_from_bits(s["local_f32"]) for s in samples])


def check_spine(editor, capture, target, replay):
    require = replay.require
    require([row["node"] for row in capture["humanoid"]["bones"] if row["name"] == "Spine"] == [target],
            "fixture Spine probe does not match the captured bone mapping")
    samples = capture["samples"]
    mid = [s for s in samples if s["local"] == .5]
    require(len(mid) == 1 and editor["spine_endpoint_tolerance_degrees"] == .25,
            "missing fixture Spine sample or endpoint tolerance")

    def rotation(sample):
        return tuple(map(replay.f32_from_bits, sample["pose"]["transforms"][target]["rotation"]))

    neutral, single, combined = map(rotation, (samples[0], mid[0], samples[-1]))
    expected = (editor["spine_start_f32"], editor["spine_stop_f32"])
    require(all(replay.angle(actual, tuple(map(replay.f32_from_bits, reference))) <= radians(.25)
                for actual, reference in zip((neutral, combined), expected)),
            "fixture Spine endpoint evidence mismatch")
    require(replay.angle(neutral, single) > radians(1) and replay.angle(single, combined) > radians(1),
            "fixture Spine samples do not show both authored movements")


def check_evidence(fixture, doc, view, replay):
    spec, capture = doc["spec"], doc["capture"]
    editor = spec["evidence"]["editor"]
    fields = {"references": ("object_reference_samples",),
              "spine": ("spine_start_f32", "spine_stop_f32", "spine_endpoint_tolerance_degrees")}
    require = replay.require
    require(all(editor.get(field) for check in fixture.checks for field in fields.get(check, ())),
            "missing fixture evidence")
    rows = check_bindings(fixture, editor, replay)
    held = "held" in fixture.checks
    require((spec["start"], spec["stop"], spec["held_boundaries"]) == ((.5, 1.5, [.75]) if held else (0, 1, [])),
            "fixture playback interval or held boundary mismatch")
    probe_path, prop, first, last = fixture.probe
    nodes = {node["address"]: node["name"] for node in spec["nodes"]}
    addresses = {"": ""}
    for address in sorted(nodes, key=lambda key: key.count("/")):
        if address:
            parent = addresses[address.rpartition("/")[0]]
            addresses[address] = (parent + "/" if parent else "") + nodes[address]
    clock = spec["evidence"]["clock"]
    probes = [clock[side]["probe"] for side in ("start", "stop")]
    values = [first, last]
    if held:
        probes += [clock["held"][0][side] for side in ("left", "right")]
        values += [first, last]
    for probe, value in zip(probes, values):
        targets = spec["renderers"] if prop == "weight" else spec["nodes"]
        require(probe["property"] == prop and probe["component"] == 0
                and probe["channel"] == (0 if prop == "weight" else -1)
                and addresses[targets[probe["target"]]["address"]] == probe_path
                and (value is None or replay.f32_from_bits(probe["value"]) == value),
                "fixture playback probe mismatch")
    if "references" in fixture.checks:
        check_references(editor, rows[REFERENCE], capture["samples"], view, replay)
    if "spine" in fixture.checks:
        check_spine(editor, capture, probes[0]["target"], replay)


def check_vectors(doc, paired, replay):
    spec, capture = doc["spec"], doc["capture"]
    clip, clip_capture = paired["spec"], paired["capture"]
    for schedule in (spec["schedule"], clip["schedule"], capture["samples"], clip_capture["samples"]):
        replay.require(tuple(point["local"] for point in schedule) == VECTOR_TIMES, "known-vector schedule mismatch")
    replay.require(all(spec[key] == clip[key] for key in
                       ("bundles", "prefab", "clip", "avatar", "binding", "nodes", "renderers",
                        "start", "stop", "sample_rate", "source_clock"))
                   and all(capture[key] == clip_capture[key] for key in
                           ("editor_version", "build_target", "humanoid", "rest", "clip_settings"))
                   and [sample["local_f32"] for sample in capture["samples"]]
                   == [sample["local_f32"] for sample in clip_capture["samples"]],
                   "known-vector recording disagrees with its clip replay")
    muscles = [row["name"] for row in capture["humanoid"]["muscles"]]
    replay.require(len(muscles) == replay.MUSCLE_COUNT and len(set(muscles)) == len(muscles)
                   and set(SPINE) <= set(muscles), "known-vector muscle mapping mismatch")
    indices = [muscles.index(name) for name in SPINE]
    for point in spec["schedule"]:
        t = point["local"]
        alpha = 2 * t - 1
        vector = (t, 0, 0) if t <= .5 else (.5 - .75 * alpha, .375 * alpha, -.5 * alpha)
        expected = [0] * len(muscles)
        for i, value in zip(indices, vector):
            expected[i] = value
        replay.require([replay.f32_bits(x) for x in point["muscles"]] == [replay.f32_bits(x) for x in expected],
                       "known-vector inputs mismatch")
