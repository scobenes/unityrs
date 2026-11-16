"""Material parity and callback preprocessing."""
import importlib
import unittest

import UnityPy
import unityrs
from unityrs import classes

from binding_y4_exports import _new
from binding_y5_modules import observe_texture_extension, observe_untextured_material


class ConverterRefusals(unittest.TestCase):
    def test_untextured_material_matches_oracle(self):
        self.assertEqual(UnityPy.__version__, "1.25.2")
        self.assertEqual(observe_untextured_material(unityrs), observe_untextured_material(UnityPy))

    def test_textured_material_requires_resolvable_pointers(self):
        exporter = importlib.import_module("unityrs.export.MeshRendererExporter")
        for key in ("_MainTex", "_BumpMap", "_SpecularTex", "_NormalMap"):
            with self.subTest(key=key):
                material = _new(classes.Material, m_Name="mapped",
                    m_Shader=classes.PPtr(m_FileID=0, m_PathID=0),
                    m_SavedProperties=_new(classes.UnityPropertySheet,
                        m_TexEnvs=[(key, _new(classes.UnityTexEnv,
                            m_Texture=classes.PPtr(m_FileID=0, m_PathID=1),
                            m_Offset=classes.Vector2f(0, 0), m_Scale=classes.Vector2f(1, 1)))],
                        m_Colors=[], m_Floats=[]))
                with self.assertRaisesRegex(ValueError, "PPtr can't deref without an assetsfile"):
                    exporter.export_material(material)
        from binding_y6_exports import observe_material_textures
        self.assertEqual(observe_material_textures(unityrs),
                         observe_material_textures(UnityPy, corrected=True))

    def test_plain_replacement_converter_matches_oracle(self):
        self.assertEqual(observe_texture_extension(unityrs), observe_texture_extension(UnityPy))

    def test_nonswizzled_switch_replacement_matches_oracle(self):
        for blob in (None, b"short", bytes(12)):
            with self.subTest(blob=blob):
                self.assertEqual(observe_texture_extension(unityrs, unityrs.enums.BuildTarget.Switch, blob),
                                 observe_texture_extension(UnityPy, UnityPy.enums.BuildTarget.Switch, blob))

    def test_switch_replacement_preprocessing_matches_oracle(self):
        from binding_y6b_images import observe_texture_dispatch
        actual = observe_texture_dispatch(unityrs)
        expected = observe_texture_dispatch(UnityPy)
        for label in ("switch-rgb", "switch-bgr"):
            with self.subTest(label=label):
                self.assertEqual(actual[label], expected[label])
                self.assertEqual(len(actual[label][1]), 1)

    def test_crunch_replacement_preprocessing_matches_oracle(self):
        from binding_y6b_images import observe_texture_dispatch
        actual = observe_texture_dispatch(unityrs)
        expected = observe_texture_dispatch(UnityPy)
        labels = [label for label in actual if "native.json" in label]
        self.assertTrue(labels)
        for label in labels + ["bad-crunch", "bad-legacy"]:
            with self.subTest(label=label):
                self.assertEqual(actual[label], expected[label])
