"""Construction expansion is charged while the native staging stays live."""
import gc
import importlib
from pathlib import Path
import struct
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))


class ConstructionBounds(unittest.TestCase):
    def test_shared_payload_copies_are_charged_before_conversion(self):
        import unityrs
        from binding_cases import S
        payload = b'x' * (4 * 1024 * 1024)
        for kind, expected in (('string', payload.decode()), ('TypelessData', payload)):
            tree = S.Node('Object', 'Base', [S.Node(kind, 'payload')])
            encoded = S.s32(payload)
            # All eight object rows point to the same stored payload.
            objects = [S.Obj(i, 0, b'', byte_size=len(encoded)) for i in range(1, 8)]
            objects.append(S.Obj(8, 0, encoded))
            data = S.serialized_file([S.Type(49, tree)], objects)
            for method in ('read', 'parse_as_dict'):
                with self.subTest(kind=kind, method=method):
                    env = unityrs.Environment(data, limits=unityrs.Limits(max_decode_work=128 * 1024))
                    for obj in env.objects:
                        with self.assertRaisesRegex(unityrs.LimitError, 'DecodeWork'):
                            getattr(obj, method)()
                    enough = unityrs.Environment(data)
                    result = getattr(enough.objects[0], method)()
                    self.assertEqual(result.payload if method == 'read' else result['payload'], expected)

    def test_unused_element_schema_is_charged_before_native_node_staging(self):
        import unityrs
        from binding_cases import S
        element = S.Node('Fixture', 'data', [S.Node('int', f'f{i}') for i in range(10_000)])
        tree = S.Node('Object', 'Base', [S.vec('items', element)])
        data = S.serialized_file([S.Type(49, tree)], [S.Obj(1, 0, struct.pack('<i', 0))])
        env = unityrs.Environment(data, limits=unityrs.Limits(max_decode_work=2 * 1024 * 1024))
        self.assertEqual(env.objects[0].parse_as_dict(), {'items': []})
        # Stop at native staging: the later Python _node_lists check cannot
        # mask materializing every unused node before enforcing the limit.
        with self.assertRaisesRegex(unityrs.LimitError, 'DecodeWork'):
            env.objects[0]._native.object_data()
        enough = unityrs.Environment(data)
        self.assertEqual(enough.objects[0].read().items, [])

    def test_default_fields_in_empty_array_elements_are_charged(self):
        import attrs
        import unityrs
        from binding_cases import S
        classes = importlib.import_module('unityrs.classes')
        fixture = attrs.make_class('Fixture', {f'f{i}': attrs.field(default=None)
                                             for i in range(200)}, slots=False)
        tree = S.Node('Object', 'Base', [S.vec('items', S.Node('Fixture', 'data'))])
        data = S.serialized_file([S.Type(49, tree)], [S.Obj(1, 0, struct.pack('<i', 200))])
        budget = unityrs.Budget(1_000_000)
        env = unityrs.Environment(data, budget=budget)
        with patch.object(classes, 'Fixture', fixture, create=True):
            for _ in range(2):
                with self.assertRaises(MemoryError):
                    env.objects[0].read()
            self.assertEqual(len(env.objects[0].parse_as_dict()['items']), 200)
        del env
        gc.collect()
        self.assertEqual(budget.used, 0)

    def test_construction_work_is_charged_to_open(self):
        import unityrs
        from binding_cases import S
        tree = S.Node('Object', 'Base', [S.Node('int', 'value')])
        data = S.serialized_file([S.Type(49, tree)], [S.Obj(1, 0, struct.pack('<i', 7))])
        # Parsing alone fits; reconstructing node records and objects does not.
        env = unityrs.Environment(data, limits=unityrs.Limits(max_decode_work=2048))
        self.assertEqual(env.objects[0].parse_as_dict()['value'], 7)
        with self.assertRaises(unityrs.LimitError):
            env.objects[0].read()

    def test_unused_reference_table_scan_is_charged(self):
        import unityrs
        from binding_cases import S
        tree = S.Node('Object', 'Base', [S.Node('int', 'value')])
        refs = [S.Type(-1, S.Node('int', 'Base'),
                       ref_names=[f'Fixture{i}', '', 'Assembly']) for i in range(100)]
        data = S.serialized_file([S.Type(49, tree)],
                                 [S.Obj(1, 0, struct.pack('<i', 7))], ref_types=refs)
        # Allow native node staging, then refuse the reference-table scan.
        env = unityrs.Environment(data, limits=unityrs.Limits(max_decode_work=12 * 1024))
        with self.assertRaises(unityrs.LimitError):
            env.objects[0]._native.object_data()


if __name__ == '__main__':
    unittest.main()
