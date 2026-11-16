"""Installed-wheel review checks: oracle materials, bounded and confined exports."""
import json
from pathlib import Path
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import UnityPy
import unityrs

import binding_y4_review_exports as review
from binding_y4_exports import mesh, _MemoryFS


class ReviewExports(unittest.TestCase):
    def test_nonempty_material_current_values_match_oracle(self):
        self.assertEqual(UnityPy.__version__, "1.25.2")
        actual = review.observe_materials(unityrs)
        self.assertEqual(actual, review.observe_materials(UnityPy))
        for case in actual.values():
            self.assertIn("materials/triangle.mtl", case["files"])
            self.assertIn("newmtl paint", case["files"]["materials/triangle.mtl"])

    def test_shader_parsed_export_uses_owning_decompression_cap(self):
        fixture = Path(__file__).resolve().parents[3] / "crates/unityrs-export/tests/fixtures/shader.json"
        case = next(c for c in json.loads(fixture.read_text())["cases"] if c["case"] == "version_5_5_0")
        env = unityrs.Environment(limits=unityrs.Limits(max_block_output=1))
        shader = env.load_file(bytes.fromhex(case["input"]), name="shader.assets").objects[1].read()
        with self.assertRaises(unityrs.LimitError):
            shader.export()

    def test_renderer_rejects_asset_leaf_paths_before_open(self):
        for name in ("", ".", "..", "../escape", "/absolute", "a/b", "a\\b", "C:escape", "a\0b"):
            with self.subTest(name=name):
                geometry = mesh(unityrs)
                geometry.m_Name = name
                fs = _MemoryFS()
                value = review.renderer(unityrs, geometry, [], fs)
                with self.assertRaises(ValueError):
                    value.export("safe")
                self.assertEqual(fs.files, {})

    def test_renderer_refuses_texture_name_and_fallback_before_any_open(self):
        for name, key in (("../escape", "_MainTex"), ("", "../escape"), ("/absolute", "_MainTex")):
            with self.subTest(name=name, key=key):
                env, value = review.material(unityrs)
                texture = SimpleNamespace(m_Name=name)
                pointer = SimpleNamespace(deref_parse_as_object=lambda: texture)
                value.m_SavedProperties.m_TexEnvs = [(key, SimpleNamespace(m_Texture=pointer))]
                fs = _MemoryFS()
                value_renderer = review.renderer(unityrs, mesh(unityrs), [value], fs)
                with patch.object(unityrs._native, "export_material", return_value="newmtl paint"):
                    with self.assertRaises(ValueError):
                        value_renderer.export("safe")
                self.assertEqual(fs.files, {})

    def test_local_renderer_replaces_symlink_and_hardlink_leaves(self):
        from scratch import for_case
        root = for_case(self)
        outside = root / "outside"
        outside.write_text("retained")
        output = root / "output"
        output.mkdir()
        (output / "triangle.obj").symlink_to(outside)
        (output / "triangle.mtl").hardlink_to(outside)
        value = review.renderer(unityrs, mesh(unityrs), [], SimpleNamespace())
        del value.object_reader.assets_file.environment.fs
        value.export(str(output))
        self.assertEqual(outside.read_text(), "retained")
        self.assertFalse((output / "triangle.obj").is_symlink())

    def test_renderer_writes_real_texture_as_png(self):
        from PIL import Image
        from binding_y4_images import texture
        from scratch import for_case
        output = for_case(self) / "output"
        env, value = review.material(unityrs)
        image = texture(unityrs, m_Name="diffuse")
        value.m_SavedProperties.m_TexEnvs = [("_MainTex", SimpleNamespace(
            m_Texture=SimpleNamespace(deref_parse_as_object=lambda: image)))]
        value_renderer = review.renderer(unityrs, mesh(unityrs), [value], None)
        del value_renderer.object_reader.assets_file.environment.fs
        with patch.object(unityrs._native, "export_material", return_value="newmtl paint"):
            value_renderer.export(str(output))
        with Image.open(output / "diffuse.png") as actual:
            self.assertEqual(actual.format, "PNG")
            self.assertEqual(actual.tobytes(), image.image.tobytes())

    def test_environment_save_validates_original_names_before_any_open(self):
        for name in ("", ".", "..", "../escape", "/absolute", "a/../escape", "a\\b", "C:escape", "a\0b"):
            with self.subTest(name=name):
                env = unityrs.Environment()
                env.files = {"safe.assets": SimpleNamespace(is_changed=True, save=lambda **kw: b"safe"),
                             name: SimpleNamespace(is_changed=True, save=lambda **kw: b"unsafe")}
                with patch("builtins.open", side_effect=AssertionError("output opened")):
                    with self.assertRaises(ValueError):
                        env.save(out_path="missing")

    def test_local_hostile_names_and_late_texture_write_nothing(self):
        from binding_y4_images import texture
        from scratch import for_case
        root = for_case(self)
        outside = root / "outside"
        outside.write_text("retained")
        bad_names = ("", ".", "..", "../outside", str(outside), "a/b", "a\\b", "C:escape", "a\0b")
        for kind in ("mesh", "texture", "fallback"):
            for index, name in enumerate(bad_names):
                if kind == "texture" and name == "":
                    # An empty texture name with a safe key has a valid
                    # effective name; the fallback loop tests an empty key.
                    continue
                with self.subTest(kind=kind, name=name):
                    output = root / f"{kind}-{index}"
                    output.mkdir()
                    geometry = mesh(unityrs)
                    env, value = review.material(unityrs)
                    if kind == "mesh":
                        geometry.m_Name = name
                    else:
                        good = texture(unityrs, m_Name="safe")
                        bad = texture(unityrs, m_Name=name if kind == "texture" else "")
                        value.m_SavedProperties.m_TexEnvs = [
                            ("_MainTex", SimpleNamespace(m_Texture=SimpleNamespace(
                                deref_parse_as_object=lambda: good))),
                            (name if kind == "fallback" else "_Other", SimpleNamespace(
                                m_Texture=SimpleNamespace(deref_parse_as_object=lambda: bad)))]
                    value_renderer = review.renderer(unityrs, geometry, [value], None)
                    del value_renderer.object_reader.assets_file.environment.fs
                    with patch.object(unityrs._native, "export_material", return_value="newmtl paint"):
                        with self.assertRaises(ValueError):
                            value_renderer.export(str(output))
                    self.assertEqual(list(output.iterdir()), [])
                    self.assertEqual(outside.read_text(), "retained")

    def test_local_renderer_refuses_symlink_destination_directory(self):
        from scratch import for_case
        root = for_case(self)
        outside = root / "outside"
        outside.mkdir()
        output = root / "output"
        output.symlink_to(outside, target_is_directory=True)
        value_renderer = review.renderer(unityrs, mesh(unityrs), [], None)
        del value_renderer.object_reader.assets_file.environment.fs
        with self.assertRaises(OSError):
            value_renderer.export(str(output))
        self.assertEqual(list(outside.iterdir()), [])


if __name__ == "__main__":
    unittest.main()
