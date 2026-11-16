"""Standalone helper observations compared against the pinned oracle."""
import unittest
import UnityPy
import unityrs
import binding_y6b_tree as observations


class StandaloneTrees(unittest.TestCase):
    def test_defaults_offsets_and_checks(self):
        self.assertEqual(observations.observe_tree_defaults(unityrs), observations.observe_tree_defaults(UnityPy))

    def test_values_and_writer_offsets(self):
        self.assertEqual(observations.observe_tree_values(unityrs), observations.observe_tree_values(UnityPy))

    def test_accelerated_callable(self):
        self.assertEqual(observations.observe_tree_boost(unityrs), observations.observe_tree_boost(UnityPy))

    def test_local_synthetic_assembly_discovery(self):
        self.assertEqual(observations.observe_generator_discovery(unityrs), observations.observe_generator_discovery(UnityPy))

    def test_explicit_optional_backend(self):
        self.assertEqual(observations.observe_generator_backend(unityrs), observations.observe_generator_backend(UnityPy))

    def test_explicit_update_uses_local_synthetic_peer(self):
        self.assertEqual(observations.observe_update_tpk(unityrs), observations.observe_update_tpk(UnityPy))

    def test_aligned_array_rules(self):
        self.assertEqual(observations.observe_tree_arrays(unityrs), observations.observe_tree_arrays(UnityPy))

    def test_failure_kinds_and_cursors(self):
        self.assertEqual(observations.observe_tree_failures(unityrs), observations.observe_tree_failures(UnityPy))

    def test_writer_failures_keep_partial_output(self):
        self.assertEqual(observations.observe_tree_write_failures(unityrs), observations.observe_tree_write_failures(UnityPy))

    def test_standalone_object_construction(self):
        self.assertEqual(observations.observe_tree_objects(unityrs), observations.observe_tree_objects(UnityPy))

    def test_accelerated_custom_class_namespace(self):
        self.assertEqual(observations.observe_tree_namespace(unityrs), observations.observe_tree_namespace(UnityPy))
