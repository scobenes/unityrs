"""Six round-six findings against UnityPy 1.25.2 and safety references."""
import unittest
import UnityPy
import unityrs
import binding_y4_round6 as review


class RoundSix(unittest.TestCase):
    def compare(self, name, corrected=False):
        self.assertEqual(UnityPy.__version__, "1.25.2")
        observe = getattr(review, name)
        expected = observe(UnityPy, corrected=True) if corrected else observe(UnityPy)
        self.assertEqual(observe(unityrs), expected)

    def test_blob_charge_survives_decode_and_encode_staging(self):
        self.compare("observe_blob_lifetime", corrected=True)

    def test_cab_inherits_existing_raw_resource_settings(self):
        self.compare("observe_resource_inheritance")

    def test_material_uses_current_texture_pointer_owner(self):
        self.compare("observe_pointer_owner", corrected=True)

    def test_serialized_save_preserves_logical_payload_length(self):
        self.compare("observe_payload_lengths")

    def test_none_clears_replacement_and_marks_parents(self):
        self.compare("observe_clear_replacement")

    def test_renderer_exports_detached_texture_free_material(self):
        self.compare("observe_detached_material")
