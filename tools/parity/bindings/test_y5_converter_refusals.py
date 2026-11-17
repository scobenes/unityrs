"""Material parity and explicit gaps in callback preprocessing."""
import importlib
import unittest

from PIL import Image
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

    def replacement(self, texture_format, platform, blob=None):
        converter = importlib.import_module("unityrs.export.Texture2DConverter")
        previous = converter.CONV_TABLE[texture_format]
        calls = []
        def convert(data, width, height):
            calls.append((data, width, height))
            return Image.new("RGBA", (width, height))
        try:
            converter.CONV_TABLE[texture_format] = (convert, ())
            with self.assertRaisesRegex(NotImplementedError, "CONV_TABLE replacement.*(Switch-swizzled|crunch)"):
                converter.parse_image_data(bytes(16), 2, 2, texture_format,
                                           (2020, 3, 0, 0), platform, blob)
            self.assertEqual(calls, [])
        finally:
            converter.CONV_TABLE[texture_format] = previous

    def test_switch_replacement_is_refused_before_callback(self):
        for name in ("RGBA32", "RGB24"):
            for exponent in (b"\x01\x00\x00\x00", b"\xff\xff\xff\xff"):
                with self.subTest(format=name, exponent=exponent):
                    self.replacement(getattr(unityrs.enums.TextureFormat, name),
                                     unityrs.enums.BuildTarget.Switch, bytes(8) + exponent)

    def test_crunch_replacement_is_refused_before_callback(self):
        for name in ("DXT1Crunched", "DXT5Crunched", "ETC_RGB4Crunched", "ETC2_RGBA8Crunched"):
            with self.subTest(format=name):
                self.replacement(getattr(unityrs.enums.TextureFormat, name), 0)
