"""Round 11's body fixtures (docs/plans/humanoid-body.md, B5) and the completion of a round 11 run.

Round 11 adds to the main run's body cases, mirroring UnityrsBodyClips.cs,
UnityrsBodyFixtures.cs and UnityrsBodyMuscles.cs:

- **E5's clip rigs** (`CLIP_RIGS`), each a bundle `body-clips-<rig>`: the
  variants B4 admits (freeze v8's E4 passing set) with their start-offset
  twins, 21 rows, on an identity and an offset root. The rigs are F5's large
  rig, the tall rig and a plain small rig (Size 0.82), at the top, middle
  and bottom of B4's human-scale range, then every training topology B3's
  topology scope admits but the compact rig's (whose clips `body-clips`
  records). No topology B3 refuses, never a hold-out rig.
- **E5's chain rigs** (`CHAIN_RIGS`): each F5 feature at a second human
  scale, with its targets and the all-Original motion clip. Size scales
  the bone offsets and the hips' own offset; the feature stays at F5's exact
  value, the armature's transform included, as B4 admits it.
- **E5's full-body clips** (anim_fixture_full): a distinct curve on every
  muscle under its Animator property (but a topology's expected-absent
  ones), on rigs mapping every bone a muscle turns, so a recording carries
  each bone's channels through a real clip: refusal evidence over the frozen
  lateral limit, and in-scope clips over B4's whole admitted matrix.
- **B5a's muscle sweeps** (anim_fixture_muscles): every one of the 95
  muscles alone at both signs out to its limits, then each bone's muscles
  jointly; each single-muscle sample must turn the muscle's bone.

Freeze v8 pins the sources of B2's checker, `complete` among them, so this
module extends their tables only while it runs (`round11_tables`), and no
pinned module imports it. A round 11 run declares its generation in
`round.json` beside `run.json`; `complete` here requires that declaration,
the whole body inventory (every earlier body case and every round 11 case
listed and recorded), and on every case B4 reads (E5's, F5's chain clips
and F4's compact clips, `B4_CASES`) B4's inputs as chain.rs reads them: the
chain exactly as B4 admits it (on the recorded rest pose and on the bundle's
serialized Transforms, the Animator node's placement, its ancestor and the
unit scales below the hips), the serialized Avatar's human scale inside
B4's range, the Avatar skeleton's Transforms as `rest_scales` resolves them
through `m_TOS` (`avatar_skeleton`), and a clip's settings as `root_settings`
reads them (`clip_configuration`). It also checks each clip's declared scope
against the freeze's limits on its compiled values and recorded poses
(anim_fixture_scope), each full-body clip's every curve and each sweep's
response from its zero-vector sample.

Completion checks the collection only. B5a's confirmation (anim_muscle_pose
on both Editors' round 11 runs) and B5b's comparison of Rust's replay
against both Editors' recordings, typed refusals included, are separate
steps run on the completed runs afterwards (docs/plans/humanoid-body.md). The plain `anim_replay.py
complete` refuses a round 11 run (its cases are unknown there), and this
one refuses a run without the declaration. B2 never reads round 11's
cases: they are not in its tables.

    python3 -I tools/parity/anim_fixture_round11.py complete <folder> [--unityrs PATH | --inspect-dir DIR]
"""

import argparse
import math
import struct
import sys
from contextlib import contextmanager
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import anim_fixture_body as body  # noqa: E402
from anim_fixture_body import CHAIN_VARIANT, CLIP_VARIANTS, PROBE_GROUPS, SPINE_PROBE, _fixture  # noqa: E402
from anim_fixture_muscles import (FINGER_BONES, FINGERS_RIG, MUSCLE_SWEEPS, MUSCLE_VALUES, finger_rows,  # noqa: E402
                                  muscle_bone, muscle_vectors)
from anim_fixture_full import (FULL_EXTRAS, FULL_RIGS, SCOPED_RIGS, expected_absent, feature, full_groups,  # noqa: E402
                               muscle_property, placements, topology, variants)
from anim_fixture_full import size as full_size  # noqa: E402
from anim_fixture_scope import check_scope  # noqa: E402
from anim_reference import angle  # noqa: E402

ROUND = 11
# F5's chain rigs as (feature, size): freeze v8's four, then round 11's (CHAIN_RIGS).
CHAIN_FEATURES = {"scaled-animator": ("scaled-animator", 1), "large": ("", 1.5), "armature": ("armature", 1),
                  "hips-scale": ("hips-scale", 1), "scaled-animator-large": ("scaled-animator", 1.5),
                  "scaled-animator-small": ("scaled-animator", .82), "hips-scale-large": ("hips-scale", 1.2),
                  "hips-scale-small": ("hips-scale", .7), "armature-large": ("armature", 1.8)}
CHAIN_RIGS = tuple(CHAIN_FEATURES)[4:]
E5_TOPOLOGIES = ("no-upper-chest", "no-shoulders", "no-left-toes", "no-right-toes", "no-toes", "no-upper-chest-toes",
                 "no-upper-chest-left-toes", "no-upper-chest-right-toes", "eyes", "jaw", "eyes-jaw", "noncoplanar")
CLIP_RIGS = ("large", "tall", "small", *E5_TOPOLOGIES)
RIG_SIZES = {"small": .82} | {rig: size for rig, (_, size) in CHAIN_FEATURES.items() if size != 1}
# B4's human-scale range, as f32 bits: the armature rig's and the large rig's recorded humanScale.
H_RANGE = tuple(struct.unpack(">f", bytes.fromhex(bits))[0] for bits in ("3f47f9bb", "3fb7a311"))
# F5's armature exactly, as B4's BETWEEN admits it: translation, Quaternion.Euler(-90, 0, 0) as Unity records it
# (0x3f3504f4), scale.
HALF = struct.unpack(">f", bytes.fromhex("3f3504f4"))[0]
ARMATURE = ((0, .05, .1), (-HALF, 0, 0, HALF), .8)
# F5's Animator node and hips scales (chain.rs ANIMATOR_SCALE, HIPS_SCALE).
ANIMATOR_SCALE, HIPS_SCALE = 1.25, 1.2
# The Animator node's offset placement (chain.rs OFFSET_PLACEMENT), as f32 bits.
OFFSET_PLACEMENT = ((2, .25, -1), tuple(struct.unpack(">f", bytes.fromhex(bits))[0]
                                       for bits in ("00000000", "3e99f61c", "00000000", "3f7426cb")))
# Every round 11 rig's (feature, size): the chain rigs', the full-body clip rigs', a sized clip rig's.
FULL_CLIP_RIGS = (*FULL_RIGS, *SCOPED_RIGS)
RIG_FEATURES = {"small": ("", RIG_SIZES["small"])} | CHAIN_FEATURES | {rig: (feature(rig), full_size(rig))
                                                                      for rig in FULL_CLIP_RIGS}
# The variants B4 admits, freeze v8's E4 passing set; with their start-offset twins, 21 rows.
E5_ADMITTED = ("bake-o0-y0-xz1", "bake-o0-y1-xz0", "bake-o0-y1-xz1", "bake-o1-y0-xz0", "bake-o1-y0-xz1",
               "bake-o1-y1-xz0", "bake-o1-y1-xz1", "keep-o0-y0-xz0", "keep-o1-y1-xz1-motion", "level",
               "orientation-offset")
E5_VARIANTS = tuple(name for name in CLIP_VARIANTS if name.removesuffix("-start") in E5_ADMITTED)


def _cases():
    cases, info = {}, {}

    def add(name, fixture, kind, rig, variant="", root="identity"):
        cases[name], info[name] = fixture, dict(kind=kind, rig=rig, variant=variant, root=root)
        # A clip's declared scope (anim_fixture_scope): only the in-scope full-body clips play inside it.
        if kind == "clip":
            info[name]["scope"] = "in" if rig in SCOPED_RIGS else "refusal"

    # In the builder's order: the chain rigs, the clip rigs, the sweeps.
    for rig in CHAIN_RIGS:
        bundle = "body-chain-" + rig
        probe = "HumanoidRig/Armature/Hips/Spine" if CHAIN_FEATURES[rig][0] == "armature" else SPINE_PROBE
        add(bundle + "-targets", _fixture(bundle, "identity", bundle + "-muscles", "targets", probe), "targets", rig)
        add(bundle + "-clip", _fixture(bundle, "identity", bundle + "-clip", "clip", probe), "clip", rig, CHAIN_VARIANT)
    for rig in CLIP_RIGS:
        for variant in E5_VARIANTS:
            for prefab, suffix in (("identity", ""), ("offset", "-offset")):
                clip = f"body-clip-{rig}-{variant}"
                add(clip + suffix, _fixture("body-clips-" + rig, prefab, clip, "clip"), "clip", rig, variant, prefab)
    for rig in FULL_CLIP_RIGS:
        probe = "HumanoidRig/Armature/Hips/Spine" if feature(rig) == "armature" else SPINE_PROBE
        for variant in variants(rig):
            for prefab, suffix in placements(rig):
                clip = f"body-clip-{rig}-{variant}"
                add(clip + suffix, _fixture("body-clips-" + rig, prefab, clip, "clip", probe), "clip", rig, variant,
                    prefab)
    for name, rig in MUSCLE_SWEEPS.items():
        bundle = name.replace("probes-", "")
        add(name, _fixture(bundle, "identity", bundle + "-muscles", "probes"), "probes", rig, "muscle")
    return cases, info


ROUND11_CASES, ROUND11_INFO = _cases()
ROUND11_BUNDLES = frozenset(fixture.bundle for fixture in ROUND11_CASES.values())
MUSCLE_CASES = frozenset(MUSCLE_SWEEPS)
E5_CASES = frozenset(ROUND11_CASES) - MUSCLE_CASES
_BASE = {name: getattr(body, name) for name in ("rig_bones", "check_chain", "probe_schedule", "schedule_name",
                                                "check_body", "check_clip")}


def rig_bones(rig):
    """The rig's mapped human bones: the fingers rig's are the compact rig's and every finger bone, a full-body
    clip rig's its topology's, the face and every finger."""
    if rig in FULL_CLIP_RIGS:
        return _BASE["rig_bones"](topology(rig)) | set(FULL_EXTRAS)
    return _BASE["rig_bones"]("compact") | set(FINGER_BONES) if rig == FINGERS_RIG else _BASE["rig_bones"](rig)


def rig_layout(rig):
    """The builder's T-pose of a round 11 rig (anim_fixture_rigs.rig_layout): a sized rig's offsets scaled, the
    fingers rig's fingers below its hands."""
    from anim_fixture_rigs import rig_layout as base

    if rig == FINGERS_RIG:
        return base("compact") + finger_rows()
    if rig in FULL_CLIP_RIGS:
        # The topology's rows with the face after the Head, as RigNodes creates them, then the fingers.
        rows = base(topology(rig))
        have = {name for name, _, _ in rows}
        face = [row for row in base("eyes-jaw") if row[0] not in have and row[1] == "Head"]
        at = [name for name, _, _ in rows].index("Head") + 1
        rows = rows[:at] + face + rows[at:] + finger_rows()
        return [(name, parent, tuple(full_size(rig) * v for v in offset)) for name, parent, offset in rows]
    if rig in RIG_SIZES and rig != "large":
        size = RIG_SIZES[rig]
        return [(name, parent, tuple(size * v for v in offset)) for name, parent, offset in base("compact")]
    return base(rig)


def f32_hex(value):
    return struct.pack(">f", value).hex()


def same(values, want):
    """B4's comparison (chain.rs `same`): each stored component (f32 bits) exactly the f32 value, either zero's
    sign aside."""
    return len(values) == len(want) and all(bits == want_bits or {bits, want_bits} <= {"00000000", "80000000"}
                                            for bits, want_bits in zip(values, map(f32_hex, want)))


def check_chain(rig, spec, capture, replay):
    """A round 11 rig's chain in the recorded rest pose: its F5 feature exactly as B4 admits it (the stored
    components bit for bit, B4's rule: the Animator node's and the hips' scale, the armature's translation,
    rotation and scale, `-q` refused) and its hips offset at its size; any other rig's as before."""
    if rig not in RIG_FEATURES or rig in body.CHAIN_RIGS:
        return _BASE["check_chain"](rig, spec, capture, replay)
    feature, size = RIG_FEATURES[rig]
    nodes = {node["address"]: (i, node["name"]) for i, node in enumerate(spec["nodes"])}
    rest = capture["rest"]["transforms"]

    def stored(address, key, width):
        body._f32s(rest[nodes[address][0]][key], replay, width)
        return rest[nodes[address][0]][key]

    def near(values, want, bound=1e-5):
        values = [replay.f32_from_bits(v) for v in values]
        return len(values) == len(want) and all(abs(v - w) <= bound for v, w in zip(values, want))

    hips = [address for address, (_, label) in nodes.items() if label == "Hips"]
    replay.require(len(hips) == 1, "body rig has no single Hips node")
    hips = hips[0]
    hips_scale = HIPS_SCALE if feature == "hips-scale" else 1
    if feature == "armature":
        translation, rotation, scale = ARMATURE
        ok = (nodes.get("0/0", (0, ""))[1] == "Armature" and hips == "0/0/0"
              and same(stored("0/0", "translation", 3), translation) and same(stored("0/0", "rotation", 4), rotation)
              and same(stored("0/0", "scale", 3), (scale,) * 3)
              and near(stored(hips, "translation", 3), (0, .125 * size, .8375 * size)))
    else:
        # The hips' height is the rig's topology's: the tall rig's 1.18, every other's .88 (UnityrsBodyFixtures.cs).
        height = 1.18 if rig in FULL_CLIP_RIGS and topology(rig) == "tall" else .88
        ok = hips == "0/0" and near(stored(hips, "translation", 3), (0, height * size * hips_scale, 0))
    ok = ok and same(stored("0", "scale", 3), (ANIMATOR_SCALE if feature == "scaled-animator" else 1,) * 3)
    ok = ok and same(stored(hips, "scale", 3), (hips_scale,) * 3)
    replay.require(ok, f"body rig chain differs from the {rig} rig")


def probe_schedule(rig, names, targets, extra=None):
    if extra in MUSCLE_SWEEPS:
        index = {name: i for i, name in enumerate(names)}
        return body._scheduled(muscle_vectors(extra, index, PROBE_GROUPS, rig_bones(rig)), index, len(names))
    return _BASE["probe_schedule"](rig, names, targets, extra)


def schedule_name(name):
    return name if name in MUSCLE_CASES else _BASE["schedule_name"](name)


# The least turn a muscle alone at MUSCLE_VALUES' smallest value gives its bone, in degrees: the smallest default
# limit (the Jaw's, 10 degrees) times 1/8, with a wide margin.
LEAST_TURN = .25


def check_response(name, doc, replay):
    """A muscle sweep's single-muscle samples each turn the muscle's bone, by LEAST_TURN or more, from the
    recorded zero-vector sample's pose (not the prefab's rest: a zero vector that already turned a bone would
    let a muscle Unity ignores pass)."""
    capture = doc["capture"]
    names = [row["name"] for row in capture["humanoid"]["muscles"]]
    nodes = {row["name"]: row["node"] for row in capture["humanoid"]["bones"]}
    zero = [sample for sample in capture["samples"]
            if all(bits in ("00000000", "80000000") for bits in sample["muscles_f32"])]
    replay.require(bool(zero), f"{name} records no zero-vector sample")
    neutral = zero[0]["pose"]["transforms"]
    for sample in capture["samples"]:
        values = [replay.f32_from_bits(v) for v in sample["muscles_f32"]]
        moved = [i for i, v in enumerate(values) if v]
        if len(moved) != 1 or abs(values[moved[0]]) not in MUSCLE_VALUES:
            continue
        muscle = names[moved[0]]
        node = nodes[muscle_bone(muscle, PROBE_GROUPS)]
        turn = math.degrees(angle(*(body._f32s(t[node]["rotation"], replay, 4)
                                    for t in (neutral, sample["pose"]["transforms"]))))
        replay.require(turn >= LEAST_TURN, f"{muscle} at {values[moved[0]]} turned no bone in {name}")


def serialized_chain(name, rig, root, spec, view, replay):
    """B4's chain rule (chain.rs `animator_chain`) on the bundle's serialized Transforms, bit for bit with either
    zero's sign: the prefab root (the Animator node's one ancestor) at the identity; the Animator node at unit
    scale or, with that feature, ANIMATOR_SCALE, and at the identity or, a plain chain's offset root only,
    OFFSET_PLACEMENT; the armature, with that feature, the one node between it and the hips, exactly ARMATURE,
    and none otherwise; the hips at unit scale or, with that feature, HIPS_SCALE; every node below the hips at
    unit scale exactly (`unit_below_hips`)."""
    feature, _ = RIG_FEATURES.get(rig, ("", 1))
    nodes = {node["address"]: node for node in spec["nodes"]}

    def local(address):
        identity = nodes.get(address, {}).get("id")
        replay.require(isinstance(identity, dict), f"no built Transform at {address!r} in {name}")
        value = view.value((identity["file_index"], int(identity["path_id"])))
        rows = []
        for field, axes in (("m_LocalPosition", "xyz"), ("m_LocalRotation", "xyzw"), ("m_LocalScale", "xyz")):
            row = value.get(field)
            replay.require(isinstance(row, dict) and all(type(row.get(a)) in (int, float) for a in axes),
                           f"unreadable built {field} at {address!r} in {name}")
            rows.append([f32_hex(row[a]) for a in axes])
        return rows

    def at(address, translation, rotation, scale):
        t, q, s = local(address)
        return same(t, translation) and same(q, rotation) and same(s, (scale,) * 3)

    hips = [address for address, node in nodes.items() if node["name"] == "Hips"]
    armature = feature == "armature"
    placed = OFFSET_PLACEMENT if root == "offset" and feature == "" else ((0, 0, 0), (0, 0, 0, 1))
    ok = (hips == ["0/0/0" if armature else "0/0"] and at("", (0, 0, 0), (0, 0, 0, 1), 1)
          and at("0", *placed, ANIMATOR_SCALE if feature == "scaled-animator" else 1)
          and (not armature or nodes.get("0/0", {}).get("name") == "Armature" and at("0/0", *ARMATURE))
          and same(local(hips[0])[2], (HIPS_SCALE if feature == "hips-scale" else 1,) * 3)
          and all(same(local(address)[2], (1,) * 3) for address in nodes if address.startswith(hips[0] + "/")))
    replay.require(ok, f"the built chain of {name} is not one B4 admits as the {rig} rig at the {root} root")


def human_scale(name, spec, view, replay):
    """The serialized Avatar's m_Scale, as B4 reads it (chain.rs `scale`), not the capture's."""
    from anim_body_runs import human_of

    avatar = view.asset(spec["avatar_asset"], 90)
    scale = human_of(avatar).get("m_Scale") if isinstance(avatar, dict) else None
    replay.require(type(scale) in (int, float), f"no serialized Avatar m_Scale in {name}")
    return scale


def check_clip(name, info, editor, capture, clip, replay):
    """A clip's checks, and a full-body clip's every muscle curve, authored and built, at every key."""
    if info["rig"] not in FULL_CLIP_RIGS:
        return _BASE["check_clip"](name, info, editor, capture, clip, replay)
    names = [row["name"] for row in capture["humanoid"]["muscles"]]
    groups = full_groups(names, info["rig"])
    authored = body._authored(editor, replay)
    # A muscle whose bone the rig omits has no curve: the declared absence is exact.
    present = sorted(name for name in expected_absent(info["rig"], names) if muscle_property(name) in authored)
    replay.require(not present, f"{name} curves expected-absent muscle(s) {present}")
    runtime = body._runtime_slots(clip, replay)
    index = clip.get("m_MuscleClip", {}).get("m_IndexArray") or []
    for group in groups:
        prop = group[0][0]
        keys = authored.get(prop, {}).get("keys") or []
        replay.require(len(keys) == len(group[0][2])
                       and [replay.f32_from_bits(key["time_f32"]) for key in keys] == list(group[0][2])
                       and body._matches(group, lambda p, i, t: replay.f32_from_bits(keys[i]["value_f32"])),
                       f"authored full-body {prop} curve mismatch for {name}")
        sampled = body._built_track(runtime, index, group[0][1])
        replay.require(sampled is not None and body._matches(group, lambda p, i, t: sampled(t)),
                       f"built clip lost its full-body {prop} slot or values in {name}")
    # The variant's own checks without the full curves (its knee check reads a knee muscle's keys).
    full = {group[0][0] for group in groups}
    stripped = dict(editor, bindings=[row for row in editor["bindings"] if row["property"] not in full
                                      or (row["path"], row["type"]) != ("", "UnityEngine.Animator")])
    return _BASE["check_clip"](name, info, stripped, capture, clip, replay)


def avatar_skeleton(name, doc, avatar, replay):
    """The Avatar skeleton's Transforms as B4 resolves them (body_solve.rs `rest_scales`, chain.rs
    `animator_chain`): each skeleton node's `m_ID` through `m_TOS` to the built Transform at that path under the
    Animator node. The hips and every mapped slot resolve to the capture's node; no two skeleton nodes share a
    Transform; below the hips each node resolves, its Transform's parent is its skeleton parent's, and every
    Transform there is one a skeleton node names."""
    from anim_body_runs import human_of, mapping

    spec, require = doc["spec"], replay.require
    human = human_of(avatar)
    skeleton = human["m_Skeleton"].get("data", human["m_Skeleton"])
    ids, rows = skeleton.get("m_ID") or [], skeleton.get("m_Node") or []
    tos = dict(map(tuple, avatar.get("m_TOS") or []))
    require(len(ids) == len(rows) > 0, f"malformed Avatar skeleton in {name}")
    names = {node["address"]: node["name"] for node in spec["nodes"]}
    base = spec["binding_address"]
    paths = {"": base}
    for address in names:
        if address.startswith(base + "/"):
            parts = address[len(base) + 1:].split("/")
            path = "/".join(names[base + "/" + "/".join(parts[:k + 1])] for k in range(len(parts)))
            require(path not in paths, f"two Transforms at {path!r} in {name}")
            paths[path] = address
    found = [paths.get(tos.get(node_id)) if tos.get(node_id) is not None else None for node_id in ids]
    resolved = [address for address in found if address is not None]
    require(len(set(resolved)) == len(resolved), f"two Avatar skeleton nodes share a Transform in {name}")
    slots = mapping(human)
    captured = {row["name"]: row["node"] for row in doc["capture"]["humanoid"]["bones"]}
    for slot, k in slots.items():
        node = captured.get(slot, -1)
        require(k < len(found) and node >= 0 and found[k] == spec["nodes"][node]["address"],
                f"the Avatar's {slot} resolves to another Transform than the capture's in {name}")
    hips = slots.get("Hips")
    require(hips is not None, f"the Avatar maps no Hips in {name}")
    below = []
    for k, row in enumerate(rows):
        parent = row.get("m_ParentId")
        parent = parent if type(parent) is int and 0 <= parent < k else None
        inside = parent is not None and (parent == hips or below[parent])
        if inside:
            require(found[k] is not None and found[k].rpartition("/")[0] == found[parent],
                    f"Avatar skeleton node {k} is missing or misparented below the hips in {name}")
        below.append(inside)
    top = found[hips]
    under = {address for address in names if address.startswith(top + "/")}
    require(under == {found[k] for k, inside in enumerate(below) if inside},
            f"a Transform below the hips is not the Avatar skeleton's in {name}")


def clip_configuration(name, info, clip, replay):
    """The built clip's settings B4 reads (root.rs `root_settings`) beyond the variant's flags, which
    anim_fixture_body.check_clip compares: start at origin, no mirror, loop or cycle offset, the motion slots all
    bound for a motion variant and none otherwise, and no additive reference pose."""
    muscle = clip.get("m_MuscleClip", {})
    built = dict(m_StartAtOrigin=True, m_Mirror=False, m_LoopTime=False, m_LoopBlend=False, m_CycleOffset=0.)
    replay.require(all(body._built(muscle.get(key), want, replay) for key, want in built.items()),
                   f"built clip settings B4 reads differ from the {info['variant']} variant in {name}")
    replay.require(not muscle.get("m_ValueArrayReferencePose"), f"built clip has an additive reference pose in {name}")
    runtime = body._runtime_slots(clip, replay)
    index = muscle.get("m_IndexArray") or []
    bound = sum(body._built_track(runtime, index, attribute) is not None for attribute in range(7))
    replay.require(bound == (7 if body.CLIP_VARIANTS[info["variant"]]["motion"] else 0),
                   f"built clip binds {bound} motion slot(s), not the {info['variant']} variant's, in {name}")


# The clips B4 reads beyond round 11's own: F5's chain clips and F4's compact clips, each checked as B4 reads it.
F5_CHAIN_CLIPS = frozenset(f"body-chain-{rig}-clip" for rig in body.CHAIN_RIGS)
F4_CLIPS = frozenset(name for name, info in body.BODY_INFO.items() if info["kind"] == "clip" and info["rig"] == "compact")
B4_CASES = E5_CASES | F5_CHAIN_CLIPS | F4_CLIPS


def check_body(name, fixture, doc, view, clip_key, replay):
    """The body checks; on every case B4 reads, B4's inputs as chain.rs and root.rs read them; an E5 clip's
    declared scope; a muscle sweep's response."""
    result = _BASE["check_body"](name, fixture, doc, view, clip_key, replay)
    if name in MUSCLE_CASES:
        check_response(name, doc, replay)
    info = (ROUND11_INFO | body.BODY_INFO)[name]
    if name in B4_CASES:
        serialized_chain(name, info["rig"], info["root"], doc["spec"], view, replay)
        scale = human_scale(name, doc["spec"], view, replay)
        replay.require(H_RANGE[0] <= scale <= H_RANGE[1], f"human scale {scale!r} of {name} is outside B4's range")
        avatar_skeleton(name, doc, view.asset(doc["spec"]["avatar_asset"], 90), replay)
        if info["kind"] == "clip":
            clip_configuration(name, info, view.value(clip_key), replay)
    if "scope" in ROUND11_INFO.get(name, {}):
        authored = body._authored(doc["spec"]["evidence"]["editor"], replay)
        check_scope(name, ROUND11_INFO[name]["scope"], doc, authored, view.value(clip_key), replay)
    return result


@contextmanager
def round11_tables():
    """The body tables and checks with round 11's, in anim_fixture_body and anim_replay_complete, while it runs."""
    import anim_replay_complete as complete

    cases, info = body.BODY_CASES | ROUND11_CASES, body.BODY_INFO | ROUND11_INFO
    patches = [(body, "BODY_CASES", cases), (body, "BODY_INFO", info), (body, "rig_bones", rig_bones),
               (body, "check_chain", check_chain), (body, "probe_schedule", probe_schedule),
               (body, "schedule_name", schedule_name), (body, "check_clip", check_clip),
               (complete, "BODY_CASES", cases), (complete, "BODY_INFO", info),
               (complete, "BODY_BUNDLES", body.BODY_BUNDLES | ROUND11_BUNDLES),
               (complete, "OPTIONAL_CASES", complete.OPTIONAL_CASES | ROUND11_CASES.keys()),
               (complete, "check_body", check_body)]
    saved = [(module, name, getattr(module, name)) for module, name, _ in patches]
    try:
        for module, name, value in patches:
            setattr(module, name, value)
        yield
    finally:
        for module, name, value in saved:
            setattr(module, name, value)


def check_inventory(folder, replay):
    """A round 11 run: its declaration, and the whole body inventory listed and recorded."""
    manifest = replay.read_json((folder / "run.json").read_text(encoding="utf-8"))
    path = folder / "round.json"
    replay.require(path.is_file(), "not a round 11 run: no round.json (complete it with anim_replay.py complete)")
    declared = replay.read_json(path.read_text(encoding="utf-8"))
    replay.require(isinstance(manifest, dict) and isinstance(manifest.get("run"), dict)
                   and isinstance(manifest.get("cases"), list)
                   and all(isinstance(row, dict) and isinstance(row.get("name"), str) for row in manifest["cases"]),
                   "invalid run.json")
    replay.require(declared == {"format": "unityrs.fixture_round", "round": ROUND,
                                "run_id": manifest["run"].get("run_id")}, "round.json does not declare this round 11 run")
    statuses = {row["name"]: row.get("status") for row in manifest["cases"]}
    missing = sorted((body.BODY_CASES.keys() | ROUND11_CASES.keys()) - statuses.keys())
    replay.require(not missing, f"round 11 run lacks {len(missing)} body case(s): " + ", ".join(missing[:20]))
    # Every body case a round 11 run expects is recorded: a refused Avatar, a dropped curve or a skipped case
    # fails the run.
    unrecorded = [f"{name} ({statuses[name]})" for name in sorted(body.BODY_CASES.keys() | ROUND11_CASES.keys())
                  if statuses[name] != "recorded"]
    replay.require(not unrecorded, f"round 11 body cases not recorded ({len(unrecorded)}): " + ", ".join(unrecorded))


def complete_round(folder, unityrs, inspect_dir, replay):
    """Complete a round 11 run: its inventory, then every check `complete` runs, with round 11's tables."""
    import anim_replay_complete as complete

    check_inventory(folder, replay)
    with round11_tables():
        return complete.complete_folder(folder, unityrs, inspect_dir, replay)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("mode", choices=("complete",))
    parser.add_argument("folder", type=Path)
    inspection = parser.add_mutually_exclusive_group()
    inspection.add_argument("--unityrs", help="unityrs executable (default: unityrs on PATH)")
    inspection.add_argument("--inspect-dir", type=Path, help="saved inspect and show JSON")
    args = parser.parse_args()
    import anim_replay

    executable = str(Path(args.unityrs).resolve()) if args.unityrs else "unityrs"
    try:
        count, uncovered, sealed = complete_round(args.folder, executable, args.inspect_dir, anim_replay)
    except (ValueError, OSError, OverflowError) as exc:
        parser.exit(1, f"anim_fixture_round11: {exc}\n")
    print(f"Completed and validated {count} replay(s) of round {ROUND}.")
    for row in uncovered:
        print(f"Optional coverage not recorded: {row}")
    for name in sealed:
        print(f"Sealed hold-out case, not opened: {name} (only E3b unseals it: anim_body_freeze.py e3b)")


if __name__ == "__main__":
    sys.exit(main())
