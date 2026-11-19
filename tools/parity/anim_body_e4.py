"""B2's E4 (docs/plans/humanoid-body.md 3.2): 2.4's root projection against the read-back, from the built clip.

Per clip sample, 2.4's (T_A, B_A) from the built clip's own playback data,
read from the run's bundle: the runtime body slots (RootT and RootQ, DOFs
7-13 through m_IndexArray) at the sample's clip time, the serialized body
start m_StartX, the runtime motion slots (MotionT and MotionQ, DOFs 0-6)
for an original-root cell, and the serialized root settings
(anim_body_model.cells). They are compared with the read-back residual body
pose within:

- epsilon, the read-back's own error, once;
- Unity's hips solve without its target error (sigma_p E_unity,s: the
  target's error is the read-back's, already counted);
- each runtime slot's f32 sample (`sample`): a streamed key's Hermite cubic
  at its operands and its clock, a dense frame's interpolation, and a
  constant exactly; RootQ's normalization; and the translation of whichever
  source each root cell takes (the body, the motion slots or the exact
  stored start);
- both projections' bounds (anim_body_model.projection_error). A yaw whose
  error is unbounded refuses the sample, so no bound is vacuous.

Each start-offset variant's read-back must match its twin's within both
samples' epsilon and Unity's solve; a variant without its twin fails, and so
does a run with no comparison. A cell 2.4 refuses or leaves undecided is
listed, not evaluated. Results are per configuration (variant, rig, Editor
version, root placement), never pooled: a sample without a read-back is
counted as excluded, and a configuration with nothing compared is excluded
(or refused, when the topology scope refused every sample), never passing.
Each Editor family the code requires (anim_body_runs.EDITORS, as E3b) needs
a comparison of its own, whichever versions the rows hold.
"""

import math
from bisect import bisect_right

import anim_replay as replay
from anim_body_check import solve_turn, verdict
from anim_body_model import (
    ANGLE_FLOOR,
    LATERAL_MUSCLES,
    Refused,
    Undecided,
    animator_body,
    cells,
    project,
    projection_error,
    turn_angle,
    yaw_error,
)
from anim_body_runs import EDITORS, F32
from anim_body_tolerance import U32, U64, decode, gamma, hips_error, norm, readback_position, turn_sensitivity
from anim_fixture_body import BODY_INFO, MUSCLE_ATTRIBUTE, SERIALIZED_ROOT
from anim_fixture_contract import compiled_curves
from anim_reference import angle, dense, normalize, streamed

BODY_T, BODY_Q, MOTION_T, MOTION_Q = (7, 8, 9), (10, 11, 12, 13), (0, 1, 2), (3, 4, 5, 6)


def runtime(clip, dofs=(*MOTION_T, *MOTION_Q, *BODY_T, *BODY_Q), strict=False):
    """{DOF: (is_streamed, keys)} of the built clip's Animator slots `dofs`, in storage order through
    m_IndexArray. A DOF m_IndexArray marks -1 is unbound; `strict` refuses any other index naming no slot."""
    tracks = compiled_curves(clip, replay)
    bindings = clip.get("m_ClipBindingConstant", {}).get("genericBindings", [])
    replay.require(tracks and len(tracks) == len(bindings), "unreadable built clip slots")
    slots = [(key, track) for key, spans in tracks.items() for track in spans]
    index = clip["m_MuscleClip"].get("m_IndexArray") or []
    result = {}
    for dof in dofs:
        slot = index[dof] if dof < len(index) and type(index[dof]) is int else None
        replay.require(not strict or slot == -1 or slot is not None and 0 <= slot < len(slots),
                       f"built clip DOF {dof} has an invalid slot index")
        slot = -1 if slot is None else slot
        if 0 <= slot < len(slots):
            replay.require(slots[slot][0] == (0, 95, dof, 8, False), f"built slot {slot} is not DOF {dof}")
            result[dof] = slots[slot][1]
    return result


def sample(track, grid, time):
    """One runtime slot at `time` as playback samples it, and a bound on Unity's f32 sample's error."""
    is_streamed, keys = track
    replay.require(keys, "a built slot without keys")
    if not is_streamed:
        values = [coefficients[3] for _, coefficients in keys]
        if len(values) == 1:
            return values[0], 0.
        begin, rate = grid["m_BeginTime"], grid["m_SampleRate"]
        i = min(len(values) - 2, max(0, math.floor((time - begin) * rate)))
        a, b = values[i], values[i + 1]
        return (dense(values, 1, 0, begin, rate, time),
                gamma(4, U32) * (abs(a) + abs(b)) + abs(b - a) * gamma(4, U32) * (1 + abs(time - begin) * rate))
    i = bisect_right(keys, time, key=lambda key: key[0]) - 1
    value = streamed(keys, time)
    if i < 0 or i == len(keys) - 1:
        return value, 0.
    start, (c3, c2, c1, c0) = keys[i]
    dt = time - start
    magnitude = ((abs(c3) * dt + abs(c2)) * dt + abs(c1)) * dt + abs(c0)
    slope = abs((3 * c3 * dt + 2 * c2) * dt + c1)
    # Horner's rounding at its operands, and the clock's: dt = time - start rounds at their magnitude.
    return value, gamma(6, U32) * magnitude + slope * U32 * (abs(time) + abs(start))


def clip_muscles(clip, names, time):
    """The spine Left-Right muscles (anim_body_model.LATERAL_MUSCLES) of a built humanoid clip at `time`, as
    playback samples them (a muscle m_IndexArray marks unbound is 0; an invalid index refuses); `names` are
    HumanTrait's muscle names in order."""
    dofs = {muscle: MUSCLE_ATTRIBUTE + names.index(muscle) for muscle in LATERAL_MUSCLES}
    tracks = runtime(clip, tuple(dofs.values()), strict=True)
    playback = clip["m_MuscleClip"]["m_Clip"]
    grid = playback.get("data", playback).get("m_DenseClip", {})
    return {muscle: sample(tracks[dof], grid, time)[0] if dof in tracks else 0. for muscle, dof in dofs.items()}


def pose_at(tracks, grid, dofs, time):
    """(T, its error) or (normalized Q, its error) from the runtime slots `dofs`; None when none is bound."""
    bound = [dof in tracks for dof in dofs]
    if not any(bound):
        return None, 0.
    replay.require(all(bound), f"a built clip binds only some of DOFs {dofs}")
    values = [sample(tracks[dof], grid, time) for dof in dofs]
    vector, error = tuple(value for value, _ in values), norm(tuple(e for _, e in values))
    if len(dofs) == 4:
        size = norm(vector)
        if not size > 4 * error:
            raise Refused(f"the body or motion rotation at DOFs {dofs} is too close to zero to normalize")
        return normalize(vector), 2 * error / (size - error)
    return vector, error


def built_pose(clip, time):
    """The built clip's serialized root settings, body (RootT, RootQ) at `time`, start m_StartX and motion
    (MotionT, MotionQ, or None) at `time`, and the errors {cell rule: (translation, rotation)}."""
    muscle = clip["m_MuscleClip"]
    serialized = {key: muscle.get(key) for key in SERIALIZED_ROOT}
    playback = muscle["m_Clip"].get("data", muscle["m_Clip"])
    grid = playback.get("m_DenseClip", {})
    tracks = runtime(clip)
    (t, e_t), (q, e_q) = pose_at(tracks, grid, BODY_T, time), pose_at(tracks, grid, BODY_Q, time)
    replay.require(t is not None and q is not None, "the built clip has no body slots")
    start_x = serialized["m_StartX"]
    start = (tuple(float(start_x["t"][a]) for a in "xyz"), normalize([float(start_x["q"][a]) for a in "xyzw"]))
    (mt, e_mt), (mq, e_mq) = pose_at(tracks, grid, MOTION_T, time), pose_at(tracks, grid, MOTION_Q, time)
    original = None if mt is None or mq is None else (mt, mq)
    errors = {"bake": (0., 0.), "original": (e_mt, e_mq), "body": (e_t, e_q)}
    return serialized, (t, q), start, original, errors


def clip_sample(row, clip):
    """(T_A, B_A, position bound, rotation bound) of 2.4 at one clip sample, from the built clip."""
    state = row["state"]
    serialized, (t, q), start, original, errors = built_pose(clip, float(state["sample"]["source"]))
    rules = cells(serialized)
    root = project(rules, (t, q), start, original, float(serialized["m_Level"]),
                   float(serialized["m_OrientationOffsetY"]))
    e_t, e_q = errors["body"]
    source = {"bake": start, "original": original, "body": (t, q)}[rules["yaw"]][1]
    h = state["scale"]
    target, rotation = animator_body((t, q), root, h)
    position, turn = 0., 0.
    for unit, dq in ((U32, errors[rules["yaw"]][1]), (U64, 0.)):
        bound = yaw_error(source, dq, unit)
        if bound == math.inf:
            raise Refused("an unbounded yaw error")
        extra = projection_error((t, q), root, h, bound, unit)
        position, turn = position + extra[0], turn + extra[1]
    unity = hips_error(*state["local"][U32], state["rest_local"][U32], state["weights"], state["links"], h,
                       state["target"], 0., state["turn"], state["body_rotation"], state["rest_frame"],
                       state["chain"][U32], state["hips_scale"], state["hips_local"])[0]
    sources = sum(errors[rules[cell]][0] for cell in ("xz", "height"))
    position += h * (e_t + sources) + row["epsilon"] + state["chain"][U32].scale * unity
    turn += turn_sensitivity(q) * e_q + state["turn"] + solve_turn(state, U32)
    return target, rotation, position, turn_angle(turn) + 2 * ANGLE_FLOOR


def configuration(row, info):
    """A clip row's E4 configuration: its variant (a start-offset variant with its twin), rig (each F5 chain
    its own), Editor version and root placement."""
    return f"{info['variant'].removesuffix('-start')}/{info['rig']}/{row['version']}/{info['root']}"


def e4(rows, clips):
    """`clips` maps a clip row's name to its built clip (the bundle's AnimationClip value). Per configuration
    (`configuration`): its status (pass, fail, or the refused or undecided 2.4 cell), compared samples, start
    offset and the samples excluded without a read-back. `passing` names the configurations that compared and
    passed, the only ones B4 and B5 implement; `excluded` those with nothing compared."""
    failures, configurations, samples, readbacks, versions = [], {}, 0, {}, {}
    for row in rows:
        info = BODY_INFO.get(row["case"])
        if info is None or info["kind"] != "clip":
            continue
        entry = configurations.setdefault(configuration(row, info),
                                          {"status": "pass", "samples": 0, "no_readback": 0, "topology": 0})
        versions.setdefault(row["version"], 0)
        if row.get("refused"):
            entry["topology"] += 1
            continue
        state = row["state"]
        if state["readback"] is None:
            entry["no_readback"] += 1
            continue
        readback = (tuple(state["scale"] * v for v in readback_position(state["readback"], state["root"], F32)),
                    decode(state["readback"]["readback_rotation"], 4, F32))
        readbacks[row["name"], row["local"]] = (readback, row, info)
        try:
            target, rotation, position, bound = clip_sample(row, clips[row["name"]])
        except (Refused, Undecided) as exc:
            if entry["status"] == "pass":
                entry.update(status="undecided" if isinstance(exc, Undecided) else "refused", reason=str(exc))
            continue
        samples += 1
        entry["samples"] += 1
        versions[row["version"]] += 1
        where = f"{row['name']}@{row['local']}"
        off, turned = math.dist(target, readback[0]), angle(rotation, readback[1])
        if not off <= position:
            failures.append(f"{where}: T_A off the read-back by {off:.3e} > {position:.3e}")
        if not turned <= bound:
            failures.append(f"{where}: B_A off the read-back by {math.degrees(turned):.4g} deg")
        if not (off <= position and turned <= bound):
            entry["status"] = "fail"
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
        entry = configurations[configuration(row, info)]
        if twin is None:
            failures.append(f"{name}@{local}: no start-offset twin ({twin_case}) at this sample")
            entry["start_offset"] = "fail"
            continue
        pairs += 1
        (other_position, other_rotation), other, _ = twin
        bound = sum(r["epsilon"] + r["state"]["chain"][U32].scale * r["unity"] for r in (row, other))
        turn = turn_angle(sum(r["state"]["turn"] + solve_turn(r["state"], U32) for r in (row, other)))
        kept = math.dist(position, other_position) <= bound and angle(rotation, other_rotation) <= turn + ANGLE_FLOOR
        if entry.get("start_offset") != "fail":
            entry["start_offset"] = "pass" if kept else "fail"
        # A configuration 2.4 refuses or leaves undecided is not supported, so its pairs only inform.
        if not kept and entry["status"] in ("pass", "fail"):
            failures.append(f"{name}@{local}: the start offset changed the residual body pose")
            entry["status"] = "fail"
    if not samples:
        failures.append("E4: no comparison")
    # Every Editor family the code requires (EDITORS, E3b's too), not only those the rows hold, compares.
    families = {family: sum(count for version, count in versions.items() if version.split(".")[0] == family)
                for family in EDITORS}
    failures += [f"E4: no comparison in Editor {family}" for family, count in families.items()
                 if not count and samples]
    failures += [f"E4: Editor version {version} is not one of {EDITORS}" for version in sorted(versions)
                 if version.split(".")[0] not in EDITORS]
    for entry in configurations.values():
        if entry["status"] == "pass" and not entry["samples"]:
            # Nothing compared: every sample refused on topology is a refusal, any other an exclusion.
            if entry["topology"] and not entry["no_readback"]:
                entry.update(status="refused", reason="topology: every sample refused")
            else:
                entry["status"] = "excluded"
    configurations = dict(sorted(configurations.items()))
    return verdict(failures, configurations=configurations, samples=samples, start_pairs=pairs,
                   passing=[key for key, entry in configurations.items() if entry["status"] == "pass"],
                   excluded=[key for key, entry in configurations.items() if entry["status"] == "excluded"])
