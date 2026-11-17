"""Installed-wheel material and converter capability boundaries."""
import importlib
import sys
import unittest
from pathlib import Path

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

    def test_replacement_callbacks_receive_preprocessed_data(self):
        from binding_y6b_images import observe_texture_dispatch
        actual = observe_texture_dispatch(unityrs)
        for label in ("switch-rgb", "switch-bgr"):
            with self.subTest(label=label):
                output, calls = actual[label]
                self.assertIn("value", output)
                self.assertEqual(len(calls), 1)
                self.assertEqual(calls[0][0], "bytes")
        for label, (output, calls) in actual.items():
            if "native.json" in label:
                with self.subTest(label=label):
                    self.assertIn("value", output)
                    self.assertEqual(len(calls), 1)
        self.assertEqual(actual["bad-crunch"][1], [])
        self.assertEqual(actual["bad-crunch"][0]["error"], "SystemError")
