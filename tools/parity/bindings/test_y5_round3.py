"""Installed-wheel reentrancy and complete array-map replay comparisons."""
import sys
import unittest
from pathlib import Path

import UnityPy
import unityrs
import binding_y5_round3 as cases
import child
from tests.dump_harness import DEADLOCK_GUARD_S


class ReviewRound3(unittest.TestCase):
    def guarded(self, function):
        script = ('import sys; sys.path.insert(0, sys.argv[1]); import unityrs; '
                  'from binding_y5_round3 import ' + function + '; ' + function + '(unityrs)')
        result = child.run([sys.executable, '-c', script, str(Path(__file__).resolve().parents[1])],
                           stdout=child.PIPE, stderr=child.PIPE, text=True, timeout=DEADLOCK_GUARD_S)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_result_allocations_reenter_on_caller(self):
        self.guarded('check_worker_output_reentry')

    def test_resource_error_callbacks_reenter_on_caller(self):
        self.guarded('check_worker_error_reentry')

    def test_snapshot_protocols_and_configuration_callbacks_reenter(self):
        self.guarded('check_snapshot_reentry')

    def test_all_native_row_readers_keep_iteration_extraction_and_drop_on_caller(self):
        self.guarded('check_row_protocol_reentry')

    def test_reentrant_sync_creates_reader_with_source_policy(self):
        self.guarded('check_reader_policy_publication')

    def test_generated_node_getters_reenter_on_caller(self):
        script = '''
import sys
sys.path.insert(0, sys.argv[1])
import UnityPy, unityrs
from binding_y5_round3 import observe_generator_reentry
assert observe_generator_reentry(unityrs) == observe_generator_reentry(UnityPy) == [77, 77]
'''
        result = child.run([sys.executable, '-c', script, str(Path(__file__).resolve().parents[1])],
                           stdout=child.PIPE, stderr=child.PIPE, text=True, timeout=DEADLOCK_GUARD_S)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_negative_and_none_reader_sizes_match_oracle(self):
        self.assertEqual(cases.observe_negative_readers(unityrs), cases.observe_negative_readers(UnityPy))

    def test_negative_string_decoding_uses_owning_budget(self):
        self.assertEqual(cases.observe_negative_string_budget(unityrs),
                         cases.observe_negative_string_budget(UnityPy, corrected=True))

    def test_array_map_values_and_cursors_match_oracle(self):
        self.assertEqual(cases.observe_array_maps(unityrs), cases.observe_array_maps(UnityPy))
