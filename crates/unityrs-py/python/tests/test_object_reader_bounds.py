"""Native reader conversion staging stays charged until public array handoff."""
import struct
import sys
import unittest
from pathlib import Path
from types import SimpleNamespace

sys.path.insert(0, str(Path(__file__).resolve().parents[4] / 'tools/parity'))
from binding_cases import S
import unityrs


class ObjectReaderBounds(unittest.TestCase):
    def test_primitive_string_map_refuses_at_decoding_budget_boundary(self):
        from binding_y5_round2 import string_asset
        from unityrs.helpers.TypeTreeHelper import FUNCTION_READ_MAP
        budget = unityrs.Budget(1_000_000)
        obj = unityrs.Environment(string_asset(65_536), budget=budget).objects[0]
        reader = obj.reader
        for target in (obj, reader):
            for read in (lambda target: target.read_aligned_string(), FUNCTION_READ_MAP['string']):
                with self.subTest(target=type(target).__name__, read=read):
                    obj.reset()
                    start = obj.Position
                    baseline = budget.used
                    held = obj._native.tree_traversal()
                    # One byte below the decoding guard, with enough space for
                    # the raw byte read that formerly bypassed it.
                    held.step(0, budget.limit - budget.used - (256 + 5 * 65_536 - 1), 0)
                    retained = budget.used
                    try:
                        with self.assertRaises(MemoryError):
                            read(target)
                        self.assertEqual(obj.Position, start + 4)
                        self.assertEqual(budget.used, retained)
                    finally:
                        del held
                    self.assertEqual(budget.used, baseline)
                    obj.reset()
                    self.assertEqual(read(target), '\udcff' * 65_536)

    def test_primitive_array_maps_keep_native_staging_guards(self):
        from unityrs.helpers.TypeTreeHelper import FUNCTION_READ_MAP_ARRAY
        tree = S.Node('Y5ArrayBudget', 'Base', [S.Node('int', 'count')])
        data = S.serialized_file([S.Type(49, tree)],
                                 [S.Obj(1, 0, struct.pack('<i', 4096) + bytes(4096 * 8))])
        budget = unityrs.Budget(1_000_000)
        obj = unityrs.Environment(data, budget=budget).objects[0]
        for name, read in FUNCTION_READ_MAP_ARRAY.items():
            if name == 'SInt8':
                continue  # Raw bytes have no tuple conversion staging.
            with self.subTest(name=name):
                obj.reset()
                held = obj._native.tree_traversal()
                held.step(0, budget.limit - budget.used - 50_000, 0)
                retained = budget.used
                try:
                    with self.assertRaises(MemoryError):
                        read(obj)
                    self.assertEqual(budget.used, retained)
                finally:
                    del held

    def test_automatic_preload_index_charges_insertions(self):
        from unityrs.helpers.ContainerHelper import ContainerHelper
        tree = S.Node('Y5PreloadBudget', 'Base', [S.Node('int', 'value')])
        data = S.serialized_file([S.Type(49, tree)], [S.Obj(1, 0, bytes(4))])
        budget = unityrs.Budget(4 * 1024 * 1024)
        env = unityrs.Environment(data, budget=budget)
        file = env.objects[0].assets_file
        target = SimpleNamespace(assets_file=file)

        class Pointer:
            def __init__(self, path_id):
                self.path_id = path_id

            def deref(self):
                return target

        # Caller construction is outside the native meter. Automatic indexing
        # must still charge every file-owner path dictionary insertion.
        preload = [Pointer(index) for index in range(50_000)]
        source = SimpleNamespace(m_PreloadTable=preload, m_Container=[
            ('path', SimpleNamespace(asset=Pointer(-1), preloadIndex=0, preloadSize=len(preload)))])
        file._container = ContainerHelper(source)
        retained = budget.used
        with self.assertRaises(MemoryError):
            env._build_container_index()
        self.assertLess(len(file._container.path_dict), len(preload))
        self.assertIs(file._container._preload_table, preload)
        self.assertFalse(env._container_index_built)
        self.assertEqual(budget.used, retained)

    def test_declared_string_array_refuses_before_iteration(self):
        tree = S.Node('Y5ArrayBudget', 'Base', [S.Node('int', 'count')])
        data = S.serialized_file([S.Type(49, tree)],
                                 [S.Obj(1, 0, struct.pack('<i', 4_000_000))])
        budget = unityrs.Budget(4 * 1024 * 1024)
        obj = unityrs.Environment(data, budget=budget).objects[0]
        obj.reset()
        start = obj.Position
        retained = budget.used
        with self.assertRaises(MemoryError):
            obj.read_string_array()
        self.assertEqual(obj.Position, start + 4)
        self.assertEqual(budget.used, retained)


if __name__ == '__main__':
    unittest.main()
