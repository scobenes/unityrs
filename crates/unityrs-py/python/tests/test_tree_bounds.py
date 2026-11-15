"""Resource refusals happen before Python graph or output expansion."""
import struct
import sys
import threading
import tracemalloc
import unittest
import weakref
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from unittest.mock import patch

from test_helpers import object_with_int, S
import child
import unityrs
from unityrs.helpers.TypeTreeNode import TypeTreeNode
from binding_y3_tree import Reader, Writer


def dag(depth=12):
    node = TypeTreeNode(0, "int", "value", 4, 1, m_TypeFlags=0, m_Index=0, m_MetaFlag=0)
    for _ in range(depth):
        node = TypeTreeNode(0, "Root", "Base", -1, 1, [node, node],
                            m_TypeFlags=0, m_Index=0, m_MetaFlag=0)
    return node


class TreeBounds(unittest.TestCase):
    def test_native_mutable_row_text_is_charged_before_copying(self):
        # Rust's bytearray copies are outside tracemalloc; use a fresh process's
        # peak RSS so earlier tests cannot hide their transient allocations.
        script = r'''
import resource, sys
sys.path.insert(0, sys.argv[1])
from test_helpers import object_with_int
import unityrs
class Buffer(bytearray):
    def __len__(self):
        return 0
obj = object_with_int(unityrs.Limits(max_input_memory=100000))
buffer = Buffer(32 << 20)
buffer[::4096] = b"x" * 8192
rows = [(0, buffer, buffer, 0, 0, None, None, None, 0, None)]
before = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
try:
    obj._native.parse_nodes(True, rows)
except MemoryError:
    pass
else:
    raise AssertionError("oversized mutable text accepted")
growth = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss - before
assert growth < 8192, f"mutable text copied before refusal: {growth} KiB"
'''
        proc = child.run([sys.executable, "-c", script, str(Path(__file__).parent)],
                         stdout=child.PIPE, stderr=child.PIPE, text=True, timeout=45)
        self.assertEqual(proc.returncode, 0, proc.stderr)

    def test_from_list_charges_keyword_count_and_text_before_unpacking(self):
        base = dict(m_Level=0, m_Type="Root", m_Name="Base")
        wide = {**base, **{f"extra{i}": 0 for i in range(100000)}}

        class Key(str):
            def __len__(self):
                return 0

        long_key = {**base, Key("x" * 100000): 0}
        for row in (wide, long_key):
            with self.subTest(keys=len(row)):
                budget = unityrs.Budget(10000)
                tracemalloc.start()
                try:
                    with self.assertRaises(MemoryError):
                        TypeTreeNode.from_list([row], budget=budget)
                    _, peak = tracemalloc.get_traced_memory()
                finally:
                    tracemalloc.stop()
                self.assertLess(peak, 100000, "keywords expanded before refusal")
                self.assertEqual(budget.used, 0)

    def test_from_list_keeps_bounded_keyword_errors(self):
        base = dict(m_Level=0, m_Type="Root", m_Name="Base")
        cases = [
            ({**base, "extra": 0}, "'extra' is an invalid keyword argument for this function"),
            ({**base, **{f"extra{i}": 0 for i in range(7)}},
             "function takes at most 11 keyword arguments (12 given)"),
            ({**base, 1: 0}, "keywords must be strings"),
        ]
        for row, message in cases:
            with self.subTest(message=message):
                with self.assertRaises(TypeError) as raised:
                    TypeTreeNode.from_list([row], budget=unityrs.Budget(100000))
                self.assertEqual(str(raised.exception), message)

    def test_list_override_refused_before_constructing_and_encoding_all_rows(self):
        class Counted(str):
            calls = 0

            def encode(self, *args):
                Counted.calls += 1
                return super().encode(*args)

        rows = [dict(m_Level=0 if i == 0 else 1, m_Type="int", m_Name=Counted("x"))
                for i in range(10000)]
        for limits in (unityrs.Limits(max_input_memory=100000),
                       unityrs.Limits(max_decode_work=10000)):
            Counted.calls = 0
            obj = object_with_int(limits)
            with self.assertRaises(MemoryError):
                obj.read_typetree(rows)
            self.assertLess(Counted.calls, 1000)

    def test_object_structure_dump_uses_object_limits(self):
        obj = object_with_int(unityrs.Limits(max_tree_depth=3))
        with self.assertRaises(MemoryError):
            obj.dump_typetree_structure(dag())

    def test_all_graph_expansions_bound_repeated_edges_and_cycles(self):
        operations = {
            "traverse": lambda n, **kw: list(n.traverse(**kw)),
            "rows": lambda n, **kw: list(n._native_rows(**kw)),
            "dump": lambda n, **kw: n.dump(Writer(), 3, **kw),
            "dump_blob": lambda n, **kw: n.dump_blob(Writer(), 18, **kw),
            "dump_structure": lambda n, **kw: n.dump_structure(**kw),
            "to_dict_list": lambda n, **kw: n.to_dict_list(**kw),
        }
        for name, operation in operations.items():
            for limits in (unityrs.Limits(max_tree_depth=3),
                           unityrs.Limits(max_decode_work=1000),
                           unityrs.Limits(max_input_memory=10000)):
                with self.subTest(operation=name, limits=limits):
                    with self.assertRaises(MemoryError):
                        operation(dag(), limits=limits)
            node = dag(0)
            node.m_Children.append(node)
            with self.subTest(operation=name, cycle=True):
                with self.assertRaisesRegex(ValueError, "cycle"):
                    operation(node)

    def test_dump_structure_charges_indentation_before_output(self):
        with self.assertRaises(MemoryError):
            dag(0).dump_structure(" " * 100000, limits=unityrs.Limits(max_input_memory=10000))

    def test_peek_bounds_child_scan_and_prefix_copy(self):
        root = dag(0)
        root.m_Name = "bounded-peek"
        root.m_Children = [dag(0)] * 10000
        with self.assertRaises(MemoryError):
            root.get_name_peek_node(limits=unityrs.Limits(max_decode_work=100))

    def test_peek_prefix_inherits_object_limits(self):
        root = dag(0)
        root.m_Name = "bounded-peek-source"
        obj = object_with_int(unityrs.Limits(max_tree_depth=3))
        root._tree_source = weakref.ref(obj.assets_file._native)
        root.m_Children = [dag(4), TypeTreeNode(1, "string", "m_Name", -1, 1)]
        prefix, _ = root.get_name_peek_node()
        with self.assertRaises(MemoryError):
            prefix.dump_structure()

    def test_equality_uses_inherited_limits_and_rejects_cycles(self):
        obj = object_with_int(unityrs.Limits(max_tree_depth=3))
        source = weakref.ref(obj.assets_file._native)
        left, right = dag(4), dag(4)
        left._tree_source = right._tree_source = source
        with self.assertRaises(MemoryError):
            left == right
        left, right = dag(0), dag(0)
        left.m_Children.append(left)
        right.m_Children.append(right)
        with self.assertRaisesRegex(ValueError, "cycle"):
            left == right

    def test_from_list_charges_before_large_name_encoding(self):
        class Unencodable(str):
            def encode(self, *args):
                raise AssertionError("encoded before refusal")

        with self.assertRaises(MemoryError):
            TypeTreeNode.from_list([dict(m_Level=0, m_Type="Root", m_Name=Unencodable("x" * 100000))],
                                   limits=unityrs.Limits(max_input_memory=10000))

    def test_parse_charges_child_count_before_allocation(self):
        data = b"Root\0Base\0" + struct.pack("<iiii", 0, 0, 1, 10000000)
        with self.assertRaises(MemoryError):
            TypeTreeNode.parse(Reader(data), 3, limits=unityrs.Limits(max_input_memory=10000))

    def test_parse_blob_charges_counts_before_read(self):
        reader = Reader(struct.pack("<ii", 10000000, 10000000))
        with self.assertRaises(MemoryError):
            TypeTreeNode.parse_blob(reader, 18, limits=unityrs.Limits(max_input_memory=10000))
        self.assertEqual(reader.Position, 8)

    def test_traversal_returns_budget_after_refusal(self):
        budget = unityrs.Budget(1000)
        with self.assertRaises(MemoryError):
            list(dag().traverse(budget=budget))
        self.assertEqual(budget.used, 0)

    def test_native_export_charges_each_decoded_name(self):
        root = S.Node("TextAsset", "Base", [S.Node("int", "x" * 500) for _ in range(100)])
        data = S.serialized_file([S.Type(49, root)], [S.Obj(1, 0, bytes(400))])
        obj = unityrs.Environment(data, limits=unityrs.Limits(max_decode_work=10000)).objects[0]
        with self.assertRaises(MemoryError):
            obj._native.nodes()

    def test_blob_dump_charges_common_string_reverse_table(self):
        table = {i: "common" + str(i) for i in range(10000)}
        with patch("unityrs.helpers.TypeTreeNode.get_common_strings", return_value=table):
            with self.assertRaises(MemoryError):
                dag(0).dump_blob(Writer(), 18, limits=unityrs.Limits(max_decode_work=100))

    def test_cold_common_string_export_refuses_before_cache_publication(self):
        import importlib
        from unityrs.helpers import _tpk
        helper = importlib.import_module("unityrs.helpers.TypeTreeNode")
        for provider in (None, _tpk.provider()):
            with self.subTest(provider=provider is not None):
                if provider is not None:
                    from unityrs._native import TreeTraversal
                    with self.assertRaises(MemoryError):
                        provider.common_string_versions(TreeTraversal(budget=unityrs.Budget(1)))
                with patch.dict(helper.COMMONSTRING_CACHE, {}, clear=True):
                    with patch.object(_tpk, "provider", return_value=provider):
                        with self.assertRaises(MemoryError):
                            dag(0).dump_blob(Writer(), 18, budget=unityrs.Budget(1))
                    self.assertEqual(helper.COMMONSTRING_CACHE, {})

    def test_versioned_common_strings_copy_only_the_selected_prefix(self):
        from unityrs.helpers import _tpk
        from unityrs.helpers.UnityVersion import UnityVersion

        class Strings(dict):
            visited = 0

            def items(self):
                for item in super().items():
                    self.visited += 1
                    yield item

        strings = Strings({i: "synthetic" for i in range(10000)})

        class Source:
            def common_strings(self, *args):
                return strings

            def common_string_versions(self, *args):
                return [(0, 1)]

        with patch.object(_tpk, "provider", return_value=Source()):
            self.assertEqual(_tpk.common_strings(UnityVersion.from_str("2021.3.2f1")), {0: "synthetic"})
        self.assertLessEqual(strings.visited, 2)

    def test_head_export_and_reconstruction_refusal_does_not_publish(self):
        import sys
        from test_helpers import ROOT
        sys.path.insert(0, str(ROOT / "tools/fixtures"))
        from head_read import sf_cases
        data = next(data for name, data in sf_cases() if name == "shared-<")
        for remaining in (1000, 15000):
            with self.subTest(remaining=remaining):
                budget = unityrs.Budget(1000000)
                obj = unityrs.Environment(data, budget=budget).objects[0]
                baseline = budget.used
                held = obj._native.tree_traversal()
                held.step(0, budget.limit - baseline - remaining, 0)
                with self.assertRaises(MemoryError):
                    obj._get_typetree_node()
                self.assertNotIn("_node", obj.__dict__)
                self.assertEqual(obj.assets_file._type_nodes, {})
                self.assertEqual(obj.assets_file._head_nodes, {})
                del held
                self.assertEqual(obj._get_typetree_node().m_Name, "Base")

    def test_lazy_same_type_graph_is_published_once_across_threads(self):
        root = S.Node("TextAsset", "Base", [S.Node("int", "value")])
        data = S.serialized_file([S.Type(49, root)],
                                 [S.Obj(i, 0, struct.pack("<i", i)) for i in (1, 2)])
        objects = unityrs.Environment(data).objects
        first_export = threading.Event()
        second_export = threading.Event()

        class NativeProxy:
            def __init__(self, native, first):
                self.native, self.first = native, first

            def nodes(self):
                rows = self.native.nodes()
                if self.first:
                    first_export.set()
                    second_export.wait(0.25)
                else:
                    second_export.set()
                return rows

            def __getattr__(self, name):
                return getattr(self.native, name)

        for i, obj in enumerate(objects):
            obj._native = NativeProxy(obj._native, i == 0)
        with ThreadPoolExecutor(2) as pool:
            first = pool.submit(objects[0]._get_typetree_node)
            self.assertTrue(first_export.wait(5))
            second = pool.submit(objects[1]._get_typetree_node)
            left, right = first.result(5), second.result(5)
        self.assertIs(left, right)
        left.m_Children[0].m_Name = "shared"
        self.assertEqual(objects[1].read_typetree(), {"shared": 2})

    def test_valid_version_never_converts_unused_fallback(self):
        with patch.object(unityrs.config, "FALLBACK_UNITY_VERSION", "bad\ud800"):
            self.assertEqual(object_with_int().read_typetree(), {"value": 7})
