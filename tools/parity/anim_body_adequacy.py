"""B1 adequacy checkpoint for the humanoid body model (docs/plans/humanoid-body.md 2.5 and 6).

    python3 -I anim_body_adequacy.py OUTPUT_FOLDER [--json REPORT]

OUTPUT_FOLDER is one Editor version's fixture output (the folder holding
run.json), after the owner's run of the body fixtures. Every recording is
read from its `.pending.json`. The checkpoint, with B1's own tolerances
(anim_body_tolerance.py):

1. The weighted design matrix A_w over the edge coefficients: its column
   rank and 2-norm condition number (at most 1e6). Fitting rows are the
   read-back body positions of every body sample of the training rigs. The
   hold-out rig takes no part in this checkpoint at all: its recordings are
   not read, so they cannot change the verdict. Only the post-freeze
   evaluation (E3b) reads them. Each sample's three scalar equations form one
   block weighted by 1/epsilon, since epsilon bounds the Euclidean norm of
   that sample's residual.
2. For every training body sample and P2b humanoid sample, each component
   k of the propagated coefficient uncertainty against the recording's own
   noise floor: u_s,k = sum over blocks b of ||(J_s A_w^+)_k,b||_2 <=
   E_unity,s, the derived bound on Unity's own error in the recorded hips
   translation (anim_body_tolerance.hips_error at f32). J_s is the full
   sensitivity of the hips-local translation to the coefficients, with the
   solved hips orientation H_A of 2.3 and through the hips parent's
   rotation and scale. The target body pose is the one the recorder passed
   to SetHumanPose for a known vector, and the read-back for a clip (with
   Unity's forward errors). The bound holds for any residual within its
   blocks' epsilon. The report lists each sample's E_unity,s, u_s, E_eval,s (the
   model's own evaluation, anim_body_tolerance.evaluation_error) and E3's
   comparison tolerance T_s = E_unity,s + u_s + E_eval,s, and the
   distribution of the margins E_unity,s / u_s.
3. The coefficient inventory: each sample's edges, from the recorded
   hierarchy, must be those HumanTrait's parents give its mapped bones, and
   every edge of the F1 and F6 training rigs must have fitting rows.
4. The topology coverage: every F1 and F6 training rig recorded by its own
   probe case, with read-back rows and the rig's mapped bones.
5. The extra probe plan: every scheduled sample of every extra probe case
   (anim_fixture_body.EXTRA_PROBES) recorded with read-back, or the missing
   ones named.

The twist sweeps (anim_fixture_body.TWIST_RIGS) take no part: they measure
2.2's frame against the shoulder-hip twist for B2, not the centre of mass.
A run with or without them gives the same checkpoint.

With --design-only, each extra probe case the run lacks is predicted from
its rig's recorded geometry (anim_body_design.py), with conservative
epsilon and E_unity,s over an envelope of the rig's recorded hips, and the
same checkpoint runs.

There is no per-sample veto from E_unity against epsilon: ||A_w A_w^+||_inf >= 1
holds for the projector as a whole, not for each of its rows. Rank and
u_s do not depend on the column scale, so the masses (E0b-folded defaults)
matter only to the condition number. The right-hand side is not needed.
Exit 0 when adequate, 1 when short (B1 then extends the fixtures before
B2), 2 when the input is unreadable.
"""

import argparse
import json
import math
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import anim_replay as replay  # noqa: E402
from anim_body_tolerance import (BODY_SLOTS, U32, U64, body_frame, body_states, bone_frames, decode,  # noqa: E402
                                 edges, evaluation_error, folded_masses, frame_error, hips_error, kinematics,
                                 matrix_quaternion, measurement_tolerance, norm, ordered, product, quaternion_matrix,
                                 readback_turn, recorded_edges, rotate, tracked, uniform)
from anim_body_design import conservative, planned  # noqa: E402
from anim_fixture_body import (BODY_INFO, EXTRA_CASES, HOLDOUT, PROBE_RIGS, TWIST_CASES, probe_schedule,  # noqa: E402
                               rig_bones)

CONDITION_LIMIT = 1e6
TRAINING_RIGS = PROBE_RIGS.keys() - {HOLDOUT}
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


def samples(name, doc, parents, masses):
    """One row per sample: its coefficient columns in A, its J_s rows, epsilon, E_unity and E_eval."""
    spec, capture = doc["spec"], doc["capture"]
    human, bones_rows = capture["humanoid"], capture["humanoid"]["bones"]
    editor = spec["evidence"]["editor"]
    readbacks = editor.get("body_samples") or [None] * len(capture["samples"])
    targets = editor.get("body_targets") or [None] * len(capture["samples"])
    info = BODY_INFO.get(name)
    nodes, animator = spec["nodes"], spec["binding_address"]
    scale = F32(human["human_scale"])
    hips_address = nodes[next(row["node"] for row in bones_rows if row["name"] == "Hips")]["address"]
    links = recorded_edges(bones_rows, nodes)
    rest_transforms = capture["rest"]["transforms"]
    rest = bone_frames(bones_rows, nodes, kinematics(nodes, rest_transforms, F32, animator))
    rest_frame = body_frame({name: frame.position for name, frame in rest.items()})
    # Unity's T-pose frame and the model's, from the hips-local rest pose.
    rest_local = {unit: body_states(nodes, rest_transforms, F32, bones_rows, links, hips_address, unit)[0]
                  for unit in (U32, U64)}
    rest_turn = frame_error({n: b.position for n, b in rest_local[U32].items()},
                            {n: b.e_position for n, b in rest_local[U32].items()})
    for sample, readback, known in zip(capture["samples"], readbacks, targets):
        transforms = sample["pose"]["transforms"]
        frames = kinematics(nodes, transforms, F32, animator)
        bones = bone_frames(bones_rows, nodes, frames)
        mapped = set(bones)
        expected = edges(parents, mapped)
        weights = folded_masses(masses, parents, mapped)
        columns = {f"{i}>{j}": tuple(weights[i] * (b - a) for a, b in zip(bones[i].position, bones[j].position))
                   for i, j in links}
        local = {unit: body_states(nodes, transforms, F32, bones_rows, links, hips_address, unit) for unit in (U32, U64)}
        chain = {unit: tracked(nodes, transforms, F32, animator, unit)[hips_address.rpartition("/")[0]]
                 for unit in (U32, U64)}
        hips_row = transforms[next(i for i, node in enumerate(nodes) if node["address"] == hips_address)]
        hips_scale = uniform(decode(hips_row["scale"], 3, F32))
        hips_local = decode(hips_row["translation"], 3, F32)
        epsilon, turn = None, 0.
        if readback is not None:
            world = decode(readback["world_position"], 3, F32)
            world_bones, differences = body_states(nodes, transforms, F32, bones_rows, links, "")
            root = tracked(nodes, transforms, F32, "")[animator]
            epsilon = measurement_tolerance(world_bones, differences, weights, links, scale, world, root)
        if known is not None:
            # A known vector: the exact f32 body pose the recorder passed to SetHumanPose.
            target, body_rotation = decode(known["position"], 3, F32), decode(known["rotation"], 4, F32)
            target_error = U32 * scale * norm(target)
        elif readback is not None:
            # A clip: the read-back stands in for Unity's target, with Unity's forward centre of mass
            # and frame and the recorder's conversions (epsilon and readback_turn).
            target, body_rotation = decode(readback["readback_position"], 3, F32), decode(readback["readback_rotation"], 4, F32)
            target_error = epsilon
            turn = readback_turn(world_bones, rest_turn, body_rotation,
                                 decode(readback["world_rotation"], 4, F32), root)
        else:
            # Neither: the model's own body position and rotation (2.1, 2.2), so H_A is the recorded hips'.
            target = tuple(sum(weights[b] * bones[b].position[k] for b in ordered(bones)) / scale for k in range(3))
            body_rotation = matrix_quaternion(product(body_frame({n: f.position for n, f in bones.items()}),
                                                      rest_frame, transpose=True))
            target_error = U32 * scale * norm(target)
        solved = solved_turn(body_rotation, rest_frame, bones)
        jacobian = {key: hips_sensitivity(column, chain[U32], solved) for key, column in columns.items()}
        unity, _ = hips_error(*local[U32], rest_local[U32], weights, links, scale, target, target_error, turn,
                              body_rotation, rest_frame, chain[U32], hips_scale, hips_local)
        evaluation = evaluation_error(*local[U64], rest_local[U64], weights, links, scale, target, body_rotation,
                                      rest_frame, chain[U64], hips_scale)
        row = dict(name=name, local=sample["local"], rig=info["rig"] if info else "p2b", columns=columns,
                   jacobian=jacobian, unity=unity, evaluation=evaluation, fit=False, epsilon=epsilon,
                   edges=None if links == expected else f"recorded {links} but HumanTrait gives {expected}")
        row["fit"] = epsilon is not None and info is not None and info["rig"] != HOLDOUT
        yield row


def solved_turn(body_rotation, rest_frame, bones):
    """H_A H_rec^-1 = B_A F(T) F(pose)^-1 (2.3 step 2): from the recorded hips orientation to the solved one.

    F_h = H_rec^-1 F(pose in A), since F depends only on directions, so the
    recorded hips rotation cancels."""
    pose = body_frame({name: frame.position for name, frame in bones.items()})
    return product(product(quaternion_matrix(body_rotation), rest_frame), pose, transpose=True)


def hips_sensitivity(column, parent, solved=None):
    """d(hips-local translation)/d(a_ij) with the solved hips orientation H_A.

    P_A = h T_A - k H_A c_h, and k H_rec c_h's column is the recorded column
    in A, so the column turns by H_A H_rec^-1 (`solved`; None when the two
    agree), then sigma_p^-1 R_p^-1 (P_A - t_p)."""
    if solved is not None:
        column = tuple(sum(solved[r][k] * column[k] for k in range(3)) for r in range(3))
    inverse = (*(-v for v in parent.rotation[:3]), parent.rotation[3])
    return tuple(-v / parent.scale for v in rotate(inverse, column))


def expected_inventory(parents):
    """Every edge of the F1 and F6 training rigs, by HumanTrait's parents."""
    return sorted({f"{i}>{j}" for rig in TRAINING_RIGS for i, j in edges(parents, rig_bones(rig))})


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


def adequacy(rows, coverage, inventory=()):
    report = {"coverage": coverage, "coefficients": [], "rank": 0, "condition": None, "free": [],
              "uncertainty_failures": [], "untrained": [], "worst_margin": None, "margins": {}, "samples": [],
              "edge_mismatches": sorted({f"{row['name']}: {row['edges']}" for row in rows if row.get("edges")})}
    fitting = [row for row in rows if row["fit"]]
    keys = sorted({key for row in fitting for key in row["columns"]})
    report["missing_coefficients"] = sorted(set(inventory) - set(keys))
    if not keys:
        report["errors"] = ["no fitting rows"]
        return report
    # Each fitting sample is a block of three equations, weighted by its epsilon.
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
    # A_w^+ = (A_w^T A_w)^+ A_w^T, with (A_w^T A_w)^+ = V diag(1/sigma^2) V^T over the kept singular values.
    gram = [[sum(vectors[k][i] * vectors[k][j] / sigma[k] ** 2 for k in kept) for j in range(width)]
            for i in range(width)]
    failures, untrained, margins, table = [], [], [], []
    for row in rows:
        if not row["jacobian"].keys() <= set(keys):
            untrained.append(f"{row['name']}@{row['local']}: {sorted(row['jacobian'].keys() - set(keys))}")
            continue
        u = max(uncertainty([row["jacobian"].get(key, (0., 0., 0.))[k] for key in keys], gram, design)
                for k in range(3))
        # Adequacy: the coefficients' worst-case uncertainty within the recording's own noise floor.
        if not u <= row["unity"]:
            failures.append(f"{row['name']}@{row['local']}: u={u:.3e} E_unity={row['unity']:.3e}")
        margin = row["unity"] / u if u > 0 else math.inf
        margins.append(margin)
        # E3's comparison tolerance, T_s = E_unity,s + u_s + E_eval,s.
        table.append(dict(name=row["name"], local=row["local"], unity=row["unity"], uncertainty=u,
                          evaluation=row["evaluation"], tolerance=row["unity"] + u + row["evaluation"]))
    finite = sorted(m for m in margins if math.isfinite(m))
    report.update(uncertainty_failures=failures, untrained=untrained, samples=table,
                  worst_margin=finite[0] if finite else None,
                  margins={f"p{q}": finite[min(len(finite) - 1, q * len(finite) // 100)] for q in (0, 1, 5, 25, 50)}
                  if finite else {})
    return report


def uncertainty(gradient, gram, design):
    """One output component's bound: the 2-norm of each block of g A_w^+, summed over blocks."""
    h = [sum(a * b for a, b in zip(gradient, column)) for column in gram]
    entries = [sum(a * b for a, b in zip(h, line)) for line in design]
    return sum(math.sqrt(sum(v * v for v in entries[b:b + 3])) for b in range(0, len(entries), 3))


def adequate(report):
    return (not report.get("errors") and report["rank"] == len(report["coefficients"])
            and report["condition"] is not None and report["condition"] <= CONDITION_LIMIT
            and not report["uncertainty_failures"] and not report["untrained"] and not report["edge_mismatches"]
            and not report["missing_coefficients"] and not report["coverage"]["missing"]
            and not report.get("missing_planned"))


def coverage_of(documents):
    """F1 and F6 rigs credited only by their own probe case with read-back rows and the rig's topology."""
    recorded = set()
    probes = {info["rig"]: name for name, info in BODY_INFO.items() if info["kind"] == "probes" and not info["variant"]}
    for rig in TRAINING_RIGS:
        doc = documents.get(probes.get(rig))
        if doc is None or not doc["spec"]["evidence"]["editor"].get("body_samples"):
            continue
        mapped = {row["name"] for row in doc["capture"]["humanoid"]["bones"]
                  if row["name"] in BODY_SLOTS and row["node"] >= 0}
        if mapped == rig_bones(rig):
            recorded.add(rig)
    return {"recorded": sorted(recorded), "missing": sorted(TRAINING_RIGS - recorded)}


def missing_planned(documents):
    """Every scheduled sample of the extra probe plan that has no recording with read-back, by name."""
    missing = []
    for name in sorted(EXTRA_CASES):
        doc = documents.get(name)
        if doc is None:
            missing.append(f"{name}: not recorded")
            continue
        names = [row["name"] for row in doc["capture"]["humanoid"]["muscles"]]
        readbacks = doc["spec"]["evidence"]["editor"].get("body_samples") or []
        have = {sample["local"] for sample, _ in zip(doc["capture"]["samples"], readbacks)}
        missing += [f"{name}@{t}" for t, _ in probe_schedule(BODY_INFO[name]["rig"], names, False, name)
                    if t not in have]
    return missing


def load(folder):
    """Every humanoid recording but the hold-out rig's, which the checkpoint never reads, and the twist
    sweeps', which it does not use."""
    documents = {}
    for path in sorted((folder / "replays").rglob("*.pending.json")):
        name = path.name.removesuffix(".pending.json")
        if BODY_INFO.get(name, {}).get("rig") == HOLDOUT or name in TWIST_CASES:
            continue
        doc = replay.read_json(path.read_text(encoding="utf-8"))
        if "humanoid" in doc.get("capture", {}):
            documents[name] = doc
    return documents


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("folder", type=Path)
    parser.add_argument("--json", type=Path)
    parser.add_argument("--design-only", action="store_true",
                        help="add every extra probe case the run lacks, predicted from its rig's recorded geometry "
                             "(anim_body_design.py)")
    args = parser.parse_args(argv)
    try:
        documents = load(args.folder)
        extra = planned(documents) if args.design_only else {}
        predicted = sorted(extra)
        parents, masses = bone_table(documents.values())
        rows = [row for name, doc in documents.items() for row in samples(name, doc, parents, masses)]
        for name, variants in extra.items():
            rows += conservative([list(samples(name, doc, parents, masses)) for doc in variants])
        report = adequacy(rows, coverage_of(documents), expected_inventory(parents))
        report["missing_planned"] = missing_planned(documents | {name: variants[0] for name, variants in extra.items()})
    except (KeyError, TypeError, IndexError, ValueError, OSError, StopIteration) as exc:
        print(f"anim_body_adequacy: {exc}", file=sys.stderr)
        return 2
    report["adequate"] = adequate(report)
    report["predicted"] = predicted
    text = json.dumps(report, indent=2, ensure_ascii=False, allow_nan=False) + "\n"
    if args.json:
        args.json.write_text(text, encoding="utf-8")
    print(f"rank {report.get('rank')}/{len(report.get('coefficients', []))}, condition {report.get('condition')}, "
          f"{len(report.get('uncertainty_failures', []))} u_s failures, worst margin E_unity,s/u_s "
          f"{report.get('worst_margin')}, margins {report.get('margins')}, "
          f"missing rigs {report['coverage']['missing']}, missing edges {report['missing_coefficients']}, "
          f"missing planned samples {report['missing_planned']}: "
          + ("adequate" if report["adequate"] else "short")
          + (f" (predicted with {', '.join(predicted)})" if predicted else ""))
    return 0 if report["adequate"] else 1


if __name__ == "__main__":
    sys.exit(main())
