"""Primitive runs past `Limits::max_values_per_object`, whose outcomes `walk.py`
records into `runs.json` (docs/plans/core.md 3.10, "Bounds").

Review round 9 found a well-formed object refused at the defaults: a `Mesh`
whose `vector<UInt8>` held 2^26 + 1 bytes, each element counted as a value
against the bound of 2^26, where UnityPy reads it. UnityPyBoost's reader
holds a primitive run as one value whose elements are the object's bytes,
which its position passes once, so the run now counts as its vector's one
value and its bytes as the open's work; the writer, likewise, counts no
primitive it writes against the bound (outside a repeated name).

Review round 10 found the same refusal on the re-save of a run whose element
type name holds a NUL (`int\0x`): UnityPyBoost reads the name to the NUL, so
a run of `int`s, while UnityPy's writer looks the whole name up, finds no
function and writes each element as a class with no children, which writes
nothing; the writer counted each such class against the bound. It now counts
only a child of such a class that writes no byte, so the run re-saves as
UnityPy re-saves it.

Each case is `Base { string m_Name; vector<T> v }` over the bytes of a
recipe, `runs`: a list of `[count, hex]`, each hex repeated `count` times
(the vector's length, then its elements: a pattern of 16 bytes, then what
is left). It is read from 0 by UnityPyBoost's `read_typetree` (as an object
of that size, `byte_size > 0`), and its value re-saved by `write_typetree`.
An outcome is `walk.py`'s (`{"ok": value, "pos": n}` or `{"err": class,
"kind": kind}`) but for the vector, too long to write out: a list of bytes'
values is `{"n": len, "u8_crc32": zlib.crc32 of the ints as bytes}`, a list
of other ints `{"n": len, "i32_crc32": zlib.crc32 of the ints packed as
little-endian 32-bit integers}` and a list of floats `{"n": len,
"f64_crc32": zlib.crc32 of the floats packed as little-endian doubles}`; a
write is `{"n": len, "crc32": zlib.crc32}` of its bytes, or `{"err":
class}`.

The pure-Python reader is not recorded here: it reads an object only when
its `byte_size` is 0, and such an object that reads a byte fails
`check_read` in UnityPy, so no well-formed object meets the bound there
(`PARITY.md` 19.3); `tests/walk_runs.rs` checks that it still refuses
these reads.

`walk.py` imports this module; it records nothing when run on its own.
"""

from __future__ import annotations

import struct
import zlib
from array import array

from UnityPy.helpers import TypeTreeHelper as TTH
from UnityPy.streams import EndianBinaryWriter
from UnityPy.UnityPyBoost import read_typetree as boost_read
from walk_build import base, enc, err, i32, leaf, s, tree, vec

BOUND = 2**26
PAST = BOUND + 1

# 16 bytes of each run: every byte value's low bits, four floats, one of
# them a signalling NaN (UnityPyBoost quiets it, so the re-save differs), and
# four ints of either sign (each one CPython holds once, so the list of 2^26
# of them holds no int of its own).
U8_PATTERN = bytes(range(16))
F32_PATTERN = struct.pack("<4I", 0x3FC00000, 0x80000000, 0x7F800001, 0xFF800000)
I32_PATTERN = struct.pack("<4i", 1, -1, 256, -5)


def recipe(count, width, pattern):
    """The vector's length, then `count` elements of `width` bytes: the pattern repeated, then its first bytes."""
    n = count * width
    whole, rest = divmod(n, len(pattern))
    runs = [[1, s("big").hex()], [1, i32(count).hex()], [whole, pattern.hex()]]
    if rest:
        runs.append([1, pattern[:rest].hex()])
    return runs


# name, element type, element width, pattern
CASES = [
    ("u8_run_past_the_values_bound", "UInt8", 1, U8_PATTERN),
    ("f32_run_past_the_values_bound", "float", 4, F32_PATTERN),
    ("nul_type_name_run_past_the_values_bound", "int\0x", 4, I32_PATTERN),
]


def summary(v):
    """The vector's value, too long to write out: its length and a CRC-32 of its elements."""
    if all(type(x) is int and 0 <= x < 256 for x in v):
        return {"n": len(v), "u8_crc32": zlib.crc32(bytes(v))}
    if all(type(x) is int for x in v):
        return {"n": len(v), "i32_crc32": zlib.crc32(array("i", v).tobytes())}
    return {"n": len(v), "f64_crc32": zlib.crc32(array("d", v).tobytes())}


def run(name, ty, width, pattern):
    rows = base(leaf("string", "m_Name"), vec("v", leaf(ty, "data")))
    root = tree(rows)
    runs = recipe(PAST, width, pattern)
    data = b"".join(bytes.fromhex(h) * n for n, h in runs)
    out = {"name": name, "nodes": rows, "runs": runs}
    try:
        value, n = boost_read(data, root, "<", True, None)
    except Exception as e:  # noqa: BLE001 - the exception class is the outcome
        out["boost"] = err(e)
        return out
    out["boost"] = {"ok": {"d": [["m_Name", enc(value["m_Name"])], ["v", summary(value["v"])]]}, "pos": n}
    w = EndianBinaryWriter(endian="<")
    try:
        TTH.write_typetree(value, root, w, None)
    except Exception as e:  # noqa: BLE001
        out["boost_write"] = {"err": type(e).__name__}
    else:
        b = bytes(w.bytes)
        out["boost_write"] = {"n": len(b), "crc32": zlib.crc32(b)}
    return out


def run_cases():
    return [run(*c) for c in CASES]
