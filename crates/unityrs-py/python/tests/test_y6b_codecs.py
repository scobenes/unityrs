"""Callback staging and reader snapshots retain the owning input's limits."""
import importlib
import struct
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[4] / 'tools/parity'))
import binding_cases as B
import unityrs


class CodecBounds(unittest.TestCase):
    def test_callback_failure_preserves_original_exception(self):
        codec = importlib.import_module('unityrs.helpers.CompressionHelper')
        original = codec.DECOMPRESSION_MAP[0]
        failure = LookupError('callback object')
        data = B.S.fs_bundle([(b'raw', b'synthetic', 0)], codec=0)
        def fail(*args):
            raise failure
        try:
            codec.DECOMPRESSION_MAP[0] = fail
            with self.assertRaises(LookupError) as raised:
                unityrs.Environment(data)
            self.assertIs(raised.exception, failure)
        finally:
            codec.DECOMPRESSION_MAP[0] = original

    def test_callback_output_uses_bundle_block_limit(self):
        codec = importlib.import_module('unityrs.helpers.CompressionHelper')
        original = codec.DECOMPRESSION_MAP[0]
        data = B.S.fs_bundle([(b'raw', b'synthetic', 0)], codec=0)
        try:
            codec.DECOMPRESSION_MAP[0] = lambda data, size: b'x' * 1024
            with self.assertRaises(unityrs.LimitError):
                unityrs.Environment(data, limits=unityrs.Limits(max_block_output=256))
        finally:
            codec.DECOMPRESSION_MAP[0] = original

    def test_zero_chunk_size_has_a_finite_refusal(self):
        codec = importlib.import_module('unityrs.helpers.CompressionHelper')
        original = codec.COMPRESSION_CHUNK_SIZE_MAP[2]
        data = B.S.fs_bundle([(b'raw', b'synthetic', 0)], codec=0)
        bundle = next(iter(unityrs.Environment(data).files.values()))
        try:
            codec.COMPRESSION_CHUNK_SIZE_MAP[2] = 0
            with self.assertRaisesRegex(ValueError, 'codec chunk size must be positive'):
                bundle.save(packer=(0x40, 2))
        finally:
            codec.COMPRESSION_CHUNK_SIZE_MAP[2] = original

    def test_reader_snapshot_refuses_before_consuming_when_budget_full(self):
        data = next(data for name, data in B.cases() if name == 'text')
        budget = unityrs.Budget(4_000_000)
        env = unityrs.Environment(data, budget=budget)
        obj = env.objects[0]
        obj.reader = unityrs.streams.EndianBinaryReader(b'\0' * obj.byte_start + struct.pack('<i', 1), '<')
        guard = obj._native.tree_traversal()
        guard.step(0, budget.limit - budget.used, 0)
        try:
            with self.assertRaises(MemoryError):
                obj.parse_as_dict()
        finally:
            del guard

    def test_reader_copy_bound_precedes_a_large_read(self):
        data = next(data for name, data in B.cases() if name == 'text')
        obj = unityrs.Environment(data).objects[0]
        obj.reader = unityrs.streams.EndianBinaryReader(b'\0' * obj.byte_start, '<')
        obj.byte_size = 2**50
        with self.assertRaises(unityrs.LimitError):
            obj.parse_as_dict()

    def test_pure_reader_failure_keeps_progress(self):
        tree = B.S.Node('Synthetic', 'Base', [B.S.Node('int', 'first'), B.S.Node('int', 'second')])
        data = B.S.serialized_file([B.S.Type(49, tree)], [B.S.Obj(1, 0, struct.pack('<i', 7), byte_size=0)])
        obj = unityrs.Environment(data).objects[0]
        obj.reader = unityrs.streams.EndianBinaryReader(struct.pack('<i', 7), '<')
        obj.byte_start = 0
        with self.assertRaises(struct.error):
            obj.parse_as_dict()
        self.assertEqual(obj.Position - obj.byte_start, 8)
