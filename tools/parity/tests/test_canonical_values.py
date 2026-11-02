"""The value encoder on every form UnityPy's readers return that review round 10 found it refusing.

The oracle's encoder (canonical_encode.py) types each member of a value by the node the reader read it with.
It refused three forms UnityPy reads, and so stopped the oracle's whole run (DumperError): an `Array` of three
children and a `pair` of three, which the pure-Python reader reads through their second and first two
children, and a ReferencedObject whose data child is named `type`, which overwrites the `type` the data was
resolved from. The sweep found more of the class: a `type` child after the data, a member whose last child
of its name the reader skipped (a registry outside the scope, a data child whose class reads as empty) typed
by that child, and a value too deep for Python's recursion. The values here have the shapes UnityPy returns
for these trees: tools/fixtures/dump.py (member_typing_inputs) has the oracle read each form from a file, and
crates/unityrs-dump/tests/oracle_fixtures.rs holds unityrs's records of them equal to the oracle's. The
expected encodings follow the spec's section 3 by hand.
"""

import struct
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import canonical as C  # noqa: E402

ALIGN = 0x4000


class Node:
    """A duck-typed typetree node."""

    def __init__(self, m_Type, m_Name="x", children=(), meta=0):
        self.m_Type, self.m_Name, self.m_Children, self.m_MetaFlag = m_Type, m_Name, list(children), meta


class RefType:
    def __init__(self, cls, node, ns="NS", asm="Asm"):
        self.m_ClassName, self.m_NameSpace, self.m_AssemblyName, self.node = cls, ns, asm, node


def f32(x):
    return {"t": "f32", "v": struct.pack(">f", x).hex()}


def f64(x):
    return {"t": "f64", "v": struct.pack(">d", x).hex()}


def d(*entries):
    return {"t": "dict", "v": [list(e) for e in entries]}


def managed(class_type="string"):
    return Node("ReferencedManagedType", "type", [Node(class_type, "class"), Node("string", "ns"), Node("string", "asm")])


def ref(*children):
    return Node("ReferencedObject", "ref", children)


def registry(name):
    return Node("ManagedReferencesRegistry", name, [Node("int", "version")])


def enc(value, node, ref_types=None, boost=True):
    return C.ValueEncoder(ref_types).encode(value, node, boost=boost)


class ArraysAndPairs(unittest.TestCase):
    def test_an_array_of_three_children_types_its_elements_by_the_second(self):
        for elem, value, want in (
            (Node("int", "data"), [7, 9], [7, 9]),
            (Node("float", "data"), [1.5, -0.0], {"t": "f32s", "v": "3fc0000080000000"}),
            (Node("UInt8", "data"), [1, 255], {"t": "u8s", "hex": "01ff"}),
        ):
            v = Node("vector", "v", [Node("Array", "Array", [Node("int", "size"), elem, Node("int", "extra")])])
            with self.subTest(elem=elem.m_Type):
                self.assertEqual(enc(value, v, boost=False), want)
        # The pure-Python reader's tuple for an aligned primitive element, typed by the second child too.
        v = Node("vector", "v", [Node("Array", "Array", [Node("int", "size"), Node("float", "d", meta=ALIGN), Node("int")])])
        self.assertEqual(enc((1.5,), v, boost=False), {"t": "tuple", "v": [f32(1.5)]})

    def test_a_pair_of_three_children_is_typed_by_its_first_two(self):
        pair = Node("pair", "p", [Node("int", "a"), Node("float", "b"), Node("double", "c")])
        self.assertEqual(enc((1, 2.5), pair, boost=False), {"t": "tuple", "v": [1, f32(2.5)]})
        with self.assertRaises(C.EncodeError):
            enc((1, 2.5, 3.0), pair, boost=False)


class ReferencedObjects(unittest.TestCase):
    OTHER = RefType("Other", Node("Other", "Base", [Node("float", "v")]))

    def test_data_named_type_is_typed_by_the_ref_type_it_was_read_with(self):
        # [rid, type, type: data]: the data overwrote the `type` it was resolved from.
        tree = ref(Node("SInt64", "rid"), managed(), Node("ReferencedObjectData", "type"))
        value = {"rid": 2, "type": {"v": 1.5}}
        want = d(("rid", 2), ("type", d(("v", f32(1.5)))))
        for boost in (True, False):
            with self.subTest(boost=boost):
                self.assertEqual(enc(value, tree, [self.OTHER], boost), want)
        # Another ref type whose tree cannot hold the data does not make it ambiguous.
        wide = RefType("Wide", Node("Wide", "Base", [Node("double", "w")]))
        self.assertEqual(enc(value, tree, [wide, self.OTHER]), want)
        # One that holds it and encodes it otherwise does: the oracle cannot tell which typed it (spec 3).
        twin = RefType("Twin", Node("Twin", "Base", [Node("double", "v")]))
        with self.assertRaisesRegex(C.EncodeError, "2 ref types encode it differently"):
            enc(value, tree, [twin, self.OTHER])

    def test_a_type_child_after_the_data_hides_its_type(self):
        tree = ref(Node("SInt64", "rid"), managed(), Node("ReferencedObjectData", "data"), Node("int", "type"))
        dbl = RefType("Other", Node("Other", "Base", [Node("double", "v")]))
        value = {"rid": 2, "type": 9, "data": {"v": 0.25}}
        self.assertEqual(enc(value, tree, [dbl]), d(("rid", 2), ("type", 9), ("data", d(("v", f64(0.25))))))

    def test_the_pure_python_array_reader_types_named_over_data_by_its_ref_type_alone(self):
        # Review round 11: [x: double, type, x: data, type: int] as an aligned array's element, read by the
        # pure-Python array reader. The later `type` hides the one the data was resolved from, so every ref type
        # is a candidate for `x`; the double before the data is not, since that reader never leaves data out.
        elem = Node(
            "ReferencedObject",
            "e",
            [Node("double", "x"), managed(), Node("ReferencedObjectData", "x"), Node("int", "type")],
            meta=ALIGN,
        )
        v = Node("vector", "v", [Node("Array", "Array", [Node("int", "size"), elem])])
        f = RefType("F", Node("float", "Base"))
        self.assertEqual(enc([{"x": 1.5, "type": 9}], v, [f], boost=False), [d(("x", f32(1.5)), ("type", 9))])

    def test_a_plain_child_types_its_name_alone(self):
        # Review round 11's sweep: [v: double, v: float], no data child. The later `v` overwrote the earlier, so
        # only the float typed it; no earlier child of the name is a candidate.
        tree = ref(Node("double", "v"), Node("float", "v"))
        for boost in (True, False):
            with self.subTest(boost=boost):
                self.assertEqual(enc({"v": 1.5}, tree, [], boost), d(("v", f32(1.5))))

    def test_data_left_out_leaves_its_name_to_the_child_before(self):
        # [rid: float, type, rid: data]; a class UnityPyBoost reads as length 0 leaves the data out (spec 7),
        # so `rid` holds the float child's value. Pure Python leaves it out for "" alone. Each class value is
        # the one its node's reader returns.
        def tree(cls_node):
            typ = Node("ReferencedManagedType", "type", [cls_node, Node("string", "ns"), Node("string", "asm")])
            return ref(Node("float", "rid"), typ, Node("ReferencedObjectData", "rid"))

        def vector(elem):
            return Node("vector", "class", [Node("Array", "Array", [Node("int", "size"), Node(elem, "data")])])

        empty = (
            (Node("int", "class"), 0),
            (Node("bool", "class"), False),
            (Node("float", "class"), 0.0),
            (Node("double", "class"), 0.0),
            (vector("UInt8"), b""),
            (vector("int"), []),
            (Node("Empty", "class"), {}),
        )
        for cls_node, cls in empty:
            value = {"rid": 1.5, "type": {"class": cls, "ns": "NS", "asm": "Asm"}}
            with self.subTest(cls=cls, node=cls_node.m_Type):
                self.assertEqual(enc(value, tree(cls_node), [self.OTHER])["v"][0], ["rid", f32(1.5)])
                with self.assertRaises(C.EncodeError):
                    enc(value, tree(cls_node), [self.OTHER], boost=False)  # 0 == "" is false: the read raised
        for cls_node, cls in (
            (Node("float", "class"), -0.0),
            (Node("int", "class"), 1),
            (Node("bool", "class"), True),
            (vector("int"), [0]),
        ):
            value = {"rid": 1.5, "type": {"class": cls, "ns": "NS", "asm": "Asm"}}
            with self.subTest(cls=cls), self.assertRaises(C.EncodeError):
                enc(value, tree(cls_node), [self.OTHER])
        value = {"rid": 1.5, "type": {"class": "", "ns": "NS", "asm": "Asm"}}
        for boost in (True, False):
            with self.subTest(cls="", boost=boost):
                got = enc(value, tree(Node("string", "class")), [self.OTHER], boost)
                self.assertEqual(got["v"][0], ["rid", f32(1.5)])
        # A matching ref type without a tree leaves the data out too.
        bare = RefType("Other", None)
        value = {"rid": 1.5, "type": {"class": "Other", "ns": "NS", "asm": "Asm"}}
        self.assertEqual(enc(value, tree(Node("string", "class")), [bare])["v"][0], ["rid", f32(1.5)])


class Registries(unittest.TestCase):
    def test_a_registry_the_reader_skips_types_nothing(self):
        # Section 3's scope: after a class's first registry, its later ones are skipped.
        thing = Node("Thing", "Base", [registry("r"), Node("float", "x"), registry("x")])
        value = {"r": {"version": 1}, "x": 2.5}
        for boost in (True, False):
            with self.subTest(boost=boost):
                self.assertEqual(enc(value, thing, boost=boost), d(("r", d(("version", 1))), ("x", f32(2.5))))
        # Inside a registry being read, every registry below is skipped, a class's first included.
        inner = Node("Inner", "i", [Node("float", "x"), registry("x")])
        outer = Node("Outer", "Base", [Node("ManagedReferencesRegistry", "reg", [inner])])
        self.assertEqual(enc({"reg": {"i": {"x": 0.5}}}, outer), d(("reg", d(("i", d(("x", f32(0.5))))))))

    def test_what_an_array_elements_members_hold_is_read_with_the_registry_check(self):
        # Review round 11's sweep: the pure-Python array reader checks no registry among an element's own
        # children, but reads each child with `read_value`, which checks the registries of what it reads: a
        # class member, a pair's members (`read_value_array`'s `lambda`) and a ReferencedObject's data. Each
        # holds a class [r: registry, x: float, x: registry] whose second registry is skipped: `x` is the float.
        holder = Node("C", "c", [registry("r"), Node("float", "x"), registry("x")])
        inner = d(("r", d(("version", 1))), ("x", f32(2.5)))
        read = {"r": {"version": 1}, "x": 2.5}

        def vector(elem):
            return Node("vector", "v", [Node("Array", "Array", [Node("int", "size"), elem])])

        member = Node("E", "data", [holder], meta=ALIGN)
        self.assertEqual(enc([{"c": read}], vector(member), boost=False), [d(("c", inner))])
        pair = Node("pair", "data", [holder, Node("int", "second")], meta=ALIGN)
        self.assertEqual(enc([(read, 7)], vector(pair), boost=False), [{"t": "tuple", "v": [inner, 7]}])
        r_type = RefType("R", Node("R", "Base", holder.m_Children))
        elem = Node(
            "ReferencedObject",
            "data",
            [Node("SInt64", "rid"), managed(), Node("ReferencedObjectData", "d")],
            meta=ALIGN,
        )
        value = [{"rid": 2, "type": {"class": "R", "ns": "NS", "asm": "Asm"}, "d": read}]
        typ = d(("class", "R"), ("ns", "NS"), ("asm", "Asm"))
        want = [d(("rid", 2), ("type", typ), ("d", inner))]
        self.assertEqual(enc(value, vector(elem), [r_type], boost=False), want)

    def test_the_pure_python_array_reader_checks_no_registry_of_an_element(self):
        elem = Node("Thing", "data", [registry("r"), Node("float", "x"), registry("x")], meta=ALIGN)
        v = Node("vector", "v", [Node("Array", "Array", [Node("int", "size"), elem])])
        value = [{"r": {"version": 1}, "x": {"version": 3}}]
        self.assertEqual(enc(value, v, boost=False), [d(("r", d(("version", 1))), ("x", d(("version", 3))))])
        # UnityPyBoost reads each element with the check: its `x` is the float.
        self.assertEqual(enc([{"r": {"version": 1}, "x": 2.5}], v), [d(("r", d(("version", 1))), ("x", f32(2.5)))])


class Depth(unittest.TestCase):
    def test_a_value_as_deep_as_unitypyboost_reads_encodes(self):
        # A ReferencedObject whose data holds the next, the shape UnityPyBoost reads deepest: 21,823 levels on
        # the oracle's 8 MiB stack (`tools/fixtures/walk.py --boost-reach`, as unityrs_dump::LIFTED_VALUE_DEPTH
        # records it). The encoder walks with a stack of its own, not Python's.
        node = Node("A", "Base", [])
        tree = ref(Node("SInt64", "rid"), managed(), Node("ReferencedObjectData", "data"))
        node.m_Children = [tree]
        ref_types = [RefType("A", node)]
        depth = 30_000
        value = {"ref": {"rid": 0, "type": {"class": "", "ns": "NS", "asm": "Asm"}}}
        for i in range(depth):
            value = {"ref": {"rid": i, "type": {"class": "A", "ns": "NS", "asm": "Asm"}, "data": value}}
        out = enc(value, node, ref_types)
        line = C.dumps(out)
        self.assertEqual(line.count('["data",'), depth)
        self.assertEqual(C.dumps(C.loads(line)), line)


if __name__ == "__main__":
    unittest.main()
