"""Installed-wheel image API parity against UnityPy 1.25.2."""

import unittest

import UnityPy
import unityrs

from binding_y4_images import observe_arrays, observe_image_formats, observe_set_image, observe_sprites, observe_textures


class Images(unittest.TestCase):
    def test_texture_images_use_current_fields(self):
        from types import SimpleNamespace
        from binding_y4_images import capture, image, texture

        self.assertEqual(observe_textures(unityrs), observe_textures(UnityPy, corrected=True))
        self.assertEqual(observe_textures(UnityPy)["zero_empty_no_stream"],
                         {"error": "ValueError", "message": "No image data found"})
        for width, height in ((0, 0), (0, 4), (4, 0), (0, -1), (-1, 0)):
            for data in (b"", b"x"):
                for stream in (None, SimpleNamespace(path="unread.resS", offset=0, size=1)):
                    obj = texture(unityrs, data, m_Width=width, m_Height=height,
                                  m_TextureFormat=999, m_StreamData=stream)
                    with self.subTest(size=(width, height), data=data, stream=stream):
                        self.assertEqual(image(obj.image), {"mode": "RGBA", "size": [0, 0], "pixels": ""})
                        if not data and stream is None:
                            self.assertEqual(capture(obj.get_image_data),
                                             {"error": "ValueError", "message": "No image data found"})

    def test_set_image_modes_setter_mips_and_stream_reset(self):
        self.assertEqual(observe_set_image(unityrs), observe_set_image(UnityPy, corrected=True))

    def test_array_images_and_errors(self):
        self.assertEqual(observe_arrays(unityrs), observe_arrays(UnityPy))
        from types import SimpleNamespace
        from binding_y4_images import _stream_file, image, make

        for module in (unityrs, UnityPy):
            file = _stream_file(module)
            obj = make(module, "Texture2DArray", m_Format=4, m_Width=1, m_Height=2,
                       m_Depth=2, m_DataSize=16, image_data=b"", object_reader=file.objects[1],
                       m_StreamData=SimpleNamespace(path="CAB-images.resS", offset=0, size=16))
            if module is UnityPy:
                with self.assertRaisesRegex(ValueError, "Texture2D has no image data"):
                    obj.images
            else:
                self.assertEqual([image(value) for value in obj.images], [
                    {"mode": "RGBA", "size": [1, 2], "pixels": "0405060700010203"},
                    {"mode": "RGBA", "size": [1, 2], "pixels": "0c0d0e0f08090a0b"}])
        for platform in (11, 38):
            obj = make(unityrs, "Texture2DArray", m_Format=4, m_Width=1, m_Height=2,
                       m_Depth=2, m_DataSize=16, image_data=bytes(range(16)), m_StreamData=None,
                       object_reader=SimpleNamespace(version=(2020, 3, 0, 3, 1), platform=platform))
            with self.assertRaisesRegex(NotImplementedError, f"unsupported Texture2DArray platform {platform}"):
                obj.images

    def test_sprite_atlas_rotation_crop_and_mask(self):
        self.assertEqual(observe_sprites(unityrs), observe_sprites(UnityPy))

    def test_raw_format_promotions_channels_and_filtered_mips(self):
        self.assertEqual(observe_image_formats(unityrs), observe_image_formats(UnityPy, corrected=True))

    def test_mode_corrections_keep_raw_oracle_failures_and_direct_packers(self):
        from PIL import Image
        from binding_y4_images import _set_image, texture

        for mode in ("1", "L", "LA", "P", "PA", "CMYK", "YCbCr", "HSV", "RGBa", "I", "F",
                     "I;16", "I;16L", "I;16B", "I;16N", "RGB", "RGBX", "RGBA", "LAB"):
            for target, pack in ((1, "A"), (63, "R"), (3, "RGB"), (4, "RGBA")):
                src = Image.new(mode, (8, 8))
                # Nonzero values expose conversion and LAB's actual a* band.
                if mode in ("RGB", "RGBX", "RGBA", "LAB"):
                    src = Image.frombytes(mode, src.size, bytes(range(len(mode))) * 64)
                elif mode in ("L", "LA", "P", "PA", "CMYK"):
                    bands = len(mode) if mode != "CMYK" else 4
                    src = Image.frombytes(mode, src.size, bytes((i * 37 + i // 11) % 256 for i in range(64 * bands)))
                actual, expected = texture(unityrs), texture(UnityPy)
                try:
                    src.tobytes("raw", pack)
                except ValueError:
                    with self.assertRaisesRegex(ValueError, "^No packer found from "):
                        expected.set_image(src, target, 2)
                with self.subTest(mode=mode, target=target):
                    _set_image(UnityPy, expected, src, target, 2, True)
                    actual.set_image(src, target, 2)
                    self.assertEqual((actual.image_data, actual.m_MipCount),
                                     (expected.image_data, expected.m_MipCount))
        for mode in ("BGR;15", "BGR;16", "BGR;24", "La"):
            obj = texture(unityrs)
            with self.assertRaisesRegex(NotImplementedError, "unsupported set_image: Pillow cannot convert image mode to RGBA"):
                obj.set_image(Image.new(mode, (8, 8)))
            self.assertEqual((obj.m_Width, obj.m_Height, obj.image_data), (1, 2, bytes(range(1, 9))))
            obj.set_image(Image.new(mode, (0, 4)))
            self.assertEqual((obj.m_Width, obj.m_Height, obj.image_data), (0, 4, b""))

    def test_switch_mip_chain_refuses_before_mutating_fields(self):
        from PIL import Image
        from types import SimpleNamespace
        from binding_y4_images import texture

        for size in ((8, 8), (16, 8)):
            for target in (3, 4, 10):
                obj = texture(unityrs, m_PlatformBlob=bytes(8) + b"\x01\x00\x00\x00",
                              object_reader=SimpleNamespace(platform=38))
                before = vars(obj).copy()
                with self.subTest(size=size, target=target):
                    with self.assertRaisesRegex(NotImplementedError, "unsupported set_image: Switch mip chain"):
                        obj.set_image(Image.new("RGBA", size), target, 2)
                    self.assertEqual(vars(obj), before)

    def test_bc7_writes_match_existing_core_tier_c_reference(self):
        import json
        from pathlib import Path
        from PIL import Image
        from binding_y4_images import texture

        fixture = json.loads((Path(__file__).resolve().parents[3] /
                              "crates/unityrs-texture/tests/fixtures/set_image_encoders.json").read_text())
        cases = [case for case in fixture["cases"] if case["format"] == 25 and
                 case["platform"] == 0 and case["mode"] == "RGBA" and case["flip"]]
        self.assertTrue(cases)
        for case in cases:
            with self.subTest(size=case["size"], mips=case["mips"]):
                src = Image.frombytes("RGBA", tuple(case["size"]), bytes.fromhex(fixture["sources"][case["source"]]))
                with self.assertRaisesRegex(ValueError, "params must be an instance of BC7CompressBlockParams"):
                    texture(UnityPy).set_image(src, 25, case["mips"])
                obj = texture(unityrs)
                obj.set_image(src, 25, case["mips"])
                self.assertEqual(obj.image_data.hex(), case["result"]["hex"])
                self.assertEqual((int(obj.m_TextureFormat), obj.m_MipCount), (25, case["result"]["mips"]))

    def test_decode_matrix_and_platforms(self):
        from binding_y4_images_matrix import observe_decode_matrix
        self.assertEqual(observe_decode_matrix(unityrs), observe_decode_matrix(UnityPy))

    def test_array_eight_bit_integer_corrections(self):
        from types import SimpleNamespace
        from binding_y4_images import capture, image, make

        for graphics_format in range(13, 21):
            channels = (graphics_format - 13) % 4 + 1
            values = dict(m_Format=graphics_format, m_Width=1, m_Height=1, m_Depth=2,
                          m_DataSize=channels * 2, image_data=bytes([127]) * channels + bytes([128]) * channels,
                          m_StreamData=None, object_reader=SimpleNamespace(version=(2020, 3, 0, 3, 1)))
            signed = graphics_format >= 17
            mode = "RGBA" if channels == 4 else "RGB"
            expected = []
            for component in ((255, 0) if signed else (127, 128)):
                pixel = bytes([component]) * channels + bytes(max(0, 3 - channels))
                expected.append({"mode": mode, "size": [1, 1], "pixels": pixel.hex()})
            with self.subTest(graphics_format=graphics_format):
                self.assertEqual([image(value) for value in make(unityrs, "Texture2DArray", **values).images], expected)
                # UnityPy selects a 16-bit decoder for these 8-bit layouts.
                observed = capture(lambda: [image(v) for v in make(UnityPy, "Texture2DArray", **values).images])
                if graphics_format == 15:
                    self.assertEqual(observed, {"value": [
                        {"mode": "RGB", "size": [1, 1], "pixels": "ffee7b"},
                        {"mode": "RGB", "size": [1, 1], "pixels": "001083"}]})
                else:
                    message = ("'raw'" if graphics_format in (14, 18) else
                               "unrecognized image mode" if graphics_format == 17 else
                               "unknown raw mode for given image mode")
                    self.assertEqual(observed, {"error": "KeyError" if graphics_format in (14, 18) else "ValueError",
                                                "message": message})

    def test_image_staging_uses_attached_budget(self):
        from PIL import Image
        from binding_cases import S
        from binding_y4_images import texture

        budget = unityrs.Budget(2 * 1024 * 1024)
        tree = S.Node("TextAsset", "Base", [S.Node("string", "m_Name"), S.Node("string", "m_Script")])
        file = unityrs.Environment(budget=budget).load_file(
            S.serialized_file([S.Type(49, tree)], [S.Obj(1, 0, S.s32("a") + S.s32("b"))]), name="budget.assets")
        obj = texture(unityrs, object_reader=file.objects[1])
        before = budget.used
        with self.assertRaises(MemoryError):
            obj.set_image(Image.new("RGBA", (512, 512)))
        self.assertEqual(obj.image_data, bytes(range(1, 9)))
        self.assertEqual((obj.m_Width, obj.m_Height), (1, 2))
        self.assertEqual(budget.used, before)

    def test_array_hdr_formats_use_full_profile_reference(self):
        import hashlib
        import json
        from types import SimpleNamespace
        from binding_y4_images import make
        from binding_y4_images_matrix import FIXTURES

        saved = json.loads((FIXTURES / "astc_hdr.json").read_text())
        for texture_format in range(66, 72):
            case = next(c for c in saved["blocks"] if c["format"] == texture_format and "valid" in c["tags"])
            graphics_format = 145 + texture_format - 66
            values = dict(m_Format=graphics_format, m_Width=case["side"], m_Height=case["side"],
                          m_Depth=1, m_DataSize=16, image_data=bytes.fromhex(case["data"]), m_StreamData=None,
                          object_reader=SimpleNamespace(version=(2020, 3, 0, 3, 1)))
            with self.subTest(graphics_format=graphics_format):
                actual, = make(unityrs, "Texture2DArray", **values).images
                self.assertEqual(actual.mode, "RGBA")
                self.assertEqual(actual.size, (case["side"], case["side"]))
                self.assertEqual(hashlib.sha256(actual.tobytes()).hexdigest(), case["sha256"])
                with self.assertRaisesRegex(NotImplementedError, f"GraphicsFormat {graphics_format} not supported yet"):
                    make(UnityPy, "Texture2DArray", **values).images


if __name__ == "__main__":
    unittest.main()
