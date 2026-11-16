"""Installed-wheel comparisons for the second Y5 review."""
import unittest
import UnityPy
import unityrs
import binding_y5_round2 as cases


class ReviewRound2(unittest.TestCase):
    def test_primitive_string_map_obeys_the_owning_input_budget(self):
        self.assertEqual(cases.observe_primitive_string_budget(unityrs),
                         cases.observe_primitive_string_budget(UnityPy, corrected=True))

    def test_nonbuffer_key_does_not_affect_ordinary_loads(self):
        self.assertEqual(cases.observe_deferred_key(unityrs), cases.observe_deferred_key(UnityPy))
