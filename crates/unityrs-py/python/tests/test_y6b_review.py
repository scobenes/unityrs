"""Helper regressions retain cursor, callback and validation contracts."""
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[4] / 'tools/parity'))
import binding_y6b_review as O
import unityrs


class HelperReview(unittest.TestCase):
    def test_schema_failure_keeps_original_cursor(self):
        for stream, wrap, result, position in O.observe_schema_failure_cursor(unityrs):
            with self.subTest(stream=stream, wrap=wrap):
                self.assertEqual(result['error'], 'KeyError')
                self.assertEqual(position, 7)

    def test_truncated_objects_keep_actual_stream_advance(self):
        for size, stream, wrap, check, result, position in O.observe_truncated_object_cursor(unityrs):
            with self.subTest(size=size, stream=stream, wrap=wrap, check=check):
                self.assertEqual(position, size if stream else 100)
                if size == 64 and not check:
                    self.assertEqual(result['value'], ['int', 23])
                else:
                    self.assertIn('error', result)

    def test_live_string_and_byte_callbacks_run_on_caller(self):
        for ty, mode, kind, result, position, events in O.observe_tree_live_callbacks(unityrs):
            with self.subTest(ty=ty, mode=mode, kind=kind):
                if kind == 'array':
                    self.assertEqual(events, [])
                    self.assertEqual(position, 8)
                    self.assertEqual(result['value'], ['list', [['str', '78']]])
                    continue
                self.assertEqual(events, [[0, True]])
                if mode == 'fail':
                    self.assertEqual(result['error'], 'LookupError')
                    self.assertEqual(position, 0)
                elif kind in ('tree', 'value') and mode == 'return':
                    self.assertEqual(result['value'], ['str', '637573746f6d'] if ty == 'string'
                                     else ['bytes', '637573746f6d'])
                    self.assertEqual(position, 0)

    def test_empty_shader_blob_still_decompresses_platforms(self):
        results = O.observe_shader_empty_blob(unityrs)
        self.assertIn('value', results[0])
        self.assertEqual(results[1]['error'], 'LZ4BlockError')
        self.assertEqual(results[2]['error'], 'LZ4BlockError')
        self.assertEqual(results[3], results[0])

    def test_lz4_signed_arguments_and_header_rules(self):
        for kind, size, result in O.observe_lz4_signed_sizes(unityrs):
            with self.subTest(kind=kind, size=size):
                if kind == 'argument' and size in (-2**31 - 1, 2**31):
                    self.assertEqual(result['error'], 'OverflowError')
                elif kind == 'header' and size >= 2**31:
                    self.assertEqual(result, {'error': 'ValueError', 'message': f'Invalid size: 0x{size}'})
                elif kind == 'header' and size in (0, 2):
                    self.assertEqual(result['error'], 'LZ4BlockError')
                else:
                    self.assertEqual(result['value'], ['bytes', '78'])
