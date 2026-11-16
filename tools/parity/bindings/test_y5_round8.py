"""Array staging policy and pinned-oracle reader behavior."""
import unittest

import UnityPy
import unityrs
import binding_y5_round8 as cases


class ReviewRound8(unittest.TestCase):
    def test_arrays_retain_oracle_values_and_cursors(self):
        self.assertEqual(cases.observe_array_reads(unityrs), cases.observe_array_reads(UnityPy))

    def test_aggregate_bytes_refuse_with_the_documented_budget(self):
        self.assertEqual(cases.observe_aggregate_bytes(UnityPy),
                         [['value', 2, True], ['value', 64, True]])
        self.assertEqual(cases.observe_aggregate_bytes(unityrs),
                         cases.observe_aggregate_bytes(UnityPy, corrected=True))
