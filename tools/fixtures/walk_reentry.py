"""Reads that hold many values open at once: `ReferencedObject` data that
re-enters a wide ref type level after level, and vectors nested in vectors.
`walk.py` records them into `reentry.json` and, for the reads that succeed,
into `cases.json` (docs/plans/core.md 3.10, "Bounds").

Review round 7 found each class's dict reserving one entry a child before
it read any, so every level of a `ReferencedObject` re-entering a ref type of
`k` children held `k` entries while the next level was read: a few kilobytes
of object data charged gigabytes, and unityrs refused with `InputMemory`
where UnityPy raised `EOFError`. A dict now grows as its children are read.
Its sweep found each vector held open reserving up to `max_prealloc`
elements of its own before its bytes were read, so `levels` vectors nested
reserved `levels` times that; together they now reserve at most that once.

The ref type `R` (class `R`, namespace `N`, assembly `A`) is a recipe, so a
case stays small however wide `R` is: its root `Base` holds a
`ReferencedObject inner` (`rid`, `type { class, ns, asm }`, `data`), then
`ints` children of type `int` named `c0`, `c1` and so on, the last named
`c0` again when `dup`. The object is `Base { ReferencedObject obj }`, whose
data nests `levels` references to `R` (`obj`'s, then each `inner`'s).

- `reentry.json`, `"recipe": "ref"`: the data ends four bytes into the next
  `rid`, so UnityPy follows every level, reads no `int`, and raises where
  the bytes end. A case is `{"name", "recipe", "ints", "dup", "levels",
  "nodes", "hex", "boost", "python"}`: `nodes` the object's rows, `hex` its
  data, each outcome `walk.py`'s `{"err": exception class, "kind": the
  dump's L2 kind}`.
- `reentry.json`, `"recipe": "nested"`: `Base { v }`, `v` a vector of
  vectors `levels` deep whose innermost holds `int`s; its data `levels`
  counts of `count`, then `4 * count - 4` zero bytes, so each vector's count
  is backed by the bytes left after it but the innermost runs out four bytes
  short. A case is `{"name", "recipe", "levels", "count", "nodes", "boost",
  "python"}`; its bytes are made from the recipe.
- `cases.json` (`r14_reentry_...`): the innermost `inner` names an empty
  class, so its data is skipped, and every level then reads its `ints`; a
  case is `walk.py`'s, its ref types written out in full.

`walk.py` imports this module; it records nothing when run on its own.
"""

from __future__ import annotations

import struct

from walk_build import base, i32, leaf, run_case, s, shift, vec

ROOT = [[0, "Base", "Base", 0]]


def ref_object(name):
    """`ReferencedObject { SInt64 rid; ReferencedManagedType type { string class, ns, asm }; ReferencedObjectData data }`."""
    return [
        [0, "ReferencedObject", name, 0],
        [1, "SInt64", "rid", 0],
        [1, "ReferencedManagedType", "type", 0],
        [2, "string", "class", 0],
        [2, "string", "ns", 0],
        [2, "string", "asm", 0],
        [1, "ReferencedObjectData", "data", 0],
    ]


def ref_type(ints, dup):
    """`R`'s rows from its recipe."""
    names = [f"c{i}" for i in range(ints)]
    if dup:
        names[-1] = "c0"
    rows = ROOT + shift(ref_object("inner"), 1) + [[1, "int", n, 0] for n in names]
    return [{"class": "R", "ns": "N", "asm": "A", "nodes": rows}]


def ref(cls, rid):
    return struct.pack("<q", rid) + s(cls) + s("N") + s("A")


NODES = base(ref_object("obj"))


def eof_case(name, ints, dup, levels):
    data = b"".join(ref("R", rid) for rid in range(1, levels + 1)) + b"\0" * 4
    c = dict(name=name, nodes=NODES, hex=data.hex(), ref_types=ref_type(ints, dup))
    out = run_case(c)
    return {
        "name": name,
        "recipe": "ref",
        "ints": ints,
        "dup": dup,
        "levels": levels,
        "nodes": NODES,
        "hex": data.hex(),
        "boost": out["boost"],
        "python": out["python"],
    }


def nested_case(name, levels, count):
    rows = leaf("int", "data")
    for _ in range(levels - 1):
        rows = vec("data", rows)
    rows = base(vec("v", rows))
    data = i32(*[count] * levels) + b"\0" * (4 * count - 4)
    out = run_case(dict(name=name, nodes=rows, hex=data.hex()))
    return {
        "name": name,
        "recipe": "nested",
        "levels": levels,
        "count": count,
        "nodes": rows,
        "boost": out["boost"],
        "python": out["python"],
    }


def ok_case(name, ints, dup, levels):
    """Every level read: `levels` references to `R`, an empty class's, then
    each level's `ints` from the innermost out."""
    data = b"".join(ref("R", rid) for rid in range(1, levels + 1)) + ref("", levels + 1)
    for level in reversed(range(levels)):
        data += i32(*(1000 * level + i for i in range(ints)))
    return dict(name=name, nodes=NODES, hex=data.hex(), ref_types=ref_type(ints, dup))


# Review round 7's scene, scaled from 750,000 `int`s and 250 levels.
EOF_CASES = [
    ("wide_ref_type_reentered_to_eof", 2**16, False, 64),
    ("wide_ref_type_with_a_repeated_name_reentered_to_eof", 2**16, True, 64),
]

# 16 vectors, each of 2^16 elements: the default `max_prealloc`.
NESTED_CASES = [("vectors_nested_16_deep_end_short", 16, 2**16)]

# Wider than the first reservation of a dict (walk::DICT_FIRST, 16), so
# each level grows its dict as it reads.
OK_CASES = [
    ("r14_reentry_of_a_wide_ref_type_reads_every_level", 40, False, 6),
    ("r14_reentry_of_a_wide_ref_type_with_a_repeated_name", 40, True, 6),
]


def reentry_cases():
    """`reentry.json`'s cases, with UnityPy's outcomes."""
    return [eof_case(*c) for c in EOF_CASES] + [nested_case(*c) for c in NESTED_CASES]


def reentry_ok_cases():
    """`cases.json`'s re-entry cases, for `walk.py`'s `run_case`."""
    return [ok_case(*c) for c in OK_CASES]
