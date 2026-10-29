"""The pure-Python reader's array rules that CANONICAL_DUMP section 7 and
plan section 3.10 state, run against UnityPy 1.25.2 and UnityPyBoost.

Each case builds a typetree and its bytes in code, reads them with
UnityPyBoost's `read_typetree` and with the pure-Python `read_value`, and
prints both outcomes: `ok <value> <position>`, or `err <exception> <kind>`,
the kind being the canonical dump's L2 error kind (`unitypy_dump.error_kind`),
since two readers can raise one exception class with different kinds. With
`--check` it compares them with the outcomes recorded below and exits 1 on
any difference. No file is read or written.

    scripts/dev python tools/fixtures/pure_python_rules.py [--check]

Owner: the scaffold (package B records these cases as fixtures, plan 7.3).
"""

import struct
import sys
from pathlib import Path

from UnityPy.helpers import TypeTreeHelper
from UnityPy.helpers.TypeTreeNode import TypeTreeNode
from UnityPy.streams.EndianBinaryReader import EndianBinaryReader
from UnityPy.UnityPyBoost import read_typetree as boost_read

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "parity"))
from unitypy_dump import error_kind  # noqa: E402

ALIGN = 0x4000


def tree(rows):
    return TypeTreeNode.from_list(
        [
            dict(m_Level=lv, m_Type=ty, m_Name=name, m_ByteSize=-1, m_Version=1, m_MetaFlag=flag)
            for lv, ty, name, flag in rows
        ]
    )


def vector_of(element_flag, element_type, children=()):
    """`Base { vector v { Array { int size; <element_type> data } } }`."""
    return [
        (0, "Base", "Base", 0),
        (1, "vector", "v", 0),
        (2, "Array", "Array", 0),
        (3, "int", "size", 0),
        (3, element_type, "data", element_flag),
        *children,
    ]


def outcome(read):
    try:
        value, position = read()
    except Exception as e:  # noqa: BLE001 - the exception class is the outcome
        return f"err {type(e).__name__} {error_kind(e)[0]}"
    return f"ok {value!r} {position}"


def run(rows, data):
    root = tree(rows)

    def boost():
        return boost_read(data, root, "<", True, None)

    def python():
        reader = EndianBinaryReader(data, "<")
        value = TypeTreeHelper.read_value(root, reader, TypeTreeHelper.TypeTreeConfig(True, None, False))
        return value, reader.Position

    return outcome(boost), outcome(python)


def i32(*values):
    return struct.pack(f"<{len(values)}i", *values)


PAIR_OF = {
    0: [],
    1: [(4, "int", "first", 0)],
    3: [(4, "int", "a", 0), (4, "int", "b", 0), (4, "int", "c", 0)],
}

# The element vector's own `Array`, with 0, 1 or 3 children.
INNER_ARRAY_OF = {
    0: [(4, "Array", "Array", 0)],
    1: [(4, "Array", "Array", 0), (5, "int", "size", 0)],
    3: [(4, "Array", "Array", 0), (5, "int", "size", 0), (5, "int", "data", 0), (5, "int", "extra", 0)],
}

# (name, rows, bytes, UnityPyBoost's outcome, the pure-Python outcome)
CASES = [
    *[
        (
            f"empty array of {n}-child pair, element {'aligned' if flag else 'not aligned'}",
            vector_of(flag, "pair", PAIR_OF[n]),
            i32(0),
            "err ValueError other",
            "err IndexError other" if flag and n < 2 else "ok {'v': []} 4",
        )
        for flag in (0, ALIGN)
        for n in (0, 1, 3)
    ],
    # `read_value_array` reads an aligned vector element's
    # `m_Children[0].m_Children[1]` before its loop (TypeTreeHelper.py:309),
    # as it reads an aligned pair's children (:282-283).
    *[
        (
            f"empty array of vector whose Array has {n} children, element {'aligned' if flag else 'not aligned'}",
            vector_of(flag, "vector", INNER_ARRAY_OF[n]),
            i32(0),
            "ok {'v': []} 4",
            "err IndexError other" if flag and n < 2 else "ok {'v': []} 4",
        )
        for flag in (0, ALIGN)
        for n in (0, 1, 3)
    ],
    (
        "empty array of aligned TypelessData",
        vector_of(ALIGN, "TypelessData", [(4, "int", "size", 0), (4, "UInt8", "data", 0)]),
        i32(0),
        "ok {'v': []} 4",
        "ok {'v': []} 4",
    ),
    (
        "array of one aligned TypelessData",
        vector_of(ALIGN, "TypelessData", [(4, "int", "size", 0), (4, "UInt8", "data", 0)]),
        i32(1, 1) + b"\x07\0\0\0",
        "ok {'v': [b'\\x07']} 12",
        "err TypeError other",
    ),
    (
        "empty array of aligned SInt8",
        vector_of(ALIGN, "SInt8"),
        i32(0),
        "ok {'v': []} 4",
        "err TypeError other",
    ),
    (
        "aligned pair array, first an aligned UInt8 (two elements)",
        vector_of(ALIGN, "pair", [(4, "UInt8", "first", ALIGN), (4, "UInt8", "second", 0)]),
        i32(2) + bytes.fromhex("01000000 02030000 00040000"),
        "ok {'v': [(1, 2), (3, 0)]} 16",
        "ok {'v': [(1, 0), (0, 0)]} 8",
    ),
    (
        "aligned pair array, first an aligned TypelessData",
        vector_of(
            ALIGN,
            "pair",
            [
                (4, "TypelessData", "first", ALIGN),
                (5, "int", "size", 0),
                (5, "UInt8", "data", 0),
                (4, "UInt8", "second", 0),
            ],
        ),
        i32(1, 1) + bytes([7, 9, 0, 0, 0, 0, 0, 0]),
        "ok {'v': [(b'\\x07', 0)]} 16",
        "ok {'v': [(b'\\x07', 9)]} 12",
    ),
    (
        "aligned pair array, first a vector<UInt8> with a flagged Array",
        vector_of(
            ALIGN,
            "pair",
            [
                (4, "vector", "first", 0),
                (5, "Array", "Array", ALIGN),
                (6, "int", "size", 0),
                (6, "UInt8", "data", 0),
                (4, "UInt8", "second", 0),
            ],
        ),
        i32(1, 1) + bytes([5, 0, 0, 0, 9, 0, 0, 0]),
        "ok {'v': [([5], 9)]} 16",
        "ok {'v': [([5], 9)]} 16",
    ),
    (
        "aligned pair array, first a non-empty string",
        vector_of(ALIGN, "pair", [(4, "string", "first", 0), (4, "UInt8", "second", 0)]),
        i32(1, 1) + b"x\0\0\0" + bytes([9, 0, 0, 0]),
        "ok {'v': [('x', 9)]} 16",
        "ok {'v': [('x', 9)]} 16",
    ),
    (
        "aligned pair array, first an aligned pair",
        vector_of(
            ALIGN,
            "pair",
            [
                (4, "pair", "first", ALIGN),
                (5, "UInt8", "first", 0),
                (5, "UInt8", "second", 0),
                (4, "UInt8", "second", 0),
            ],
        ),
        i32(1) + bytes([1, 2, 0, 0, 9, 0, 0, 0]),
        "ok {'v': [((1, 2), 9)]} 12",
        "ok {'v': [((1, 2), 9)]} 12",
    ),
    (
        "aligned pair array, first an aligned class",
        vector_of(
            ALIGN,
            "pair",
            [(4, "Foo", "first", ALIGN), (5, "UInt8", "x", 0), (4, "UInt8", "second", 0)],
        ),
        i32(1) + bytes([1, 0, 0, 0, 2, 0, 0, 0]),
        "ok {'v': [({'x': 1}, 2)]} 12",
        "ok {'v': [({'x': 1}, 2)]} 12",
    ),
    # Outside an array the pure-Python reader reads a vector's length, and
    # refuses a negative one, before it indexes its `Array`'s element
    # (TypeTreeHelper.py:213-216), and reads a pair's first child before it
    # indexes its second (:194-195); UnityPyBoost checks each arity first
    # (TypeTreeHelper.cpp:820-833, :328-332). So a malformed one fails with
    # whatever those reads raise first (round 5).
    *[
        (
            f"vector whose Array has {n} children, length {length}",
            [(0, "Base", "Base", 0), (1, "vector", "v", 0), (2, "Array", "Array", 0), *children],
            i32(length),
            "err ValueError other",
            python,
        )
        for n, children in ((0, []), (1, [(3, "int", "size", 0)]))
        for length, python in ((-1, "err ValueError negative_length"), (0, "err IndexError other"))
    ],
    (
        "pair of one child, a vector of length -1",
        [
            (0, "Base", "Base", 0),
            (1, "pair", "p", 0),
            (2, "vector", "first", 0),
            (3, "Array", "Array", 0),
            (4, "int", "size", 0),
            (4, "int", "data", 0),
        ],
        i32(-1),
        "err ValueError other",
        "err ValueError negative_length",
    ),
]


def main(argv):
    check = "--check" in argv
    failed = 0
    for name, rows, data, want_boost, want_python in CASES:
        got_boost, got_python = run(rows, data)
        print(f"{name}\n  boost : {got_boost}\n  python: {got_python}")
        if check and (got_boost, got_python) != (want_boost, want_python):
            failed += 1
            print(f"  EXPECTED boost {want_boost} / python {want_python}")
    if check:
        print(f"{len(CASES) - failed} of {len(CASES)} cases as recorded")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
