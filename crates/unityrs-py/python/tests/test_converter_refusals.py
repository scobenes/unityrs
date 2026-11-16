"""Installed-wheel material and converter capability boundaries."""
import importlib
import sys
import unittest
from pathlib import Path

from PIL import Image
import unityrs
from unityrs import classes

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT / "tools/parity"))
from binding_y4_exports import _new


class ConverterRefusals(unittest.TestCase):
    def test_material_maps_resolve_current_pointers(self):
        exporter = importlib.import_module("unityrs.export.MeshRendererExporter")
        material = _new(classes.Material, m_Name="mapped",
            m_Shader=classes.PPtr(m_FileID=0, m_PathID=0),
            m_SavedProperties=_new(classes.UnityPropertySheet,
                m_TexEnvs=[("_MainTex", _new(classes.UnityTexEnv,
                    m_Texture=classes.PPtr(m_FileID=0, m_PathID=1),
                    m_Offset=classes.Vector2f(0, 0), m_Scale=classes.Vector2f(1, 1)))],
                m_Colors=[], m_Floats=[]))
        with self.assertRaisesRegex(ValueError, "PPtr can't deref without an assetsfile"):
            exporter.export_material(material)
        from binding_y6_exports import textured_material
        material, _, _, _ = textured_material(unityrs)
        self.assertIn("map_Kd diffuse.png", exporter.export_material(material))

    def test_replacement_callbacks_refuse_preprocessing_gaps(self):
        converter = importlib.import_module("unityrs.export.Texture2DConverter")
        for key, platform, blob, reason in (
            (unityrs.enums.TextureFormat.RGBA32, unityrs.enums.BuildTarget.Switch,
             bytes(8) + b"\x01\x00\x00\x00", "Switch-swizzled"),
            (unityrs.enums.TextureFormat.DXT1Crunched, 0, None, "crunch"),
        ):
            with self.subTest(reason=reason):
                previous = converter.CONV_TABLE[key]
                calls = []
                def convert(data, width, height):
                    calls.append(data)
                    return Image.new("RGBA", (width, height))
                try:
                    converter.CONV_TABLE[key] = (convert, ())
                    with self.assertRaisesRegex(NotImplementedError, "CONV_TABLE replacement.*" + reason):
                        converter.parse_image_data(bytes(16), 2, 2, key, (2020, 3, 0, 0), platform, blob)
                    self.assertEqual(calls, [])
                finally:
                    converter.CONV_TABLE[key] = previous
