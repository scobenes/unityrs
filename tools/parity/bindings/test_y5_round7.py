"""Extractor destination conversion order and current-directory parity."""
import unittest

import UnityPy
import unityrs
import binding_y5_round7 as cases


class ReviewRound7(unittest.TestCase):
    def test_unused_destinations_are_not_converted(self):
        expected = cases.observe_unused_destinations(UnityPy)
        self.assertEqual(expected['tree'], {})
        for name, row in expected.items():
            if name == 'tree':
                continue
            if name.endswith('/real_empty'):
                self.assertEqual(row, {'value': ['list', []]})
            else:
                self.assertEqual(row[0], {'value': ['list', []]})
                self.assertEqual(row[1], ['reader', 'instance'] if '/parsed_filter/' in name else (
                    ['reader'] if '/filtered/' in name else []))
        self.assertEqual(cases.observe_unused_destinations(unityrs), expected)

    def test_source_errors_precede_destination_conversion(self):
        expected = cases.observe_source_error_order(UnityPy)
        for outcome, calls in expected.values():
            self.assertEqual(outcome, {'error': 'RuntimeError', 'message': 'source load failed'})
            self.assertEqual(calls, ['source'])
        self.assertEqual(cases.observe_source_error_order(unityrs), expected)

    def test_empty_destination_exports_into_scratch_cwd(self):
        expected = cases.observe_empty_root(UnityPy)
        self.assertTrue(all(row == expected[0] for row in expected))
        self.assertEqual(expected[0][1], {b'named.old'.hex(): b'hello\xff'.hex(),
                                        b'.txt'.hex(): b'empty'.hex()})
        self.assertEqual(cases.observe_empty_root(unityrs), expected)

    def test_empty_destination_keeps_name_validation(self):
        cases.check_empty_root_confinement(unityrs)
