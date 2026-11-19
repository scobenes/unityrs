"""B2's evidence checks E0 to E4 (docs/plans/humanoid-body.md 3.2) on completed runs' sample rows.

Each check returns a report with `passed`. Rows are anim_body_runs.observe's,
the model anim_body_model's, and every bound is derived from
anim_body_tolerance's operation model (Unity's f32 path) plus the model's own
f64 path:

- **E0** per recorded Avatar (every one the training recordings play; the
  hold-out's in E3b only). E0a: m_HumanBoneMass and m_HumanBoneIndex hold
  one entry per slot of the 25-slot order (HumanBodyBones 0-8, UpperChest,
  9-23), finite and non-negative, and each mapped slot's skeleton node is
  the captured bone's node, by path (m_Skeleton.m_ID through m_TOS against
  the capture's node names), no two slots on one node.
  E0b: each slot's mass against the E0b fold of HumanTrait's default (own)
  masses, which the recorder's hierarchy masses give
  (anim_body_adequacy.default_masses), an absent slot exactly zero, within
  gamma(50) of the value at f32 and at f64 (Unity's sums of up to 25 terms,
  the division and the store) plus the own masses' error through the fold.
  E0c: c(T) from m_SkeletonPose with the Avatar's masses and the
  coefficients against m_RootX.t, and its height against m_Scale, within
  Unity's and the model's centre-of-mass bounds at the skeleton pose, the
  store, the mass fold's own bound and the coefficients' uncertainty (an
  edge the design lacks leaves it unbounded, which fails), and
  m_RootX.t.y against m_Scale within two f32 roundings. E0d: each mapped
  bone of the recorded rest pose against m_SkeletonPose, which 2.2's F(T)
  and the hips solve take, within both forward kinematics' errors.
- **E1**: the checkpoint of anim_body_adequacy on the training rows (rank,
  condition, u_s <= E_unity,s, inventory, coverage, planned samples), the
  fitted coefficients within |a| <= 1 (anim_body_tolerance's centre-of-mass
  bound assumes it), each fitting row's residual within epsilon plus the
  fitted centre of mass's uncertainty, and each terminal probe (H-pos): a
  sample that turns only bones with no mapped descendant leaves the hips
  where its case's neutral sample has them, within both samples' E_unity.
- **E2**: per sample with read-back, the read-back body rotation against
  2.2 on the recorded bones, within the read-back's own turn
  (anim_body_tolerance.readback_turn) and the model's frame at f64; for a
  known vector also against the requested rotation, within the read-back's
  turn and Unity's solve (`solve_turn`).
- **E3**: the 2.3 hips from the recorded non-hips pose and the target body
  pose (a known vector's request, a clip's read-back), against the recorded
  hips: u_s within E_unity,s, each translation component within
  T_s = E_unity,s + u_s + E_eval,s, the rotation within Unity's and the
  model's solve turns, the translations below the hips exactly the Avatar's
  m_SkeletonPose (the source Unity's playback writes; the prefab's differ by
  f32 rounding), and the scales below the hips and the hips' scale exactly
  the rest's. A body case's sample without a target
  fails; a P2b case's is excluded by its role and listed.
- **E4** is anim_body_e4's.

No check passes on nothing: E0, E2, E3 and E4 fail with no comparison, and
a bound that bounds nothing (anim_body_model.turn_angle's NaN, an infinite
u_s) fails every comparison against it.
"""

import math

import anim_body_adequacy as adequacy
import anim_replay as replay
from anim_body_model import (
    ANGLE_FLOOR,
    Refused,
    body_rotation,
    centre,
    columns,
    lateral_scope,
    quaternion,
    spine_lateral,
    solve_hips,
    turn_angle,
)
from anim_body_runs import (F32, SLOTS, avatar_paths, captured_paths, floats, human_of, mapping, residuals,
                            skeleton_rows, solve)
from anim_body_tolerance import (
    U32,
    U64,
    body_frame,
    body_states,
    com_error,
    compose_rounding,
    conjugate,
    decode,
    edges,
    folded_masses,
    frame_error,
    g,
    gamma,
    norm,
    orientation_rounding,
    readback_turn,
    turn_sensitivity,
)
from anim_fixture_body import BODY_INFO, PROBE_GROUPS
from anim_reference import angle

COEFFICIENT_LIMIT = 1.
FOLD_DEPTH = 50
MUSCLE_BONES = {muscle: bone for bone, group in PROBE_GROUPS for muscle in group}


def verdict(failures, **fields):
    return dict(passed=not failures, failures=failures, **fields)


# The lateral domain (section 8): the step kept below the smallest failing spine Left-Right value, and the
# smallest limit that still validates anything (below it the checks fail, never pass on nothing).
LATERAL_STEP = 1 / 32
LATERAL_FLOOR = 1 / 16


def muscles_of(row):
    state = row["state"]
    return {name: F32(bits) for name, bits in zip(state["muscle_names"], state["sample"].get("muscles_f32") or [])}


def lateral_of(row):
    """The row's largest spine Left-Right muscle value."""
    return spine_lateral(muscles_of(row))


def outside(row, limit):
    """Whether `limit` (None for none) refuses the row's pose (anim_body_model.lateral_scope)."""
    if limit is None:
        return False
    try:
        lateral_scope(muscles_of(row), limit)
    except Refused:
        return True
    return False


def lateral_limit(rows, design, coefficients):
    """(limit, smallest failing value): the largest spine Left-Right value of a training sample the model
    compared (2.2's frame in E2, or the hips in E3) below every sample failing E2 or E3, and at least
    LATERAL_STEP below them; with no failure, the largest one. A sample the scope refused validates nothing."""
    compared = set()
    failed = {line.split(": ", 1)[0] for line in e2(rows, compared=compared)["failures"]
              + e3(rows, design, coefficients, compared=compared)["failures"]}
    values = {f"{row['name']}@{row['local']}": lateral_of(row) for row in rows if not row.get("refused")}
    worst = min((values[key] for key in failed if key in values), default=math.inf)
    if failed - values.keys():
        worst = 0.
    passing = [value for key, value in values.items() if value < worst and key in compared - failed]
    limit = min(max(passing, default=0.), worst - LATERAL_STEP)
    return limit, worst


# E0: the Avatar's masses and rest body.

def vector(value, axes):
    return tuple(float(value[axis]) for axis in axes)


def e0_avatar(label, avatar, doc, parents, masses, errors, design, coefficients):
    """E0a, E0b, E0c and E0d on one recorded Avatar; `doc` is a recording that plays it."""
    failures = []
    human = human_of(avatar)
    weights = floats(human.get("m_HumanBoneMass"), "m_HumanBoneMass")
    replay.require(len(weights) == len(SLOTS) and min(weights) >= 0, f"{label}: malformed m_HumanBoneMass")
    index = mapping(human)
    # E0a: every mapped slot on the captured bone's node, by path, and no two slots on one node.
    paths, captured = avatar_paths(avatar), captured_paths(doc)
    mapped = set(captured)
    for slot in sorted(index.keys() | mapped):
        if paths.get(slot) is None or paths.get(slot) != captured.get(slot):
            failures.append(f"{label}: E0a {slot} on {paths.get(slot)!r}, captured on {captured.get(slot)!r}")
    for kind, table in (("Avatar", index), ("capture", captured)):
        if len(set(table.values())) != len(table):
            failures.append(f"{label}: E0a two slots share one node in the {kind}")
    assigned = not failures
    present = set(index)
    avatar_masses = dict(zip(SLOTS, weights))
    folded = folded_masses(masses, parents, mapped)
    # The own masses' errors from the hierarchy masses (anim_body_adequacy.default_masses), through the fold.
    total, spread = sum(masses.values()), sum(errors.values())
    moved = {slot: share * spread for slot, share in folded_masses(errors, parents, mapped).items()}
    for slot in SLOTS:
        want = folded.get(slot, 0.)
        bound = ((gamma(FOLD_DEPTH, U32) + gamma(FOLD_DEPTH, U64)) * want
                 + (moved.get(slot, 0.) + want * spread) / total)
        if not abs(avatar_masses[slot] - want) <= bound:
            failures.append(f"{label}: E0b {slot} mass {avatar_masses[slot]!r} against {want!r} (bound {bound:.3e})")
    row = dict(avatar=label, rest_distance=None, rest_bound=None)
    if not assigned:
        return failures, row
    refused = adequacy.scope_refusal(present, avatar_masses)
    if refused:
        # Outside the frozen topology scope the model refuses the Avatar: E0a and E0b only.
        return failures, row | dict(refused=refused)
    # E0c: c(T) from m_SkeletonPose with the Avatar's masses and the coefficients against m_RootX.t.
    nodes, transforms, bones_rows = skeleton_rows(human)
    links = edges(parents, present)
    weights_present = {slot: avatar_masses[slot] for slot in present}
    states = {unit: body_states(nodes, transforms, F32, bones_rows, links, "r", unit) for unit in (U32, U64)}
    points = {name: state.position for name, state in states[U64][0].items()}
    c = centre(points, weights_present, links, coefficients)
    error = sum(com_error(states[unit][0], weights_present, links, states[unit][1], unit)[0]
                for unit in (U32, U64))
    magnitude = com_error(states[U64][0], weights_present, links, states[U64][1], U64)[1]
    gradients = columns(points, weights_present, links)
    u = design.uncertainty([{key: column[k] for key, column in gradients.items()} for k in range(3)])
    root = human["m_RootX"]
    rest = vector(root["t"], "xyz")
    bound = error + gamma(FOLD_DEPTH, U32) * magnitude + U32 * norm(rest) + math.sqrt(sum(v * v for v in u))
    distance = math.dist(c, rest)
    if not math.isfinite(bound):
        failures.append(f"{label}: E0c unbounded (an edge the design lacks)")
    elif not distance <= bound:
        failures.append(f"{label}: E0c m_RootX.t {rest} against c(T) {c} ({distance:.3e} > {bound:.3e})")
    scale = float(human["m_Scale"])
    if not abs(scale - rest[1]) <= 2 * U32 * abs(scale):
        failures.append(f"{label}: E0c m_Scale {scale!r} against m_RootX.t.y {rest[1]!r}")
    # E0d: the recorded rest pose against m_SkeletonPose, which 2.2's F(T) and the solve's rest take.
    spec, capture = doc["spec"], doc["capture"]
    recorded, _ = body_states(spec["nodes"], capture["rest"]["transforms"], F32, capture["humanoid"]["bones"],
                              links, spec["binding_address"])
    for slot in sorted(present):
        apart = math.dist(points[slot], recorded[slot].position)
        allowed = states[U32][0][slot].e_position + recorded[slot].e_position
        if not apart <= allowed:
            failures.append(f"{label}: E0d {slot} rest {apart:.3e} off m_SkeletonPose (bound {allowed:.3e})")
    return failures, row | dict(rest_distance=distance, rest_bound=bound)


def e0(runs, avatar_of, design, coefficients, table=None):
    """E0 on every recorded Avatar: one per bundle, Avatar asset and Editor version. `avatar_of(run, doc)` reads
    it; `table` is the HumanTrait table (parents, own masses, errors), by default each run's own."""
    failures, avatars = [], []
    for run in runs:
        parents, masses, errors = table or adequacy.default_masses(run.documents.values())
        seen = set()
        for case, doc in sorted(run.documents.items()):
            key = (doc["spec"]["bundles"][0]["name"], doc["spec"]["avatar_asset"])
            if key in seen:
                continue
            seen.add(key)
            found, row = e0_avatar(f"{run.version}/{key[0]}", avatar_of(run, doc), doc, parents, masses, errors,
                                   design, coefficients)
            failures += found
            avatars.append(row)
    if not avatars:
        failures.append("E0: no recorded Avatar")
    return verdict(failures, avatars=avatars)


# E1: the identifiable coefficients.

def moved_bones(state):
    """The bones a known vector's nonzero muscles turn; None for a muscle off the probe groups."""
    values = [F32(bits) for bits in state["sample"].get("muscles_f32") or []]
    return {MUSCLE_BONES.get(name) for name, value in zip(state["muscle_names"], values) if value}


def terminals(rows):
    """H-pos: each probe sample turning only terminal bones against its case's neutral sample."""
    failures, checked, by_case = [], 0, {}
    for row in rows:
        if BODY_INFO.get(row["case"], {}).get("kind") == "probes" and not row.get("refused"):
            by_case.setdefault(row["name"], []).append(row)
    for name, case_rows in by_case.items():
        neutral = next((row for row in case_rows if not moved_bones(row["state"])), None)
        if neutral is None:
            continue
        parents_of = {i for i, _ in case_rows[0]["state"]["links"]}
        for row in case_rows:
            moved = moved_bones(row["state"])
            if not moved or moved & parents_of or None in moved:
                continue
            checked += 1
            shift = math.dist(row["state"]["hips_local"], neutral["state"]["hips_local"])
            if not shift <= row["unity"] + neutral["unity"]:
                failures.append(f"{name}@{row['local']}: terminal {sorted(moved)} moved the hips by {shift:.3e}")
    return failures, checked


def e1(rows, design, inventory, coverage, planned):
    report = adequacy.adequacy(rows, coverage, inventory)
    report["missing_planned"] = planned
    coefficients = solve(design)
    failures = [] if adequacy.adequate(report) else ["the adequacy checkpoint is short"]
    failures += [f"coefficient {key} = {a!r} outside |a| <= {COEFFICIENT_LIMIT}"
                 for key, a in coefficients.items() if not abs(a) <= COEFFICIENT_LIMIT]
    fit = residuals(design, coefficients)
    failures += [f"{row['name']}@{row['local']}: fit residual {row['residual']:.3e} > {row['bound']:.3e}"
                 for row in fit if not row["residual"] <= row["bound"]]
    found, checked = terminals(rows)
    failures += found
    return verdict(failures, coefficients=coefficients, rank=report["rank"], condition=report["condition"],
                   free=report["free"], worst_margin=report["worst_margin"], terminal_samples=checked,
                   checkpoint={key: report[key] for key in ("coverage", "uncertainty_failures", "untrained",
                                                            "edge_mismatches", "missing_coefficients",
                                                            "missing_planned")},
                   worst_residual=max((row["residual"] / row["bound"] for row in fit), default=None))


# E2 and E3: the frame and the hips.

def solve_turn(state, unit):
    """A bound on how far Unity's (f32) or the model's (f64) solved hips orientation turns a vector: both
    frames, H_A's rounding, the hips parent's rotation, the local product and the quaternion conversion."""
    bones = state["local"][unit][0]
    rest = state["rest_local"][unit]
    points = {name: bone.position for name, bone in bones.items()}
    turn = (frame_error(points, {name: bone.e_position for name, bone in bones.items()}, unit)
            + frame_error({name: bone.position for name, bone in rest.items()},
                          {name: bone.e_position for name, bone in rest.items()}, unit))
    rounding, solved = orientation_rounding(state["body_rotation"], state["rest_frame"], body_frame(points), unit)
    parent = state["chain"][unit]
    q = quaternion(solved)
    return (turn + rounding + turn_sensitivity(parent.rotation) * parent.e_rotation
            + turn_sensitivity(q) * (g("unquaternion", 1, unit)
                                     + compose_rounding(conjugate(parent.rotation), q, unit)))


def readback_rotation(state):
    readback = state["readback"]
    rotation = decode(readback["readback_rotation"], 4, F32)
    if state["target_kind"] == "readback":
        return rotation, state["turn"]
    return rotation, readback_turn(state["world_bones"], state["rest_turn"], rotation,
                                   decode(readback["world_rotation"], 4, F32), state["root"])


def e2(rows, limit=None, compared=None):
    """E2 within `limit` (lateral_scope); `compared` (a set) collects each sample whose frame was compared."""
    failures, checked, worst, refused = [], 0, 0., 0
    for row in rows:
        state = row["state"]
        if row.get("refused") or outside(row, limit):
            refused += 1
            continue
        if state["readback"] is None:
            continue
        observed, turn = readback_rotation(state)
        points = {name: frame.position for name, frame in state["bones"].items()}
        pairs = []
        try:
            pairs.append(("model", body_rotation(points, state["rest_frame"]),
                          turn_angle(turn + solve_turn(state, U64)) + 2 * ANGLE_FLOOR))
            if compared is not None:
                compared.add(f"{row['name']}@{row['local']}")
        except Refused:
            refused += 1
        if state["target_kind"] == "known":
            pairs.append(("request", state["body_rotation"], turn_angle(turn + solve_turn(state, U32)) + ANGLE_FLOOR))
        for label, want, bound in pairs:
            checked += 1
            residual = angle(want, observed)
            worst = max(worst, residual / bound)
            if not residual <= bound:
                failures.append(f"{row['name']}@{row['local']}: {label} to read-back {math.degrees(residual):.4g} deg "
                                f"> {math.degrees(bound):.4g} deg")
    if not checked:
        failures.append("E2: no comparison")
    return verdict(failures, residuals=checked, worst_ratio=worst, refused=refused)


def hips(row, design, coefficients):
    """E3 at one sample: (failures, |dt| / T_s)."""
    state = row["state"]
    where = f"{row['name']}@{row['local']}"
    local = {name: bone.position for name, bone in state["local"][U64][0].items()}
    t, q = solve_hips(state["target"], state["body_rotation"], state["scale"], local, state["weights"],
                      state["links"], coefficients, state["rest_frame"], state["chain"][U64], state["hips_scale"])
    # u_s within Unity's own floor (the bounds assume it); an edge the design lacks leaves it unbounded.
    u = design.hips_uncertainty(row)
    failures = [] if u <= row["unity"] else [f"{where}: u_s {u:.3e} over E_unity,s {row['unity']:.3e}"]
    tolerance = row["unity"] + u + row["evaluation"]
    delta = max(abs(a - b) for a, b in zip(t, state["hips_local"]))
    turn = solve_turn(state, U32) + state["turn"] + solve_turn(state, U64)
    bound = turn_angle(turn) + ANGLE_FLOOR
    residual = angle(q, state["hips_rotation"])
    if not delta <= tolerance:
        failures.append(f"{where}: hips translation off by {delta:.3e} > T_s {tolerance:.3e}")
    if not residual <= bound:
        failures.append(f"{where}: hips rotation off by {math.degrees(residual):.4g} deg")
    # 2.3 keeps the Avatar's translations and the rest scales below the hips and the hips' rest scale, exactly.
    if state["drift"]:
        failures.append(f"{where}: translation or scale off the rest at {state['drift']}")
    ratio = delta / tolerance if 0 < tolerance < math.inf else math.inf
    return failures, ratio


def e3(rows, design, coefficients, limit=None, compared=None):
    """E3 on every row with a body target. A P2b case's row without one (no known vector, no read-back) is
    excluded by its role and listed; a body case's row without one fails. `limit` refuses a pose past it
    (lateral_scope); `compared` (a set) collects each sample compared."""
    failures, checked, worst, refused, excluded = [], 0, 0., 0, set()
    for row in rows:
        if row.get("refused") or row["state"]["target_kind"] is not None and outside(row, limit):
            refused += 1
            continue
        if row["state"]["target_kind"] is None:
            if row["case"] in BODY_INFO:
                failures.append(f"{row['name']}@{row['local']}: a body case's sample without a target")
            else:
                excluded.add(row["name"])
            continue
        try:
            found, ratio = hips(row, design, coefficients)
        except Refused:
            refused += 1
            continue
        checked += 1
        failures += found
        worst = max(worst, ratio)
        if compared is not None:
            compared.add(f"{row['name']}@{row['local']}")
    if not checked:
        failures.append("E3: no comparison")
    return verdict(failures, samples=checked, worst_ratio=worst, refused=refused, excluded=sorted(excluded))
