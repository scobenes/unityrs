"""Round-five fixes compared with the pinned oracle and explicit safety references."""
import unittest
import UnityPy
import unityrs
import binding_y4_round5 as review


class RoundFive(unittest.TestCase):
    def compare(self, name, corrected=False):
        self.assertEqual(UnityPy.__version__, "1.25.2")
        observe = getattr(review, name)
        expected = observe(UnityPy, corrected=True) if corrected else observe(UnityPy)
        self.assertEqual(observe(unityrs), expected)

    def test_typeless_data_preserves_logical_buffer_length(self):
        self.compare("observe_buffer_lengths")

    def test_vectors_measure_before_iterating(self):
        self.compare("observe_vector_lengths")

    def test_cab_path_validation_precedes_all_mutation(self):
        self.compare("observe_cab_validation", corrected=True)

    def test_array_images_charge_storage_before_slicing(self):
        self.compare("observe_array_storage", corrected=True)

    def test_output_names_discard_subclass_protocols(self):
        self.compare("observe_exact_output_names", corrected=True)

    def test_native_snapshot_allows_competing_python_thread(self):
        self.compare("observe_snapshot_threads", corrected=True)

    def test_budget_modes_reach_the_named_argument(self):
        from binding_y4_budget import native_cases, python_case
        for name, call in native_cases(unityrs):
            if name in ("typetree_endian", "web_packer", "bundle_packer", "bundle_mode", "encode_mode", "sprite_mode"):
                with self.subTest(boundary=name), self.assertRaisesRegex(MemoryError, "refused 32000256 bytes"):
                    call()
        for name in ("bundle", "bundle_fs", "bundle_web_raw", "web"):
            with self.subTest(boundary=name), self.assertRaisesRegex(MemoryError, "refused 67108864 bytes"):
                python_case(unityrs, name)()

    def test_array_images_keep_builtin_sequence_inputs(self):
        self.compare("observe_array_inputs")
