"""B5a's muscle pose: the f64 reference with its floating-point order written out, the goldens Rust's
`humanoid::muscle_pose` must equal bit for bit, and its agreement with Unity's recordings.

**Reference 2** (REFERENCE; docs/plans/humanoid-body.md B5a (e), pre-registered before any code changed):
each muscle's angle picks its limit by the sign of the value, then applies the axis sign `s = m_Sgn`,
`theta = s * (v * m_Max)` for `v >= 0`, else `s * (v * -m_Min)`. Reference 1 was
`anim_humanoid_qualify.muscle_rotation`'s hypothesis, `s` first with the stored pair (`hypothesis_angles`, kept
for the regression check): on the round 10 recordings it disagreed on exactly the axes with `s = -1` and
unequal limits (LeftShoulder, LeftUpperArm and the eyes' z), by `-|v| * (m_Max + m_Min)`. The two agree bit for
bit on every axis with `s = +1` or `m_Max = -m_Min` exactly. Each product is exact in f64 (two f32 operands),
so the angle has the same bits in Rust and Python. `anim_humanoid_qualify`, P2b's evidence, is unchanged.

The rest of the equation is `anim_humanoid_qualify.muscle_rotation`'s (crates/unityrs-anim/src/humanoid/
muscle_pose.rs states it), with each sum's order fixed: a quaternion product's components left to right as `multiply` writes
them, and a norm as `sqrt(((x*x + y*y) + z*z) + w*w)`. CPython's `sum` adds that way up to 3.11 only (from 3.12
it compensates), so this module writes the order out instead of calling `sum`, and gives the same bits under
any Python. `tan` is the platform's C library's, in Python and in Rust alike, and the one step whose bits differ
between platforms (glibc's and Apple's in the last bit for about one angle in ten): each row records its angles,
their tangents and the local rotation, so Rust checks the angles bit for bit anywhere, the tangents where its
`tan` is glibc's (CI), and the rest bit for bit given the tangents. The goldens are generated in the container.

The bit-for-bit claim has a narrower domain than "any input": the same finite f32 inputs, and the same `tan`
bits (one C library, or rows that record the tangents); across C libraries only the angles are bit for bit.
`angles`, `normalize` and the rotations built on them return None where Rust's return `None`: an axis past z,
or a zero or NaN quaternion, which a NaN or infinite muscle value gives (`tangents` gives such an angle's tangent as NaN, as C's `tan` does, where Python's raises).

    python3 -I tools/parity/anim_muscle_pose.py golden [--check]
        The goldens (GOLDENS): synthetic axes for every bone a muscle turns, each muscle alone at every value
        of VALUES, each bone's muscles jointly, as the f64 bits of the angles, tangents and local rotation.
    python3 -I tools/parity/anim_muscle_pose.py cases --unityrs PATH --out FILE <run folder>...
        Real-Avatar cases from training runs (never a hold-out recording): every recorded known vector and
        clip-playback sample (its muscle values the clip's own authored curves at the sample's source time,
        each with the curve's own f32 evaluation error), each with Unity's recorded local rotation and its
        derived bound, and the golden schedule on each distinct real axes record (generate them in the
        container, whose `tan` is CI's). A round 11 run adds its sweeps and full-body clips
        (anim_muscle_round11) as confirmation entries. The file records each Editor's required cases and the
        ones its runs lack. They stay private; `UNITYRS_MUSCLE_POSE_CASES=FILE cargo test` checks Rust
        against them.
    python3 -I tools/parity/anim_muscle_pose.py agree FILE
        Each bone's worst angle to Unity's recording against its bound, the Spine first, for acceptance
        (rounds 1 to 10) and confirmation (round 11), and the refusal cases apart; how many of its recorded
        rows reference 2 changes from reference 1. It exits non-zero (`failures`) on an Editor with no
        comparison, any acceptance or confirmation sample outside its bound, a required case missing or
        never compared, or a changed row on a bone reference 1 agreed on.

**The agreement bound** (`unity_bound`) is derived, never fitted: Unity's own f32 error along the same path,
each operation bounded at its actual operands as section 2.5 of docs/plans/humanoid-body.md derives E3a's,
as a bound on the quaternion error's 2-norm. Each limit product rounds once (u = 2^-24), `tanf` is within one
ulp, a product of quaternions a, b adds |a|E_b + |b|E_a and 2*gamma_4|a||b| of rounding, and normalizing q
gives 2E/|q| + 4u. The SO(3) angle of an error E between unit quaternions is at most 4 asin(E/2). The f64
reference's own error adds `anim_replay.F64_ANGLE_FLOOR`. It assumes Unity evaluates this equation in f32;
a recording outside it is a disagreement to report, not a bound to widen.
"""

import argparse
import hashlib
import json
import math
import struct
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import anim_replay as replay  # noqa: E402
from anim_humanoid_qualify import MUSCLE_COUNT, muscle_bone  # noqa: E402
from anim_reference import angle  # noqa: E402

FORMAT = "unityrs.muscle_pose"
REFERENCE = 2
GOLDENS = Path(__file__).resolve().parents[2] / "crates/unityrs-anim/tests/humanoid/muscle_pose.json"
U32 = 2.0 ** -24
# Each muscle's axis (0 x twist, 1 y, 2 z), in MuscleName order (Rust MUSCLE_AXES).
MUSCLE_AXES = (2, 1, 0) * 5 + (2, 1) * 3 + (2, 1, 0, 2, 0, 2, 1, 2) * 2 + (2, 1, 2, 1, 0, 2, 0, 2, 1) * 2 \
    + (2, 1, 2, 2) * 10
# Each muscle alone: B5a's sweep values (anim_fixture_muscles.MUSCLE_VALUES), both zeros, a value past each
# limit, and 2^-24.
VALUES = (1., -1., .75, -.75, .5, -.5, .25, -.25, .125, -.125, 0., -0., 1.5, -1.5, 2. ** -24)
COMBINED = ((.6, -.5, .4), (-.6, .5, -.4))


def f32(value):
    return struct.unpack("<f", struct.pack("<f", value))[0]


def f64_hex(value):
    return struct.pack(">d", value).hex()


def f64_of(text):
    return struct.unpack(">d", bytes.fromhex(text))[0]


def multiply(a, b):
    ax, ay, az, aw = a
    bx, by, bz, bw = b
    return (aw * bx + ax * bw + ay * bz - az * by, aw * by - ax * bz + ay * bw + az * bx,
            aw * bz + ax * by - ay * bx + az * bw, aw * bw - ax * bx - ay * by - az * bz)


def normalize(q):
    """`q` over its norm; None for a norm that is not positive (zero or NaN), as Rust's `normalize`."""
    norm = math.sqrt(((q[0] * q[0] + q[1] * q[1]) + q[2] * q[2]) + q[3] * q[3])
    return tuple(x / norm for x in q) if norm > 0 else None


def angles(axes, values):
    """Reference 2: each axis's angle, the limit by the value's sign, then the axis sign (a later muscle on the
    same axis replaces an earlier one). None for an axis past z, as Rust's `muscle_angles`."""
    result = [0., 0., 0.]
    for value, axis in values:
        if axis not in (0, 1, 2):
            return None
        limit = axes["max"][axis] if value >= 0 else -axes["min"][axis]
        result[axis] = axes["sign"][axis] * (value * limit)
    return result


def hypothesis_angles(axes, values):
    """Reference 1, anim_humanoid_qualify's hypothesis: the axis sign first, then the limit by d's sign."""
    result = [0., 0., 0.]
    for value, axis in values:
        d = axes["sign"][axis] * value
        result[axis] = d * axes["max"][axis] if d >= 0 else d * -axes["min"][axis]
    return result


def tangents(half_turns):
    """The platform's tan of each half angle: the one step whose bits are the C library's. A non-finite angle's
    is NaN, as C's `tan` gives it (Python's raises)."""
    return [math.tan(a / 2) if math.isfinite(a) else math.nan for a in half_turns]


def joint_rotation(axes, tangent):
    """None for a zero or NaN quaternion, as Rust's."""
    tx, ty, tz = tangent
    joint = normalize(multiply((0., ty, tz, 1.), (tx, 0., 0., 1.)))
    if joint is None:
        return None
    post = axes["post"]
    return normalize(multiply(multiply(axes["pre"], joint), (-post[0], -post[1], -post[2], post[3])))


def muscle_rotation(axes, values, rule=angles):
    turns = rule(axes, values)
    return None if turns is None else joint_rotation(axes, tangents(turns))


def bone_rotation(rest, axes, values, rule=angles):
    """A bone's local rotation; None where Rust's `bone_rotation` is None (a zero or NaN quaternion, which a
    non-finite muscle value gives)."""
    rotation = muscle_rotation(axes, values, rule)
    return None if rotation is None else normalize(multiply(rest, rotation))


def unity_bound(rest, axes, values, value_errors=()):
    """The derived bound on the angle between Unity's f32 local rotation and the exact one, in radians.
    `value_errors`, one per value, bound each muscle value's own error (a clip curve's f32 evaluation); each
    moves its angle by at most the larger limit's magnitude times the error, since an error can carry the
    value across 0, to the other limit's side."""
    tangents, errors = [], []
    moved = [0., 0., 0.]
    for (_, axis), error in zip(values, value_errors):
        moved[axis] = max(abs(axes["max"][axis]), abs(axes["min"][axis])) * error
    for a, shift in zip(angles(axes, values), moved):
        t = math.tan(a / 2)
        tangents.append(t)
        # The limit product's rounding and the value's own error through d tan(a/2)/da, then tanf's own ulp.
        errors.append((1 + t * t) / 2 * (abs(a) * U32 + shift) + 2 * abs(t) * U32)
    norm = math.hypot

    def product(a, error_a, b, error_b):
        return norm(*a) * error_b + norm(*b) * error_a + 2 * 4 * U32 / (1 - 4 * U32) * norm(*a) * norm(*b)

    def unit(q, error):
        return 2 * error / norm(*q) + 4 * U32

    tx, ty, tz = tangents
    swing, twist = (0., ty, tz, 1.), (tx, 0., 0., 1.)
    raw = multiply(swing, twist)
    error = unit(raw, product(swing, errors[1] + errors[2], twist, errors[0]))
    joint = normalize(raw)
    post = axes["post"]
    error = product(axes["pre"], 0, joint, error)
    turned = multiply(axes["pre"], joint)
    error = product(turned, error, post, 0)
    raw = multiply(turned, (-post[0], -post[1], -post[2], post[3]))
    error = unit(raw, error)
    error = unit(multiply(rest, normalize(raw)), product(rest, 0, normalize(raw), error))
    return 4 * math.asin(min(1., error / 2)) + replay.F64_ANGLE_FLOOR


def bone_muscles(bone):
    return [m for m in range(MUSCLE_COUNT) if muscle_bone(m) == bone]


def schedule(bone):
    """The known vectors on one bone, as (value, axis) lists: each muscle alone at every value, then jointly."""
    muscles = bone_muscles(bone)
    vectors = [[(f32(value), MUSCLE_AXES[m])] for m in muscles for value in VALUES]
    vectors += [[(f32(values[k % 3]), MUSCLE_AXES[m]) for k, m in enumerate(muscles)] for values in COMBINED]
    vectors += [[(f32(2 * _unit(f"vector/{bone}/{n}/{m}") - 1), MUSCLE_AXES[m]) for m in muscles] for n in range(2)]
    return vectors


def _unit(label):
    return int.from_bytes(hashlib.sha256(label.encode()).digest()[:8], "big") / 2 ** 64


def _quaternion(label):
    q = [2 * _unit(f"{label}/{k}") - 1 for k in range(4)]
    return [f32(x) for x in normalize(q)]


def synthetic_axes(bone):
    """Deterministic f32 axes for a bone: unit pre and post rotations, signs of both kinds, and limits that are
    zero, symmetric or not, out to pi."""
    label = f"muscle-pose/{bone}"
    limits = []
    for axis in range(3):
        kind = _unit(f"{label}/limit/{axis}")
        low, high = -math.pi * _unit(f"{label}/min/{axis}"), math.pi * _unit(f"{label}/max/{axis}")
        limits.append((0., 0.) if kind < .15 else (-math.pi, math.pi) if kind < .25 else (low, -low) if kind < .45
                      else (low, high))
    return dict(pre=_quaternion(label + "/pre"), post=_quaternion(label + "/post"),
                sign=[-1. if _unit(f"{label}/sign/{a}") < .5 else 1. for a in range(3)],
                min=[f32(low) for low, _ in limits], max=[f32(high) for _, high in limits])


def f32_hexes(values):
    return [replay.f32_bits(v) for v in values]


def vector_row(rest, axes, values, recorded=None, value_errors=()):
    turns = angles(axes, values)
    local = bone_rotation(rest, axes, values)
    replay.require(local is not None, f"no rotation for muscle values {values}")
    row = dict(values=[[replay.f32_bits(v), axis] for v, axis in values], angles=[f64_hex(v) for v in turns],
               tangents=[f64_hex(v) for v in tangents(turns)],
               local=[f64_hex(v) for v in local])
    if recorded is not None:
        row.update(recorded=f32_hexes(recorded), bound=unity_bound(rest, axes, values, value_errors))
    return row


def axes_row(axes):
    return {key: f32_hexes(axes[key]) for key in ("pre", "post", "sign", "min", "max")}


def axes_of(row):
    return {key: [replay.f32_from_bits(v) for v in row[key]] for key in ("pre", "post", "sign", "min", "max")}


def goldens():
    bones = sorted({muscle_bone(m) for m in range(MUSCLE_COUNT)})
    entries = []
    for bone in bones:
        axes, rest = synthetic_axes(bone), _quaternion(f"muscle-pose/{bone}/rest")
        entries.append(dict(bone=bone, axes=axes_row(axes), rest=f32_hexes(rest),
                            vectors=[vector_row(rest, axes, values) for values in schedule(bone)]))
    return dict(format=FORMAT, kind="goldens", reference=REFERENCE, muscle_axes=list(MUSCLE_AXES), entries=entries)


def dump(document):
    return json.dumps(document, separators=(",", ":"), ensure_ascii=False) + "\n"


FINGER_NAMES = ("Thumb", "Index", "Middle", "Ring", "Little")


def muscle_property(name):
    """The Animator curve property Unity binds for a HumanTrait muscle: `LeftHand.Thumb.1 Stretched` for
    `Left Thumb 1 Stretched`, a body muscle's own name."""
    side, _, rest = name.partition(" ")
    finger, _, part = rest.partition(" ")
    return f"{side}Hand.{finger}.{part}" if side in ("Left", "Right") and finger in FINGER_NAMES and part else name


def clip_values(doc, sample, names):
    """A clip-playback sample's muscle values from the clip's own authored curves at its time on the clip's
    source clock (`effective_source`, the time the clip is sampled at, not the playable's `local`), and each
    one's error bound: Unity's f32 curve evaluation (anim_body_model.curve's operand magnitude at gamma_12), the
    time's own f32 rounding, and the f32 rounding of the value itself. A muscle without a curve is exactly 0."""
    from anim_body_model import Refused, curve

    authored = {row["property"]: row["keys"] for row in doc["spec"]["evidence"]["editor"]["bindings"]
                if (row["path"], row["type"]) == ("", "UnityEngine.Animator") and row.get("keys")}
    time = sample.get("effective_source")
    replay.require(type(time) in (int, float) and math.isfinite(time), "a clip sample without a source time")
    time_f32 = f32(time)
    gamma = 12 * U32 / (1 - 12 * U32)
    values, errors = [], []
    for name in names:
        keys = authored.get(muscle_property(name))
        if keys is None:
            values.append(0.)
            errors.append(0.)
            continue
        try:
            value, magnitude = curve(keys, time)
            other, _ = curve(keys, time_f32)
        except Refused as error:
            raise ValueError(f"{name}: {error}") from None
        # Unity's value is an f32, as each row stores it: the rounding joins the error.
        values.append(f32(value))
        errors.append(gamma * magnitude + abs(value - other) + abs(value) * U32)
    return values, errors


def cases(unityrs, folders):
    """Real-Avatar cases from training runs: each bone's recorded samples, known vectors and clip playback alike,
    and the schedule on its axes. A round 11 run (`round.json`) adds its inventory (anim_muscle_round11: the
    sweeps and full-body clips), and its entries are confirmation, never acceptance. Each Editor's required
    cases and the ones its runs lack are recorded for `agree`."""
    import anim_body_runs as runs
    import anim_muscle_round11 as round11
    from anim_body_freeze import readers
    from anim_fixture_coverage import REFUSALS
    from anim_humanoid_qualify import vec

    avatar_of, _ = readers(unityrs)
    entries, generated, required, missing = [], {}, {}, {}
    for folder in folders:
        run = runs.Run(folder, "training", pending=True)
        editor = run.version.split(".")[0]
        confirmation = round11.declared_round(folder) == round11.ROUND
        documents = dict(run.documents)
        missing.setdefault(editor, set()).update(run.missing)
        if confirmation:
            extra = round11.Recordings(folder)
            replay.require(not documents.keys() & extra.documents.keys(), f"{folder.name}: a case is read twice")
            documents |= extra.documents
            required.setdefault(editor, set()).update(extra.required)
            missing[editor].update(extra.missing)
        for case, doc in sorted(documents.items()):
            capture = doc["capture"]
            if not capture["samples"] or "humanoid" not in capture:
                continue
            known = bool(capture["samples"][0].get("muscles_f32"))
            playback = not known and doc["spec"]["mode"] == "animator" and bool(doc["spec"]["clip_asset"])
            if not (known or playback):
                continue
            muscles = capture["humanoid"]["muscles"]
            replay.require([(row["bone"], row["axis"]) for row in muscles]
                           == [(muscle_bone(m), MUSCLE_AXES[m]) for m in range(MUSCLE_COUNT)],
                           f"{case}: the recorded muscle axes differ from MUSCLE_AXES")
            names = [row["name"] for row in muscles]
            sampled = []
            for sample in capture["samples"]:
                if known:
                    sampled.append(([replay.f32_from_bits(v) for v in sample["muscles_f32"]], [0.] * MUSCLE_COUNT))
                else:
                    sampled.append(clip_values(doc, sample, names))
            human = runs.human_of(avatar_of(run, doc))
            reason = refusal_reason(case, human, REFUSALS)
            skeleton = human["m_Skeleton"].get("data", human["m_Skeleton"])
            bones = capture["humanoid"]["bones"]
            for bone in sorted({row["bone"] for row in muscles}):
                # A bone the capture marks absent is skipped before the Avatar is indexed for it.
                if bones[bone]["node"] < 0:
                    continue
                node = avatar_node(human, bone)
                if node is None or skeleton["m_Node"][node]["m_AxesId"] < 0:
                    continue
                record = skeleton["m_AxesArray"][skeleton["m_Node"][node]["m_AxesId"]]
                axes = {key: [f32(v) for v in vec(record[name])] for key, name in
                        (("pre", "m_PreQ"), ("post", "m_PostQ"), ("sign", "m_Sgn"))}
                axes |= {key: [f32(v) for v in vec(record["m_Limit"][name])] for key, name in
                         (("min", "m_Min"), ("max", "m_Max"))}
                transforms = capture["rest"]["transforms"]
                rest = [replay.f32_from_bits(v) for v in transforms[bones[bone]["node"]]["rotation"]]
                ordinals = bone_muscles(bone)
                rows, seen = [], set()
                for sample, (vector, value_errors) in zip(capture["samples"], sampled):
                    values = [(vector[m], MUSCLE_AXES[m]) for m in ordinals]
                    recorded = [replay.f32_from_bits(v) for v in sample["pose"]["transforms"][bones[bone]["node"]]
                                ["rotation"]]
                    key = (tuple(map(f64_hex, (v for v, _ in values))), tuple(recorded))
                    if key not in seen:
                        seen.add(key)
                        rows.append(vector_row(rest, axes, values, recorded, [value_errors[m] for m in ordinals]))
                entries.append(dict(editor=editor, case=case, bone=bone, name=bones[bone]["name"],
                                    playback=playback, refusal=reason is not None, refusal_reason=reason,
                                    confirmation=confirmation,
                                    axes=axes_row(axes),
                                    rest=f32_hexes(rest), vectors=rows))
                generated.setdefault((editor, bone, json.dumps(axes_row(axes))), (axes, rest))
    for (editor, bone, _), (axes, rest) in sorted(generated.items()):
        entries.append(dict(editor=editor, case="generated", bone=bone, axes=axes_row(axes), rest=f32_hexes(rest),
                            vectors=[vector_row(rest, axes, values) for values in schedule(bone)]))
    return dict(format=FORMAT, kind="cases", reference=REFERENCE, muscle_axes=list(MUSCLE_AXES),
                required={editor: sorted(cases) for editor, cases in sorted(required.items())},
                missing={editor: sorted(cases) for editor, cases in sorted(missing.items())}, entries=entries)


def refusal_reason(case, human, refusals):
    """Why a case's samples are refusal evidence, or None when they are in B5a's scope. B5a claims all 95 muscles,
    limb twists included, on Avatars whose four twist settings are neutral (0, `humanoid::NEUTRAL_TWIST`): the
    runtime refuses a clip on any other Avatar that animates a twisted limb (`unsupported_humanoid_twist`), and
    Unity spreads a limb's twist over its neighbours there, which one bone's equation does not model. A coverage
    refusal case (anim_fixture_coverage.REFUSALS) is refusal evidence too."""
    from anim_humanoid_qualify import TWIST

    if case in refusals:
        return refusals[case]
    values = [human.get(field) for field in TWIST]
    replay.require(all(type(v) in (int, float) for v in values), f"{case}: the Avatar lacks a twist setting")
    return None if all(v == 0 for v in values) else "unsupported_humanoid_twist"


def avatar_node(human, bone):
    """The Avatar's human skeleton node of a HumanBodyBones index, or None, as Rust's lookup: a finger of a hand
    the Avatar does not map (an empty `m_HandBoneIndex`) is None, never indexed."""
    from anim_humanoid_qualify import bone_node

    if 24 <= bone <= 53:
        hand = human[("m_LeftHand", "m_RightHand")[(bone - 24) // 15]]
        if len(hand.get("data", hand)["m_HandBoneIndex"]) <= (bone - 24) % 15:
            return None
    return bone_node(human, bone)


def agreement(document, refusal=False, confirmation=False):
    """{(editor, bone name): (worst angle, its bound, samples, samples outside, rows reference 2 changes from
    reference 1, samples outside under reference 1)} over recorded vectors: acceptance's (rounds 1 to 10), with
    `confirmation` round 11's, or with `refusal` the refusal evidence of any round (`refusal_reason`: a coverage
    refusal case, or any case on an Avatar with a non-neutral twist setting, whose twist Unity spreads over
    neighbouring bones), reported apart and never part of acceptance or confirmation, whether it agrees or not."""
    result = {}
    for entry in document["entries"]:
        if entry.get("refusal", False) != refusal or not refusal and entry.get("confirmation", False) != confirmation:
            continue
        axes, rest = axes_of(entry["axes"]), [replay.f32_from_bits(v) for v in entry["rest"]]
        for row in entry["vectors"]:
            if "recorded" not in row:
                continue
            recorded = [replay.f32_from_bits(v) for v in row["recorded"]]
            values = [(replay.f32_from_bits(v), axis) for v, axis in row["values"]]
            local = [f64_of(v) for v in row["local"]]
            before = bone_rotation(rest, axes, values, hypothesis_angles)
            turn, earlier = angle(local, recorded), angle(before, recorded)
            key = (entry["editor"], entry["name"])
            worst, bound, count, outside, changed, was = result.get(key, (0., 0., 0, 0, 0, 0))
            if turn > worst:
                worst, bound = turn, row["bound"]
            result[key] = (worst, bound, count + 1, outside + (turn > row["bound"]),
                           changed + (list(map(f64_hex, before)) != row["local"]), was + (earlier > row["bound"]))
    return result


def regressions(rows):
    """The bones reference 1 agreed on whose recorded rows reference 2 changes: none is allowed."""
    return sorted(key for key, (_, _, _, _, changed, was) in rows.items() if not was and changed)


EDITORS = ("2022", "6000")


def failures(document):
    """Why `agree` fails, empty when it passes: an Editor with no comparison; an acceptance or confirmation
    sample outside its bound, on any bone (one reference 1 already failed included); a required case missing
    from its run or with no compared sample; a regression from reference 1. The refusal table never counts."""
    found = []
    tables = {"acceptance": agreement(document), "confirmation": agreement(document, confirmation=True)}
    for title, table in tables.items():
        outside = [f"{editor} {name} in {rows[3]}" for (editor, name), rows in sorted(table.items()) if rows[3]]
        if outside:
            found.append(f"{title} samples outside their bound: " + ", ".join(outside))
    for editor in EDITORS:
        if not sum(rows[2] for table in tables.values() for (e, _), rows in table.items() if e == editor):
            found.append(f"no comparison in Editor {editor}")
    for editor, cases in sorted(document["missing"].items()):
        if cases:
            found.append(f"Editor {editor}'s runs lack {len(cases)} required case(s): " + ", ".join(cases[:20]))
    for editor, cases in sorted(document["required"].items()):
        # Only an in-scope sample confirms a required case; refusal evidence never does.
        compared = {entry["case"] for entry in document["entries"] if entry["editor"] == editor
                    and not entry.get("refusal", False) and any("recorded" in row for row in entry["vectors"])}
        lacking = sorted(set(cases) - compared)
        if lacking:
            found.append(f"Editor {editor} compares no sample of {len(lacking)} required case(s): "
                         + ", ".join(lacking[:20]))
    if regressions(tables["acceptance"]):
        found.append(f"reference 2 changes bones reference 1 agreed on: {regressions(tables['acceptance'])}")
    return found


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="mode", required=True)
    golden = sub.add_parser("golden")
    golden.add_argument("--check", action="store_true", help="fail unless the committed goldens are these")
    made = sub.add_parser("cases")
    made.add_argument("--unityrs", required=True)
    made.add_argument("--out", type=Path, required=True)
    made.add_argument("folders", type=Path, nargs="+")
    agree = sub.add_parser("agree")
    agree.add_argument("file", type=Path)
    args = parser.parse_args()
    try:
        if args.mode == "golden":
            text = dump(goldens())
            if args.check:
                replay.require(GOLDENS.read_text(encoding="utf-8") == text, f"{GOLDENS.name} is stale")
                print(f"{GOLDENS.name} holds the goldens")
            else:
                GOLDENS.write_text(text, encoding="utf-8")
        elif args.mode == "cases":
            args.out.write_text(dump(cases(str(Path(args.unityrs).resolve()), args.folders)), encoding="utf-8")
        else:
            document = replay.read_json(args.file.read_text(encoding="utf-8"))
            replay.require(document.get("reference") == REFERENCE, f"{args.file.name} is not reference {REFERENCE}'s")
            replay.require("required" in document and "missing" in document,
                           f"{args.file.name} records no required cases: make it again with `cases`")
            for title, table in (("Acceptance (rounds 1 to 10)", agreement(document)),
                                 ("Confirmation (round 11)", agreement(document, confirmation=True)),
                                 ("Refusal cases (not acceptance)", agreement(document, True))):
                print(f"{title}:")
                for (editor, name), (worst, bound, count, outside, changed, was) in sorted(
                        table.items(), key=lambda kv: (kv[0][1] != "Spine", kv[0][1], kv[0][0])):
                    verdict = "agrees" if not outside else f"DISAGREES in {outside}"
                    print(f"{editor} {name:16s} worst {math.degrees(worst):.3e} deg  bound "
                          f"{math.degrees(bound):.3e} deg  {count} samples  {verdict}  ({changed} changed from "
                          f"reference 1, outside in {was})")
            found = failures(document)
            replay.require(not found, "; ".join(found))
    except (ValueError, OSError) as exc:
        parser.exit(1, f"anim_muscle_pose: {exc}\n")


if __name__ == "__main__":
    sys.exit(main())
