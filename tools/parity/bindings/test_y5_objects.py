"""Direct oracle comparisons for final object and file binding rows."""
import unittest

import UnityPy
import unityrs

import binding_y5_objects as cases


class FinalObjectRows(unittest.TestCase):
    def compare(self, observe):
        self.assertEqual(UnityPy.__version__, '1.25.2')
        self.assertEqual(observe(unityrs), observe(UnityPy))

    def test_patch_forwards_nodes_writer_and_preserves_original_reads(self):
        self.compare(cases.observe_patch)

    def test_reader_forwarding_and_class_map(self):
        self.compare(cases.observe_forwarding)

    def test_gameobject_component_spellings_and_accessors(self):
        self.assertEqual(cases.observe_gameobject(unityrs),
                         cases.observe_gameobject(UnityPy, corrected=True))

    def test_file_container_union_and_own_object_paths(self):
        self.compare(cases.observe_containers)

    def test_file_generators_preserve_empty_and_nested_cases(self):
        self.assertEqual(cases.observe_iteration(unityrs, corrected=True),
                         cases.observe_iteration(UnityPy, corrected=True))

    def test_monobehaviour_explicit_and_automatic_generator_hooks(self):
        self.compare(cases.observe_generators)


if __name__ == '__main__':
    unittest.main()
