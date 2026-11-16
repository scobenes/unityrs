"""Installed wheel checks for current material names and raw float WAVs."""
import importlib
import struct
import unittest

import UnityPy
import unityrs

import binding_y6_exports as observations


class ExportAdapters(unittest.TestCase):
    def test_material_textures_match_corrected_oracle(self):
        self.assertEqual(UnityPy.__version__, "1.25.2")
        actual = observations.observe_material_textures(unityrs)
        self.assertEqual(actual, observations.observe_material_textures(UnityPy, corrected=True))
        for case in actual.values():
            self.assertIn("value", case)
        initial = bytes.fromhex(actual["initial"]["value"][1]).decode()
        self.assertIn("map_Kd diffuse.png", initial)
        self.assertIn("map_bump _BumpMap.png\nbump _BumpMap.png", initial)
        self.assertIn("map_Ks diffuse.png", initial)
        self.assertIn("map_Kd changed.png", bytes.fromhex(actual["changed_owner"]["value"][1]).decode())
        self.assertIn("map_Kd _MainTex.png", bytes.fromhex(actual["duplicate"]["value"][1]).decode())

    def test_material_oracle_refusals_remain_recorded(self):
        actual = observations.observe_material_textures(UnityPy)
        self.assertEqual({case["error"] for case in actual.values()}, {"ValueError"})

    def test_audio_float_bytes_match_subsound_reference(self):
        actual = observations.observe_audio_float(unityrs)
        self.assertEqual(actual, observations.observe_audio_float(UnityPy, corrected=True))
        for case in actual.values():
            self.assertIn("value", case)

    def test_audio_float_oracle_bank_refusals_remain_recorded(self):
        actual = observations.observe_audio_float(UnityPy)
        self.assertEqual({case["error"] for case in actual.values()}, {"FmodError"})

    def test_audio_false_preserves_passthrough_and_integer_samples(self):
        actual = observations.observe_audio_options(unityrs)
        self.assertEqual(actual, observations.observe_audio_options(UnityPy))
        for converted, unchanged in actual.values():
            self.assertEqual(converted, unchanged)

    def test_audio_default_converts_float_and_false_preserves_bits(self):
        C = importlib.import_module("unityrs.classes")
        converter = importlib.import_module("unityrs.export.AudioClipConverter")
        payload = struct.pack("<2f", .5, -.5)
        clip = C.AudioClip(m_Name="tone", m_AudioData=observations.float_bank(payload))
        converted = converter.extract_audioclip_samples(clip)["tone.wav"]
        raw = converter.extract_audioclip_samples(clip, False)["tone.wav"]
        self.assertEqual(struct.unpack_from("<HH", converted, 20), (1, 1))
        self.assertEqual(struct.unpack_from("<H", converted, 34), (16,))
        self.assertEqual(converted[44:], struct.pack("<2h", 16384, -16384))
        self.assertEqual(clip.samples, {"tone.wav": converted})
        self.assertEqual(struct.unpack_from("<HH", raw, 20), (3, 1))
        self.assertEqual(struct.unpack_from("<H", raw, 34), (32,))
        self.assertEqual(raw[44:], payload)

    def test_material_resolution_honors_owning_budget(self):
        from binding_y4_save import _asset
        value, pointer, unnamed, edited = observations.textured_material(unityrs)
        budget = unityrs.Budget(1_000_000)
        env = unityrs.Environment(budget=budget)
        value.object_reader = env.load_file(_asset(), name="owner.assets").objects[1]
        exporter = importlib.import_module("unityrs.export.MeshRendererExporter")
        baseline = budget.used
        held = value.object_reader._native.tree_traversal()
        held.step(0, budget.limit - budget.used, 0)
        try:
            with self.assertRaises(MemoryError):
                exporter.export_material(value)
        finally:
            del held
        self.assertEqual(budget.used, baseline)

    def test_raw_audio_refusal_releases_owning_budget(self):
        from binding_y4_save import _asset
        C = importlib.import_module("unityrs.classes")
        converter = importlib.import_module("unityrs.export.AudioClipConverter")
        budget = unityrs.Budget(1_000_000)
        env = unityrs.Environment(budget=budget)
        owner = env.load_file(_asset(), name="owner.assets").objects[1]
        clip = C.AudioClip(m_Name="tone", m_AudioData=observations.float_bank(struct.pack("<2f", .5, -.5)))
        clip.object_reader = owner
        baseline = budget.used
        held = owner._native.tree_traversal()
        held.step(0, budget.limit - budget.used, 0)
        try:
            with self.assertRaises(MemoryError):
                converter.extract_audioclip_samples(clip, False)
        finally:
            del held
        self.assertEqual(budget.used, baseline)


if __name__ == "__main__":
    unittest.main()
