"""Synthetic body recordings that follow B2's f64 model, for the E0-E4 checks' tests.

anim_body_support.body_doc poses each sample by its muscles; `consistent`
then places the hips by anim_body_model.solve_hips for the sample's target
(a known vector's request, or a clip's 2.4 projection of its authored body
curves) and rewrites the read-back from the model, so every check passes on
the result. A test perturbs one value to see a check fail. `avatar` builds
the Avatar the recording plays (its skeleton pose, folded masses, m_RootX
and m_Scale), and `write_run` lays recordings out as a completed run.
"""

import copy
import json
import struct
import zlib
from unittest import mock

from anim_body_support import COEFFICIENT, body_doc, clip_case
from anim_fixture_support import RUN

import anim_body_adequacy as adequacy  # noqa: E402  (test_anim_replay puts it on the path)
import anim_body_e4 as e4  # noqa: E402
import anim_body_model as model  # noqa: E402
import anim_body_tolerance as tolerance  # noqa: E402
import anim_replay as replay  # noqa: E402
from anim_fixture_body import BODY_CASES, BODY_INFO, SERIALIZED_ROOT  # noqa: E402

F32 = replay.f32_from_bits
# The built clip of each clip case clip_doc made, by case: what E4's reader returns.
CLIPS = {}
BITS = replay.f32_bits
HUMAN_SCALE = 1.0625


def f32(value):
    return struct.unpack(">f", struct.pack(">f", value))[0]


def layout(doc):
    spec, capture = doc["spec"], doc["capture"]
    nodes, animator = spec["nodes"], spec["binding_address"]
    bones = capture["humanoid"]["bones"]
    hips = next(row["node"] for row in bones if row["name"] == "Hips")
    parents, masses = adequacy.bone_table([doc])
    mapped = {row["name"] for row in bones if row["name"] in tolerance.BODY_SLOTS and row["node"] >= 0}
    return nodes, animator, bones, hips, tolerance.folded_masses(masses, parents, mapped), \
        tolerance.recorded_edges(bones, nodes)


def points(nodes, transforms, bones, base, include=False):
    frames = tolerance.kinematics(nodes, transforms, F32, base, include_base=include)
    return {name: frame.position for name, frame in tolerance.bone_frames(bones, nodes, frames).items()}, frames


def coefficients(links, value=COEFFICIENT):
    return {f"{i}>{j}": value for i, j in links}


def clip_target(doc, sample, scale):
    """The 2.4 body pose in A (normalized) at a clip sample, from its built clip as E4 reads it."""
    serialized, body, start, original, _ = e4.built_pose(CLIPS[doc["spec"]["evidence"]["editor"]["fixture_case"]],
                                                         sample["source"])
    root = model.project(model.cells(serialized), body, start, original, serialized["m_Level"],
                         serialized["m_OrientationOffsetY"])
    position, rotation = model.animator_body(body, root, scale)
    return tuple(v / scale for v in position), rotation


def consistent(doc, value=COEFFICIENT):
    """Place every sample's hips by the model and rewrite its read-back from the model (in place)."""
    nodes, animator, bones, hips, weights, links = layout(doc)
    a = coefficients(links, value)
    human = doc["capture"]["humanoid"]
    human["human_scale"] = BITS(HUMAN_SCALE)
    rest, _ = points(nodes, doc["capture"]["rest"]["transforms"], bones, animator)
    rest_frame = tolerance.body_frame(rest)
    editor = doc["spec"]["evidence"]["editor"]
    hips_address = nodes[hips]["address"]
    for i, sample in enumerate(doc["capture"]["samples"]):
        pose = sample["pose"]["transforms"]
        try:
            if editor.get("body_targets"):
                known = editor["body_targets"][i]
                target, rotation = [tuple(F32(b) for b in known[key]) for key in ("position", "rotation")]
            else:
                target, rotation = clip_target(doc, sample, HUMAN_SCALE)
        except (model.Refused, model.Undecided):
            # A clip the model cannot project keeps its hips; only its read-back follows the model.
            target = None
        if target is not None:
            local, _ = points(nodes, pose, bones, hips_address)
            _, frames = points(nodes, pose, bones, animator)
            parent = frames[hips_address.rpartition("/")[0]]
            hips_scale = tolerance.uniform([F32(b) for b in pose[hips]["scale"]])
            # Unity places the hips at any twist; the model's twist scope only limits what it claims.
            with mock.patch.object(model, "frame_scope", lambda points: None):
                t, q = model.solve_hips(target, rotation, HUMAN_SCALE, local, weights, links, a, rest_frame,
                                        parent, hips_scale)
            pose[hips]["translation"] = [BITS(v) for v in t]
            pose[hips]["rotation"] = [BITS(v) for v in q]
        placed, _ = points(nodes, pose, bones, animator)
        _, frames = points(nodes, pose, bones, "", True)
        root, row = frames[animator], editor["body_samples"][i]
        # As Unity's: GetHumanPose's h w is the Animator's position plus its rotation of c in A, free of
        # its scale, and the recorder's InverseTransformPoint divides by that scale once more.
        body = model.centre(placed, weights, links, a)
        row["readback_position"] = [BITS(v / (HUMAN_SCALE * root.scale)) for v in body]
        turn = tolerance.matrix_quaternion(tolerance.product(tolerance.body_frame(placed), rest_frame, transpose=True))
        row["readback_rotation"] = [BITS(v) for v in turn]
        row["world_position"] = [BITS((p + v) / HUMAN_SCALE) for p, v in zip(root.position,
                                                                            tolerance.rotate(root.rotation, body))]
        row["world_rotation"] = [BITS(v) for v in tolerance.multiply(frames[animator].rotation, turn)]
    return doc


def clip_doc(name, samples=None):
    """A clip case with its authored curves, captured settings and completion's built root settings; its built
    clip goes to CLIPS."""
    doc = body_doc(name, RUN, samples=samples)
    built = CLIPS[name] = clip_case(doc, BODY_INFO[name]["variant"])
    doc["spec"]["start"], doc["spec"]["stop"] = 0., 1.
    doc["spec"]["evidence"]["body_settings"] = dict(
        serialized={key: built["m_MuscleClip"].get(key) for key in SERIALIZED_ROOT}, animator_attributes=[])
    return doc


def recording(name, samples=None, value=COEFFICIENT):
    doc = clip_doc(name, samples) if BODY_INFO[name]["kind"] == "clip" else body_doc(name, RUN, samples=samples)
    bundle = BODY_CASES[name].bundle
    doc["spec"].update(bundles=[dict(name=bundle)], avatar_asset=f"assets/unityrsfixtures/{bundle}/avatar.asset",
                       clip_asset=f"assets/unityrsfixtures/{bundle}/{name}.anim")
    doc["capture"]["bundle_sha256"] = ["0" * 64]
    return consistent(replay.read_json(json.dumps(doc)), value)


def avatar(doc, value=COEFFICIENT):
    """The Avatar a synthetic recording plays: its skeleton from the Animator node down."""
    nodes, animator, bones, _, weights, links = layout(doc)
    rest = doc["capture"]["rest"]["transforms"]
    under = [i for i, node in enumerate(nodes) if node["address"] == animator
             or node["address"].startswith(animator + "/")]
    under.sort(key=lambda i: (nodes[i]["address"].count("/"), nodes[i]["address"]))
    skeleton = {nodes[i]["address"]: k for k, i in enumerate(under)}
    parent = [-1 if nodes[i]["address"] == animator else skeleton[nodes[i]["address"].rpartition("/")[0]]
              for i in under]
    pose = [dict(t=dict(zip("xyz", map(F32, rest[i]["translation"]))),
                 q=dict(zip("xyzw", map(F32, rest[i]["rotation"]))),
                 s=dict(zip("xyz", map(F32, rest[i]["scale"])))) for i in under]
    slot_of = {row["name"]: skeleton[nodes[row["node"]]["address"]] for row in bones if row["node"] >= 0}
    slots = (*tolerance.NAMES[:9], "UpperChest", *tolerance.NAMES[9:24])
    masses = [f32(weights.get(slot, 0.)) for slot in slots]
    rest_points, _ = points(nodes, rest, bones, animator)
    c = model.centre(rest_points, {slot: m for slot, m in zip(slots, masses) if slot in rest_points}, links,
                     coefficients(links, value))
    names = {node["address"]: node["name"] for node in nodes}

    def path(address):
        parts = address[len(animator) + 1:].split("/") if address != animator else []
        return "/".join(names[animator + "/" + "/".join(parts[:k + 1])] for k in range(len(parts)))
    paths = [path(nodes[i]["address"]) for i in under]
    human = dict(m_HumanBoneMass={"t": "f32s", "v": "".join(BITS(m) for m in masses)},
                 m_HumanBoneIndex=[slot_of.get(slot, -1) for slot in slots],
                 m_Skeleton=dict(data=dict(m_Node=[dict(m_ParentId=p, m_AxesId=-1) for p in parent],
                                           m_ID=[zlib.crc32(p.encode()) for p in paths])),
                 m_SkeletonPose=dict(data=dict(m_X=pose)),
                 m_RootX=dict(t=dict(zip("xyz", map(f32, c))), q=dict(x=0., y=0., z=0., w=1.)),
                 m_Scale=f32(c[1]))
    return {"m_Avatar": {"m_Human": {"data": human}}, "m_TOS": [[zlib.crc32(p.encode()), p] for p in paths]}


def planned_margins(failures=()):
    """A stand-in for anim_body_holdout.predicted: these short-run synthetic designs (a few samples per case)
    cannot predict the hold-out, so the E0-E4 tests take its margins as given; test_anim_body_holdout checks the
    prediction itself."""
    def predicted(runs, *args, **kwargs):
        return {run.version: dict(worst_margin=.9 if failures else 2., in_frame=1, refused=0, failures=list(failures),
                                  samples=[]) for run in runs}
    return predicted


def write_run(folder, docs, version=RUN["editor_version"]):
    """A completed run folder: run.json and each recording under replays/body/ (a clip completed)."""
    body = folder / "replays" / "body"
    body.mkdir(parents=True, exist_ok=True)
    run = dict(RUN, editor_version=version)
    cases = [dict(name=name, status="recorded" if name in docs else "skipped", reason="" if name in docs else "-")
             for name in BODY_CASES]
    (folder / "run.json").write_text(json.dumps(dict(format="unityrs.fixture_run", version=1, run=run, cases=cases)),
                                     encoding="utf-8")
    for name, doc in docs.items():
        doc = copy.deepcopy(doc)
        doc["capture"]["editor_version"] = version
        clip = BODY_INFO[name]["kind"] == "clip"
        (body / f"{name}.pending.json").write_text(json.dumps(doc), encoding="utf-8")
        if clip:
            (body / f"{name}.json").write_text(json.dumps(doc), encoding="utf-8")
    return folder
