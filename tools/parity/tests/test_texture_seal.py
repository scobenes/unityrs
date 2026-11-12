"""Every Texture2D construction in either reference refuses image assignment (round 6, S1).

The testkit's sprite_attributes scene puts a complete path-valued Texture2D in an unused field of a texture,
Sprite and atlas. The engine keeps that field as data; UnityPy's refused setter leaves an unused UnknownObject.
Both still export the parent. Requires the pinned oracle; the real setter's successful host-image read remains
outside the references in test_sprites_kind.py.
"""

import sys
import unittest
from pathlib import Path
from unittest import mock

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE / "tests"))

from rows_support import ORACLE  # noqa: E402

if ORACLE:
    from PIL import Image
    from UnityPy.classes import Texture2D
    from UnityPy.classes.UnknownObject import UnknownObject

    import ngui_cutter as nc
    import ngui_read as rd
    import sprites_kind as sk
    import sprites_scenes


@unittest.skipUnless(ORACLE, "needs UnityPy 1.25.2 (the oracle image)")
class TextureSeal(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.scene = sprites_scenes.scenes()["sprite_attributes"]

    def objects(self):
        return rd.load(self.scene.view("x.bundle"), "x.bundle").files["CAB-o4"].objects

    def unsealed_opens(self, obj):
        # Positive control: these exact bytes reach the real setter outside the references. A setter's
        # exception can become UnknownObject, so assert the I/O itself, never just the parent's read result.
        path = obj.parse_as_dict()["unused"]["image"]
        self.assertIsInstance(path, str)
        with mock.patch.object(Image, "open", side_effect=TypeError("unsealed open")) as opened:
            obj.read()
            opened.assert_called_with(path)

    def test_a_nested_texture_is_sealed_in_the_texture_read(self):
        obj = self.objects()[8]
        self.unsealed_opens(obj)
        with mock.patch.object(Image, "open", side_effect=AssertionError("opened")) as opened:
            texture = rd.texture_of(obj)
            self.assertIsInstance(texture, Texture2D)
            self.assertIsInstance(texture.unused, UnknownObject)
            self.assertEqual((texture.m_Width, texture.m_Height), (4, 4))
            opened.assert_not_called()

    def test_a_nested_texture_is_sealed_in_the_direct_sprite_read(self):
        obj = self.objects()[21]
        self.unsealed_opens(obj)
        with mock.patch.object(Image, "open", side_effect=AssertionError("opened")) as opened:
            sprite, _, texture, _ = sk.resolve(obj)
            self.assertIsInstance(sprite.unused, UnknownObject)
            self.assertEqual(texture[0].path_id, 1)
            opened.assert_not_called()

    def test_a_nested_texture_is_sealed_in_the_direct_atlas_read(self):
        objects = self.objects()
        self.unsealed_opens(objects[9])
        with mock.patch.object(Image, "open", side_effect=AssertionError("opened")) as opened:
            _, data, texture, _ = sk.resolve(objects[22])
            self.assertEqual((data.textureRect.x, data.textureRect.y), (1.0, 1.0))
            self.assertEqual(texture[0].path_id, 1)
            self.assertIsInstance(rd.checked(objects[9].read).unused, UnknownObject)
            opened.assert_not_called()

    def test_both_complete_references_never_open_an_image_path(self):
        original = Texture2D.image
        with mock.patch.object(Image, "open", side_effect=AssertionError("opened")) as opened:
            view = self.scene.view("x.bundle")
            ngui = nc.cut("x.bundle", view, nc.Options(texture_key="{name}.png"))
            sprites = sk.cut("x.bundle", view, self.scene.key)
            self.assertIn("nested_sheet.png", {o.key for o in ngui.outputs})
            by = {o.key: o for o in sprites.outputs}
            self.assertTrue({"nested_sprite.png", "nested_atlas.png", "nested_texture.png"} <= by.keys())
            self.assertEqual(by["nested_sprite.png"].data, by["fine.png"].data)
            self.assertEqual(by["nested_atlas.png"].data, by["atlas_assets_file.png"].data)
            for result in (ngui, sprites):
                skip = next(s for s in result.skips if s.path_id == (7 if result is ngui else 20))
                self.assertEqual((skip.reason, skip.detail), rd.READ)
            opened.assert_not_called()
        self.assertIs(Texture2D.image, original)

    def test_the_setter_is_restored_after_nested_scopes_and_reference_failure(self):
        original = Texture2D.image
        with rd.sealed_images():
            sealed = Texture2D.image
            self.assertIs(sealed.fget, original.fget)
            with self.assertRaises(TypeError):
                Texture2D.__new__(Texture2D).image = "unused.png"
            with rd.sealed_images():
                self.assertIsNot(Texture2D.image, sealed)
            self.assertIs(Texture2D.image, sealed)
        self.assertIs(Texture2D.image, original)
        for reference in (lambda: nc.cut("absent", {}), lambda: sk.cut("absent", {}, "{name}.png")):
            with self.assertRaises(KeyError):
                reference()
            self.assertIs(Texture2D.image, original)
