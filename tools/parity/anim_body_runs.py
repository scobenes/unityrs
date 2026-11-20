"""B2's inputs: a completed run's humanoid recordings by role, their digest, and the coefficient fit.

A run is one Editor version's `<version>/<target>/` folder (run.json and
replays/). Each recording is read from its completed `.json` where
completion wrote one (a clip, with its built root settings) and from its
`.pending.json` otherwise (a known vector keeps the pending schema).

**Inventory and roles.** The cases come from run.json, never from file
names: each case it lists as recorded that the builder's case tables
(anim_fixture_body, anim_fixture_contract, anim_fixture_coverage) give a
humanoid Avatar is read from the path the table lays out for it, and its
role comes from the same table: `holdout` for the hold-out rig's cases
(anim_fixture_body.HOLDOUT_RIGS), `e5` for round 11's E5 cases
(anim_fixture_body.E5_CASES, never loaded here), `training` for every other. The role is
decided before the file is opened, so a training load never reads a
hold-out byte, and no load reads a hold-out file at all: a hold-out run is
made (`Run.sealed`) only from the recordings sealed completion validated
and handed to E3b (anim_body_seal). A read recording must name its own case and the table's
bundle, so a hold-out recording under a training case's name refuses.
Every training body case is required (REQUIRED): `Run.missing` lists those
run.json does not record, which fails every check and refuses the freeze.
`load` checks the loaded rigs again, and `solve` refuses any hold-out row,
so no fit can take one in by another path.

**Digest.** Each loaded file is named by its run's Editor version, case,
rig and sha256; the digest is the sha256 of that sorted list as canonical
JSON. The freeze records the training digest; E3b recomputes it.

**Avatars.** With an Avatar reader (anim_body_freeze.readers), `observe`
takes each recording's masses from its Avatar's m_HumanBoneMass (decision
1), decides the topology scope from the Avatar's m_HumanBoneIndex before any
forward kinematics (a case outside it, or whose Avatar maps other slots
than the capture, yields refused rows only), and takes 2.2's F(T) from the
Avatar's m_SkeletonPose (`skeleton_points`).

**Fit.** The fitting rows (anim_body_adequacy's: read-back body positions of
the training body cases) give a block-weighted least-squares problem in
the edge coefficients: A_w a = b_w with b = h readback - sum_i m_i p_i in A,
each sample's block weighted by 1/epsilon. `design` factors A_w (Householder,
then one-sided Jacobi on R, as the checkpoint) and keeps the pseudo-inverse's
pieces that the uncertainty u_s needs; `solve` adds b and returns the
minimum-norm solution over the kept singular values. Only `solve` fits.
"""

import hashlib
import json
import math

import anim_body_adequacy as adequacy
import anim_replay as replay
from anim_body_model import centre
from anim_body_tolerance import NAMES, U64, body_states, edges, readback_position
from anim_fixture_body import BODY_CASES, BODY_INFO, E5_CASES, HOLDOUT_RIGS
from anim_fixture_contract import REPLAY_CASES, VECTOR_CASES
from anim_fixture_coverage import COVERAGE_CASES

# The Editor families every freeze, E4 and E3b needs (a version's leading number).
EDITORS = ("2022", "6000")

ROLES = ("training", "holdout")
# The 25 slots of m_HumanBoneIndex and m_HumanBoneMass: HumanBodyBones 0-8, UpperChest, 9-23.
SLOTS = (*NAMES[:9], "UpperChest", *NAMES[9:24])
F32 = replay.f32_from_bits
# Every humanoid case of the builder's tables: (its fixture, its folder under replays/).
CASES = {**{name: (fixture, "") for name, fixture in (REPLAY_CASES | COVERAGE_CASES).items() if fixture.avatar},
         **{name: (fixture, "known-vectors") for name, fixture in VECTOR_CASES.items() if fixture.avatar},
         **{name: (fixture, "body") for name, fixture in BODY_CASES.items()}}


def role_of(case):
    """A body case's role: a hold-out rig's, E5's (round 11's recordings, which B2 never reads), or training."""
    return "holdout" if BODY_INFO.get(case, {}).get("rig") in HOLDOUT_RIGS else "e5" if case in E5_CASES else "training"


REQUIRED = frozenset(name for name in BODY_CASES if role_of(name) == "training")


def statuses(manifest):
    """run.json's case statuses by name."""
    rows = manifest["cases"]
    replay.require(isinstance(rows, list) and all(isinstance(row, dict) and isinstance(row.get("name"), str)
                                                  and isinstance(row.get("status"), str) for row in rows),
                   "invalid run.json case rows")
    result = {row["name"]: row["status"] for row in rows}
    replay.require(len(result) == len(rows), "duplicate run.json case")
    return result


def recording_paths(folder, role, *, pending=False):
    """{case: path} for one role: run.json's recorded humanoid cases at the case table's paths, a completed
    `.json` over its `.pending.json` unless `pending`."""
    replay.require(role in ROLES, f"unknown role {role}")
    manifest = replay.read_json((folder / "run.json").read_text(encoding="utf-8"))
    paths = {}
    for case, status in sorted(statuses(manifest).items()):
        if status != "recorded" or case not in CASES or role_of(case) != role:
            continue
        path = folder / "replays" / CASES[case][1] / f"{case}.pending.json"
        completed = path.with_name(case + ".json")
        paths[case] = completed if completed.is_file() and not pending else path
    return paths


class Run:
    """One run's recordings of one role: documents, the files' digests, and the required cases it lacks. A
    training run reads its files; a hold-out run is never read here, only made (`sealed`) from the recordings
    sealed completion validated and handed over (anim_body_seal)."""

    def __init__(self, folder, role, *, pending=False):
        replay.require(role == "training", "a hold-out recording opens only through sealed completion (E3b)")
        manifest = replay.read_json((folder / "run.json").read_text(encoding="utf-8"))
        self.folder, self.role = folder, role
        self.version = manifest["run"]["editor_version"]
        recorded = {case for case, status in statuses(manifest).items() if status == "recorded"}
        self.missing = sorted(REQUIRED - recorded)
        self.documents, self.inputs = {}, []
        for case, path in recording_paths(folder, role, pending=pending).items():
            data = path.read_bytes()
            self.add(case, hashlib.sha256(data).hexdigest(), replay.read_json(data.decode("utf-8")), path.name)
        self.check_rigs()

    @classmethod
    def sealed(cls, folder, version, recordings):
        """A hold-out run from completion's validated recordings, {case: (sha256, document)}."""
        run = cls.__new__(cls)
        run.folder, run.role, run.version, run.missing = folder, "holdout", version, []
        run.documents, run.inputs = {}, []
        for case, (sha256, document) in sorted(recordings.items()):
            replay.require(case in CASES and role_of(case) == "holdout", f"{case} is not a hold-out case")
            run.add(case, sha256, document, case)
        run.check_rigs()
        return run

    def add(self, case, sha256, doc, label):
        spec = doc["spec"]
        replay.require("humanoid" in doc.get("capture", {}), f"{case}: a humanoid case without a humanoid capture")
        replay.require(spec["evidence"]["editor"].get("fixture_case") == case, f"{label} records another case than {case}")
        replay.require([bundle["name"] for bundle in spec["bundles"]] == [CASES[case][0].bundle],
                       f"{case} plays another bundle than the case table's {CASES[case][0].bundle}")
        replay.require(doc["capture"].get("editor_version", self.version) == self.version,
                       f"{case} was recorded by another Editor version")
        self.documents[case] = doc
        self.inputs.append(dict(run=self.version, case=case, rig=BODY_INFO.get(case, {}).get("rig", "p2b"),
                                sha256=sha256))

    def check_rigs(self):
        loaded = {row["rig"] in HOLDOUT_RIGS for row in self.inputs}
        replay.require(loaded <= {self.role == "holdout"}, f"a {self.role} load read the wrong rig")


def digest(inputs):
    rows = sorted((row["run"], row["case"], row["rig"], row["sha256"]) for row in inputs)
    return hashlib.sha256(json.dumps(rows, separators=(",", ":")).encode("utf-8")).hexdigest()


def load(folders, role):
    """The runs of one role, at most one per Editor version."""
    runs = [Run(folder, role) for folder in folders]
    replay.require(len({run.version for run in runs}) == len(runs), "two runs of one Editor version")
    return runs


def floats(value, label):
    """A float array as canonical show writes it (packed `f32s` or a list), finite."""
    if isinstance(value, dict):
        replay.require(value.get("t") == "f32s" and len(value["v"]) % 8 == 0, f"malformed {label}")
        try:
            value = [F32(value["v"][i:i + 8]) for i in range(0, len(value["v"]), 8)]
        except ValueError as exc:
            raise ValueError(f"malformed {label}: {exc}") from None
    replay.require(isinstance(value, list) and all(type(v) in (int, float) and math.isfinite(v) for v in value),
                   f"malformed {label}")
    return [float(v) for v in value]


def human_of(avatar):
    human = avatar["m_Avatar"]["m_Human"]
    return human.get("data", human)


def avatar_masses(avatar):
    """The Avatar's m_HumanBoneMass by body slot (decision 1), an absent slot's as stored."""
    weights = floats(human_of(avatar).get("m_HumanBoneMass"), "m_HumanBoneMass")
    replay.require(len(weights) == len(SLOTS) and min(weights) >= 0, "malformed m_HumanBoneMass")
    return dict(zip(SLOTS, weights))


def skeleton_rows(human):
    """m_SkeletonPose as rows anim_body_tolerance can walk: (nodes, transforms, bone rows), the root node "r"
    at the Animator, and each mapped slot's m_HumanBoneIndex node."""
    skeleton = human["m_Skeleton"].get("data", human["m_Skeleton"])
    pose = human["m_SkeletonPose"].get("data", human["m_SkeletonPose"])["m_X"]
    parent_ids = [row["m_ParentId"] for row in skeleton["m_Node"]]
    replay.require(len(pose) == len(parent_ids) and parent_ids and parent_ids[0] == -1
                   and all(0 <= p < k for k, p in enumerate(parent_ids) if k), "malformed Avatar skeleton")
    addresses = []
    for k, p in enumerate(parent_ids):
        addresses.append("r" if p < 0 else f"{addresses[p]}/{k}")

    def bits(value, axes):
        return [replay.f32_bits(float(value[axis])) for axis in axes]

    transforms = [dict(translation=bits(x["t"], "xyz"), rotation=bits(x["q"], "xyzw"), scale=bits(x["s"], "xyz"))
                  for x in pose]
    index = mapping(human)
    replay.require(all(node < len(pose) for node in index.values()), "m_HumanBoneIndex outside the skeleton")
    return [dict(address=a) for a in addresses], transforms, [dict(name=slot, node=node)
                                                             for slot, node in index.items()]


def mapping(human):
    """{slot: skeleton node} of m_HumanBoneIndex's mapped slots."""
    index = human.get("m_HumanBoneIndex")
    replay.require(isinstance(index, list) and len(index) == len(SLOTS)
                   and all(type(i) is int and i >= -1 for i in index), "malformed m_HumanBoneIndex")
    return {slot: node for slot, node in zip(SLOTS, index) if node >= 0}


def skeleton_points(human, parents, unit=U64):
    """The mapped slots' m_SkeletonPose positions in A, and their position errors at `unit`."""
    nodes, transforms, bones = skeleton_rows(human)
    states, _ = body_states(nodes, transforms, F32, bones, edges(parents, {row["name"] for row in bones}), "r", unit)
    return {name: state.position for name, state in states.items()}, \
        {name: state.e_position for name, state in states.items()}


def node_paths(doc):
    """{capture node index: its path under the Animator node} for every node below the Animator."""
    spec = doc["spec"]
    names = {node["address"]: node["name"] for node in spec["nodes"]}
    base = spec["binding_address"]
    out = {}
    for i, node in enumerate(spec["nodes"]):
        if node["address"].startswith(base + "/"):
            parts = node["address"][len(base) + 1:].split("/")
            out[i] = "/".join(names[base + "/" + "/".join(parts[:k + 1])] for k in range(len(parts)))
    return out


def rest_translations(avatar, doc):
    """{capture node index: its m_SkeletonPose local translation bits} for every node below the hips, through
    m_Skeleton.m_ID and m_TOS: the translations Unity's humanoid playback writes (E3a), against which the prefab's
    differ by f32 rounding. A node below the hips the Avatar's skeleton lacks refuses."""
    human = human_of(avatar)
    skeleton = human["m_Skeleton"].get("data", human["m_Skeleton"])
    pose = human["m_SkeletonPose"].get("data", human["m_SkeletonPose"])["m_X"]
    tos = dict(map(tuple, avatar.get("m_TOS") or []))
    by_path = {tos.get(node_id): k for k, node_id in enumerate(skeleton.get("m_ID") or []) if k < len(pose)}
    spec = doc["spec"]
    hips = spec["nodes"][next(row["node"] for row in doc["capture"]["humanoid"]["bones"] if row["name"] == "Hips")]
    out = {}
    for i, path in node_paths(doc).items():
        if not spec["nodes"][i]["address"].startswith(hips["address"] + "/"):
            continue
        k = by_path.get(path)
        replay.require(k is not None, f"{path}: below the hips but not in the Avatar's skeleton")
        out[i] = [replay.f32_bits(float(pose[k]["t"][axis])) for axis in "xyz"]
    return out


def captured_paths(doc):
    """{slot: the bone's path under the Animator node} from the capture's bone rows and nodes."""
    spec = doc["spec"]
    names = {node["address"]: node["name"] for node in spec["nodes"]}
    base = spec["binding_address"]
    paths = {}
    for row in doc["capture"]["humanoid"]["bones"]:
        if row["name"] in SLOTS and row["node"] >= 0:
            address = spec["nodes"][row["node"]]["address"]
            replay.require(address.startswith(base + "/"), f"{row['name']} is not under the Animator")
            parts = address[len(base) + 1:].split("/")
            paths[row["name"]] = "/".join(names[base + "/" + "/".join(parts[:k + 1])] for k in range(len(parts)))
    return paths


def avatar_paths(avatar):
    """{slot: its skeleton node's path} through m_HumanBoneIndex, m_Skeleton.m_ID and the Avatar's m_TOS."""
    human = human_of(avatar)
    skeleton = human["m_Skeleton"].get("data", human["m_Skeleton"])
    ids, tos = skeleton.get("m_ID") or [], dict(map(tuple, avatar.get("m_TOS") or []))
    return {slot: tos.get(ids[node]) if node < len(ids) else None for slot, node in mapping(human).items()}


def avatar_scope(avatar, doc, parents):
    """(the Avatar's 2.2 rest points, or None, and the refusal, or None) for one recording, from its Avatar
    alone before any forward kinematics: its slots' nodes against the capture's, then the topology scope."""
    human = human_of(avatar)
    paths, captured = avatar_paths(avatar), captured_paths(doc)
    if paths != captured:
        moved = sorted(slot for slot in paths.keys() | captured.keys() if paths.get(slot) != captured.get(slot))
        return None, f"the Avatar maps {moved} unlike the capture"
    mapped = set(paths)
    refused = adequacy.scope_refusal(mapped, avatar_masses(avatar))
    return (None, refused) if refused else (skeleton_points(human, parents)[0], None)


def observe(runs, table=None, avatar_of=None):
    """Every sample row of the runs (anim_body_adequacy.samples), named `version/case`. `table` is the
    HumanTrait table (parents, masses) to use; by default each run's own. With `avatar_of(run, doc)`, each
    recording's masses are its Avatar's m_HumanBoneMass (decision 1), the checkpoint's fold then keeping
    them renormalized over the mapped slots, its scope and F(T) are the Avatar's (avatar_scope), and the
    translations below its hips are its m_SkeletonPose's (rest_translations)."""
    rows = []
    for run in runs:
        if not run.documents:
            continue
        parents, masses = table or adequacy.bone_table(run.documents.values())
        for case, doc in run.documents.items():
            skeleton, refused, weights, translations = None, None, masses, None
            if avatar_of:
                avatar = avatar_of(run, doc)
                weights = avatar_masses(avatar)
                skeleton, refused = avatar_scope(avatar, doc, parents)
                translations = None if refused else rest_translations(avatar, doc)
            if refused:
                # Out of scope: no forward kinematics, no bound; every sample is refused.
                case_rows = [dict(local=sample["local"], refused=refused, fit=False, state=None, edges=None,
                                  columns={}, jacobian={}) for sample in doc["capture"]["samples"]]
            else:
                case_rows = adequacy.samples(case, doc, parents, weights, skeleton, translations)
            for row in case_rows:
                row.update(name=f"{run.version}/{case}", case=case, version=run.version,
                           rig=BODY_INFO.get(case, {}).get("rig", "p2b"))
                rows.append(row)
    return rows


def rhs(row):
    """b for a fitting row: h readback - sum_i m_i p_i, in A (metres)."""
    state = row["state"]
    readback = readback_position(state["readback"], state["root"], F32)
    points = {name: frame.position for name, frame in state["bones"].items()}
    base = centre(points, state["weights"], [], {})
    return tuple(state["scale"] * r - c for r, c in zip(readback, base))


class Design:
    """The weighted design matrix of the fitting rows and its pseudo-inverse's pieces."""

    def __init__(self, rows):
        replay.require(all(row["rig"] not in HOLDOUT_RIGS for row in rows), "the fit refuses hold-out rows")
        self.fitting = [row for row in rows if row["fit"]]
        self.keys = sorted({key for row in self.fitting for key in row["columns"]})
        replay.require(self.keys, "no fitting rows")
        width = len(self.keys)
        self.matrix = [[row["columns"].get(key, (0., 0., 0.))[k] / row["epsilon"] for key in self.keys]
                       for row in self.fitting for k in range(3)]
        self.r = adequacy.householder_r(self.matrix, width)
        self.sigma, self.vectors = adequacy.jacobi_svd(self.r, width)
        top = max(self.sigma)
        threshold = top * max(len(self.matrix), width) * 2. ** -52
        self.kept = [k for k in range(width) if self.sigma[k] > threshold]
        self.rank = len(self.kept)
        self.condition = top / min(self.sigma) if self.rank == width else None
        self.free = [{key: round(self.vectors[k][i], 6) for i, key in enumerate(self.keys)
                      if abs(self.vectors[k][i]) > .1} for k in range(width) if k not in self.kept]
        self.gram = [[sum(self.vectors[k][i] * self.vectors[k][j] / self.sigma[k] ** 2 for k in self.kept)
                      for j in range(width)] for i in range(width)]

    def uncertainty(self, gradients):
        """u per component: a gradient (by key) per output component, through A_w^+ block by block.
        A gradient on a key the design lacks is unbounded."""
        if any(set(gradient) - set(self.keys) for gradient in gradients):
            return [math.inf] * len(gradients)
        return [adequacy.uncertainty([gradient.get(key, 0.) for key in self.keys], self.gram, self.matrix)
                for gradient in gradients]

    def hips_uncertainty(self, row):
        """u_s: the largest component of the hips-local translation's uncertainty."""
        return max(self.uncertainty([{key: column[k] for key, column in row["jacobian"].items()}
                                     for k in range(3)]))


def solve(design):
    """The minimum-norm weighted least-squares coefficients over the kept singular values."""
    width = len(design.keys)
    augmented = [line + [b / row["epsilon"]]
                 for row, block in zip(design.fitting,
                                       (design.matrix[i:i + 3] for i in range(0, len(design.matrix), 3)))
                 for line, b in zip(block, rhs(row))]
    projected = [line[width] for line in adequacy.householder_r(augmented, width + 1)[:width]]
    coefficients = [0.] * width
    for k in design.kept:
        column = [sum(design.r[i][j] * design.vectors[k][j] for j in range(width)) for i in range(width)]
        weight = sum(a * b for a, b in zip(column, projected)) / design.sigma[k] ** 2
        for j in range(width):
            coefficients[j] += weight * design.vectors[k][j]
    return dict(zip(design.keys, coefficients))


def residuals(design, coefficients):
    """Each fitting row's residual |A_s a - b_s| (metres) and its bound epsilon_s + |u_c,s|, with u_c,s the
    uncertainty of the fitted centre of mass at that sample."""
    out = []
    for row in design.fitting:
        b = rhs(row)
        fitted = [sum(coefficients[key] * column[k] for key, column in row["columns"].items()) for k in range(3)]
        u = design.uncertainty([{key: column[k] for key, column in row["columns"].items()} for k in range(3)])
        out.append(dict(name=row["name"], local=row["local"], residual=math.dist(fitted, b),
                        bound=row["epsilon"] + math.sqrt(sum(v * v for v in u))))
    return out
