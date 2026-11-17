"""Every reader-bearing standalone tree path retains the input account."""
import gc
import importlib
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[4] / 'tools/parity'))
import binding_y6b_meter as O
import unityrs

helper = importlib.import_module('unityrs.helpers.TypeTreeHelper')


class TreeMeter(unittest.TestCase):
    def test_detached_schema_refuses_count_expansion_on_reader_budget(self):
        for kind in ('pure', 'value', 'array'):
            budget = unityrs.Budget(16_000_000)
            obj, payload = O.tree_reader(unityrs, 20_000, budget=budget)
            node = O.detached_node(unityrs)
            baseline = budget.used
            held = obj._native.tree_traversal()
            held.step(0, budget.limit - budget.used - 32768, 0)
            try:
                with self.subTest(kind=kind), self.assertRaises(MemoryError):
                    O.tree_call(unityrs, kind, node, obj, payload)
                self.assertEqual(obj.reader.Position - obj.byte_start, 4)
            finally:
                del held
            self.assertEqual(budget.used, baseline)

    def test_reader_account_precedes_a_different_schema_owner(self):
        schema_owner, _ = O.tree_reader(unityrs)
        node = schema_owner._get_typetree_node()
        for kind in ('pure', 'accelerated', 'value', 'array'):
            budget = unityrs.Budget(4_000_000)
            obj, payload = O.tree_reader(unityrs, budget=budget,
                                        limits=unityrs.Limits(max_value_depth=0))
            with self.subTest(kind=kind), self.assertRaises(unityrs.LimitError):
                O.tree_call(unityrs, kind, node, obj, payload, schema_owner.assets_file)

    def test_tree_read_schema_and_file_matrix_budget_boundary(self):
        # Binary search measures the actual peak, then checks the one-byte edge.
        for kind in ('pure', 'accelerated', 'value', 'array', 'boost'):
            for owned in (False, True):
                for attached in (False, True):
                    if kind == 'boost' and not (owned or attached):
                        continue  # Only caller bytes/schema: no reader argument.
                    budget = unityrs.Budget(4_000_000)
                    obj, payload = O.tree_reader(unityrs, budget=budget)
                    node = obj._get_typetree_node() if owned else O.detached_node(unityrs)
                    file = obj.assets_file if attached else None
                    baseline = budget.used
                    def attempt(available):
                        obj.reader.Position = obj.byte_start
                        held = obj._native.tree_traversal()
                        held.step(0, budget.limit - budget.used - available, 0)
                        try:
                            value = O.tree_call(unityrs, kind, node, obj, payload, file)
                            expected = (([23] * 3, 16) if kind == 'boost' else
                                        (23,) * 3 if kind == 'array' else [23] * 3)
                            self.assertEqual(value, expected)
                            return True
                        except MemoryError:
                            return False
                        finally:
                            del held
                            gc.collect()
                            self.assertEqual(budget.used, baseline)
                    with self.subTest(kind=kind, owned=owned, attached=attached):
                        self.assertFalse(attempt(0))
                        low, high = 0, 131072
                        self.assertTrue(attempt(high))
                        while high - low > 1:
                            mid = (low + high) // 2
                            if attempt(mid):
                                high = mid
                            else:
                                low = mid
                        self.assertFalse(attempt(high - 1))
                        self.assertTrue(attempt(high))

    def test_detached_schema_inherits_reader_value_limits_in_both_parsers(self):
        for kind in ('pure', 'accelerated', 'value', 'array'):
            obj, payload = O.tree_reader(unityrs, limits=unityrs.Limits(max_value_depth=0))
            with self.subTest(kind=kind), self.assertRaises(unityrs.LimitError):
                O.tree_call(unityrs, kind, O.detached_node(unityrs), obj, payload)

    def test_accelerated_detached_schema_keeps_guard_budget_inside_native_parser(self):
        budget = unityrs.Budget(16_000_000)
        obj, payload = O.tree_reader(unityrs, 10_000, budget=budget)
        baseline = budget.used
        held = obj._native.tree_traversal()
        held.step(0, budget.limit - budget.used - 100000, 0)
        guard = obj._native.tree_traversal()
        try:
            with self.assertRaises(MemoryError):
                helper.read_typetree_boost(payload, O.detached_node(unityrs), '<', True, _guard=guard)
        finally:
            del guard, held
        self.assertEqual(budget.used, baseline)

    def test_tree_writes_schema_and_file_matrix_budget_boundary(self):
        for owned, attached in ((True, False), (False, True), (True, True), (False, False)):
            for kind in ('tree', 'value'):
                budget = unityrs.Budget(4_000_000)
                obj, _ = O.tree_reader(unityrs, budget=budget)
                node = obj._get_typetree_node() if owned else O.detached_node(unityrs)
                file = obj.assets_file if attached else None
                baseline = budget.used
                for available in (0, 32768):
                    writer = unityrs.streams.EndianBinaryWriter(endian='<')
                    held = obj._native.tree_traversal()
                    held.step(0, budget.limit - budget.used - available, 0)
                    # Fully detached writes only have caller-owned values; opt in.
                    options = {} if owned or attached else {'budget': budget}
                    def write():
                        if kind == 'tree':
                            helper.write_typetree([23] * 3, node, writer, file, **options)
                        else:
                            helper.write_value([23] * 3, node, writer, helper.TypeTreeConfig(True, file), **options)
                    try:
                        with self.subTest(owned=owned, attached=attached, kind=kind, available=available):
                            if available:
                                write()
                                self.assertEqual(writer.bytes, b'\3\0\0\0' + b'\x17\0\0\0' * 3)
                            else:
                                with self.assertRaises(MemoryError):
                                    write()
                                self.assertEqual(writer.Position, 0)
                    finally:
                        del held
                    self.assertEqual(budget.used, baseline)
