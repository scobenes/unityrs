"""Installed-wheel module observations against pinned UnityPy."""
import unittest
import UnityPy
import unityrs
import binding_y5_modules as observations


class Modules(unittest.TestCase):
    def compare(self, name):
        self.assertEqual(UnityPy.__version__, "1.25.2")
        observe = getattr(observations, name)
        self.assertEqual(observe(unityrs), observe(UnityPy))

    def test_reexports(self):
        self.compare("observe_reexports")

    def test_streams(self):
        self.compare("observe_streams")

    def test_module_tables(self):
        self.compare("observe_module_tables")

    def test_primitive_maps(self):
        self.compare("observe_primitive_maps")

    def test_compression(self):
        self.compare("observe_compression")

    def test_texture_converter(self):
        self.compare("observe_texture_converter")

    def test_converter_helpers(self):
        self.compare("observe_converter_helpers")

    def test_export_modules(self):
        self.compare("observe_export_modules")
        from binding_y4_save import _asset
        from binding_y4_images import texture
        converter = observations._module(unityrs, "export.Texture2DConverter")
        budget = unityrs.Budget(1_000_000)
        env = unityrs.Environment(budget=budget)
        owner = env.load_file(_asset(), name="owner.assets").objects[1]
        bound = texture(unityrs, object_reader=owner)
        baseline = budget.used
        held = owner._native.tree_traversal()
        held.step(0, budget.limit - budget.used, 0)
        try:
            with self.assertRaises(MemoryError):
                converter.get_image_from_texture2d(bound)
        finally:
            del held
        self.assertEqual(budget.used, baseline)

    def test_texture_extension(self):
        self.compare("observe_texture_extension")

    def test_decrypt_key(self):
        self.compare("observe_decrypt_key")

    def test_generator(self):
        self.compare("observe_generator")

    def test_generator_cache(self):
        self.compare("observe_generator_cache")

    def test_encrypted_load(self):
        self.compare("observe_encrypted_load")
