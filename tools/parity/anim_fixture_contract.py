"""Required case identities and evidence for the Windows Editor fixture run."""

from dataclasses import dataclass
from math import radians

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
