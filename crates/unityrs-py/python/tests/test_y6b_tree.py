"""Standalone helpers retain traversal charges and refuse expansion first."""
import unittest
from unittest.mock import patch
import unityrs
from unityrs.helpers import TypeTreeHelper as helper
from unityrs.helpers.TypeTreeNode import TypeTreeNode


def node(ty='int', children=()):
    return TypeTreeNode(0, ty, 'value', -1, 1, list(children), m_MetaFlag=0)


class StandaloneBounds(unittest.TestCase):
    def test_selected_reference_schema_has_its_own_graph_bounds(self):
        from types import SimpleNamespace
        def named(ty, name, children=()):
            result = node(ty, children)
            result.m_Name = name
            return result
        descriptor = named('Descriptor', 'type', [named('string', part) for part in ('class', 'ns', 'asm')])
        root = named('ReferencedObject', 'Base', [descriptor, named('ReferencedObjectData', 'data')])
        selected, value = node(), 23
        for _ in range(8):
            selected, value = node('Synthetic', [selected]), {'value': value}
        names = {'class': 'Synthetic', 'ns': 'NS', 'asm': 'ASM'}
        file = SimpleNamespace(ref_types=[SimpleNamespace(m_ClassName='Synthetic', m_NameSpace='NS',
                                                          m_AssemblyName='ASM', node=selected)])
        prefix = unityrs.streams.EndianBinaryWriter(endian='<')
        helper.write_typetree(names, descriptor, prefix)
        data = prefix.bytes + b'\x17\0\0\0'
        expected = {'type': names, 'data': value}
        for tree_depth in (3, 9):
            limits = unityrs.Limits(max_tree_depth=tree_depth, max_value_depth=100)
            for accelerated in (False, True):
                reader = unityrs.streams.EndianBinaryReader(data, '<')
                if tree_depth == 3:
                    with self.assertRaises(unityrs.LimitError):
                        helper.read_typetree(root, reader, byte_size=len(data) if accelerated else None,
                                             check_read=False, assetsfile=file, limits=limits)
                    if not accelerated:
                        self.assertEqual(reader.Position, len(prefix.bytes))
                else:
                    self.assertEqual(helper.read_typetree(root, reader, byte_size=len(data) if accelerated else None,
                                     check_read=False, assetsfile=file, limits=limits), expected)
            writer = unityrs.streams.EndianBinaryWriter(endian='<')
            if tree_depth == 3:
                with self.assertRaises(unityrs.LimitError):
                    helper.write_typetree(expected, root, writer, assetsfile=file, limits=limits)
                self.assertEqual(writer.bytes, prefix.bytes)
            else:
                helper.write_typetree(expected, root, writer, assetsfile=file, limits=limits)
                self.assertEqual(writer.bytes, data)
        selected.m_Children.append(selected)
        reader = unityrs.streams.EndianBinaryReader(data, '<')
        with self.assertRaisesRegex(ValueError, 'cycle'):
            helper.read_typetree(root, reader, check_read=False, assetsfile=file)
        self.assertEqual(reader.Position, len(prefix.bytes))

    def test_value_depth_zero_refuses_pure_reads_and_writes_before_io(self):
        config = helper.TypeTreeConfig(True)
        for read, expected in (
                (lambda r, lim: helper.read_typetree(node(), r, check_read=False, limits=lim), 23),
                (lambda r, lim: helper.read_value(node(), r, config, limits=lim), 23),
                (lambda r, lim: helper.read_value_array(node(), r, config, 1, limits=lim), (23,))):
            reader = unityrs.streams.EndianBinaryReader(b'\x17\0\0\0', '<')
            with self.assertRaises(unityrs.LimitError):
                read(reader, unityrs.Limits(max_value_depth=0))
            self.assertEqual(reader.Position, 0)
            self.assertEqual(read(reader, unityrs.Limits(max_value_depth=1)), expected)
        writer = unityrs.streams.EndianBinaryWriter(endian='<')
        with self.assertRaises(unityrs.LimitError):
            helper.write_typetree(23, node(), writer, limits=unityrs.Limits(max_value_depth=0))
        self.assertEqual(writer.Position, 0)
        helper.write_typetree(23, node(), writer, limits=unityrs.Limits(max_value_depth=1))
        self.assertEqual(writer.bytes, b'\x17\0\0\0')

    def test_schema_and_value_depth_boundaries_are_independent(self):
        root = node('Synthetic', [node('Synthetic', [node()])])
        value = {'value': {'value': 23}}
        for tree_depth, value_depth, accepted in ((3, 3, True), (2, 10, False), (10, 2, False)):
            limits = unityrs.Limits(max_tree_depth=tree_depth, max_value_depth=value_depth)
            reader = unityrs.streams.EndianBinaryReader(b'\x17\0\0\0', '<')
            writer = unityrs.streams.EndianBinaryWriter(endian='<')
            if accepted:
                self.assertEqual(helper.read_typetree(root, reader, check_read=False, limits=limits), value)
                helper.write_typetree(value, root, writer, limits=limits)
                self.assertEqual(writer.bytes, b'\x17\0\0\0')
            else:
                with self.assertRaises(unityrs.LimitError):
                    helper.read_typetree(root, reader, check_read=False, limits=limits)
                with self.assertRaises(unityrs.LimitError):
                    helper.write_typetree(value, root, writer, limits=limits)
                self.assertEqual(reader.Position, 0)
                self.assertEqual(writer.Position, 0)

    def test_array_comprehension_depth_boundary(self):
        root = node('vector', [node('Array', [node(), node()])])
        for depth, accepted in ((2, False), (3, True)):
            limits = unityrs.Limits(max_value_depth=depth)
            reader = unityrs.streams.EndianBinaryReader(b'\1\0\0\0\x17\0\0\0', '<')
            writer = unityrs.streams.EndianBinaryWriter(endian='<')
            if accepted:
                self.assertEqual(helper.read_typetree(root, reader, check_read=False, limits=limits), [23])
                helper.write_typetree([23], root, writer, limits=limits)
                self.assertEqual(writer.bytes, b'\1\0\0\0\x17\0\0\0')
            else:
                with self.assertRaises(unityrs.LimitError):
                    helper.read_typetree(root, reader, check_read=False, limits=limits)
                with self.assertRaises(unityrs.LimitError):
                    helper.write_typetree([23], root, writer, limits=limits)

        # Direct array reads retain the oracle's comprehension/lambda depth,
        # independently of how many coroutine helpers implement them.
        config = helper.TypeTreeConfig(True)
        cases = [(node('string'), b'\0\0\0\0', 2, ['']),
                 (node('Synthetic', [node()]), b'\x17\0\0\0', 4, [{'value': 23}]),
                 (node('pair', [node('Synthetic', [node()]), node()]),
                  b'\x17\0\0\0\x18\0\0\0', 5, [({'value': 23}, 24)])]
        for root, data, required, expected in cases:
            reader = unityrs.streams.EndianBinaryReader(data, '<')
            with self.assertRaises(unityrs.LimitError):
                helper.read_value_array(root, reader, config, 1,
                                        limits=unityrs.Limits(max_value_depth=required - 1))
            reader.Position = 0
            self.assertEqual(helper.read_value_array(root, reader, config, 1,
                             limits=unityrs.Limits(max_value_depth=required)), expected)

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
        try:
            with patch('urllib.request.urlopen', side_effect=AssertionError('implicit download')):
                importlib.reload(update)
        finally:
            importlib.reload(update)

    def test_update_replaces_leaf_symlink_without_changing_its_target(self):
        import contextlib
        import importlib
        import io
        import scratch
        from binding_y6b_tree import update_tpk_peer
        update = importlib.import_module('unityrs.cli.update_tpk')
        old_tpk = getattr(unityrs.config, 'TPK', None)
        self.addCleanup(setattr, unityrs.config, 'TPK', old_tpk)
        with scratch.folder('tpk-link-') as root, update_tpk_peer() as (url, _):
            cache = root / 'cache'
            cache.mkdir()
            former = root / 'former.tpk'
            former.write_bytes(b'unchanged')
            target = cache / 'lzma.tpk'
            target.symlink_to(former)
            with patch.object(update, 'generate_classes'), contextlib.redirect_stdout(io.StringIO()):
                update.update_tpk(url=url, resource_path=cache)
            self.assertFalse(target.is_symlink())
            self.assertTrue(target.is_file())
            self.assertEqual(target.read_bytes(), b'synthetic-tpk')
            self.assertEqual(former.read_bytes(), b'unchanged')
            self.assertEqual(sorted(path.name for path in cache.iterdir()), ['lzma.tpk'])

    def test_update_refuses_symlinked_destination_components_before_writing(self):
        import contextlib
        import importlib
        import io
        import scratch
        from binding_y6b_tree import update_tpk_peer
        update = importlib.import_module('unityrs.cli.update_tpk')
        old_tpk = getattr(unityrs.config, 'TPK', None)
        self.addCleanup(setattr, unityrs.config, 'TPK', old_tpk)
        with scratch.folder('tpk-link-') as root, update_tpk_peer() as (url, _):
            outside = root / 'outside'
            outside.mkdir()
            link = root / 'link'
            link.symlink_to(outside, target_is_directory=True)
            for destination in (link, link / 'new' / 'cache'):
                with self.subTest(destination=destination.relative_to(root)), \
                     patch.object(update, 'generate_classes'), contextlib.redirect_stdout(io.StringIO()):
                    with self.assertRaises(OSError):
                        update.update_tpk(url=url, resource_path=destination)
                self.assertEqual(list(outside.iterdir()), [])
                self.assertEqual(unityrs.config.TPK, old_tpk)

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
