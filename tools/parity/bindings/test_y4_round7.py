"""Round-seven findings against UnityPy 1.25.2 and the budget reference."""
import unittest
import UnityPy
import unityrs
import binding_y4_round7 as review


class RoundSeven(unittest.TestCase):
    def compare(self, name, corrected=False):
        self.assertEqual(UnityPy.__version__, "1.25.2")
        observe = getattr(review, name)
        expected = observe(UnityPy, corrected=True) if corrected else observe(UnityPy)
        self.assertEqual(observe(unityrs), expected)

    def test_streamed_resource_charge_survives_blob_staging_and_decode(self):
        self.compare("observe_streamed_lifetime", corrected=True)

    def test_cab_typed_protocol_and_save_fs(self):
        self.compare("observe_cab_protocol")

    def test_invalid_replacements_defer_errors_and_mark_parents(self):
        self.compare("observe_deferred_replacements")
