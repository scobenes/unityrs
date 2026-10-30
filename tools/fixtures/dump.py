"""Oracle fixtures for package E: the dump's encodings and its input selection (docs/plans/core.md 7.6).

Runs in the oracle image and writes small JSON files under
crates/unityrs-dump/tests/fixtures/dump/, which `cargo test -p unityrs-dump`
replays without Python (crates/unityrs-dump/tests/oracle_fixtures.rs):

- encode.json: tools/parity/canonical.py's encoding of synthetic values (every
  value type, float edge cases, packing, the 64-byte threshold), of names (the
  metadata-string rule, invalid UTF-8, JSON escapes), and Python's decimal form of
  packed integers past 64 bits (UnityPy's UnityVersion, a 4,300-digit group included);
- select.json: unitypy_dump.py's input selection (collect_inputs) over a synthetic
  tree and corpus index built here in a temporary folder, and fnmatch.fnmatchcase
  (what --name means) over names and patterns, invalid UTF-8 included; and what
  the oracle's whole run (its exit status, its index_sha256_mismatch count) makes
  of an index entry's sha256 of every JSON kind, of a repeated name's pins, and of
  an index whose files are no list of objects;
- l2_kinds.json: dump_records.py's error_kind on read_typetree's check_read message,
  for a real pure-Python read whose bytes_read is negative, and on positive ones;
- records.json: dump_records.py's first_difference (an L3 record's first_diff) on
  pairs of byte strings, and the oracle's records (unitypy_dump.py dump_input) of
  SerializedFiles built here with tools/parity/tests/synth.py, the input's bytes
  beside them: an object whose resave differs from its raw bytes, and one whose
  embedded tree is deeper than the dump's test lowers max_tree_depth to.

Every input is built here in code; no file outside the temporary folder is read.

    scripts/dev python tools/fixtures/dump.py           # write the fixtures
    scripts/dev python tools/fixtures/dump.py --check   # regenerate and compare
"""

from __future__ import annotations

import argparse
import fnmatch
import json
import os
import struct
import sys
import tempfile
from pathlib import Path
from types import SimpleNamespace

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "crates/unityrs-dump/tests/fixtures/dump"
sys.path.insert(0, str(ROOT / "tools" / "parity"))
sys.path.insert(0, str(ROOT / "tools" / "parity" / "tests"))

import canonical as C  # noqa: E402
import dump_records  # noqa: E402
import synth as S  # noqa: E402
import unitypy_dump  # noqa: E402
import UnityPy  # noqa: E402
from UnityPy.helpers import TypeTreeHelper as TTH  # noqa: E402
from UnityPy.helpers.TypeTreeNode import TypeTreeNode  # noqa: E402
from UnityPy.helpers.UnityVersion import UnityVersion  # noqa: E402
from UnityPy.streams import EndianBinaryReader  # noqa: E402

# A 4,300-digit version group packs to more digits than str(int) allows by default.
sys.set_int_max_str_digits(0)

# --------------------------------------------------------------------- values
#
# A value is described once, as JSON both sides read: this script builds the
# Python value UnityPy would hold and the typetree node that typed it, and
# canonical.py encodes them; the Rust side builds the `Value` the reader would
# make and encodes it from the value alone.
#
#   ["bool", b]                      bool node
#   ["int", "decimal", node_type]    any integer node
#   ["f32", "8 hex"]  ["f64", "16 hex"]   float / double node, by bit pattern
#   ["str", "hex"]                   string node: the bytes UnityPy decodes with surrogateescape
#   ["bytes", "hex"]                 TypelessData
#   ["prims", elem, items]           vector of a primitive, read as one run (items: hex for
#                                    u8/char, bit patterns for f32/f64, bools, decimal strings)
#   ["list", template, [values]]     vector whose element node is template's, read item by item
#   ["pair", a, b]                   a pair node: a Python tuple
#   ["tuple_array", template, [values]]  the pure-Python reader's tuple for an aligned primitive array
#   ["dict", [["name hex", value], ...]]  a class node with those children

PRIM_NODE = {
    "u8": "UInt8",
    "char": "char",
    "i8": "SInt8",
    "u16": "UInt16",
    "i16": "SInt16",
    "u32": "UInt32",
    "i32": "SInt32",
    "u64": "UInt64",
    "i64": "SInt64",
    "bool": "bool",
    "f32": "float",
    "f64": "double",
}


def name_str(h: str) -> str:
    return bytes.fromhex(h).decode("utf-8", "surrogateescape")


def node(m_type, m_name="v", children=()):
    return SimpleNamespace(m_Type=m_type, m_Name=m_name, m_Children=list(children))


def vector(elem, m_name="v"):
    elem.m_Name = "data"
    return node("vector", m_name, [node("Array", "Array", [node("int", "size"), elem])])


def f32_value(bits: str) -> float:
    return struct.unpack(">f", bytes.fromhex(bits))[0]


def f64_value(bits: str) -> float:
    return struct.unpack(">d", bytes.fromhex(bits))[0]


def prim_value(elem, item):
    if elem == "f32":
        return f32_value(item)
    if elem == "f64":
        return f64_value(item)
    if elem == "bool":
        return item
    return int(item)


def build(desc, m_name="v"):
    """(Python value, typetree node) for a value description."""
    kind = desc[0]
    if kind == "bool":
        return desc[1], node("bool", m_name)
    if kind == "int":
        return int(desc[1]), node(desc[2], m_name)
    if kind == "f32":
        return f32_value(desc[1]), node("float", m_name)
    if kind == "f64":
        return f64_value(desc[1]), node("double", m_name)
    if kind == "str":
        return name_str(desc[1]), node("string", m_name)
    if kind == "bytes":
        return bytes.fromhex(desc[1]), node("TypelessData", m_name)
    if kind == "prims":
        elem, items = desc[1], desc[2]
        value = list(bytes.fromhex(items)) if elem in ("u8", "char") else [prim_value(elem, x) for x in items]
        return value, vector(node(PRIM_NODE[elem]), m_name)
    if kind in ("list", "tuple_array"):
        _, elem_node = build(desc[1])
        items = [build(d)[0] for d in desc[2]]
        return (items if kind == "list" else tuple(items)), vector(elem_node, m_name)
    if kind == "pair":
        a, an = build(desc[1], "first")
        b, bn = build(desc[2], "second")
        return (a, b), node("pair", m_name, [an, bn])
    if kind == "dict":
        value, children = {}, []
        for h, d in desc[1]:
            v, n = build(d, name_str(h))
            value[n.m_Name] = v
            children.append(n)
        return value, node("Klass", m_name, children)
    raise ValueError(f"unknown description {kind!r}")


def h(text: str) -> str:
    return text.encode().hex()


def i(v, t="SInt32"):
    return ["int", str(v), t]


def value_cases() -> list[dict]:
    """Every value type, float edge cases, packing, the 64-byte threshold, invalid UTF-8."""
    f32s = [
        "00000000",
        "80000000",  # -0.0
        "3f800000",
        "bf800000",
        "7f7fffff",  # largest finite
        "00000001",  # smallest subnormal
        "807fffff",  # largest negative subnormal
        "7f800000",  # +inf
        "ff800000",  # -inf
        "7fc00000",  # quiet NaN
        "7fc00001",  # quieted signalling NaN, as UnityPyBoost delivers it
        "ffc12345",  # negative quiet NaN with a payload
        "7fffffff",
    ]
    f64s = [
        "0000000000000000",
        "8000000000000000",
        "3ff0000000000000",
        "7fefffffffffffff",
        "0000000000000001",
        "7ff0000000000000",
        "fff0000000000000",
        "7ff8000000000000",
        "7ff0000000000001",  # signalling NaN: float64 keeps every bit
        "fff8000000000abc",
    ]
    cases = [
        ("bool_true", ["bool", True]),
        ("bool_false", ["bool", False]),
        ("int_zero", i(0)),
        ("int_neg", i(-5)),
        ("int_i64_min", i(-(2**63), "SInt64")),
        ("int_i64_max", i(2**63 - 1, "SInt64")),
        ("int_u64_above_i64", i(2**63, "UInt64")),
        ("int_u64_max", i(2**64 - 1, "UInt64")),
        ("str_empty", ["str", ""]),
        ("str_ascii", ["str", h("hello")]),
        ("str_escapes", ["str", bytes(range(0x00, 0x21)).hex() + h('"\\/\x7f')]),
        ("str_unicode", ["str", h("café    \U0001f600")]),
        ("str_invalid_short", ["str", "61ff62"]),
        ("str_overlong_nul", ["str", "c080"]),
        ("str_encoded_surrogate", ["str", "eda080"]),
        ("str_above_max_code_point", ["str", "f4908080"]),
        ("str_truncated", ["str", "e282"]),
        ("str_invalid_64", ["str", ("ff" * 64)]),
        ("str_invalid_65", ["str", ("ff" * 65)]),
        ("str_valid_200", ["str", h("x" * 200)]),
        ("bytes_empty", ["bytes", ""]),
        ("bytes_short", ["bytes", "0001"]),
        ("bytes_64", ["bytes", bytes(range(64)).hex()]),
        ("bytes_65", ["bytes", bytes(range(65)).hex()]),
        ("bytes_utf8", ["bytes", h("text")]),
        ("prims_u8", ["prims", "u8", "007fff"]),
        ("prims_u8_64", ["prims", "u8", bytes(range(64)).hex()]),
        ("prims_u8_65", ["prims", "u8", bytes(range(65)).hex()]),
        ("prims_char", ["prims", "char", h("abc")]),
        ("prims_u8_empty", ["prims", "u8", ""]),
        ("prims_i8", ["prims", "i8", ["-128", "0", "127"]]),
        ("prims_u16", ["prims", "u16", ["0", "65535"]]),
        ("prims_i16", ["prims", "i16", ["-32768", "32767"]]),
        ("prims_u32", ["prims", "u32", ["0", str(2**32 - 1)]]),
        ("prims_i32", ["prims", "i32", [str(-(2**31)), str(2**31 - 1)]]),
        ("prims_u64", ["prims", "u64", ["0", str(2**64 - 1)]]),
        ("prims_i64", ["prims", "i64", [str(-(2**63)), str(2**63 - 1)]]),
        ("prims_bool", ["prims", "bool", [True, False, True]]),
        ("prims_bool_empty", ["prims", "bool", []]),
        ("prims_f32", ["prims", "f32", f32s]),
        ("prims_f64", ["prims", "f64", f64s]),
        ("prims_f32_empty", ["prims", "f32", []]),
        ("prims_f64_empty", ["prims", "f64", []]),
        ("list_empty", ["list", i(0), []]),
        ("list_ints", ["list", i(0), [i(1), i(-2), i(3)]]),
        ("list_f32_item_by_item", ["list", ["f32", "00000000"], [["f32", b] for b in f32s]]),
        ("list_f64_item_by_item", ["list", ["f64", "0000000000000000"], [["f64", b] for b in f64s]]),
        ("list_strs", ["list", ["str", ""], [["str", h("a")], ["str", "ff"], ["str", ""]]]),
        ("list_bytes", ["list", ["bytes", ""], [["bytes", "00"], ["bytes", ""]]]),
        ("list_of_u8_vectors", ["list", ["prims", "u8", ""], [["prims", "u8", "01"], ["prims", "u8", ""]]]),
        ("list_of_lists", ["list", ["list", i(0), []], [["list", i(0), [i(1)]], ["list", i(0), []]]]),
        ("pair_int_str", ["pair", i(7), ["str", h("k")]]),
        ("pair_f32_f64", ["pair", ["f32", "3f800000"], ["f64", "3ff0000000000000"]]),
        ("map_as_list_of_pairs", ["list", ["pair", ["str", ""], i(0, "SInt64")],
                                  [["pair", ["str", h("k")], i(-5, "SInt64")]]]),
        ("tuple_array_ints", ["tuple_array", i(0, "UInt16"), [i(65535, "UInt16"), i(1, "UInt16")]]),
        ("tuple_array_u8_never_packed", ["tuple_array", i(0, "UInt8"), [i(1, "UInt8"), i(2, "UInt8")]]),
        ("tuple_array_f32_never_packed", ["tuple_array", ["f32", "00000000"], [["f32", "3f800000"]]]),
        ("tuple_array_empty", ["tuple_array", i(0), []]),
        ("dict_empty", ["dict", []]),
        ("dict_named_t", ["dict", [[h("t"), i(1)], [h("v"), i(2)]]]),
        ("dict_invalid_key", ["dict", [["61ff", i(1)], [("ff" * 100), i(2)]]]),
        ("dict_key_escapes", ["dict", [[h('a"b\\c\n'), ["bool", True]]]]),
        (
            "dict_nested",
            [
                "dict",
                [
                    [h("m_Name"), ["str", h("Cube")]],
                    [h("m_Data"), ["dict", [[h("x"), ["f32", "3fc00000"]], [h("blob"), ["bytes", "0001"]]]]],
                    [h("fv"), ["prims", "f32", ["3fc00000", "7f800000"]]],
                    [h("u8v"), ["prims", "u8", "007fff"]],
                    [h("items"), ["list", ["dict", [[h("a"), i(0)]]], [["dict", [[h("a"), i(1)]]]]]],
                ],
            ],
        ),
    ]
    out = []
    for name, desc in cases:
        value, root = build(desc)
        out.append({"name": name, "value": desc, "encoded": C.dumps(C.ValueEncoder().encode(value, root))})
    return out


def name_cases() -> list[dict]:
    """The metadata-string rule (enc_name) and the JSON string rules, as dumps writes them."""
    raws = [
        b"",
        b"CAB-0123456789abcdef",
        bytes(range(0x00, 0x80)),
        '"\\/'.encode(),
        "café".encode(),
        "  \u0085 ".encode(),
        "\U0001f600\U0010ffff".encode(),
        "﻿￾￿".encode(),
        b"\xff",
        b"a\xffb",
        b"\xc0\x80",
        b"\xed\xa0\x80",
        b"\xed\xbf\xbf",
        b"\xf4\x90\x80\x80",
        b"\xf5\x80\x80\x80",
        b"\xe2\x82",
        b"\xe2\x82\x41",
        b"\x80\x80",
        b"\xfa" * 8,
        "ú".encode() * 8,
        b"\xff" * 100,
        b"x" * 300,
    ]
    return [{"hex": raw.hex(), "encoded": C.dumps(C.enc_name(raw.decode("utf-8", "surrogateescape")))} for raw in raws]


def limbs(n: int) -> list[str]:
    out = []
    while n:
        out.append(f"{n & (2**64 - 1):016x}")
        n >>= 64
    return out


def int_cases() -> list[dict]:
    """Packed integers past 64 bits, as Python prints them (the dump writes the exact integer)."""
    values = [0, 1, 2**64 - 1, 2**64, 2**127, 2**128 - 1, 2**128, 2**128 + 1, 10**38, 10**38 + 1, 3**200, 2**4000 - 1]
    for version in ("133091.1.1f1", "65536.1.1f1", "9" * 4300 + ".1.1f1", "1." + "0" * 4299 + "7" + ".0f1"):
        values.append(int(UnityVersion.from_str(version)))
    return [{"limbs": limbs(v), "decimal": str(v)} for v in values]


# ------------------------------------------------------------------ selection


def fnmatch_cases() -> list[dict]:
    """fnmatch.fnmatchcase(name, pattern), both from bytes decoded with surrogateescape (argv, os.fsdecode)."""
    patterns = [
        "*", "?", "", "*.bundle", "*.bin", "a?c", "a*", "*a", "a*b*c", "**", "*/x", "a/*",
        "[abc]", "[!abc]", "[a-c]", "[z-a]", "[a-]", "[-a]", "[]]", "[!]]", "[!]", "[]", "[",
        "[a", "a[", "\\", "[\\]", "[a--z]", "[a-b-c]", "[^a]", "[[]", "[&&]", "[~]", "[|]",
        "[!a-c]", "[à-ÿ]", "?é", "[\udcff]", "*\udcff*", ".*", "a.c", "(a)", "a+",
        "[a-c-e]", "[--0]", "[!-]", "[%--]",
    ]
    names = [
        "", "a", "abc", "abbc", "a/b/c.bundle", "x/x", "a/x", "b.bin", "é", "aé",
        "\udcff", "x\udcffy", "-", "]", "[", "^", "\\", "&", "~", "|", "a\nb", "a.c", "abc\n",
        ".", "d", "z", "0", "%", ",", "(a)", "aa",
    ]
    enc = os.fsencode
    return [
        {"name": enc(n).hex(), "pattern": enc(p).hex(), "match": fnmatch.fnmatchcase(n, p)}
        for p in patterns
        for n in names
    ]


TREE = [
    # (relative path bytes, kind, symlink target bytes)
    (b"A", "file", None),
    (b"a/x.bin", "file", None),
    (b"a/.hidden", "file", None),
    (b"a/sub/y", "file", None),
    (b"a b/z", "file", None),
    (b"b.bundle", "file", None),
    (b"\xff.bin", "file", None),
    ("é/f".encode(), "file", None),
    (b"empty_dir", "dir", None),
    (b"link_file", "symlink", b"b.bundle"),
    (b"link_dir", "symlink", b"a"),
    (b"dangling", "symlink", b"nowhere"),
    (b"corpus/one.bin", "file", None),
    (b"corpus/sub/two.bin", "file", None),
    (b"corpus/three", "file", None),
]

INDEX = {
    "files": [
        {"path": "sub/two.bin", "kind": "bundle", "build": 4100, "sha256": "aa" * 32},
        {"path": "one.bin", "kind": "unitypy-sample", "build": None},
        {"path": "three", "kind": "other", "build": 100056},
        {"path": "one.bin", "kind": "bundle", "build": "x"},
        {"path": "missing-kind.bin"},
        {"path": "three", "kind": "bundle", "build": 100056, "sha256": "bb" * 32},
    ]
}


def make_tree(root: Path) -> None:
    for rel, kind, target in TREE:
        p = os.path.join(os.fsencode(root), rel)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        if kind == "file":
            with open(p, "wb") as fh:
                fh.write(rel)
        elif kind == "dir":
            os.makedirs(p, exist_ok=True)
        else:
            os.symlink(target, p)
    (root / "corpus" / "index.json").write_text(json.dumps(INDEX))


def selection_cases(root: Path) -> list[dict]:
    """collect_inputs over the tree: (dump name, path read, expected sha256) in order, per argument set."""
    defaults = {"index": False, "kind": list(unitypy_dump.INDEX_KINDS), "build": None, "name": None,
                "sample": None, "limit": None}
    argsets = [
        ("folder", "", {}),
        ("folder_name_bin", "", {"name": ["*.bin"]}),
        ("folder_names", "", {"name": ["a/*", "A"]}),
        ("folder_name_invalid_utf8", "", {"name": [os.fsdecode(b"\xff*")]}),
        ("folder_sample_3", "", {"sample": 3}),
        ("folder_sample_100", "", {"sample": 100}),
        ("folder_sample_0", "", {"sample": 0}),
        ("folder_sample_negative", "", {"sample": -1}),
        ("folder_limit_2", "", {"limit": 2}),
        ("folder_limit_negative", "", {"limit": -2}),
        ("folder_limit_0", "", {"limit": 0}),
        ("folder_sample_then_limit", "", {"sample": 4, "limit": 2}),
        ("subfolder", "a", {}),
        ("folder_through_symlink", "link_dir", {}),
        ("file", "b.bundle", {}),
        ("file_through_symlink", "link_file", {}),
        ("index", "corpus/index.json", {"index": True}),
        ("index_kind_other", "corpus/index.json", {"index": True, "kind": ["other"]}),
        ("index_build_4100", "corpus/index.json", {"index": True, "build": ["4100"]}),
        ("index_build_none", "corpus/index.json", {"index": True, "build": ["None"]}),
        ("index_build_x_and_100056", "corpus/index.json", {"index": True, "build": ["x", "100056"]}),
        ("index_name", "corpus/index.json", {"index": True, "name": ["t*"]}),
    ]
    rootb = os.fsencode(os.path.realpath(root))
    out = []
    for name, given, extra in argsets:
        args = SimpleNamespace(**{**defaults, **extra}, input=os.fsdecode(os.path.join(rootb, os.fsencode(given))))
        items = unitypy_dump.collect_inputs(args)
        rows = []
        for path, rel, sha in items:
            if not path.startswith(rootb + b"/"):
                raise AssertionError(f"{path!r} is outside the tree")
            path = path[len(rootb) + 1 :]
            rows.append([os.fsencode(rel).hex(), path.hex(), sha])
        # Patterns as the bytes argv gives them (a lone surrogate is no JSON a strict reader takes).
        args = {k: v for k, v in extra.items() if k != "name"}
        if "name" in extra:
            args["name_hex"] = [os.fsencode(p).hex() for p in extra["name"]]
        out.append({"name": name, "input": given, "args": args, "inputs": rows})
    return out


# ----------------------------------------------------------------------- pins
#
# What the oracle's run does with an index entry's `sha256` (`unitypy_dump.py`
# `main`: `if expected[rel] and expected[rel] != sha`, `expected` a dict, so a
# repeated name takes its last entry's), and with an index that is not a list of
# objects: the oracle itself is run on each index, and its own counts and exit
# status recorded.

REAL = "@REAL@"  # stands for the file's real sha256, lowercase hex
UPPER = "@UPPER@"  # the same in upper case
WRONG = '"' + "00" * 32 + '"'

# (case, [sha256 member of each entry, as JSON text; None for no member]): each case
# names one file of its own, in one entry or (a repeated name) several.
PIN_CASES = [
    ("absent", [None]),
    ("null", ["null"]),
    ("false", ["false"]),
    ("true", ["true"]),
    ("zero", ["0"]),
    ("negative_zero", ["-0"]),
    ("zero_float", ["0.0"]),
    ("negative_zero_float", ["-0.0"]),
    ("zero_exponent", ["0e5"]),
    ("underflow", ["1e-400"]),
    ("one", ["1"]),
    ("half", ["0.5"]),
    ("empty_string", ['""']),
    ("other_string", ['"x"']),
    ("real", ['"' + REAL + '"']),
    ("real_upper", ['"' + UPPER + '"']),
    ("empty_list", ["[]"]),
    ("list", ["[0]"]),
    ("empty_object", ["{}"]),
    ("object", ['{"a":0}']),
    ("repeated_last_null", [WRONG, "null"]),
    ("repeated_first_null", ["null", WRONG]),
    ("repeated_last_empty", [WRONG, '""']),
    ("repeated_last_real", ['"x"', '"' + REAL + '"']),
]

# (case, the index's text): what the oracle makes of an index whose `files` is
# no list of objects.
INDEX_SHAPES = [
    ("files_empty_object", '{"files":{}}'),
    ("files_empty_string", '{"files":""}'),
    ("files_object", '{"files":{"a":1}}'),
    ("files_string", '{"files":"a"}'),
    ("files_number", '{"files":0}'),
    ("no_files", "{}"),
    ("index_list", "[]"),
    ("entry_number", '{"files":[{"path":"f","kind":"bundle"},1]}'),
    ("entry_string_unselected", '{"files":["bundle"]}'),
    ("entry_list", '{"files":[[]]}'),
    ("entry_null", '{"files":[null]}'),
    ("path_number", '{"files":[{"path":5,"kind":"bundle"}]}'),
    ("no_path", '{"files":[{"kind":"bundle"}]}'),
    ("none_selected", '{"files":[{"path":"f","kind":"other"}]}'),
    ("one", '{"files":[{"path":"f","kind":"bundle"}]}'),
]


def run_oracle(index: Path) -> tuple[int, str]:
    """The oracle's exit status on `index`, run with --allow-index-mismatch, and its stderr."""
    import subprocess

    out = index.parent / "out.jsonl"
    argv = [sys.executable, str(ROOT / "tools/parity/unitypy_dump.py"), "--index", str(index), "-q",
            "--allow-index-mismatch", "-o", str(out)]
    proc = subprocess.run(argv, stdin=subprocess.DEVNULL, capture_output=True, check=False)
    out.unlink(missing_ok=True)
    return proc.returncode, proc.stderr.decode("utf-8", "replace")


def pin_cases(root: Path) -> dict:
    """The pin cases, in one index the oracle dumps once: each case's entries (its file's path below the index
    is `<case>/f`), its file's bytes, and how many index mismatches the oracle warned of for it. Then each index
    shape, dumped on its own: the index's text and the oracle's exit status."""
    import hashlib

    pins, entries = [], []
    for name, members in PIN_CASES:
        (root / name).mkdir()
        data = f"the file of {name}".encode()
        (root / name / "f").write_bytes(data)
        real = hashlib.sha256(data).hexdigest()
        mine = []
        for m in members:
            sha = "" if m is None else ',"sha256":' + m.replace(REAL, real).replace(UPPER, real.upper())
            mine.append('{"path":"' + name + '/f","kind":"bundle"' + sha + "}")
        entries += mine
        pins.append({"name": name, "data": data.hex(), "entries": mine})
    (root / "index.json").write_text('{"files":[' + ",".join(entries) + "]}")
    status, err = run_oracle(root / "index.json")
    if status != 0:
        raise AssertionError(f"the oracle stopped ({status}): {err}")
    warned = [line.split(": sha256 ")[0].removeprefix("warning: ") for line in err.splitlines()
              if line.startswith("warning: ") and " differs from the index's " in line]
    for case in pins:
        case["mismatches"] = warned.count(case["name"] + "/f")
    if sum(c["mismatches"] for c in pins) != len(warned):
        raise AssertionError(f"a warning names no case: {warned}")
    shapes = []
    for name, text in INDEX_SHAPES:
        folder = root / "shapes" / name
        folder.mkdir(parents=True)
        (folder / "f").write_bytes(b"f")
        (folder / "index.json").write_text(text)
        shapes.append({"name": name, "index": text, "status": run_oracle(folder / "index.json")[0]})
    return {"pins": pins, "shapes": shapes}


# ------------------------------------------------------------------- L2 kinds


def l2_kind_cases() -> list[dict]:
    """error_kind on read_typetree's check_read message: a real negative bytes_read, and positive ones."""
    out = []
    # The pure-Python reader (byte_size 0) after a negative TypelessData length moves
    # back (CANONICAL_DUMP.md section 7): [TypelessData blob, int after] at offset 16 of
    # 32 bytes, with the length -12, reads `after` at 8 and ends at 12, 4 bytes before
    # it started.
    rows = [
        TypeTreeNode(m_Level=0, m_Type="Base", m_Name="Base", m_ByteSize=-1, m_Version=1, m_MetaFlag=0),
        TypeTreeNode(m_Level=1, m_Type="TypelessData", m_Name="blob", m_ByteSize=-1, m_Version=1, m_MetaFlag=0),
        TypeTreeNode(m_Level=2, m_Type="int", m_Name="size", m_ByteSize=4, m_Version=1, m_MetaFlag=0),
        TypeTreeNode(m_Level=2, m_Type="UInt8", m_Name="data", m_ByteSize=1, m_Version=1, m_MetaFlag=0),
        TypeTreeNode(m_Level=1, m_Type="int", m_Name="after", m_ByteSize=4, m_Version=1, m_MetaFlag=0),
    ]
    root = TypeTreeNode.from_list(rows)
    data = bytes(16) + struct.pack("<i", -12) + bytes(12)
    for start in (16, 0):
        reader = EndianBinaryReader(data, endian="<")
        reader.Position = start
        try:
            TTH.read_typetree(root, reader, as_dict=True, byte_size=0, check_read=True)
            message = None
        except Exception as e:  # noqa: BLE001
            message = str(e)
        kind, bytes_read = unitypy_dump.error_kind(ValueError(message)) if message else (None, None)
        out.append({"probe": f"pure_python_negative_typeless_at_{start}", "message": message,
                    "kind": kind, "bytes_read": bytes_read})
    for expected, read in ((0, 4), (8, 4), (4, 8), (4294967295, 0), (0, -4), (8, -96)):
        message = f"Expected to read {expected} bytes, but only read {read} bytes"
        kind, bytes_read = unitypy_dump.error_kind(ValueError(message))
        out.append({"probe": "message", "expected": expected, "read": read, "kind": kind, "bytes_read": bytes_read})
    return out


# -------------------------------------------------------------------- records


FIRST_DIFF = [("616263", "616264"), ("786263", "616263"), ("6162", "616263"), ("61626364", "6162"),
              ("", "61"), ("", ""), ("00" * 9 + "01", "00" * 10)]


def own_tree_input() -> bytes:
    """A SerializedFile of two objects. Object 1's embedded tree is 5 levels deep (`MyThing`, three `C` levels,
    an `int`): it reads, and a dump whose max_tree_depth is 3 cannot compile it. Object 2's tree is
    [int i, bool b] over the bytes 9 and 2: the bool reads as True and is written back as 1, so its resave
    differs at offset 4."""
    deep = S.Node("int", "x")
    for i in range(3):
        deep = S.Node(f"C{i}", "c", [deep])
    deep = S.Node("MyThing", "Base", [deep])
    flat = S.Node("Thing", "Base", [S.Node("int", "i"), S.Node("bool", "b")])
    objects = [S.Obj(1, 0, struct.pack("<i", 7)), S.Obj(2, 1, struct.pack("<i", 9) + b"\x02")]
    return S.serialized_file([S.Type(114, deep), S.Type(114, flat)], objects)


def record_cases(root: Path) -> dict:
    inputs = []
    for name, data in (("own_tree", own_tree_input()),):
        path = root / name
        path.write_bytes(data)
        _, text, _, _ = unitypy_dump.dump_input((str(path), name, C.LEVELS))
        inputs.append({"name": name, "input": data.hex(), "records": [line for line in text.split("\n") if line]})
    first = [{"a": a, "b": b, "first_diff": dump_records.first_difference(bytes.fromhex(a), bytes.fromhex(b))}
             for a, b in FIRST_DIFF]
    return {"first_diff": first, "inputs": inputs}


# ----------------------------------------------------------------------- main


def fixtures() -> dict[str, dict]:
    meta = {"unitypy": UnityPy.__version__, "python": sys.version.split()[0]}
    with tempfile.TemporaryDirectory(prefix="unityrs-fixtures-") as tmp:
        make_tree(Path(tmp))
        selection = selection_cases(Path(tmp))
    with tempfile.TemporaryDirectory(prefix="unityrs-fixtures-") as tmp:
        records = record_cases(Path(tmp))
    with tempfile.TemporaryDirectory(prefix="unityrs-fixtures-") as tmp:
        pins = pin_cases(Path(tmp))
    return {
        "encode.json": {**meta, "values": value_cases(), "names": name_cases(), "ints": int_cases()},
        "select.json": {
            **meta,
            "tree": [[rel.hex(), kind, None if t is None else t.hex()] for rel, kind, t in TREE],
            "index": INDEX,
            "selections": selection,
            "fnmatch": fnmatch_cases(),
            **pins,
        },
        "l2_kinds.json": {**meta, "cases": l2_kind_cases()},
        "records.json": {**meta, **records},
    }


def dump(body: dict) -> str:
    """JSON with one list element per line, so a regenerated fixture diffs by case."""

    def compact(v: object) -> str:
        return json.dumps(v, sort_keys=True, separators=(",", ":"))

    items = sorted(body.items())
    lines = ["{"]
    for n, (key, value) in enumerate(items):
        comma = "," if n < len(items) - 1 else ""
        if isinstance(value, list):
            lines.append(f" {json.dumps(key)}: [")
            lines += [f"  {compact(e)}" + ("," if j < len(value) - 1 else "") for j, e in enumerate(value)]
            lines.append(" ]" + comma)
        else:
            lines.append(f" {json.dumps(key)}: {compact(value)}{comma}")
    lines.append("}")
    return "\n".join(lines) + "\n"


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--check", action="store_true", help="regenerate and compare, write nothing")
    args = ap.parse_args()
    if UnityPy.__version__ != "1.25.2":
        print(f"needs UnityPy 1.25.2, found {UnityPy.__version__}", file=sys.stderr)
        return 2
    stale = []
    for name, body in fixtures().items():
        text = dump(body)
        path = OUT / name
        if args.check:
            if not path.exists() or path.read_text() != text:
                stale.append(name)
        else:
            OUT.mkdir(parents=True, exist_ok=True)
            path.write_text(text)
    if stale:
        print("stale fixtures: " + ", ".join(stale), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
