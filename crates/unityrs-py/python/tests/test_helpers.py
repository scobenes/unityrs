"""Ownership and stack checks for mutable Python node overrides."""
import struct
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT / "tools/parity"))
sys.path.insert(0, str(ROOT / "tools/parity/tests"))
import child
import synth as S
import unityrs
from unityrs.helpers.TypeTreeNode import TypeTreeNode


def object_with_int(limits=None):
    root = S.Node("TextAsset", "Base", [S.Node("int", "value")])
    data = S.serialized_file([S.Type(49, root)], [S.Obj(1, 0, struct.pack("<i", 7))])
    return unityrs.Environment(data, limits=limits).objects[0]


def test_row_copy_releases_gil_between_python_extractions(save=False):
    """Invoked by the Rust test harness, which supplies the cfg(test) barrier."""
    import threading
    from unityrs import _native

    obj = object_with_int()
    barrier = _native._RowCopyBarrier()
    progress = []
    errors = []

    def other():
        try:
            barrier.wait_for_copy()
            progress.append(True)
            barrier.acknowledge()
        except BaseException as error:
            errors.append(error)

    thread = threading.Thread(target=other)
    thread.start()
    fields = (0, b"Root", b"Base", 0, 0, None, None, None, 0, None)

    class Row:
        def __getitem__(self, index):
            if index == 9:
                barrier.arm()
            return fields[index]

    def rows():
        yield Row()
        assert progress == [True], "copy resumed before Python acknowledgement"
        yield (1, b"int", b"value", 4, 1, None, None, None, 0, None)

    interval = sys.getswitchinterval()
    sys.setswitchinterval(30)
    try:
        if save:
            assert obj._native.save_typetree({"value": 13}, rows(), 8192) == struct.pack("<i", 13)
        else:
            assert obj._native.parse_nodes(True, rows()) == {"value": 7}
    finally:
        sys.setswitchinterval(interval)
        thread.join()
    assert not errors, errors


def test_core_call_releases_gil():
    """Invoked by the Rust test harness, which supplies the cfg(test) barrier:
    `parse_as_dict`'s core call, as it starts detached, waits for another
    Python thread to run and acknowledge, which it can only do while the GIL
    is released (held, the wait times out and the read raises)."""
    import threading
    from unityrs import _native

    node = S.Node("TextAsset", "Base", [S.vec("items", S.Node("int", "data"))])
    data = S.serialized_file(
        [S.Type(49, node)], [S.Obj(1, 0, struct.pack("<i", 32768) + b"\0" * (32768 * 4))])
    obj = unityrs.Environment(data).objects[0]
    barrier = _native._RowCopyBarrier()
    progress = []
    errors = []

    def other():
        try:
            barrier.wait_for_copy()
            progress.append(True)
            barrier.acknowledge()
        except BaseException as error:
            errors.append(error)

    thread = threading.Thread(target=other)
    thread.start()
    interval = sys.getswitchinterval()
    # No switch while the caller holds the GIL: the other thread runs only
    # where the core call releases it.
    sys.setswitchinterval(30)
    try:
        barrier.arm_core()
        value = obj.parse_as_dict()
    finally:
        sys.setswitchinterval(interval)
        thread.join()
    assert not errors, errors
    assert progress == [True], "no other Python thread ran during the core call"
    assert value == {"items": [0] * 32768}, "the read's value changed"


class TypeTreeHelpers(unittest.TestCase):
    def test_head_reconstruction_retries_after_python_allocation_failure(self):
        from unittest.mock import patch
        sys.path.insert(0, str(ROOT / "tools/fixtures"))
        from head_read import sf_cases
        data = next(data for name, data in sf_cases() if name == "shared-<")
        obj = unityrs.Environment(data).objects[0]
        initialise = TypeTreeNode.__init__
        calls = 0

        def failing_init(self, *args, **kwargs):
            nonlocal calls
            calls += 1
            if calls == 2:
                raise MemoryError("injected node allocation refusal")
            initialise(self, *args, **kwargs)

        with patch.object(TypeTreeNode, "__init__", failing_init):
            with self.assertRaises(MemoryError):
                obj._get_typetree_node()
        node = obj._get_typetree_node()
        self.assertEqual(len(node.m_Children), 2)
        self.assertEqual(obj.parse_as_dict(), {"left": {"answer": 37}, "right": {"answer": 38}})
        self.assertIs(node.m_Children[0].m_Children, node.m_Children[1].m_Children)

    def test_head_replay_needs_no_unitypy_import(self):
        script = r'''
import builtins, importlib.util, sys
sys.path.insert(0, sys.argv[1])
original_spec = importlib.util.find_spec
importlib.util.find_spec = lambda name, *a, **kw: None if name == "UnityPy" else original_spec(name, *a, **kw)
original_import = builtins.__import__
def checked_import(name, *a, **kw):
    if name == "UnityPy" or name.startswith("UnityPy."):
        raise AssertionError("replay imported the oracle")
    return original_import(name, *a, **kw)
builtins.__import__ = checked_import
import unityrs
from binding_y3_objects import observe_head
assert len(observe_head(unityrs)) == 100
assert "UnityPy" not in sys.modules
'''
        proc = child.run([sys.executable, "-c", script, str(ROOT / "tools/parity")],
                         stdout=child.PIPE, stderr=child.PIPE, text=True, timeout=45)
        self.assertEqual(proc.returncode, 0, proc.stderr)

    def test_flatten_uses_links_and_preserves_repeated_node_identities(self):
        leaf = TypeTreeNode(99, "int", "value", 4, 1)
        branch = TypeTreeNode(20, "Thing", "branch", -1, 1, [leaf])
        root = TypeTreeNode(10, "Root", "Base", -1, 1, [branch, branch])
        rows = list(root._native_rows())
        self.assertEqual([row[0] for row in rows], [0, 1, 2, 1, 2])
        self.assertEqual([row[2] for row in rows], ["Base", "branch", "value", "branch", "value"])
        self.assertIs(root.m_Children[0], root.m_Children[1])
        self.assertEqual(leaf.m_Level, 99)

    def test_explicit_dag_refused_before_expansion_and_encoding(self):
        class Counted(str):
            calls = 0

            def encode(self, *args):
                Counted.calls += 1
                return super().encode(*args)

        class CountedNode(TypeTreeNode):
            visits = 0

            def __getattribute__(self, name):
                if name == "m_Name":
                    CountedNode.visits += 1
                return super().__getattribute__(name)

        for limits in (unityrs.Limits(max_tree_depth=3),
                       unityrs.Limits(max_decode_work=10000),
                       unityrs.Limits(max_input_memory=100000)):
            with self.subTest(limits=limits):
                obj = object_with_int(limits)
                node = TypeTreeNode(0, "int", "leaf", 4, 1)
                for _ in range(16):
                    node = CountedNode(0, "Thing", Counted("x"), -1, 1, [node, node])
                Counted.calls = 0
                CountedNode.visits = 0
                with self.assertRaises(MemoryError):
                    obj.read_typetree(node)
                self.assertLess(Counted.calls, 1000)
                self.assertLess(CountedNode.visits, 1000)

    def test_explicit_large_name_refused_before_encoding(self):
        class Unencodable(str):
            def encode(self, *args):
                raise AssertionError("encoded before memory refusal")

        obj = object_with_int(unityrs.Limits(max_input_memory=100000))
        node = TypeTreeNode(0, "Root", "Base", -1, 1)
        node.m_Name = Unencodable("x" * 1000000)
        with self.assertRaises(MemoryError):
            obj.read_typetree(node)

    def test_from_list_preserves_and_relinks_existing_nodes(self):
        leaf = TypeTreeNode(1, "int", "value", 4, 1)
        root = TypeTreeNode(0, "Root", "Base", -1, 1, [leaf])
        records = root.to_dict_list()
        self.assertIs(records[0]["m_Children"], root.m_Children)
        rebuilt = TypeTreeNode.from_list(list(root.traverse()))
        self.assertIs(rebuilt, root)
        self.assertIs(root.m_Children[0], root.m_Children[1])
        self.assertIs(records[0]["m_Children"][1], leaf)

    def test_explicit_node_cycle_is_refused_before_native_conversion(self):
        obj = object_with_int()
        root = TypeTreeNode(0, "Root", "Base", -1, 1)
        root.m_Children.append(root)
        with self.assertRaisesRegex(ValueError, "cycle"):
            obj.read_typetree(root)
        root.m_Children.clear()
        self.assertEqual(obj.read_typetree(), {"value": 7})

    def test_explicit_nodes_respect_configured_depth_limit(self):
        obj = object_with_int(unityrs.Limits(max_tree_depth=3))
        root = TypeTreeNode(0, "Root", "Base", -1, 1)
        branch = root
        for _ in range(3):
            node = TypeTreeNode(1, "Thing", "x", -1, 1)
            branch.m_Children.append(node)
            branch = node
        branch.m_Children.append(TypeTreeNode(1, "int", "value", 4, 1))
        with self.assertRaises(unityrs.LimitError):
            obj.read_typetree(root)
        self.assertEqual(obj.read_typetree(), {"value": 7})

    def test_deep_explicit_nodes_on_small_python_stack(self):
        script = r'''
import gc, struct, sys, threading
sys.path.insert(0, sys.argv[1])
import synth as S
import unityrs
from unityrs.helpers.TypeTreeNode import TypeTreeNode
root = S.Node("TextAsset", "Base", [S.Node("int", "value")])
data = S.serialized_file([S.Type(49, root)], [S.Obj(1, 0, struct.pack("<i", 7))])
depth = 3000
threading.stack_size(512 * 1024)
answers = []
errors = []
def read():
    try:
        budget = unityrs.Budget(1 << 28)
        obj = unityrs.Environment(data, budget=budget, limits=unityrs.Limits(
            max_tree_depth=depth + 2, max_value_depth=depth + 2)).objects[0]
        root = TypeTreeNode(0, "Root", "Base", -1, 1)
        nodes = [root]
        for _ in range(depth):
            node = TypeTreeNode(0, "Thing", "x", -1, 1)
            nodes[-1].m_Children.append(node)
            nodes.append(node)
        nodes[-1].m_Children.append(TypeTreeNode(0, "int", "value", 4, 1))
        value = obj.read_typetree(root)
        for _ in range(depth):
            value = value["x"]
        answers.append(value["value"])
        for node in nodes:
            node.m_Children.clear()
        del obj, value
        gc.collect()
        assert budget.used == 0, budget.used
    except BaseException as error:
        errors.append(repr(error))
thread = threading.Thread(target=read)
thread.start()
thread.join()
assert not errors, errors
assert answers == [7], answers
'''
        proc = child.run([sys.executable, "-c", script, str(ROOT / "tools/parity/tests")],
                         stdout=child.PIPE, stderr=child.PIPE, text=True, timeout=45)
        self.assertEqual(proc.returncode, 0, proc.stderr)
