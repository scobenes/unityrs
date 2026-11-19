"""B2's inputs: a completed run's humanoid recordings by role, their digest, and the coefficient fit.

A run is one Editor version's `<version>/<target>/` folder (run.json and
replays/). Each recording is read from its completed `.json` where
completion wrote one (a clip, with its built root settings) and from its
`.pending.json` otherwise (a known vector keeps the pending schema).

**Roles.** `training` is every humanoid recording but the hold-out rig's
(anim_fixture_body.HOLDOUT), and `holdout` is the hold-out rig's alone. The
role is decided from the file's case name before the file is opened, so a
training load never reads a hold-out byte, and `load` checks the loaded
rigs again. `solve` refuses any hold-out row as well, so no fit can take
one in by another path.

**Digest.** Each loaded file is named by its run's Editor version, case,
rig and sha256; the digest is the sha256 of that sorted list as canonical
JSON. The freeze records the training digest; E3b recomputes it.

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
from anim_body_tolerance import NAMES, readback_position
from anim_fixture_body import BODY_INFO, HOLDOUT

ROLES = ("training", "holdout")
# The 25 slots of m_HumanBoneIndex and m_HumanBoneMass: HumanBodyBones 0-8, UpperChest, 9-23.
SLOTS = (*NAMES[:9], "UpperChest", *NAMES[9:24])
F32 = replay.f32_from_bits


def role_of(case):
    return "holdout" if BODY_INFO.get(case, {}).get("rig") == HOLDOUT else "training"


def recording_paths(folder, role):
    """{case: path} for one role, chosen by name alone: a completed `.json` over its `.pending.json`."""
    replay.require(role in ROLES, f"unknown role {role}")
    paths = {}
    for pending in sorted((folder / "replays").rglob("*.pending.json")):
        case = pending.name.removesuffix(".pending.json")
        if role_of(case) != role:
            continue
        completed = pending.with_name(case + ".json")
        replay.require(case not in paths, f"two recordings of {case}")
        paths[case] = completed if completed.is_file() else pending
    return paths


class Run:
    """One run's recordings of one role: documents, the files' digests and the HumanTrait table."""

    def __init__(self, folder, role):
        manifest = replay.read_json((folder / "run.json").read_text(encoding="utf-8"))
        self.folder, self.role = folder, role
        self.version = manifest["run"]["editor_version"]
        self.documents, self.inputs = {}, []
        for case, path in recording_paths(folder, role).items():
            data = path.read_bytes()
            doc = replay.read_json(data.decode("utf-8"))
            if "humanoid" not in doc.get("capture", {}):
                continue
            replay.require(doc["capture"].get("editor_version", self.version) == self.version,
                           f"{case} was recorded by another Editor version")
            self.documents[case] = doc
            self.inputs.append(dict(run=self.version, case=case, rig=BODY_INFO.get(case, {}).get("rig", "p2b"),
                                    sha256=hashlib.sha256(data).hexdigest()))
        loaded = {row["rig"] == HOLDOUT for row in self.inputs}
        replay.require(loaded <= {role == "holdout"}, f"a {role} load read the wrong rig")


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


def observe(runs, table=None, avatar_of=None):
    """Every sample row of the runs (anim_body_adequacy.samples), named `version/case`. `table` is the
    HumanTrait table (parents, masses) to use; by default each run's own. With `avatar_of(run, doc)`, each
    recording's masses are its Avatar's m_HumanBoneMass (decision 1); the checkpoint's fold then keeps
    them, renormalized over the mapped slots."""
    rows = []
    for run in runs:
        if not run.documents:
            continue
        parents, masses = table or adequacy.bone_table(run.documents.values())
        for case, doc in run.documents.items():
            weights = avatar_masses(avatar_of(run, doc)) if avatar_of else masses
            for row in adequacy.samples(case, doc, parents, weights):
                row.update(name=f"{run.version}/{case}", case=case, version=run.version)
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
        replay.require(all(row["rig"] != HOLDOUT for row in rows), "the fit refuses hold-out rows")
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
