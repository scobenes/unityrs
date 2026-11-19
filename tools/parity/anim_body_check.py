"""B2's evidence checks E0 to E4 (docs/plans/humanoid-body.md 3.2) on completed runs' sample rows.

Each check returns a report with `passed`. Rows are anim_body_runs.observe's,
the model anim_body_model's, and every bound is derived from
anim_body_tolerance's operation model (Unity's f32 path) plus the model's own
f64 path:

- **E0** per recorded Avatar. E0a: m_HumanBoneMass and m_HumanBoneIndex hold
  one entry per slot of the 25-slot order (HumanBodyBones 0-8, UpperChest,
  9-23), finite and non-negative, with a node exactly for the mapped slots.
  E0b: each slot's mass against the E0b fold of HumanTrait's default (own)
  masses, which the recorder's hierarchy masses give
  (anim_body_adequacy.default_masses), an absent slot exactly zero, within
  gamma(50) of the value at f32 and at f64 (Unity's sums of up to 25 terms,
  the division and the store) plus the own masses' error through the fold.
  E0c: c(T) from m_SkeletonPose with the Avatar's masses and the
  coefficients against m_RootX.t, and its height against m_Scale, within
  Unity's and the model's centre-of-mass bounds at the skeleton pose, the
  store, the mass fold's own bound and the coefficients' uncertainty, and
  m_RootX.t.y against m_Scale within two f32 roundings.
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
  hips: each translation component within T_s = E_unity,s + u_s + E_eval,s,
  the rotation within Unity's and the model's solve turns, the scale exact.
- **E4**: per clip sample, 2.4's (T_A, B_A) from the authored body curves
  and the built clip's root settings against the read-back residual body
  pose, within epsilon, Unity's solve (sigma_p E_unity,s), the f32 curve
  sample and both projections' bounds; and each start-offset variant's
  read-back against its twin's. A cell 2.4 refuses or leaves undecided is
  listed, not evaluated.
"""

import math

import anim_body_adequacy as adequacy
import anim_replay as replay
from anim_body_model import (
    ANGLE_FLOOR,
    Refused,
    Undecided,
    animator_body,
    body_rotation,
    cells,
    centre,
    columns,
    project,
    projection_error,
    quaternion,
    sample_curves,
    solve_hips,
    turn_angle,
    yaw_error,
)
from anim_body_runs import F32, SLOTS, floats, human_of, residuals, solve
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
    readback_position,
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


# E0: the Avatar's masses and rest body.

def vector(value, axes):
    return tuple(float(value[axis]) for axis in axes)


def e0_avatar(label, avatar, doc, parents, masses, errors, design, coefficients):
    """E0a, E0b and E0c on one recorded Avatar; `doc` is a recording that plays it."""
    failures = []
    human = human_of(avatar)
    weights = floats(human.get("m_HumanBoneMass"), "m_HumanBoneMass")
    index = human.get("m_HumanBoneIndex")
    replay.require(len(weights) == len(SLOTS) and isinstance(index, list) and len(index) == len(SLOTS)
                   and all(type(i) is int and i >= -1 for i in index) and min(weights) >= 0,
                   f"{label}: malformed m_HumanBoneMass or m_HumanBoneIndex")
    mapped = {row["name"] for row in doc["capture"]["humanoid"]["bones"] if row["name"] in SLOTS and row["node"] >= 0}
    present = {slot for slot, node in zip(SLOTS, index) if node >= 0}
    if present != mapped:
        failures.append(f"{label}: E0a slot mapping {sorted(present ^ mapped)} differs from the captured bones")
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
    if present != mapped:
        return failures, dict(avatar=label, rest_distance=None, rest_bound=None)
    refused = adequacy.scope_refusal(present)
    if refused:
        # Outside the frozen topology scope the model refuses the Avatar: E0a and E0b only.
        return failures, dict(avatar=label, rest_distance=None, rest_bound=None, refused=refused)
    # E0c: forward kinematics of m_SkeletonPose from its root node, as rows anim_body_tolerance can walk.
    skeleton = human["m_Skeleton"].get("data", human["m_Skeleton"])
    pose = human["m_SkeletonPose"].get("data", human["m_SkeletonPose"])["m_X"]
    parent_ids = [row["m_ParentId"] for row in skeleton["m_Node"]]
    replay.require(len(pose) == len(parent_ids) and parent_ids and parent_ids[0] == -1
                   and all(0 <= p < k for k, p in enumerate(parent_ids) if k), f"{label}: malformed skeleton")
    addresses = []
    for k, p in enumerate(parent_ids):
        addresses.append("r" if p < 0 else f"{addresses[p]}/{k}")
    nodes = [dict(address=a) for a in addresses]
    transforms = [dict(translation=[replay.f32_bits(v) for v in vector(x["t"], "xyz")],
                       rotation=[replay.f32_bits(v) for v in vector(x["q"], "xyzw")],
                       scale=[replay.f32_bits(v) for v in vector(x["s"], "xyz")]) for x in pose]
    bones_rows = [dict(name=slot, node=node) for slot, node in zip(SLOTS, index) if node >= 0]
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
    if not distance <= bound:
        failures.append(f"{label}: E0c m_RootX.t {rest} against c(T) {c} ({distance:.3e} > {bound:.3e})")
    scale = float(human["m_Scale"])
    if not abs(scale - rest[1]) <= 2 * U32 * abs(scale):
        failures.append(f"{label}: E0c m_Scale {scale!r} against m_RootX.t.y {rest[1]!r}")
    return failures, dict(avatar=label, rest_distance=distance, rest_bound=bound)


def e0(runs, avatar_of, design, coefficients):
    """E0 on every recorded Avatar: one per bundle and Editor version. `avatar_of(run, doc)` reads it."""
    failures, avatars = [], []
    for run in runs:
        parents, masses, errors = adequacy.default_masses(run.documents.values())
        seen = set()
        for case, doc in sorted(run.documents.items()):
            key = (doc["spec"]["bundles"][0]["name"], doc["spec"]["avatar_asset"])
            if case not in BODY_INFO or key in seen:
                continue
            seen.add(key)
            found, row = e0_avatar(f"{run.version}/{key[0]}", avatar_of(run, doc), doc, parents, masses, errors,
                                   design, coefficients)
            failures += found
            avatars.append(row)
    replay.require(avatars, "no recorded body Avatar")
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
        if BODY_INFO.get(row["case"], {}).get("kind") == "probes":
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


def e2(rows):
    failures, checked, worst, refused = [], 0, 0., 0
    for row in rows:
        state = row["state"]
        if state["readback"] is None or row.get("refused"):
            refused += bool(row.get("refused"))
            continue
        observed, turn = readback_rotation(state)
        points = {name: frame.position for name, frame in state["bones"].items()}
        pairs = []
        try:
            pairs.append(("model", body_rotation(points, state["rest_frame"]),
                          turn_angle(turn + solve_turn(state, U64)) + 2 * ANGLE_FLOOR))
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
    return verdict(failures, residuals=checked, worst_ratio=worst, refused=refused)


def hips(row, design, coefficients):
    """E3 at one sample: (failures, |dt| / T_s)."""
    state = row["state"]
    local = {name: bone.position for name, bone in state["local"][U64][0].items()}
    t, q = solve_hips(state["target"], state["body_rotation"], state["scale"], local, state["weights"],
                      state["links"], coefficients, state["rest_frame"], state["chain"][U64], state["hips_scale"])
    tolerance = row["unity"] + design.hips_uncertainty(row) + row["evaluation"]
    delta = max(abs(a - b) for a, b in zip(t, state["hips_local"]))
    turn = solve_turn(state, U32) + state["turn"] + solve_turn(state, U64)
    bound = turn_angle(turn) + ANGLE_FLOOR
    residual = angle(q, state["hips_rotation"])
    where = f"{row['name']}@{row['local']}"
    failures = [] if delta <= tolerance else [f"{where}: hips translation off by {delta:.3e} > T_s {tolerance:.3e}"]
    if not residual <= bound:
        failures.append(f"{where}: hips rotation off by {math.degrees(residual):.4g} deg")
    return failures, delta / tolerance if tolerance > 0 else math.inf


def e3(rows, design, coefficients):
    failures, checked, worst, refused = [], 0, 0., 0
    for row in rows:
        if row["state"]["target_kind"] is None:
            continue
        if row.get("refused"):
            refused += 1
            continue
        try:
            found, ratio = hips(row, design, coefficients)
        except Refused:
            refused += 1
            continue
        checked += 1
        failures += found
        worst = max(worst, ratio)
    replay.require(checked, "no sample with a body target")
    return verdict(failures, samples=checked, worst_ratio=worst, refused=refused)


# E4: the root projection.

def authored(doc):
    return {row["property"]: row for row in doc["spec"]["evidence"]["editor"]["bindings"]
            if (row["path"], row["type"]) == ("", "UnityEngine.Animator")}


def clip_sample(row, doc, serialized):
    """(T_A, B_A, position bound, rotation bound) of 2.4 at one clip sample."""
    state, curves = row["state"], authored(doc)
    time = float(state["sample"]["source"])
    start_time = float(doc["spec"]["start"])
    rules = cells(serialized)
    (t, e_t), (q, e_q) = sample_curves(curves, "RootT", time), sample_curves(curves, "RootQ", time)
    replay.require(t is not None and q is not None, f"{row['name']}: no authored body curves")
    start = tuple(sample_curves(curves, prefix, start_time)[0] for prefix in ("RootT", "RootQ"))
    motion = [sample_curves(curves, prefix, time) for prefix in ("MotionT", "MotionQ")]
    original = None if motion[0][0] is None or motion[1][0] is None else (motion[0][0], motion[1][0])
    root = project(rules, (t, q), start, original, float(serialized["m_Level"]),
                   float(serialized["m_OrientationOffsetY"]))
    source = {"bake": start, "original": original, "body": (t, q)}[rules["yaw"]][1]
    source_error = e_q if rules["yaw"] == "body" else motion[1][1] if rules["yaw"] == "original" else 0.
    h = state["scale"]
    target, rotation = animator_body((t, q), root, h)
    position, turn = 0., 0.
    for unit, dq in ((U32, source_error), (U64, 0.)):
        extra = projection_error((t, q), root, h, yaw_error(source, dq, unit), unit)
        position, turn = position + extra[0], turn + extra[1]
    position += h * e_t + row["epsilon"] + state["chain"][U32].scale * row["unity"]
    turn += turn_sensitivity(q) * e_q + state["turn"] + solve_turn(state, U32)
    return target, rotation, position, turn_angle(turn) + 2 * ANGLE_FLOOR


def e4(rows, documents):
    """`documents` maps a row's name to its recording."""
    failures, combinations, samples, readbacks = [], {}, 0, {}
    for row in rows:
        info = BODY_INFO.get(row["case"])
        if info is None or info["kind"] != "clip" or row["state"]["readback"] is None:
            continue
        doc = documents[row["name"]]
        settings = doc["spec"]["evidence"].get("body_settings")
        replay.require(settings is not None, f"{row['name']}: not completed (no built root settings)")
        combination = combinations.setdefault(info["variant"].removesuffix("-start"), {"status": "pass"})
        state = row["state"]
        readback = (tuple(state["scale"] * v for v in readback_position(state["readback"], state["root"], F32)),
                    decode(state["readback"]["readback_rotation"], 4, F32))
        readbacks[row["name"], row["local"]] = (readback, row, info)
        try:
            target, rotation, position, bound = clip_sample(row, doc, settings["serialized"])
        except (Refused, Undecided) as exc:
            if combination["status"] == "pass":
                combination.update(status="undecided" if isinstance(exc, Undecided) else "refused", reason=str(exc))
            continue
        samples += 1
        where = f"{row['name']}@{row['local']}"
        off, turned = math.dist(target, readback[0]), angle(rotation, readback[1])
        if not off <= position:
            failures.append(f"{where}: T_A off the read-back by {off:.3e} > {position:.3e}")
        if not turned <= bound:
            failures.append(f"{where}: B_A off the read-back by {math.degrees(turned):.4g} deg")
        if not (off <= position and turned <= bound):
            combination["status"] = "fail"
    # The start-offset variants: the same residual body pose as their twins.
    pairs = 0
    for (name, local), ((position, rotation), row, info) in readbacks.items():
        if not info["variant"].endswith("-start"):
            continue
        twin_case = next((case for case, other in BODY_INFO.items() if other["kind"] == "clip"
                          and other["variant"] == info["variant"].removesuffix("-start")
                          and other["root"] == info["root"]
                          and other["rig"] == info["rig"]), None)
        twin = readbacks.get((f"{row['version']}/{twin_case}", local))
        if twin is None:
            continue
        pairs += 1
        (other_position, other_rotation), other, _ = twin
        bound = sum(r["epsilon"] + r["state"]["chain"][U32].scale * r["unity"] for r in (row, other))
        turn = turn_angle(sum(r["state"]["turn"] + solve_turn(r["state"], U32) for r in (row, other)))
        combination = combinations[info["variant"].removesuffix("-start")]
        kept = math.dist(position, other_position) <= bound and angle(rotation, other_rotation) <= turn + ANGLE_FLOOR
        if combination.get("start_offset") != "fail":
            combination["start_offset"] = "pass" if kept else "fail"
        # A combination 2.4 refuses or leaves undecided is not supported, so its pairs only inform.
        if not kept and combination["status"] in ("pass", "fail"):
            failures.append(f"{name}@{local}: the start offset changed the residual body pose")
            combination["status"] = "fail"
    return verdict(failures, combinations=dict(sorted(combinations.items())), samples=samples, start_pairs=pairs)
