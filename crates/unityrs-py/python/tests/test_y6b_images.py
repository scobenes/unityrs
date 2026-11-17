"""Ownership and thread isolation of standalone image adapters."""
import importlib
import threading
import unittest
from types import SimpleNamespace
from PIL import Image
import unityrs


class ImageAdapterBounds(unittest.TestCase):
    def test_switch_predicate_never_expands_the_exponent(self):
        from unittest.mock import patch
        helper = importlib.import_module('unityrs.helpers.TextureSwizzler')
        with patch.object(helper, 'get_switch_gobs_per_block', side_effect=AssertionError('expanded exponent')):
            for exponent in (0, 1, 31, 32, 8_000_000, 16_000_000, 0xffffffff):
                blob = bytes(8) + exponent.to_bytes(4, 'little')
                self.assertEqual(helper.is_switch_swizzled(38, blob), exponent > 0)

    def test_switch_dispatch_full_owner_budget_refuses_without_integer_expansion(self):
        import tracemalloc
        from unittest.mock import patch
        from binding_y4_save import _asset
        converter = importlib.import_module('unityrs.export.Texture2DConverter')
        budget = unityrs.Budget(1_000_000)
        env = unityrs.Environment(budget=budget)
        reader = env.load_file(_asset(), name='owner.assets').objects[1]
        held = reader._native.tree_traversal()
        held.step(0, budget.limit - budget.used, 0)
        try:
            for exponent in (8_000_000, 16_000_000):
                for patched in (False, True):
                    table = dict(converter.CONV_TABLE)
                    if patched:
                        table[unityrs.enums.TextureFormat.RGBA32] = (lambda *args: self.fail('codec called'), ())
                    blob = bytes(8) + exponent.to_bytes(4, 'little')
                    with patch.dict(converter.CONV_TABLE, table), self.subTest(exponent=exponent, patched=patched):
                        tracemalloc.start()
                        try:
                            with self.assertRaises(MemoryError):
                                converter._parse_image_data(b'\0' * 4, 1, 1, 4, None, 38, blob,
                                    _guard=reader._native.tree_traversal(), _reader=reader._native)
                            _, peak = tracemalloc.get_traced_memory()
                        finally:
                            tracemalloc.stop()
                        self.assertLess(peak, 128 * 1024)
        finally:
            del held

    def test_standalone_gob_shift_charges_before_expansion(self):
        from unittest.mock import Mock, patch
        helper = importlib.import_module('unityrs.helpers.TextureSwizzler')
        from unityrs import _native
        guard = Mock()
        guard.step.side_effect = MemoryError('full budget')
        with patch.object(_native, 'TreeTraversal', return_value=guard):
            for exponent in (31, 32, 8_000_000):
                with self.assertRaises(MemoryError):
                    helper.get_switch_gobs_per_block(bytes(8) + exponent.to_bytes(4, 'little'))
        self.assertEqual(guard.step.call_count, 3)

    def test_switch_dispatch_retains_native_layout_refusal(self):
        from unittest.mock import patch
        converter = importlib.import_module('unityrs.export.Texture2DConverter')
        for exponent in (32, 0xffffffff):
            blob = bytes(8) + exponent.to_bytes(4, 'little')
            table = {unityrs.enums.TextureFormat.RGBA32: (lambda *args: self.fail('codec called'), ())}
            with patch.dict(converter.CONV_TABLE, table), self.subTest(exponent=exponent):
                with self.assertRaisesRegex(NotImplementedError, f'Switch GOB exponent {exponent} exceeds the bounded layout'):
                    converter.parse_image_data(b'\0' * 4, 1, 1, 4, None, 38, blob)

    def test_geometry_honors_owner_budget_and_releases_staging(self):
        import sys
        from pathlib import Path
        sys.path.insert(0, str(Path(__file__).resolve().parents[4] / 'tools/parity'))
        from binding_y4_save import _asset
        from binding_y6b_images import _mesh
        helper = importlib.import_module('unityrs.export.SpriteHelper')
        budget = unityrs.Budget(1_000_000)
        env = unityrs.Environment(budget=budget)
        reader = env.load_file(_asset(), name='owner.assets').objects[1]
        sprite = SimpleNamespace(object_reader=reader, m_PixelsToUnits=1.)
        baseline = budget.used
        held = reader._native.tree_traversal()
        held.step(0, budget.limit - budget.used, 0)
        try:
            with self.assertRaises(MemoryError):
                helper.mask_sprite(sprite, _mesh(), Image.new('RGBA', (4, 4)))
        finally:
            del held
        self.assertEqual(budget.used, baseline)

    def test_astc_context_identity_is_thread_local(self):
        converter = importlib.import_module('unityrs.export.Texture2DConverter')
        converter._get_astc_context.cache_clear()
        current = converter.get_astc_context((4, 4))
        result = []
        peer = threading.Thread(target=lambda: result.append(converter.get_astc_context((4, 4))))
        peer.start()
        peer.join()
        self.assertFalse(peer.is_alive())
        self.assertEqual(len(result), 1)
        self.assertIsNot(result[0], current)
        self.assertIs(converter.get_astc_context((4, 4)), current)
        converter._get_astc_context.cache_clear()

    def test_matrix_factorial_work_refuses_before_expansion(self):
        helper = importlib.import_module('unityrs.export.SpriteHelper')
        with self.assertRaises(unityrs.LimitError):
            helper.get_matrix_determinant([[1] * 20 for _ in range(20)])

    def test_rgb_mask_mutates_and_returns_same_image(self):
        from binding_y6b_images import _mesh
        helper = importlib.import_module('unityrs.export.SpriteHelper')
        original = Image.new('RGB', (4, 4), (1, 2, 3))
        result = helper.mask_sprite(SimpleNamespace(object_reader=None, m_PixelsToUnits=1.), _mesh(), original)
        self.assertIs(result, original)
        self.assertEqual(result.mode, 'RGBA')
