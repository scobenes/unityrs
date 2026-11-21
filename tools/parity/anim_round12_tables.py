"""Round 12's generated tables (round 12 design v4 section 3): every case the round 12 builder records, its vectors
and its sample times, as one canonical JSON document the C# builder carries verbatim.

`render_cs` writes it into tools/fixtures/unity/Editor/UnityrsRound12Tables.cs with its sha256. The builder checks
that sha256 before it reads a byte, builds every case from the document, re-derives what its own code computes
(each SetHumanPose schedule's times, the sweeps' B5a vectors, the clip cases' names and key times) and refuses a
difference before anything is built; at the end of the session it checks every capture's cases, sample count and
times against the document (inventory.json). The families:

- **sweep**: B5a's body sweeps (`body-probes-muscles-{compact,tall}`'s vectors, anim_fixture_muscles) at all four
  settings 0, plus each limb group's two `MUSCLE_COMBINED` vectors with its twist muscle left at 0;
- **probe** and **toe-absent**: anim_twist_tables' SetHumanPose probes, each tuple its own Avatar;
- **subset**: the core vectors as clip keys (key k at k/16 s, the zero vector first, Linear tangents) on compact
  and tall at t0, t1 and m1, sampled on the recorder's clip schedule (keys and one off-key sample per interval);
- **twist-clip** and **twin**: round 11's 318 in-scope full-body clips on their rigs, at t = .5 (curves unchanged)
  and at t = 0 without the ten limb-twist curves.

Vectors name their muscles (HumanTrait names); every value and time is f32 bits. Each clip case names its muscle
binding list (`bindings`: a full-body rig's, its twin's without the ten limb-twist curves, the subset's), which the
builder requires of every clip it authors. `curves` holds what those clips author, so the committed file pins it too:
every muscle's in-scope full-body curve (anim_fixture_full.full_values, at its HumanTrait ordinal), the subset's
curves, each with its key values and its intervals' Linear slopes, and the body (RootT, RootQ) and motion (MotionT,
MotionQ) curves' key values. The run writes every clip's curves to authored.json (each binding's key times, values,
tangents, weights, weighted modes and tangent modes); `check_authored` compares it with `curves`. Run with `--check`
(the committed file is current), `--write`, or `--authored FILE` (a run's authored.json).
"""

import argparse
import hashlib
import json
import math
import struct
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import anim_twist_tables as tables  # noqa: E402

FORMAT = "unityrs.round12_tables"
CS_PATH = Path(__file__).resolve().parents[1] / "fixtures" / "unity" / "Editor" / "UnityrsRound12Tables.cs"
SWEEP_RIGS = ("compact", "tall")
SUBSET = (("compact", "t0"), ("compact", "t1"), ("compact", "m1"), ("tall", "t0"), ("tall", "t1"), ("tall", "m1"))
# The recorder's off-key sample, this far into each interval (UnityrsFixtureRecording.cs Schedule, Scheduled).
OFFSET = 391 / 1024
ANIMATOR = "UnityEngine.Animator"
# HumanTrait.MuscleName's head, eye and jaw muscles (ordinals 0 to 20) and shoulders (37, 38; the right's 46, 47).
HEAD_MUSCLES = ("Spine Front-Back", "Spine Left-Right", "Spine Twist Left-Right", "Chest Front-Back", "Chest Left-Right",
                "Chest Twist Left-Right", "UpperChest Front-Back", "UpperChest Left-Right", "UpperChest Twist Left-Right",
                "Neck Nod Down-Up", "Neck Tilt Left-Right", "Neck Turn Left-Right", "Head Nod Down-Up",
                "Head Tilt Left-Right", "Head Turn Left-Right", "Left Eye Down-Up", "Left Eye In-Out", "Right Eye Down-Up",
                "Right Eye In-Out", "Jaw Close", "Jaw Left-Right")
SHOULDERS = {"Left Shoulder Down-Up": 37, "Left Shoulder Front-Back": 38, "Right Shoulder Down-Up": 46,
             "Right Shoulder Front-Back": 47}


def bits(value):
    return struct.pack(">f", value).hex()


def f32(value):
    return struct.unpack(">f", struct.pack(">f", value))[0]


def scheduled(count):
    """The builder's Scheduled times for `count` vectors (UnityrsBodyFixtures.cs): anchors at 0, .5 and 1 around
    the two halves, every time on a 2^-16 grid (round half to even, as C#'s Math.Round)."""
    first = (count + 1) // 2
    second = count - first
    times = [0.] + [.5 * (k + OFFSET) / first for k in range(first)] + [.5]
    times += [.5 + .5 * (k + OFFSET) / second for k in range(second)] + [1.]
    return [round(t * 65536) / 65536 for t in times]


def clip_times(keys):
    from anim_muscle_confirm import clip_times as recorder

    return list(recorder(keys))


def sweep_vectors(rig):
    """{name: value} vectors of a round 12 sweep: B5a's, then each limb group's joint vectors without its twist."""
    from anim_fixture_body import PROBE_GROUPS
    from anim_fixture_muscles import LIMB_TWISTS, MUSCLE_COMBINED, groups, muscle_vectors
    from anim_fixture_round11 import rig_bones

    name, mapped = f"body-probes-muscles-{rig}", rig_bones(rig)
    names = {muscle: muscle for _, group in PROBE_GROUPS for muscle in group} | {m: m for m, _ in LIMB_TWISTS}
    vectors = muscle_vectors(name, names, PROBE_GROUPS, mapped)
    twists = {muscle for muscle, _ in LIMB_TWISTS}
    for _, group in groups(name, PROBE_GROUPS, mapped):
        if twists & set(group):
            vectors += [{m: values[k % len(values)] for k, m in enumerate(group) if m not in twists}
                        for values in MUSCLE_COMBINED]
    return vectors


def subset_vectors():
    """The core vectors as clip keys: the zero vector first."""
    core = tables.core()
    return [{}] + [v for v in core if v]


def _named(vector, names):
    return {names[m] if isinstance(m, int) else m: v for m, v in vector.items()}


def _rows(vectors, names):
    rows = []
    for vector in vectors:
        named = sorted(_named(vector, names).items())
        rows.append(dict(muscles=[m for m, _ in named], bits=[bits(v) for _, v in named]))
    return rows


def binding_lists():
    """{list name: sorted Animator muscle properties}: each full-body clip rig's (its twist clips'), its twin's (the
    same without the ten limb-twist curves) and the clip subset's."""
    from anim_fixture_full import full_groups, muscle_property
    from anim_fixture_muscles import LIMB_TWISTS
    from anim_muscle_confirm import builder_muscles

    names, twists = list(builder_muscles()), {muscle_property(m) for m, _ in LIMB_TWISTS}
    found = {}
    for rig in sorted({row[2] for row in full_clips()}):
        properties = sorted(prop for (prop, *_), in full_groups(names, rig))
        found[rig], found[f"{rig}-twin"] = properties, [p for p in properties if p not in twists]
    limbs = {m: n for n, m in tables.NAMES.items()}
    found["subset"] = sorted({muscle_property(n) for v in subset_vectors() for n in _named(v, limbs)})
    return found


def humantrait_order():
    """HumanTrait.MuscleName's 95 names in order: the head muscles, the limbs at anim_twist_tables.NAMES' ordinals
    and the shoulders', then each side's fingers (anim_fixture_muscles.FINGER_GROUPS, HumanTrait's order). A run's
    authored.json carries Unity's own, which `check_authored` requires to be this."""
    from anim_fixture_muscles import FINGER_GROUPS
    from anim_muscle_confirm import builder_muscles

    order = [None] * 95
    order[:len(HEAD_MUSCLES)] = HEAD_MUSCLES
    for name, ordinal in (tables.NAMES | SHOULDERS).items():
        order[ordinal] = name
    order[55:] = [name for group in FINGER_GROUPS for name in group]
    if len(order) != 95 or sorted(order, key=str) != sorted(builder_muscles()):
        raise ValueError("the HumanTrait order is not the builder's 95 muscles")
    return order


def slope_bits(times, values):
    """Each interval's Linear slope, the f32 difference over the f32 step (AnimationUtility's
    CalculateLinearTangent), as f32 bits."""
    return [bits(f32(f32(values[i + 1] - values[i]) / f32(times[i + 1] - times[i]))) for i in range(len(times) - 1)]


def curve_sets(subset):
    """The curves round 12's clips author: `full` (every muscle's in-scope curve), `subset` (the subset's vector k at
    k/16 s, 0 where a vector leaves the muscle out), the body curves `root` and `root-start` (anim_fixture_body's path
    and turn, the start variant's turned and offset) and `motion`. A muscle row has its f32 key values and slopes; a
    body or motion row its key values (the builder computes them in f32, so `check_authored` allows 1e-5)."""
    from anim_fixture_body import BODY_PATH, BODY_TIMES, BODY_TURN, MOTION_PATH, MOTION_YAW, START_OFFSET, START_YAW
    from anim_fixture_full import full_values, muscle_property
    from anim_reference import euler, multiply

    def muscle_rows(times, curves):
        return [dict(row, values=[bits(v) for v in values], slopes=slope_bits(times, [f32(v) for v in values]))
                for row, values in curves]

    def vector_rows(prefix, axes, rows):
        return [dict(property=f"{prefix}.{axis}", values=[bits(row[i]) for row in rows]) for i, axis in enumerate(axes)]

    def root(start):
        turn = euler((0, START_YAW if start else 0, 0), 4)
        c, s = turn[3] ** 2 - turn[1] ** 2, 2 * turn[1] * turn[3]
        offset = START_OFFSET if start else (0, 0, 0)
        path = [(c * x + s * z + offset[0], y + offset[1], -s * x + c * z + offset[2]) for x, y, z in BODY_PATH]
        return (vector_rows("RootT", "xyz", path)
                + vector_rows("RootQ", "xyzw", [multiply(turn, euler(row, 4)) for row in BODY_TURN]))

    order = humantrait_order()
    body_times = [bits(t) for t in BODY_TIMES]
    full = [(dict(property=muscle_property(name), name=name, ordinal=m), full_values(m, name, True))
            for m, name in enumerate(order)]
    keys = [k / 16 for k in range(len(subset))]
    limbs = {m: n for n, m in tables.NAMES.items()}
    named = [_named(vector, limbs) for vector in subset]
    moved = sorted({muscle_property(n) for vector in named for n in vector})
    by_property = [{muscle_property(n): v for n, v in vector.items()} for vector in named]
    return [dict(name="full", times=body_times, rows=muscle_rows(BODY_TIMES, full)),
            dict(name="subset", times=[bits(t) for t in keys],
                 rows=muscle_rows(keys, [(dict(property=p), [v.get(p, 0.) for v in by_property]) for p in moved])),
            dict(name="root", times=body_times, rows=root(False)),
            dict(name="root-start", times=body_times, rows=root(True)),
            dict(name="motion", times=body_times, rows=vector_rows("MotionT", "xyz", MOTION_PATH) + vector_rows(
                "MotionQ", "xyzw", [euler((0, yaw, 0), 4) for yaw in MOTION_YAW]))]


def full_clips():
    """[(name, family, rig, variant, root)]: round 11's in-scope full-body clips, each as a twist clip and a twin."""
    import anim_fixture_round11 as r11

    found = []
    for family, tag in (("twist-clip", "twist"), ("twin", "twin")):
        for case, info in r11.ROUND11_INFO.items():
            if info["kind"] == "clip" and info.get("scope") == "in":
                suffix = "-offset" if info["root"] == "offset" else ""
                found.append((f"body-clip-{info['rig']}-{tag}-{info['variant']}{suffix}", family, info["rig"],
                              info["variant"], info["root"]))
    return found


def document():
    from anim_muscle_confirm import clip_keys

    names = {m: n for n, m in tables.NAMES.items()}
    sets = {f"sweep-{rig}": sweep_vectors(rig) for rig in SWEEP_RIGS}
    sets |= {name: make() for name, make in tables.SETS.items()}
    sets["subset"] = subset_vectors()
    cases = []

    def case(name, family, rig, tuple_name, set_name, partition, times, bindings=""):
        cases.append(dict(name=name, family=family, rig=rig, tuple=tuple_name, set=set_name, partition=partition,
                          bindings=bindings, times=[bits(t) for t in times]))
    for rig in SWEEP_RIGS:
        name = f"sweep-{rig}"
        case(f"body-twist-{name}", "sweep", rig, "t0", name, "confirmation", scheduled(len(sets[name])))
    for name, (rig, tuple_name, kind, part) in tables.cases().items():
        family = "toe-absent" if kind == "toe-absent" else "probe"
        case(name, family, rig, tuple_name, kind, part, scheduled(len(sets[kind])))
    for rig, tuple_name in SUBSET:
        keys = [k / 16 for k in range(len(sets["subset"]))]
        case(f"body-twist-clip-{rig}-{tuple_name}", "subset", rig, tuple_name, "subset", "confirmation",
             clip_times(keys), "subset")
    for name, family, rig, variant, _ in full_clips():
        case(name, family, rig, "t5" if family == "twist-clip" else "t0", "", "confirmation",
             clip_times(clip_keys(rig, variant)), rig if family == "twist-clip" else f"{rig}-twin")
    return dict(format=FORMAT, version=1, tables_digest=tables.digest(),
                tuples=[dict(name=n, bits=[bits(v) for v in t]) for n, t in tables.TUPLES.items()],
                names=[dict(name=n, ordinal=m) for n, m in sorted(tables.NAMES.items(), key=lambda row: row[1])],
                sets=[dict(name=n, vectors=_rows(v, names)) for n, v in sets.items()],
                bindings=[dict(name=n, properties=p) for n, p in binding_lists().items()],
                curves=curve_sets(sets["subset"]), cases=cases)


def expected_curves(row, doc, variant=None):
    """{Animator property: (times, values, slopes, tangent mode)} a clip case authors, from the tables' `curves`
    (f32 bits): its binding list's muscle curves (the subset's, or every muscle's in-scope curve), each with its
    slopes and Linear tangents; a full-body clip's (its `variant`'s) body curves too, and its motion curves if it has
    them, each with no slopes (its PolyLine keys' own) and Free tangents."""
    from anim_fixture_body import CLIP_VARIANTS

    sets = {curve["name"]: curve for curve in doc["curves"]}
    wanted = set({row2["name"]: row2["properties"] for row2 in doc["bindings"]}[row["bindings"]])
    muscles = sets["subset" if row["family"] == "subset" else "full"]
    found = {r["property"]: (muscles["times"], r["values"], r["slopes"], "Linear") for r in muscles["rows"]
             if r["property"] in wanted}
    if set(found) != wanted:
        raise ValueError(f"the tables' curves lack {sorted(wanted - set(found))}")
    if row["family"] != "subset":
        clip = CLIP_VARIANTS[variant]
        if clip["knee"] or clip["goals"]:
            raise ValueError(f"round 12 authors no knee or foot goal variant ({variant})")
        for name in ("root-start" if clip["start"] else "root",) + (("motion",) if clip["motion"] else ()):
            found |= {r["property"]: (sets[name]["times"], r["values"], None, "Free") for r in sets[name]["rows"]}
    return found


def _value(hex_bits):
    return struct.unpack(">f", bytes.fromhex(hex_bits))[0]


def _floats(curve, key, count):
    """A curve's f32 list `key` as floats, or None unless it is `count` f32 bit patterns."""
    texts = curve.get(key)
    if not isinstance(texts, list) or len(texts) != count:
        return None
    try:
        return [_value(text) for text in texts]
    except (TypeError, ValueError):
        return None


def _curve_problem(curve, times, values, slopes, mode):
    """Why an authored curve is not the tables' (None if it is): its key times, its key values (a muscle's bit for
    bit), unweighted keys with finite weights, `mode` tangents on both sides of every key, and each interval's
    tangents the slope between its keys (a muscle's the tables' slope), to a millionth."""
    n = len(times)
    got = {key: _floats(curve, key, n) for key in ("times", "values", "in_tangents", "out_tangents", "in_weights",
                                                     "out_weights")}
    modes = [curve.get(key) for key in ("weighted_modes", "left_modes", "right_modes")]
    if any(value is None for value in got.values()) or any(not isinstance(m, list) or len(m) != n for m in modes):
        return "keys are incomplete"
    if curve["times"] != times:
        return "key times differ from the tables'"
    if slopes is not None and curve["values"] != values:
        return "key values differ from the tables'"
    if any(m != "None" for m in modes[0]) or not all(map(math.isfinite, got["in_weights"] + got["out_weights"])):
        return "keys are weighted"
    if any(m != mode for m in modes[1] + modes[2]):
        return f"tangent modes are not {mode}"
    want = ([_value(b) for b in slopes] if slopes is not None
            else [_value(b) for b in slope_bits(got["times"], got["values"])])
    for i, slope in enumerate(want):
        for tangent in (got["out_tangents"][i], got["in_tangents"][i + 1]):
            if not (math.isfinite(tangent) and abs(tangent - slope) <= 1e-6 * max(1., abs(slope))):
                return "tangents are not its keys' slopes"
    return None


def _body_problem(curves, want):
    """Why the authored body or motion curves' values are not the tables' to 1e-5 (a rotation's at either sign), or
    None."""
    groups = {}
    for prop, (_, values, slopes, _) in want.items():
        if slopes is None:
            groups.setdefault(prop.split(".")[0], []).append(prop)
    for prefix, props in sorted(groups.items()):
        got = [[_value(b) for b in curves[prop]["values"]] for prop in props]
        expected = [[_value(b) for b in want[prop][1]] for prop in props]
        for k in range(len(expected[0])):
            g, e = [row[k] for row in got], [row[k] for row in expected]
            if not (max(abs(a - b) for a, b in zip(g, e)) <= 1e-5
                    or len(props) == 4 and max(abs(a + b) for a, b in zip(g, e)) <= 1e-5):
                return f"{prefix}'s key {k} differs from the tables'"
    return None


def check_authored(authored, doc=None):
    """The problems with a run's authored.json against the tables: every subset, twist-clip and twin case authored
    once; each clip's bindings, keyed by (path, type, property) and never twice, exactly its case's muscle list (a
    twin's no limb-twist curve) and, on a full-body clip, its body and motion curves; each curve's keys the tables'
    `curves`, unweighted, on its tangent mode, its tangents its slopes."""
    doc = document() if doc is None else doc
    problems = []
    names = authored.get("muscle_names") or []
    full = next(curve for curve in doc["curves"] if curve["name"] == "full")
    if len(names) != 95 or any(names[row["ordinal"]] != row["name"] for row in doc["names"] + full["rows"]):
        return ["authored.json's muscle names are not HumanTrait's (anim_round12_tables.humantrait_order)"]
    rows = {row["name"]: row for row in doc["cases"] if row["bindings"]}
    variants = {name: variant for name, _, _, variant, _ in full_clips()}
    seen = set()
    for clip in authored.get("clips") or []:
        label, cases = clip.get("clip"), clip.get("cases") or []
        if not cases or any(case not in rows or case in seen for case in cases):
            problems.append(f"{label}: cases {cases} are not unseen round 12 clip cases")
            continue
        seen |= set(cases)
        row = rows[cases[0]]
        if any((rows[case]["bindings"], rows[case]["rig"], variants.get(case))
               != (row["bindings"], row["rig"], variants.get(cases[0])) for case in cases):
            problems.append(f"{label}: its cases {cases} differ in rig, bindings or variant")
            continue
        curves, twice = {}, set()
        for b in clip.get("bindings") or []:
            key = (b.get("path"), b.get("type"), b.get("property"))
            if key in curves:
                twice.add(key)
            curves[key] = b
        if twice:
            problems.append(f"{label}: bindings {sorted(twice, key=str)} appear more than once")
            continue
        want = expected_curves(row, doc, variants.get(cases[0]))
        keys = {("", ANIMATOR, prop) for prop in want}
        if set(curves) != keys:
            problems.append(f"{label}: bindings {sorted(set(curves) - keys, key=str)} extra, "
                            f"{sorted(keys - set(curves), key=str)} missing (list {row['bindings']})")
            continue
        by_property = {prop: curves[("", ANIMATOR, prop)] for prop in want}
        found = [f"{label}: {prop}'s {problem}" for prop, (times, values, slopes, mode) in want.items()
                 if (problem := _curve_problem(by_property[prop], times, values, slopes, mode))]
        problems += found
        if not found and (problem := _body_problem(by_property, want)):
            problems.append(f"{label}: {problem}")
    problems += [f"{case}: no authored clip" for case in rows if case not in seen]
    return problems


def canonical(doc=None):
    """The document's bytes: compact JSON, keys in insertion order, ASCII only."""
    return json.dumps(document() if doc is None else doc, separators=(",", ":"), ensure_ascii=False).encode()


def render_cs(data=None):
    """UnityrsRound12Tables.cs: the document as C# verbatim string pieces (quotes doubled), one array element per
    line (an array, never a chain of thousands of `+`), and the sha256 of their concatenation."""
    data = canonical() if data is None else data
    text = data.decode()
    lines = [text[i:i + 110] for i in range(0, len(text), 110)]
    pieces = [line.replace('"', '""') for line in lines]
    body = "".join(f'        @"{piece}",\n' for piece in pieces)
    return ("#if UNITY_EDITOR\n"
            "// Generated by tools/parity/anim_round12_tables.py; do not edit. Round 12's cases, vectors and sample\n"
            "// times (anim_round12_tables.document), read by UnityrsRound12.cs.\n"
            "static partial class UnityrsFixtureBuilder\n{\n"
            f"    const string Round12TablesSha256 = \"{hashlib.sha256(data).hexdigest()}\";\n"
            "    static readonly string[] Round12TablesParts = {\n"
            f"{body}"
            "    };\n}\n#endif\n")


def parse_cs(source):
    """The document bytes a UnityrsRound12Tables.cs carries: the concatenation the C# compiler reads from its
    pieces."""
    start = source.index("Round12TablesParts = {\n") + len("Round12TablesParts = {\n")
    end = source.index("    };\n}", start)
    pieces = []
    for line in source[start:end].splitlines():
        if not (line.startswith('        @"') and line.endswith('",')):
            raise ValueError(f"not a verbatim piece: {line[:40]!r}")
        pieces.append(line[len('        @"'):-2].replace('""', '"'))
    return "".join(pieces).encode()


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--check", action="store_true")
    mode.add_argument("--write", action="store_true")
    mode.add_argument("--authored", metavar="FILE", help="a round 12 run's authored.json")
    args = parser.parse_args()
    if args.authored:
        problems = check_authored(json.loads(Path(args.authored).read_text(encoding="utf-8")))
        for problem in problems:
            print(problem)
        return 1 if problems else 0
    text = render_cs()
    if args.write:
        CS_PATH.write_text(text, encoding="utf-8", newline="\n")
        return 0
    if CS_PATH.read_text(encoding="utf-8") != text:
        parser.exit(1, f"{CS_PATH.name} is stale: run anim_round12_tables.py --write\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
