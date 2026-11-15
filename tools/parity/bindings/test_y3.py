"""Installed-wheel Y3 parity; every comparison also has a recorded Rust replay."""

import importlib
import unittest

import UnityPy
import unityrs


class Helpers(unittest.TestCase):
    def compare(self, observer, function):
        self.assertEqual(UnityPy.__version__, "1.25.2")
        observe = getattr(importlib.import_module(observer), function)
        self.assertEqual(observe(unityrs), observe(UnityPy))

    def test_enums(self):
        self.compare("binding_y3_values", "observe_enums")

    def test_enum_fixture_keeps_pairs_and_checks_derived_behavior(self):
        from enum import IntEnum
        from binding_y3_values import _enum_record

        class Example(IntEnum):
            One = 1
            Alias = 1
        self.assertEqual(_enum_record(Example)["members"], [["One", 1], ["Alias", 1]])
        # Losing recorded redundancy must not lose the corresponding checks.
        for method, replacement in (("__int__", lambda self: 7),
                                    ("__str__", lambda self: "wrong"),
                                    ("__repr__", lambda self: "wrong"),
                                    ("__hash__", lambda self: 7),
                                    ("__bool__", lambda self: False)):
            with self.subTest(method=method):
                setattr(Example, method, replacement)
                try:
                    with self.assertRaises(AssertionError):
                        _enum_record(Example)
                finally:
                    delattr(Example, method)

    def test_versions(self):
        self.compare("binding_y3_values", "observe_versions")

    def test_trees(self):
        self.compare("binding_y3_tree", "observe")

    def test_config(self):
        self.compare("binding_y3_config", "observe_config")

    def test_mutable_nodes(self):
        self.compare("binding_y3_tree", "observe_mutation")

    def test_container(self):
        self.compare("binding_y3_config", "observe_container")

    def test_config_loads(self):
        self.compare("binding_y3_config", "observe_config_loads")

    def test_peek(self):
        self.compare("binding_y3_config", "observe_peek")

    def test_objects(self):
        self.compare("binding_y3_objects", "observe_objects")

    def test_head(self):
        self.compare("binding_y3_objects", "observe_head")

    def test_head_shared_mutations_across_lazy_types(self):
        from binding_y3_objects import _head_cross_type
        from binding_cases import typed
        for module in (unityrs, UnityPy):
            with self.subTest(module=module.__name__):
                result = _head_cross_type(module)
                self.assertEqual(result[0], {"value": typed({"part": {"across": 8}})})
                self.assertEqual(result[1:], [True, True, True])

    def test_common_strings_are_runtime_only(self):
        from UnityPy.helpers.TypeTreeNode import get_common_strings as oracle
        from unityrs.helpers.TypeTreeNode import get_common_strings as candidate
        from unityrs.helpers.UnityVersion import UnityVersion
        for version in (None, 0, (5, 6, 0), (2020, 1, 0, 3, 1), UnityVersion.from_str("5.6.0f1"), UnityVersion.from_str("2020.1.0f1")):
            with self.subTest(version=version):
                self.assertEqual(candidate(version), oracle(version))


if __name__ == "__main__":
    unittest.main()
