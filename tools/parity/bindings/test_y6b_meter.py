"""Reader-backed standalone calls agree with the pinned oracle."""
import unittest
import UnityPy
import unityrs
import binding_y6b_meter as O


class ReaderMeterParity(unittest.TestCase):
    def test_tree_schema_and_file_matrix(self):
        self.assertEqual(O.observe_tree_asset_meter(unityrs), O.observe_tree_asset_meter(UnityPy))

    def test_detached_schema_count_expansion(self):
        self.assertEqual(O.observe_tree_count_expansion(unityrs), O.observe_tree_count_expansion(UnityPy))

    def test_reference_lookup_with_native_asset_file(self):
        self.assertEqual(O.observe_reference_asset_meter(unityrs), O.observe_reference_asset_meter(UnityPy))

    def test_empty_shader_blob_with_native_asset_reader(self):
        self.assertEqual(O.observe_shader_asset_stages(unityrs), O.observe_shader_asset_stages(UnityPy))

    def test_decryptor_with_native_asset_reader(self):
        self.assertEqual(O.observe_decryptor_asset_meter(unityrs), O.observe_decryptor_asset_meter(UnityPy))
