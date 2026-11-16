"""Standalone helpers retain traversal charges and refuse expansion first."""
import unittest
from unittest.mock import patch
import unityrs
from unityrs.helpers import TypeTreeHelper as helper
from unityrs.helpers.TypeTreeNode import TypeTreeNode


def node(ty='int', children=()):
    return TypeTreeNode(0, ty, 'value', -1, 1, list(children), m_MetaFlag=0)


class StandaloneBounds(unittest.TestCase):
    def test_budget_refuses_before_reader_or_writer_side_effects(self):
        for accelerated in (False, True):
            reader = unityrs.streams.EndianBinaryReader(b'\0' * 4, '<')
            with self.assertRaises(MemoryError):
                helper.read_typetree(node(), reader, byte_size=4 if accelerated else None,
                                     check_read=False, budget=unityrs.Budget(0))
            self.assertEqual(reader.Position, 0)
        writer = unityrs.streams.EndianBinaryWriter(endian='<')
        with self.assertRaises(MemoryError):
            helper.write_typetree(23, node(), writer, budget=unityrs.Budget(0))
        self.assertEqual(writer.Position, 0)

    def test_cycle_refused_before_reader_or_writer(self):
        root = node('Synthetic')
        root.m_Children.append(root)
        reader = unityrs.streams.EndianBinaryReader(b'\0' * 4)
        with self.assertRaisesRegex(ValueError, 'cycle'):
            helper.read_typetree(root, reader, check_read=False)
        self.assertEqual(reader.Position, 0)

    def test_array_count_charged_before_output_allocation(self):
        import struct
        root = node('vector', [node('Array', [node(), node()])])
        reader = unityrs.streams.EndianBinaryReader(struct.pack('<i', 1000000), '<')
        with self.assertRaises(MemoryError):
            helper.read_typetree(root, reader, check_read=False, limits=unityrs.Limits(max_input_memory=100000))
        self.assertEqual(reader.Position, 4)

    def test_update_import_performs_no_download(self):
        import importlib
        update = importlib.import_module('unityrs.cli.update_tpk')
        with patch('urllib.request.urlopen', side_effect=AssertionError('implicit download')):
            importlib.reload(update)

    def test_downloaded_raw_cache_is_discovered_without_oracle(self):
        import os
        import scratch
        from pathlib import Path
        from unityrs import _class_tpk
        from unityrs.helpers import _tpk
        with scratch.folder('y6b-tree-') as directory:
            cache = Path(directory) / 'unityrs'
            cache.mkdir()
            raw = cache / 'lzma.tpk'
            raw.write_bytes(b'synthetic-tpk')
            with patch.dict(os.environ, {'XDG_CACHE_HOME': str(directory)}), \
                 patch.dict(os.environ, {}, clear=False), \
                 patch.object(_class_tpk.importlib.util, 'find_spec', return_value=None):
                old = getattr(unityrs.config, 'TPK', None)
                unityrs.config.TPK = None
                try:
                    os.environ.pop('UNITYRS_TPK', None)
                    self.assertEqual(_class_tpk.source(), b'synthetic-tpk')
                    self.assertEqual(_tpk.path(), str(raw))
                finally:
                    unityrs.config.TPK = old

    def test_runtime_regeneration_keeps_schema_only_in_memory(self):
        import sys
        import child
        script = '''
import unityrs
from unityrs.cli.update_tpk import generate_classes
from unityrs.classes import generated
# A stale runtime class must disappear when the TPK no longer defines it.
generated.SyntheticStale = type('SyntheticStale', (), {'__module__': generated.__name__})
unityrs.classes.SyntheticStale = generated.SyntheticStale
generate_classes()
assert not hasattr(generated, 'SyntheticStale')
assert not hasattr(unityrs.classes, 'SyntheticStale')
from unityrs.classes.ClassIDTypeToClassMap import ClassIDTypeToClassMap
assert ClassIDTypeToClassMap[unityrs.enums.ClassIDType.TextAsset] is unityrs.classes.TextAsset
assert hasattr(unityrs.classes.Texture2D, 'image')
'''
        result = child.run([sys.executable, '-c', script], stdout=child.PIPE, stderr=child.PIPE,
                           text=True)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_accelerated_owned_node_inherits_native_value_limits_without_assetsfile(self):
        from test_helpers import object_with_int
        obj = object_with_int(unityrs.Limits(max_values_per_object=0))
        root = obj._get_typetree_node()
        root.m_Children.append(root.m_Children[0])
        with self.assertRaises((MemoryError, unityrs.LimitError)):
            helper.read_typetree_boost(b'\x07\0\0\0\x08\0\0\0', root, '<', True)

    def test_pure_owned_node_inherits_value_limit_without_assetsfile(self):
        from test_helpers import object_with_int
        obj = object_with_int(unityrs.Limits(max_values_per_object=1))
        reader = unityrs.streams.EndianBinaryReader(b'\x07\0\0\0', '<')
        with self.assertRaises(unityrs.LimitError):
            helper.read_typetree(obj._get_typetree_node(), reader, check_read=False)
        self.assertEqual(reader.Position, 0)

    def test_zero_size_limit_counts_discarded_work_only(self):
        import struct
        duplicate = node('Synthetic', [node('Empty'), node('Empty')])
        root = node('vector', [node('Array', [node('int'), duplicate])])
        limits = unityrs.Limits(max_zero_size_elements=0)
        reader = unityrs.streams.EndianBinaryReader(struct.pack('<i', 1), '<')
        with self.assertRaises(unityrs.LimitError):
            helper.read_typetree(root, reader, check_read=False, limits=limits)
        kept = node('vector', [node('Array', [node('int'), node('Empty')])])
        reader = unityrs.streams.EndianBinaryReader(struct.pack('<i', 2), '<')
        self.assertEqual(helper.read_typetree(kept, reader, check_read=False, limits=limits), [{}, {}])

    def test_accelerated_owned_node_inherits_near_full_budget_without_assetsfile(self):
        import struct
        from test_helpers import S
        schema = S.Node('TextAsset', 'Base', [S.Node('int', 'value')])
        data = S.serialized_file([S.Type(49, schema)], [S.Obj(1, 0, struct.pack('<i', 7))])
        budget = unityrs.Budget(4_000_000)
        obj = unityrs.Environment(data, budget=budget).objects[0]
        root = obj._get_typetree_node()
        held = obj._native.tree_traversal()
        held.step(0, budget.limit - budget.used - 100000, 0)
        with self.assertRaises(MemoryError):
            helper.read_typetree_boost(struct.pack('<i', 7) + b'\0' * 1000000, root, '<', True)
