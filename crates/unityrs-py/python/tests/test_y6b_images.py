"""Ownership and thread isolation of standalone image adapters."""
import importlib
import threading
import unittest
from types import SimpleNamespace
from PIL import Image
import unityrs


class ImageAdapterBounds(unittest.TestCase):
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
