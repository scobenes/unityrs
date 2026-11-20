"""B5a's muscle pose: the f64 reference with its floating-point order written out, the goldens Rust's
`humanoid::muscle_pose` must equal bit for bit, and its agreement with Unity's recordings.

The equation is `anim_humanoid_qualify.muscle_rotation`'s (crates/unityrs-anim/src/humanoid/muscle_pose.rs
states it), with each sum's order fixed: a quaternion product's components left to right as `multiply` writes
them, and a norm as `sqrt(((x*x + y*y) + z*z) + w*w)`. CPython's `sum` adds that way up to 3.11 only (from 3.12
it compensates), so this module writes the order out instead of calling `sum`, and gives the same bits under
any Python. `tan` is the platform's C library's, in Python and in Rust alike, and the one step whose bits differ
between platforms (glibc's and Apple's in the last bit for about one angle in ten): each row records its angles,
their tangents and the local rotation, so Rust checks the angles bit for bit anywhere, the tangents where its
`tan` is glibc's (CI), and the rest bit for bit given the tangents. The goldens are generated in the container.

    python3 -I tools/parity/anim_muscle_pose.py golden [--check]
        The goldens (GOLDENS): synthetic axes for every bone a muscle turns, each muscle alone at every value
        of VALUES, each bone's muscles jointly, as the f64 bits of the angles, tangents and local rotation.
    python3 -I tools/parity/anim_muscle_pose.py cases --unityrs PATH --out FILE <run folder>...
        Real-Avatar cases from training runs (never a hold-out recording): every recorded known vector and
        clip sample, each with Unity's recorded local rotation and its derived bound, and the golden schedule
        on each distinct real axes record (generate them in the container, whose `tan` is CI's). They stay
        private; `UNITYRS_MUSCLE_POSE_CASES=FILE cargo test`
        checks Rust against them.
    python3 -I tools/parity/anim_muscle_pose.py agree FILE
        Each bone's worst angle to Unity's recording against its bound, the Spine first.

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
    norm = math.sqrt(((q[0] * q[0] + q[1] * q[1]) + q[2] * q[2]) + q[3] * q[3])
    replay.require(norm > 0, "zero quaternion")
    return tuple(x / norm for x in q)


def angles(axes, values):
    result = [0., 0., 0.]
    for value, axis in values:
        d = axes["sign"][axis] * value
        result[axis] = d * axes["max"][axis] if d >= 0 else d * -axes["min"][axis]
    return result


def tangents(half_turns):
    """The platform's tan of each half angle: the one step whose bits are the C library's."""
    return [math.tan(a / 2) for a in half_turns]


def joint_rotation(axes, tangent):
    tx, ty, tz = tangent
    joint = normalize(multiply((0., ty, tz, 1.), (tx, 0., 0., 1.)))
    post = axes["post"]
    return normalize(multiply(multiply(axes["pre"], joint), (-post[0], -post[1], -post[2], post[3])))


def muscle_rotation(axes, values):
    return joint_rotation(axes, tangents(angles(axes, values)))


def bone_rotation(rest, axes, values):
    return normalize(multiply(rest, muscle_rotation(axes, values)))


def unity_bound(rest, axes, values):
    """The derived bound on the angle between Unity's f32 local rotation and the exact one, in radians."""
    tangents, errors = [], []
    for a in angles(axes, values):
        t = math.tan(a / 2)
        tangents.append(t)
        # The limit product's rounding through d tan(a/2)/da, then tanf's own ulp.
        errors.append((1 + t * t) / 2 * abs(a) * U32 + 2 * abs(t) * U32)
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


def vector_row(rest, axes, values, recorded=None):
    turns = angles(axes, values)
    row = dict(values=[[replay.f32_bits(v), axis] for v, axis in values], angles=[f64_hex(v) for v in turns],
               tangents=[f64_hex(v) for v in tangents(turns)],
               local=[f64_hex(v) for v in bone_rotation(rest, axes, values)])
    if recorded is not None:
        row.update(recorded=f32_hexes(recorded), bound=unity_bound(rest, axes, values))
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
    return dict(format=FORMAT, kind="goldens", muscle_axes=list(MUSCLE_AXES), entries=entries)


def dump(document):
    return json.dumps(document, separators=(",", ":"), ensure_ascii=False) + "\n"


def cases(unityrs, folders):
    """Real-Avatar cases from training runs: each bone's recorded samples, and the schedule on its axes."""
    import anim_body_runs as runs
    from anim_body_freeze import readers
    from anim_humanoid_qualify import bone_node, vec

    avatar_of, _ = readers(unityrs)
    entries, generated = [], {}
    for folder in folders:
        run = runs.Run(folder, "training", pending=True)
        editor = run.version.split(".")[0]
        for case, doc in sorted(run.documents.items()):
            capture = doc["capture"]
            if not capture["samples"] or not capture["samples"][0].get("muscles_f32"):
                continue
            muscles = capture["humanoid"]["muscles"]
            replay.require([(row["bone"], row["axis"]) for row in muscles]
                           == [(muscle_bone(m), MUSCLE_AXES[m]) for m in range(MUSCLE_COUNT)],
                           f"{case}: the recorded muscle axes differ from MUSCLE_AXES")
            human = runs.human_of(avatar_of(run, doc))
            skeleton = human["m_Skeleton"].get("data", human["m_Skeleton"])
            bones = capture["humanoid"]["bones"]
            for bone in sorted({row["bone"] for row in muscles}):
                node = bone_node(human, bone)
                if node is None or bones[bone]["node"] < 0 or skeleton["m_Node"][node]["m_AxesId"] < 0:
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
                for sample in capture["samples"]:
                    values = [(replay.f32_from_bits(sample["muscles_f32"][m]), MUSCLE_AXES[m]) for m in ordinals]
                    recorded = [replay.f32_from_bits(v) for v in sample["pose"]["transforms"][bones[bone]["node"]]
                                ["rotation"]]
                    key = (tuple(sample["muscles_f32"][m] for m in ordinals), tuple(recorded))
                    if key not in seen:
                        seen.add(key)
                        rows.append(vector_row(rest, axes, values, recorded))
                entries.append(dict(editor=editor, case=case, bone=bone, name=bones[bone]["name"],
                                    axes=axes_row(axes), rest=f32_hexes(rest), vectors=rows))
                generated.setdefault((editor, bone, json.dumps(axes_row(axes))), (axes, rest))
    for (editor, bone, _), (axes, rest) in sorted(generated.items()):
        entries.append(dict(editor=editor, case="generated", bone=bone, axes=axes_row(axes), rest=f32_hexes(rest),
                            vectors=[vector_row(rest, axes, values) for values in schedule(bone)]))
    return dict(format=FORMAT, kind="cases", muscle_axes=list(MUSCLE_AXES), entries=entries)


def agreement(document):
    """{(editor, bone name): (worst angle, its bound, samples, samples outside)} over recorded vectors."""
    result = {}
    for entry in document["entries"]:
        for row in entry["vectors"]:
            if "recorded" not in row:
                continue
            local = [f64_of(v) for v in row["local"]]
            turn = angle(local, [replay.f32_from_bits(v) for v in row["recorded"]])
            key = (entry["editor"], entry["name"])
            worst, bound, count, outside = result.get(key, (0., 0., 0, 0))
            if turn > worst:
                worst, bound = turn, row["bound"]
            result[key] = (worst, bound, count + 1, outside + (turn > row["bound"]))
    return result


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
            rows = agreement(replay.read_json(args.file.read_text(encoding="utf-8")))
            for (editor, name), (worst, bound, count, outside) in sorted(
                    rows.items(), key=lambda kv: (kv[0][1] != "Spine", kv[0][1], kv[0][0])):
                verdict = "agrees" if not outside else f"DISAGREES in {outside}"
                print(f"{editor} {name:16s} worst {math.degrees(worst):.3e} deg  bound {math.degrees(bound):.3e} deg"
                      f"  {count} samples  {verdict}")
    except (ValueError, OSError) as exc:
        parser.exit(1, f"anim_muscle_pose: {exc}\n")


if __name__ == "__main__":
    sys.exit(main())
