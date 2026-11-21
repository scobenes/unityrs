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

Vectors name their muscles (HumanTrait names); every value and time is f32 bits. Run with `--check` (the committed
file is current) or `--write`.
"""

import argparse
import hashlib
import json
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

    def case(name, family, rig, tuple_name, set_name, partition, times):
        cases.append(dict(name=name, family=family, rig=rig, tuple=tuple_name, set=set_name, partition=partition,
                          times=[bits(t) for t in times]))
    for rig in SWEEP_RIGS:
        name = f"sweep-{rig}"
        case(f"body-twist-{name}", "sweep", rig, "t0", name, "confirmation", scheduled(len(sets[name])))
    for name, (rig, tuple_name, kind, part) in tables.cases().items():
        family = "toe-absent" if kind == "toe-absent" else "probe"
        case(name, family, rig, tuple_name, kind, part, scheduled(len(sets[kind])))
    for rig, tuple_name in SUBSET:
        keys = [k / 16 for k in range(len(sets["subset"]))]
        case(f"body-twist-clip-{rig}-{tuple_name}", "subset", rig, tuple_name, "subset", "confirmation",
             clip_times(keys))
    for name, family, rig, variant, _ in full_clips():
        case(name, family, rig, "t5" if family == "twist-clip" else "t0", "", "confirmation",
             clip_times(clip_keys(rig, variant)))
    return dict(format=FORMAT, version=1, tables_digest=tables.digest(),
                tuples=[dict(name=n, bits=[bits(v) for v in t]) for n, t in tables.TUPLES.items()],
                names=[dict(name=n, ordinal=m) for n, m in sorted(tables.NAMES.items(), key=lambda row: row[1])],
                sets=[dict(name=n, vectors=_rows(v, names)) for n, v in sets.items()], cases=cases)


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
    args = parser.parse_args()
    text = render_cs()
    if args.write:
        CS_PATH.write_text(text, encoding="utf-8", newline="\n")
        return 0
    if CS_PATH.read_text(encoding="utf-8") != text:
        parser.exit(1, f"{CS_PATH.name} is stale: run anim_round12_tables.py --write\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
