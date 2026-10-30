"""PARITY.md rows on images that a zero side, an image mode or an alpha texture changes, run against the oracle:
`set_image`'s packer, `Texture2D.image` and XBOX360's read of an empty texture, and sprites.

Sections 15.2, 15.7, 15.8, 18.1, 19.1 and 19.2. Like `test_parity_rows.py`, every input is built here, so nothing
needs the corpus, and each test names the rows it pins; a row's test column names the test back. Needs UnityPy
1.25.2 (the oracle image); skipped elsewhere.
"""

import shutil
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE / "tests"))

from test_parity_rows import ORACLE, load  # noqa: E402
from test_rows_exports import element  # noqa: E402
from test_rows_objects import VERSION, pptr, tpk_file, tpk_object  # noqa: E402

import pillow_tables  # noqa: E402

if ORACLE:
    import attrs
    from PIL import Image
    from UnityPy.classes import Texture2D
    from UnityPy.enums import BuildTarget, TextureFormat
    from UnityPy.export import Texture2DConverter
    from UnityPy.helpers import TextureSwizzler
    from UnityPy.tools import extractor

ZERO = ((0, 0), (0, 4), (4, 0))


def bare_texture():
    """A Texture2D with every field `None`, no object reader and no platform blob, tagged RGBA32."""
    texture = Texture2D.__new__(Texture2D)
    for field in attrs.fields(Texture2D):
        object.__setattr__(texture, field.name, None)
    texture.object_reader, texture.m_PlatformBlob, texture.m_TextureFormat = None, None, TextureFormat.RGBA32
    return texture


def texture_object(data, size, fmt):
    width, height = size
    fields = dict(m_Width=width, m_Height=height, m_CompleteImageSize=len(data), m_TextureFormat=fmt)
    return tpk_object(28, VERSION, m_Name="t", m_ImageCount=1, m_TextureDimension=2, **fields, **{"image data": data})


def sprite_object(box, texture=1, alpha=0, settings=2):
    """A sprite of rectangle packing (settings 2), not packed, cropping `box` (x, y, width, height) of `texture`."""
    rd = element(213, "m_RD")
    rect = dict(zip(("x", "y", "width", "height"), box, strict=True))
    rd.update(texture=pptr(texture), alphaTexture=pptr(alpha), textureRect=rect, settingsRaw=settings)
    return tpk_object(213, m_Name="s", m_PixelsToUnits=1.0, m_RD=rd)


@unittest.skipUnless(ORACLE, "needs UnityPy 1.25.2 (the oracle image)")
class ImageRows(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="unityrs-images-"))
        self.addCleanup(shutil.rmtree, self.tmp)

    # Sections 15.1, 18.1, 19.1, 19.2: a zero-size image reaches no packer, whatever its mode.
    def test_zero_size_images_skip_the_packer(self):
        modes = pillow_tables.image_modes()
        self.assertEqual(len(modes), 23)
        self.assertTrue({"L", "P", "La", "BGR;15", "RGB", "RGBA"} <= set(modes))
        # Pillow 9.5.0's tobytes returns b"" for an image with a zero side before it looks a packer up, in every mode
        # and for each raw mode a target packs; at 4×4 an unpackable mode raises.
        for mode in modes:
            for raw in ("A", "R", "RGB", "RGBA"):
                for size in ZERO:
                    self.assertEqual(Image.new(mode, size).tobytes("raw", raw), b"", (mode, raw, size))
        with self.assertRaisesRegex(ValueError, "^No packer found from La to RGBA$"):
            Image.new("La", (4, 4)).tobytes("raw", "RGBA")

        # image_to_texture2d in section 19.1's zero-size configurations, none of them Switch-swizzled, with etcpak
        # replaced by a recorder (so nothing crashes): every mode does what an RGBA image does, the calls it makes to
        # etcpak included, but that an RGB image is written as ASTC_RGB_ for an ASTC_HDR_ target.
        configs = ((0, None), (BuildTarget.StandaloneWindows64, None), (BuildTarget.XBOX360, None))
        configs += ((BuildTarget.Switch, [0] * 12),)
        self.assertEqual([TextureSwizzler.is_switch_swizzled(*config) for config in configs], [False] * 4)
        calls = []

        def recorder(data, width, height, fmt):
            calls.append((data, width, height, fmt))
            return b"etcpak"

        def outcome(mode, fmt, size, config):
            calls.clear()
            try:
                data, tag = Texture2DConverter.image_to_texture2d(Image.new(mode, size), fmt, *config)
            except Exception as error:  # the error is the outcome compared
                return type(error), str(error)
            return data, tag, tuple(calls)

        def expected(mode, fmt, rgba):
            if mode == "RGB" and fmt.name.startswith("ASTC_HDR_") and len(rgba) == 3:
                return rgba[0], TextureFormat[fmt.name.replace("HDR", "RGB")], rgba[2]
            return rgba

        compared, kinds = 0, set()
        with mock.patch.object(Texture2DConverter, "compress_etcpak", recorder):
            self.assertEqual(
                outcome("RGBA", TextureFormat.DXT1, (0, 4), (0, None)),
                (b"etcpak", TextureFormat.DXT1, ((b"", 0, 4, TextureFormat.DXT1),)),
            )
            for config in configs:
                for fmt in TextureFormat:
                    for size in (*ZERO, (0, 5), (5, 0)):
                        rgba = outcome("RGBA", fmt, size, config)
                        kinds.add("raises" if len(rgba) == 2 else "etcpak" if rgba[2] else "writes")
                        for mode in modes:
                            compared += 1
                            got = outcome(mode, fmt, size, config)
                            if got != expected(mode, fmt, rgba):
                                self.fail(f"{mode} {fmt.name} {size} {config[0]}: {got!r} where RGBA gives {rgba!r}")
        self.assertEqual(compared, len(configs) * len(TextureFormat) * 5 * len(modes))
        self.assertEqual(kinds, {"raises", "etcpak", "writes"})
        # A non-empty RGB image is written as ASTC_RGB_ for an ASTC_HDR_ target too (section 17).
        rgb = Texture2DConverter.image_to_texture2d(Image.new("RGB", (4, 4)), TextureFormat.ASTC_HDR_4x4)
        self.assertEqual(rgb[1], TextureFormat.ASTC_RGB_4x4)

        # So set_image of an L, P, La or BGR;15 image with a zero side to RGBA32 writes 0 bytes with the image's
        # size, where the same mode at 4×4 raises "No packer found".
        for mode in ("L", "P", "La", "BGR;15"):
            for size in ZERO:
                texture = bare_texture()
                texture.set_image(Image.new(mode, size), TextureFormat.RGBA32)
                written = (texture.image_data, texture.m_Width, texture.m_Height, texture.m_TextureFormat)
                self.assertEqual(written, (b"", *size, TextureFormat.RGBA32), (mode, size))
            with self.assertRaisesRegex(ValueError, f"^No packer found from {mode} to RGBA$"):
                bare_texture().set_image(Image.new(mode, (4, 4)), TextureFormat.RGBA32)

    # Sections 14.2, 15.7, 19.1, 19.2: Texture2D.image of a texture with a zero side fetches its data first.
    def test_zero_side_texture_image_needs_its_data(self):
        texture = bare_texture()
        texture.m_Width, texture.m_Height = 4, 0
        texture.image_data = b"\x01" * 16
        self.assertEqual((texture.image.mode, texture.image.size), ("RGBA", (0, 0)), "the fetch succeeds")
        # Right after a zero-size set_image, image_data is empty and there is no m_StreamData: the fetch raises, and
        # with it exportTexture2D, before Pillow's save.
        texture.set_image(Image.new("RGBA", (4, 0)), TextureFormat.RGBA32)
        self.assertEqual((texture.image_data, texture.m_StreamData, texture.m_Width), (b"", None, 4))
        with self.assertRaisesRegex(ValueError, "^No image data found$"):
            _ = texture.image
        with self.assertRaisesRegex(ValueError, "^No image data found$"):
            extractor.exportTexture2D(texture, str(self.tmp / "zero"), ".png")
        self.assertEqual(list(self.tmp.iterdir()), [])

    # Sections 19.1, 19.2: on XBOX360 a texture with a zero side reads as a 0×0 image before any swap.
    def test_xbox360_reads_an_empty_texture_before_swapping(self):
        swapped = ("RGB565", "DXT1", "DXT1Crunched", "DXT5", "DXT5Crunched")
        self.assertEqual(sorted(f.name for f in Texture2DConverter.XBOX_SWAP_FORMATS), sorted(swapped))
        version = (2021, 3, 1, 3)
        with mock.patch.object(Texture2DConverter, "swap_bytes_for_xbox", side_effect=AssertionError("swapped")):
            for name in swapped:
                for size in ZERO:
                    for data in (b"", bytes(16)):
                        with self.subTest(name, size=size, data=len(data)):
                            image = Texture2DConverter.parse_image_data(
                                data, *size, TextureFormat[name], version, BuildTarget.XBOX360
                            )
                            self.assertEqual((image.mode, image.size), ("RGBA", (0, 0)))
            # With no zero side the swap comes first.
            with self.assertRaisesRegex(AssertionError, "^swapped$"):
                Texture2DConverter.parse_image_data(bytes(8), 4, 4, TextureFormat.DXT1, version, BuildTarget.XBOX360)

    # Sections 15.2, 15.7, 15.8, 19.2: a sprite whose texture rect rounds to a zero side.
    def test_zero_side_sprites_save_no_png(self):
        objects = [
            (1, 28, texture_object(bytes(range(64)), (4, 4), TextureFormat.RGBA32)),
            (10, 213, sprite_object((1, 0, 0, 3))),
            (11, 213, sprite_object((1, 0, 0.4, 3))),
            (12, 213, sprite_object((1, 0, 2, 0))),
            (13, 213, sprite_object((1, 0, 2, 0.4))),
            (14, 213, sprite_object((0, 0, 0, 0))),
            (15, 213, sprite_object((1, 0, 0, 3), settings=3 | 4 << 2)),  # packed, a quarter turn
        ]
        sf = load(tpk_file(objects))[0]
        sizes = {10: (0, 3), 11: (0, 3), 12: (2, 0), 13: (2, 0), 14: (0, 0), 15: (3, 0)}
        for path_id, size in sizes.items():
            sprite = sf.objects[path_id].parse_as_object()
            with self.subTest(path_id=path_id, size=size):
                self.assertEqual((sprite.image.mode, sprite.image.size), ("RGBA", size))
                fp = str(self.tmp / f"s{path_id}")
                for extension in (".png", ".tga"):
                    with self.assertRaisesRegex(SystemError, "^tile cannot extend outside image$"):
                        extractor.exportSprite(sprite, fp, extension)
                # ICO keeps only the sizes of its default list that fit: a file holding no image.
                extractor.exportSprite(sprite, fp, ".ico")
                self.assertEqual((self.tmp / f"s{path_id}.ico").read_bytes(), bytes.fromhex("000001000000"))

    # Sections 15.2, 19.2: Image.merge of a sprite's texture and alpha texture.
    def test_sprite_alpha_merge_needs_like_sizes_and_three_bands(self):
        objects = [
            (1, 28, texture_object(bytes(range(64)), (4, 4), TextureFormat.RGBA32)),
            (2, 28, texture_object(bytes(16), (4, 4), TextureFormat.Alpha8)),
            (3, 28, texture_object(bytes(4), (2, 2), TextureFormat.Alpha8)),
            (4, 28, texture_object(bytes([200, 10]) + bytes(6), (4, 4), TextureFormat.BC4)),
            (5, 28, texture_object(bytes(range(48)), (4, 4), TextureFormat.RGB24)),
            (20, 213, sprite_object((0, 0, 4, 4), texture=4, alpha=2)),
            (21, 213, sprite_object((0, 0, 4, 4), texture=1, alpha=3)),
            (22, 213, sprite_object((0, 0, 4, 4), texture=4, alpha=3)),
            (23, 213, sprite_object((0, 0, 4, 4), texture=5, alpha=2)),
            (24, 213, sprite_object((0, 0, 4, 4), texture=1, alpha=4)),
        ]
        sf = load(tpk_file(objects))[0]
        self.assertEqual(sf.objects[4].parse_as_object().image.mode, "L", "BC4 decodes to one band")
        for path_id, message in ((20, "wrong number of bands"), (21, "size mismatch"), (22, "wrong number of bands")):
            with self.subTest(path_id=path_id), self.assertRaisesRegex(ValueError, f"^{message}$"):
                _ = sf.objects[path_id].parse_as_object().image
        # An RGB texture has its three bands, and an alpha texture of one band merges.
        for path_id in (23, 24):
            image = sf.objects[path_id].parse_as_object().image
            self.assertEqual((image.mode, image.size), ("RGBA", (4, 4)))


if __name__ == "__main__":
    unittest.main()
