"""Arrays at and past the zero-size element bound, whose outcomes `walk.py`
records into `elements.json` (docs/plans/core.md 3.10, "Bounds").

`Limits::max_zero_size_elements` (2^20 by default) refuses an array once more
than that many of its elements have read nothing: an element whose read ends
no further on than it began. Each case here holds 2^20 or 2^20 + 1 elements,
so its bytes are a recipe, `runs`: a list of `[count, hex]`, each hex
repeated `count` times. Its tree's elements either read bytes (a `pair` of
three children and an `Array` of three, both of which only the pure-Python
reader reads; a class whose only bytes are its registry's), which unityrs
must read at any count as UnityPy does (review round 4 found them refused),
or read nothing (an empty class; the same registry class with a registry in
scope; in the pure-Python reader a `TypelessData` whose negative length
moves the position back), which unityrs refuses past the bound (`PARITY.md`
19.3) and, with the bound lifted, reads as UnityPy does. A case names the
readers that refuse it in `refused`; that is the case's claim, not UnityPy's
outcome.

Every case is read from 0 by UnityPyBoost's `read_typetree` (as an object
with `byte_size > 0`) and by the pure-Python `read_value` (`byte_size ==
0`), and each value is re-saved by `write_typetree`. An outcome is `walk.py`'s
(`{"ok": value, "pos": n}` or `{"err": class, "kind": kind}`) but for the
sizes: a list is `{"runs": [[count, item], ...]}`, equal items in a row
merged; a write is `{"words": [[count, hex], ...]}`, its bytes as 4-byte
words, equal words in a row merged (the last may be shorter), or
`{"err": class}`.

`walk.py` imports this module; it records nothing when run on its own.
"""

from __future__ import annotations

from UnityPy.helpers import TypeTreeHelper as TTH
from UnityPy.streams import EndianBinaryReader, EndianBinaryWriter
from UnityPy.UnityPyBoost import read_typetree as boost_read
from walk_build import base, enc, err, i32, leaf, tree, vec

BOUND = 2**20
PAST = BOUND + 1

INT = lambda name: leaf("int", name)  # noqa: E731


def pair3(name):
    """A `pair` of three `UInt8`s: UnityPyBoost raises, the pure-Python reader reads the first two."""
    return [[0, "pair", name, 0], [1, "UInt8", "first", 0], [1, "UInt8", "second", 0], [1, "UInt8", "third", 0]]


def bad_vector(name):
    """A vector whose `Array` has three children: UnityPyBoost raises, the pure-Python reader reads its length and `m_Children[1]`."""
    return [[0, "vector", name, 0], [1, "Array", "Array", 0], [2, "int", "size", 0], [2, "int", "data", 0], [2, "int", "extra", 0]]


def registry_class(name):
    """A class whose only bytes are its registry's: read when no registry is in scope, skipped when one is."""
    return [[0, "Foo", name, 0], [1, "ManagedReferencesRegistry", "r", 0], [2, "int", "x", 0]]


EMPTY = [[0, "Empty", "data", 0]]
TYPELESS = [[0, "Foo", "data", 0], [1, "TypelessData", "d", 0]]
REGISTRY = [[0, "ManagedReferencesRegistry", "reg", 0], [1, "int", "version", 0]]

# name, rows, runs, readers that refuse it (unityrs only)
CASES = [
    # Review round 4's three inputs, as given: each element reads bytes.
    (
        "pair3_elements_past_the_bound",
        base(vec("v", pair3("data")), vec("w", INT("data"))),
        [[1, i32(PAST).hex()], [PAST, "0000"], [1, i32(-1).hex()]],
        [],
    ),
    (
        "bad_vector_elements_past_the_bound",
        base(vec("v", bad_vector("data")), vec("w", INT("data"))),
        [[1, i32(PAST).hex()], [PAST, i32(0).hex()], [1, i32(-1).hex()]],
        [],
    ),
    (
        "registry_class_elements_past_the_bound",
        base(vec("v", registry_class("data"))),
        [[1, i32(PAST).hex()], [PAST, i32(7).hex()]],
        [],
    ),
    # Elements that read nothing: refused one past the bound, read at it.
    ("empty_elements_at_the_bound", base(vec("v", EMPTY)), [[1, i32(BOUND).hex()]], []),
    ("empty_elements_past_the_bound", base(vec("v", EMPTY)), [[1, i32(PAST).hex()]], ["boost", "python"]),
    (
        "registry_class_elements_in_scope_past_the_bound",
        base(REGISTRY, vec("v", registry_class("data"))),
        [[1, i32(1).hex()], [1, i32(PAST).hex()]],
        ["boost", "python"],
    ),
    # Each element's -4 moves the pure-Python reader back to that element's
    # start; UnityPyBoost refuses the negative length.
    ("rewinding_elements_past_the_bound", base(vec("v", TYPELESS)), [[1, i32(PAST).hex()], [1, i32(-4).hex()]], ["python"]),
]


def enc_runs(v):
    """`walk_build.enc`, with every list as runs of equal items."""
    t = type(v)
    if t is list:
        runs = []
        for x in v:
            e = enc_runs(x)
            if runs and runs[-1][1] == e:
                runs[-1][0] += 1
            else:
                runs.append([1, e])
        return {"runs": runs}
    if t is tuple:
        return {"t": [enc_runs(x) for x in v]}
    if t is dict:
        return {"d": [[k, enc_runs(x)] for k, x in v.items()]}
    return enc(v)


def words(b):
    """`b` as runs of equal 4-byte words."""
    runs = []
    for i in range(0, len(b), 4):
        w = b[i : i + 4].hex()
        if runs and runs[-1][1] == w:
            runs[-1][0] += 1
        else:
            runs.append([1, w])
    return runs


def write(value, root):
    w = EndianBinaryWriter(endian="<")
    try:
        TTH.write_typetree(value, root, w, None)
    except Exception as e:  # noqa: BLE001 - the exception class is the outcome
        return {"err": type(e).__name__}
    return {"words": words(bytes(w.bytes))}


def run(name, rows, runs, refused):
    root = tree(rows)
    data = b"".join(bytes.fromhex(h) * n for n, h in runs)
    out = {"name": name, "nodes": rows, "runs": runs, "refused": refused}
    try:
        value, n = boost_read(data, root, "<", True, None)
    except Exception as e:  # noqa: BLE001
        out["boost"] = err(e)
    else:
        out["boost"] = {"ok": enc_runs(value), "pos": n}
        out["boost_write"] = write(value, root)
    reader = EndianBinaryReader(data, "<")
    try:
        value = TTH.read_value(root, reader, TTH.TypeTreeConfig(True, None, False))
    except Exception as e:  # noqa: BLE001
        out["python"] = err(e)
    else:
        out["python"] = {"ok": enc_runs(value), "pos": reader.Position}
        out["python_write"] = write(value, root)
    return out


def element_cases():
    return [run(*c) for c in CASES]
