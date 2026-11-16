"""Helper regressions retain cursor, callback and validation contracts."""
import importlib
import struct
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[4] / 'tools/parity'))
import binding_y6b_review as O
import unityrs


class HelperReview(unittest.TestCase):
    def test_shader_constructors_retain_owner_budget_to_handoff(self):
        module = importlib.import_module('unityrs.export.ShaderConverter')
        for program, boundary in ((False, 1024), (True, 1288)):
            budget = unityrs.Budget(4_000_000)
            obj, reader = O.shader_reader(unityrs, 2, program, budget=budget)
            baseline = budget.used
            for available in (boundary - 1, boundary):
                with self.subTest(program=program, available=available):
                    reader.Position = obj.byte_start
                    held = obj._native.tree_traversal()
                    held.step(0, budget.limit - budget.used - available, 0)
                    def construct():
                        return module.ShaderProgram(reader, (2019, 3)) if program else module.ShaderSubProgram(reader)
                    try:
                        if available < boundary:
                            with self.assertRaises(MemoryError):
                                construct()
                        else:
                            value = construct()
                            sub = value.m_SubPrograms[0] if program else value
                            self.assertEqual(sub.m_Keywords, ['', ''])
                            self.assertEqual(sub.m_ProgramCode, b'')
                            self.assertEqual(reader.Position - obj.byte_start, 44 if program else 40)
                    finally:
                        del held
                    self.assertEqual(budget.used, baseline)

    def test_shader_keyword_expansion_refuses_small_owner_budget(self):
        module = importlib.import_module('unityrs.export.ShaderConverter')
        for program in (False, True):
            budget = unityrs.Budget(16_000_000)
            obj, reader = O.shader_reader(unityrs, 20_000, program, budget=budget)
            baseline = budget.used
            held = obj._native.tree_traversal()
            held.step(0, budget.limit - budget.used - 4096, 0)
            try:
                with self.subTest(program=program), self.assertRaises(MemoryError):
                    if program:
                        module.ShaderProgram(reader, (2019, 3))
                    else:
                        module.ShaderSubProgram(reader)
            finally:
                del held
            self.assertEqual(budget.used, baseline)

    def test_tree_strings_accept_memoryview_and_align(self):
        for data, kind, result, position in O.observe_tree_memoryview_strings(unityrs):
            with self.subTest(data=data, kind=kind):
                value = ['str', 'edb3bf' if data == 'ff' else data]
                self.assertEqual(result, {'value': ['list', [value]] if kind == 'array' else value})
                self.assertEqual(position, 8)

    def test_memoryview_string_copy_is_charged_before_reading(self):
        helper = importlib.import_module('unityrs.helpers.TypeTreeHelper')
        node = unityrs.helpers.TypeTreeNode.TypeTreeNode(0, 'string', 'value', -1, 1, [])
        budget = unityrs.Budget(10_000)
        reader = unityrs.streams.EndianBinaryReader(struct.pack('<i', 4096) + b'x' * 4096, '<')
        original = reader.read_bytes
        def read_bytes(size):
            self.fail('string buffer copied before its charge')
        reader.read_bytes = read_bytes
        with self.assertRaises(MemoryError):
            helper.read_typetree(node, reader, check_read=False, budget=budget)
        self.assertEqual(reader.Position, 4)
        self.assertEqual(budget.used, 0)
        reader.Position = 0
        reader.read_bytes = lambda size: memoryview(original(size))
        self.assertEqual(helper.read_typetree(node, reader, check_read=False), 'x' * 4096)
        self.assertEqual(reader.Position, 4100)

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
