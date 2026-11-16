"""Installed-wheel object writer review regressions."""

import sys
import unittest
from pathlib import Path

import UnityPy
import unityrs

import binding_y4_review_object
import child


class ObjectWriterReview(unittest.TestCase):
    def test_sequence_getters_run_in_source_order(self):
        actual = binding_y4_review_object.observe_sequence_writer(unityrs)
        expected = binding_y4_review_object.observe_sequence_writer(UnityPy)
        self.assertEqual(actual["order"], expected["order"])
        self.assertEqual(actual["calls"], [1, 2])

    def test_sequence_child_protocols_follow_target_shape(self):
        actual = binding_y4_review_object.observe_sequence_writer(unityrs)
        expected = binding_y4_review_object.observe_sequence_writer(UnityPy)
        for name in ("tuple_vector", "list_pair", "tuple_float_vector"):
            self.assertEqual(actual[name], expected[name])

    def test_nonnumeric_writer_exact_exceptions(self):
        self.assertEqual(binding_y4_review_object.observe_nonnumeric_writer(unityrs),
                         binding_y4_review_object.observe_nonnumeric_writer(UnityPy))

    def test_deep_writer_snapshot_on_small_python_stack(self):
        script = r'''
import struct, sys, threading
sys.path.insert(0, sys.argv[1])
import unityrs
from binding_cases import S
from unityrs.helpers.TypeTreeNode import TypeTreeNode

threading.stack_size(512 * 1024)
answers, errors = [], []
def save():
    try:
        tree = S.Node("Writer", "Base", [S.Node("int", "value")])
        data = S.serialized_file([S.Type(49, tree)], [S.Obj(1, 0, struct.pack("<i", 7))])
        file = unityrs.Environment(limits=unityrs.Limits(max_tree_depth=512)).load_file(data, name="deep.assets")
        root = TypeTreeNode(0, "Root", "Base", -1, 1)
        node = root
        value = cursor = {}
        for _ in range(500):
            child = TypeTreeNode(0, "Branch", "child", -1, 1)
            node.m_Children.append(child)
            node = child
            cursor["child"] = {}
            cursor = cursor["child"]
        node.m_Children.append(TypeTreeNode(0, "int", "value", 4, 1))
        cursor["value"] = 13
        answers.append(file.objects[1].save_typetree(value, nodes=root).hex())
    except BaseException as error:
        errors.append(repr(error))
thread = threading.Thread(target=save)
thread.start()
thread.join()
assert not errors, errors
assert answers == ["0d000000"], answers
'''
        result = child.run(
            [sys.executable, "-c", script, str(Path(__file__).resolve().parents[1])],
            stdout=child.PIPE, stderr=child.PIPE, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_writer_getter_reenters_owning_object_on_calling_thread(self):
        self.assertEqual(binding_y4_review_object.observe_reentrant_writer(unityrs),
                         binding_y4_review_object.observe_reentrant_writer(UnityPy))

    def test_numeric_kinds_match_writer_protocols_and_range_errors(self):
        self.assertEqual(UnityPy.__version__, "1.25.2")
        expected = binding_y4_review_object.observe_numeric_writer(UnityPy)
        actual = binding_y4_review_object.observe_numeric_writer(unityrs)
        for kind, cases in expected.items():
            for name, outcome in cases.items():
                with self.subTest(kind=kind, value=name):
                    self.assertEqual(actual[kind][name], outcome)


if __name__ == "__main__":
    unittest.main()
