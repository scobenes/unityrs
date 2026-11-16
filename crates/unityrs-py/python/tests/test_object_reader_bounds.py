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
