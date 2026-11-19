"""B1 adequacy checkpoint for the humanoid body model (docs/plans/humanoid-body.md 2.5 and 6).

    python3 -I anim_body_adequacy.py OUTPUT_FOLDER [--json REPORT]

OUTPUT_FOLDER is one Editor version's fixture output (the folder holding
run.json), after the owner's run of the body fixtures. Every recording is
read from its `.pending.json`. The checkpoint, with B1's own tolerances
(anim_body_tolerance.py):

1. For each fitting sample, whether its hips tolerance is at least twice
   its measurement tolerance, the necessary condition 2.5 reports first.
2. The weighted design matrix A_w = diag(1/epsilon) A over the edge
   coefficients: its column rank and 2-norm condition number (at most 1e6).
   Fitting rows are the read-back body positions of every body sample of
   the training rigs (all but the hold-out rig), three per sample.
3. For every body sample, hold-out and P2b humanoid sample, the propagated
   coefficient uncertainty u_s = ||J_s A_w^+||_inf <= tau_s / 2, where J_s is
   the sensitivity of its hips-local translation to the coefficients.
4. The topology coverage: every F1 and F6 rig recorded.

Rank, u_s and the first condition do not depend on the column scale, so
the masses (E0b-folded defaults) matter only to the condition number. The
right-hand side is not needed. Exit 0 when adequate, 1 when short (B1
then extends the fixtures before B2), 2 when the input is unreadable.
"""

import argparse
import json
import math
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import anim_replay as replay  # noqa: E402
from anim_body_tolerance import (BODY_SLOTS, bone_frames, decode, edges, folded_masses, hips_tolerance, kinematics,  # noqa: E402
                                 measurement_tolerance, rotate, uniform)
from anim_fixture_body import BODY_INFO, HOLDOUT, PROBE_RIGS  # noqa: E402
from anim_reference import multiply  # noqa: E402

CONDITION_LIMIT = 1e6
F32 = replay.f32_from_bits


def bone_table(documents):
    """HumanTrait's parents and default masses, identical in every body recording."""
    tables = {json.dumps(doc["spec"]["evidence"]["editor"]["human_bones"], sort_keys=True)
              for doc in documents if doc["spec"]["evidence"]["editor"].get("human_bones")}
    if len(tables) != 1:
        raise ValueError("expected one HumanTrait bone table across the body recordings")
    rows = json.loads(tables.pop())
    names = [row["name"] for row in rows]
    parents = {row["name"]: names[row["parent"]] if row["parent"] >= 0 else None for row in rows}
    masses = {row["name"]: F32(row["mass"]) for row in rows if row["name"] in BODY_SLOTS}
    if masses.keys() != BODY_SLOTS:
        raise ValueError("the bone table lacks a body slot")
    return parents, masses


def compose(parent, child):
    """(translation, rotation, scale) of child in parent's parent."""
    t, q, s = parent
    return (tuple(a + s * b for a, b in zip(t, rotate(q, child[0]))), multiply(q, child[1]), s * child[2])


def local(row):
    return (decode(row["translation"], 3, F32), decode(row["rotation"], 4, F32), uniform(decode(row["scale"], 3, F32)))


def samples(name, doc, parents, masses):
    """One row per sample: its coefficient columns in A, its J_s rows, epsilon and tau."""
    spec, capture = doc["spec"], doc["capture"]
    human = capture["humanoid"]
    editor = spec["evidence"]["editor"]
    readbacks = editor.get("body_samples") or [None] * len(capture["samples"])
    info = BODY_INFO.get(name)
    nodes, animator = spec["nodes"], spec["binding_address"]
    index = {node["address"]: i for i, node in enumerate(nodes)}
    scale = F32(human["human_scale"])
    for sample, readback in zip(capture["samples"], readbacks):
        transforms = sample["pose"]["transforms"]
        frames = kinematics(nodes, transforms, F32, animator)
        bones = bone_frames(human["bones"], nodes, frames)
        links = edges(parents, set(bones))
        weights = folded_masses(masses, parents, set(bones))
        hips_address = nodes[next(row["node"] for row in human["bones"] if row["name"] == "Hips")]["address"]
        parent = frames[hips_address.rpartition("/")[0]]
        columns = {f"{i}>{j}": tuple(weights[i] * (b - a) for a, b in zip(bones[i].position, bones[j].position))
                   for i, j in links}
        inverse = (*(-v for v in parent.rotation[:3]), parent.rotation[3])
        jacobian = {key: tuple(-v / parent.scale for v in rotate(inverse, column)) for key, column in columns.items()}
        if readback is not None:
            target = decode(readback["readback_position"], 3, F32)
        else:
            target = tuple(sum(weights[b] * bones[b].position[k] for b in bones) / scale for k in range(3))
        hips_local = decode(transforms[index[hips_address]]["translation"], 3, F32)
        tau = hips_tolerance(bones, weights, links, scale, target, parent, hips_local)
        row = dict(name=name, local=sample["local"], rig=info["rig"] if info else "p2b", columns=columns,
                   jacobian=jacobian, tau=tau, fit=False, epsilon=None, parent_scale=parent.scale)
        if readback is not None:
            root = ((0., 0., 0.), (0., 0., 0., 1.), 1.)
            for address in ("", animator):
                root = compose(root, local(transforms[index[address]]))
            world = decode(readback["world_position"], 3, F32)
            row["epsilon"] = measurement_tolerance(bones, weights, links, scale, target, world, (root[0], root[2]))
            row["fit"] = info["rig"] != HOLDOUT
        yield row


def householder_r(rows, width):
    """The N-by-N triangular factor of the R-by-N matrix `rows` (columns kept as lists)."""
    columns = [[row[k] for row in rows] for k in range(width)]
    height = len(rows)
    for k in range(min(width, height)):
        x = columns[k][k:]
        alpha = -math.copysign(math.sqrt(sum(v * v for v in x)), x[0] if x[0] else 1.)
        v = [x[0] - alpha, *x[1:]]
        vv = sum(c * c for c in v)
        if vv == 0:
            continue
        for j in range(k, width):
            col = columns[j]
            factor = 2 * sum(a * b for a, b in zip(v, col[k:])) / vv
            for i, a in enumerate(v):
                col[k + i] -= factor * a
    return [[columns[j][i] if i <= j and i < height else 0. for j in range(width)] for i in range(width)]


def jacobi_svd(matrix, width, sweeps=60):
    """One-sided Jacobi: singular values and right singular vectors (as columns of V)."""
    b = [[matrix[i][j] for i in range(width)] for j in range(width)]
    v = [[float(i == j) for i in range(width)] for j in range(width)]
    for _ in range(sweeps):
        rotated = False
        for p in range(width - 1):
            for q in range(p + 1, width):
                alpha = sum(x * x for x in b[p])
                beta = sum(x * x for x in b[q])
                gamma = sum(x * y for x, y in zip(b[p], b[q]))
                if abs(gamma) <= 2. ** -52 * math.sqrt(alpha * beta) or gamma == 0:
                    continue
                rotated = True
                zeta = (beta - alpha) / (2 * gamma)
                t = math.copysign(1., zeta) / (abs(zeta) + math.sqrt(1 + zeta * zeta))
                c = 1 / math.sqrt(1 + t * t)
                s = c * t
                for m in (b, v):
                    m[p], m[q] = ([c * x - s * y for x, y in zip(m[p], m[q])], [s * x + c * y for x, y in zip(m[p], m[q])])
        if not rotated:
            break
    return [math.sqrt(sum(x * x for x in col)) for col in b], v


def adequacy(rows, coverage):
    report = {"coverage": coverage, "coefficients": [], "rank": 0, "condition": None, "free": [],
              "uncertainty_failures": [], "untrained": [], "worst_ratio": None}
    fitting = [row for row in rows if row["fit"]]
    report["necessary"] = [f"{row['name']}@{row['local']}" for row in fitting
                           if row["tau"] < 2 * row["epsilon"] / row["parent_scale"]]
    keys = sorted({key for row in fitting for key in row["columns"]})
    if not keys:
        report["errors"] = ["no fitting rows"]
        return report
    design = [[row["columns"].get(key, (0., 0., 0.))[k] / row["epsilon"] for key in keys]
              for row in fitting for k in range(3)]
    width = len(keys)
    sigma, vectors = jacobi_svd(householder_r(design, width), width)
    top = max(sigma)
    threshold = top * max(len(design), width) * 2. ** -52
    kept = [k for k in range(width) if sigma[k] > threshold]
    report["coefficients"] = keys
    report["rank"] = len(kept)
    report["condition"] = top / min(sigma) if len(kept) == width else None
    report["free"] = [{key: round(vectors[k][i], 6) for i, key in enumerate(keys) if abs(vectors[k][i]) > .1}
                      for k in range(width) if k not in kept]
    # (A_w^T A_w)^+ = V diag(1/sigma^2) V^T over the kept singular values.
    gram = [[sum(vectors[k][i] * vectors[k][j] / sigma[k] ** 2 for k in kept) for j in range(width)]
            for i in range(width)]
    failures, untrained, worst = [], [], 0.
    for row in rows:
        if not row["jacobian"].keys() <= set(keys):
            untrained.append(f"{row['name']}@{row['local']}: {sorted(row['jacobian'].keys() - set(keys))}")
            continue
        u = 0.
        for k in range(3):
            g = [row["jacobian"].get(key, (0., 0., 0.))[k] for key in keys]
            h = [sum(a * b for a, b in zip(g, column)) for column in gram]
            u = max(u, sum(abs(sum(a * b for a, b in zip(h, line))) for line in design))
        worst = max(worst, u / row["tau"])
        if u > row["tau"] / 2:
            failures.append(f"{row['name']}@{row['local']}: u={u:.3e} tau={row['tau']:.3e}")
    report.update(uncertainty_failures=failures, untrained=untrained, worst_ratio=worst)
    return report


def adequate(report):
    return (not report.get("errors") and not report["necessary"] and report["rank"] == len(report["coefficients"])
            and report["condition"] is not None and report["condition"] <= CONDITION_LIMIT and not report["uncertainty_failures"]
            and not report["untrained"] and not report["coverage"]["missing"])


def load(folder):
    documents = {}
    for path in sorted((folder / "replays").rglob("*.pending.json")):
        doc = replay.read_json(path.read_text(encoding="utf-8"))
        if "humanoid" in doc.get("capture", {}):
            documents[path.name.removesuffix(".pending.json")] = doc
    return documents


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("folder", type=Path)
    parser.add_argument("--json", type=Path)
    args = parser.parse_args(argv)
    try:
        documents = load(args.folder)
        parents, masses = bone_table(documents.values())
        rows = [row for name, doc in documents.items() for row in samples(name, doc, parents, masses)]
        recorded = {BODY_INFO[name]["rig"] for name in documents if name in BODY_INFO}
        coverage = {"recorded": sorted(recorded & PROBE_RIGS.keys()), "missing": sorted(PROBE_RIGS.keys() - recorded)}
        report = adequacy(rows, coverage)
    except (KeyError, TypeError, IndexError, ValueError, OSError, StopIteration) as exc:
        print(f"anim_body_adequacy: {exc}", file=sys.stderr)
        return 2
    report["adequate"] = adequate(report)
    text = json.dumps(report, indent=2, ensure_ascii=False, allow_nan=False) + "\n"
    if args.json:
        args.json.write_text(text, encoding="utf-8")
    print(f"rank {report.get('rank')}/{len(report.get('coefficients', []))}, condition {report.get('condition')}, "
          f"{len(report.get('uncertainty_failures', []))} u_s failures, {len(report['necessary'])} rows with "
          f"tau < 2 epsilon, missing rigs {report['coverage']['missing']}: "
          + ("adequate" if report["adequate"] else "short"))
    return 0 if report["adequate"] else 1


if __name__ == "__main__":
    sys.exit(main())
