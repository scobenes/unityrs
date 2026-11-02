"""The inputs of tools/fixtures/dump.py's records.json, and the oracle's records of them.

`dump.py` imports this module; it records nothing when run on its own. Each input is a SerializedFile built in
code with tools/parity/tests/synth.py, and `record_cases` has the oracle (unitypy_dump.py's `dump_input`) dump it.
"""

from __future__ import annotations

import hashlib
import struct
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools" / "parity"))
sys.path.insert(0, str(ROOT / "tools" / "parity" / "tests"))

import canonical as C  # noqa: E402
import dump_records  # noqa: E402
import synth as S  # noqa: E402
import unitypy_dump  # noqa: E402

ALIGN = 0x4000  # a node's align flag (m_MetaFlag)

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


def member_typing_inputs() -> list[tuple[str, bytes]]:
    """SerializedFiles of one object each, whose values the oracle's encoder once refused (review round 10), so
    its run stopped: a member's node is not the last child of its name, or not a child the encoder looked at.
    `py` reads with the pure-Python reader (byte_size 0), the rest with UnityPyBoost (CANONICAL_DUMP.md 3)."""

    def one(tree, data, byte_size=None, ref_types=None):
        obj = S.Obj(1, 0, data, byte_size=byte_size)
        return S.serialized_file([S.Type(114, tree)], [obj], ref_types=ref_types)

    def managed(cls="string"):
        return S.Node("ReferencedManagedType", "type", [S.Node(cls, "class"), S.Node("string", "ns"), S.Node("string", "asm")])

    def refs(*children):
        return S.Node("MyRefs", "Base", [S.Node("ReferencedObject", "ref", list(children))])

    def ref_type(cls, *fields):
        return S.Type(114, S.Node(cls, "Base", list(fields)), ref_names=[cls.encode(), b"NS", b"Asm"])

    def registry(name):
        return S.Node("ManagedReferencesRegistry", name, [S.Node("int", "version")])

    def head(cls):
        return struct.pack("<q", 2) + S.s32(cls) + S.s32("NS") + S.s32("Asm")

    out = []
    # An `Array` of three children: the pure-Python reader reads its elements by the second.
    for elem, data in (("int", struct.pack("<iii", 2, 7, 9)), ("float", struct.pack("<iff", 2, 1.5, -0.0))):
        kids = [S.Node("int", "size"), S.Node(elem, "data"), S.Node("int", "extra")]
        vec = S.Node("vector", "v", [S.Node("Array", "Array", kids, flags=1)])
        out.append((f"array_of_three_{elem}_py", one(S.Node("MyThing", "Base", [vec]), data, byte_size=0)))
    # A `pair` of three: the pure-Python reader reads its first two.
    pair = S.Node("pair", "p", [S.Node("int", "a"), S.Node("float", "b"), S.Node("int", "c")])
    out.append(("pair_of_three_py", one(S.Node("MyThing", "Base", [pair]), struct.pack("<ifi", 1, 2.5, 3), byte_size=0)))
    # A ReferencedObject's data named `type`: the data overwrites the type it was resolved from. A second ref
    # type whose tree cannot hold the data leaves only the first to have typed it.
    a, c = ref_type("A", S.Node("float", "v")), ref_type("C", S.Node("double", "w"))
    tree = refs(S.Node("SInt64", "rid"), managed(), S.Node("ReferencedObjectData", "type"))
    for tag, byte_size in (("", None), ("_py", 0)):
        out.append((f"data_named_type{tag}", one(tree, head("A") + struct.pack("<f", 1.5), byte_size, [a, c])))
    # A `type` child after the data: the value's `type` is that int, the data's type is lost to it.
    d = ref_type("D", S.Node("double", "v"))
    tree = refs(S.Node("SInt64", "rid"), managed(), S.Node("ReferencedObjectData", "data"), S.Node("int", "type"))
    body = head("D") + struct.pack("<d", 0.25) + struct.pack("<i", 9)
    for tag, byte_size in (("", None), ("_py", 0)):
        out.append((f"type_after_data{tag}", one(tree, body, byte_size, [d])))
    # Data left out (a class UnityPyBoost reads as length 0: the int 0) leaves its name to the float before it.
    tree = refs(S.Node("float", "rid"), managed("int"), S.Node("ReferencedObjectData", "rid"))
    body = struct.pack("<f", 1.5) + struct.pack("<i", 0) + S.s32("NS") + S.s32("Asm")
    out.append(("data_left_out", one(tree, body, ref_types=[d])))
    def in_aligned_array(elem):
        """`elem` as the element of an aligned vector, which the pure-Python reader reads by `read_value_array`."""
        elem.meta = ALIGN
        array = S.Node("Array", "Array", [S.Node("int", "size"), elem])
        return S.Node("MyThing", "Base", [S.Node("vector", "v", [array])])

    # Review round 11. An aligned array's element [x: double, type, x: data, type: int], read by the pure-Python
    # array reader, which never leaves data out: `x` is the data's, typed by ref type `F` (a `float`) alone.
    f = S.Type(114, S.Node("float", "Base"), ref_names=[b"F", b"NS", b"Asm"])
    kids = [S.Node("double", "x"), managed(), S.Node("ReferencedObjectData", "x"), S.Node("int", "type")]
    body = struct.pack("<id", 1, 2.5) + head("F")[8:] + struct.pack("<fi", 1.5, 9)
    out.append(("array_data_named_over_py", one(in_aligned_array(S.Node("ReferencedObject", "e", kids)), body, 0, [f])))
    # [v: double, v: float], no data: the later `v` overwrote the earlier, and types it alone.
    twice = refs(S.Node("double", "v"), S.Node("float", "v"))
    for tag, byte_size in (("", None), ("_py", 0)):
        out.append((f"plain_child_named_twice{tag}", one(twice, struct.pack("<df", 2.5, 1.5), byte_size)))
    # An aligned array's element that is a class, a pair or a ReferencedObject, each holding a class
    # [r: registry, x: float, x: registry]: `read_value` reads what the element holds, checking registries.
    holder, held = [registry("r"), S.Node("float", "x"), registry("x")], struct.pack("<if", 1, 2.5)
    data_kids = [S.Node("SInt64", "rid"), managed(), S.Node("ReferencedObjectData", "d")]
    for name, elem, ref_types, data in (
        ("class", S.Node("E", "data", [S.Node("C", "c", holder)]), None, held),
        ("pair", S.Node("pair", "data", [S.Node("C", "first", holder), S.Node("int", "second")]), None,
         held + struct.pack("<i", 7)),
        ("ref", S.Node("ReferencedObject", "data", data_kids), [ref_type("R", *holder)], head("R") + held),
    ):
        body = struct.pack("<i", 1) + data
        out.append((f"array_{name}_holds_registries_py", one(in_aligned_array(elem), body, 0, ref_types)))
    # A registry the reader skips (a class's second) names no member: `x` is the float's.
    tree = S.Node("MyThing", "Base", [registry("r"), S.Node("float", "x"), registry("x")])
    for tag, byte_size in (("", None), ("_py", 0)):
        out.append((f"skipped_registry{tag}", one(tree, struct.pack("<if", 1, 2.5) + bytes(8), byte_size)))
    return out


# A ReferencedObject nest as deep as review round 11's: past the 4,096 levels of value (two a level) at which the
# dump's encoder once refused, and within what UnityPyBoost reads on the oracle's stack (21,823 levels of
# tools/fixtures/walk.py's `ref` shape, `walk.py --boost-reach`).
DEEP_LEVELS = 3_000


def deep_ref_input(levels: int = DEEP_LEVELS) -> tuple[bytes, dict]:
    """A SerializedFile of one object of ref type `R`'s tree, `R { ReferencedObject ref { rid, type, data } }`:
    each level's data is the next `R`, and the last names no class, so UnityPyBoost leaves its data out. Returned
    with its bytes as `head + level * levels + tail` in hex, which is how records.json holds them."""
    managed = S.Node("ReferencedManagedType", "type", [S.Node("string", n) for n in ("class", "ns", "asm")])
    ref = S.Node("ReferencedObject", "ref", [S.Node("SInt64", "rid"), managed, S.Node("ReferencedObjectData", "data")])
    tree = S.Node("R", "Base", [ref])
    level, last = (struct.pack("<q", 1) + S.s32(cls) + S.s32("NS") + S.s32("Asm") for cls in ("R", ""))
    body = level * (levels - 1) + last
    r = S.Type(114, tree, ref_names=[b"R", b"NS", b"Asm"])
    data = S.serialized_file([S.Type(114, tree)], [S.Obj(1, 0, body)], ref_types=[r])
    at = data.index(body)
    end = at + len(level) * (levels - 1)
    parts = {"head": data[:at].hex(), "level": level.hex(), "levels": levels - 1, "tail": data[end:].hex()}
    assert bytes.fromhex(parts["head"]) + level * parts["levels"] + bytes.fromhex(parts["tail"]) == data
    return data, parts


def without_value(line: str) -> tuple[str, str | None]:
    """An object record's line with its L2 value's text cut out (as `null`), and that text: a value thousands of
    levels deep, which the fixture holds by its sha256."""
    value = C.loads(line)["l2"]["value"]
    if value is None:
        return line, None
    text = C.dumps(value)
    assert line.count(text) == 1
    return line.replace(text, "null"), text


def record_cases(root: Path) -> dict:
    inputs = []
    for name, data in (("own_tree", own_tree_input()), *member_typing_inputs()):
        path = root / name
        path.write_bytes(data)
        _, text, _, _ = unitypy_dump.dump_input((str(path), name, C.LEVELS))
        inputs.append({"name": name, "input": data.hex(), "records": [line for line in text.split("\n") if line]})
    first = [{"a": a, "b": b, "first_diff": dump_records.first_difference(bytes.fromhex(a), bytes.fromhex(b))}
             for a, b in FIRST_DIFF]
    data, parts = deep_ref_input()
    name = f"ref_nest_{DEEP_LEVELS}"
    (root / name).write_bytes(data)
    _, text, _, _ = unitypy_dump.dump_input((str(root / name), name, C.LEVELS))
    records, values = [], []
    for line in filter(None, text.split("\n")):
        line, value = without_value(line) if line.startswith('{"rec":"object"') else (line, None)
        records.append(line)
        if value is not None:
            values.append({"bytes": len(value.encode()), "sha256": hashlib.sha256(value.encode()).hexdigest()})
    deep = {"name": name, "input": parts, "records": records, "values": values}
    return {"first_diff": first, "inputs": inputs, "deep": deep}
