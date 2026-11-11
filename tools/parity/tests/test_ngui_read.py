"""The NGUI reference's reads (`ngui_read.py`): its Environment sealed to the view, the bounds it decides before
Pillow does, and UnityPy's own behaviour where the reference departs from it, each checked on the oracle.

Needs UnityPy 1.25.2 and Pillow 9.5.0 (the oracle image), skipped elsewhere; the scenes are cargo's
(`ngui_scenes.py`).
"""

import json
import os
import sys
import textwrap
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE / "tests"))

from rows_support import ORACLE  # noqa: E402

import child  # noqa: E402
import scratch  # noqa: E402

if ORACLE:
    import UnityPy
    from PIL import Image
    from UnityPy.enums import TextureFormat
    from UnityPy.export import Texture2DConverter
    from UnityPy.files import SerializedFile

    import ngui_read as rd
    import ngui_scenes

GONE = "outside_view(archive:/CAB-x16/CAB-missing.resS)"
# A child that reads the texture `gone` through UnityPy's own `Texture2D.image`, unsealed, in its working directory.
UNSEALED = textwrap.dedent("""
    import sys, UnityPy
    from UnityPy.files import SerializedFile
    env = UnityPy.Environment(path="")
    bundle = env.load_file(open("x.bundle", "rb").read(), name="x.bundle")
    for sf in bundle.files.values():
        if isinstance(sf, SerializedFile):
            for obj in sf.objects.values():
                if obj.type.name == "Texture2D" and obj.peek_name() == "gone":
                    print(list(obj.read().image.getpixel((0, 0))))
""")


def objects(bundle):
    return [o for sf in bundle.files.values() if isinstance(sf, SerializedFile) for o in sf.objects.values()]


@unittest.skipUnless(ORACLE, "needs UnityPy 1.25.2 (the oracle image)")
class Sealed(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.scenes = ngui_scenes.scenes()

    def test_a_file_in_the_working_directory_is_never_read(self):
        """UnityPy's resource lookup loads a missing resource's names from the working directory
        (`load_dependencies`); the reference, in a working directory that holds one, answers `outside_view`."""
        tmp = scratch.for_case(self, "unityrs-ngui-seal-")
        scene = self.scenes["streamed_sheets"]
        args = [sys.executable, str(HERE / "ngui_cutter.py"), "--input", "x.bundle", "--texture-key", "{name}.png"]
        for bid, data in scene.bundles.items():
            (tmp / bid).write_bytes(data)
            args += ["--bundle", f"{bid}={bid}"]
        planted = bytes([10, 11, 12, 13])
        (tmp / "CAB-missing.resS").write_bytes(planted)
        (tmp / "archive:" / "CAB-x16").mkdir(parents=True)
        (tmp / "archive:/CAB-x16/CAB-missing.resS").write_bytes(planted)
        # UnityPy, unsealed, reads the planted file: so it would decide the record.
        proc = child.run([sys.executable, "-c", UNSEALED], cwd=tmp, capture_output=True, text=True, check=True)
        self.assertEqual(json.loads(proc.stdout), [10, 11, 12, 13])
        proc = child.run(args, cwd=tmp, capture_output=True, text=True, check=True)
        reasons = {(s["path_id"], s["reason"]) for s in json.loads(proc.stdout)["skips"]}
        self.assertIn((4, GONE), reasons)
        self.assertIn((10, GONE), reasons)

    @unittest.skipUnless(os.path.isdir("/etc"), "needs a host whose root holds /etc")
    def test_the_root_is_never_probed(self):
        """With an empty `path`, UnityPy's `find_file` checks `/<name>` (environment.py:330-332): unsealed, a name
        the view does not register that names a directory at the root raises; sealed, it is not found."""
        with self.assertRaises(IsADirectoryError):
            UnityPy.Environment(path="").find_file("etc")
        scene = self.scenes["missing_sheets"]
        bundle = rd.load(scene.view("x.bundle"), "x.bundle")
        self.assertIsNone(bundle.environment.find_file("etc"))
        self.assertIsNone(bundle.environment.load_file("etc", True))
        with self.assertRaises(RuntimeError):
            bundle.environment.fs.exists("/etc")
        n8 = next(o for o in objects(bundle) if o.path_id == 17)
        self.assertIsNone(rd.maintex_name(n8.parse_as_dict(), n8))  # n8's material: `archive:/etc/etc`

    def test_pointers_by_their_deref_error(self):
        scene = self.scenes["missing_sheets"]
        bundle = rd.load(scene.view("x.bundle"), "x.bundle")
        sf = next(o for o in objects(bundle) if o.path_id == 10).assets_file
        cases = [
            ((0, 0), "bad_pointer"),  # null
            ((-1, 2), "bad_pointer"),  # below the start
            ((5, 2), "bad_pointer"),  # past the externals
            ((0, 98), "bad_pointer"),  # a missing object
            ((3, 1), "bad_pointer"),  # the raw `.resS` row
            ((1, 99), "outside_view(CAB-gone)"),
            ((4, 1), "outside_view(etc)"),
        ]
        for (file_id, path_id), reason in cases:
            with self.subTest(pointer=(file_id, path_id)), self.assertRaises(rd.Failure) as caught:
                rd.deref(file_id, path_id, sf)
            self.assertEqual(caught.exception.reason, reason)
        self.assertEqual(rd.deref(2, 1, sf).peek_name(), "elsewhere")  # the dependency's CAB


@unittest.skipUnless(ORACLE, "needs UnityPy 1.25.2 (the oracle image)")
class UnityPyDeparts(unittest.TestCase):
    """Where UnityPy's own path gives another answer than section 5's, the reference's reason."""

    @classmethod
    def setUpClass(cls):
        scene = ngui_scenes.scenes()["streamed_sheets"]
        cls.bundle = rd.load(scene.view("x.bundle"), "x.bundle")
        cls.textures = {o.peek_name(): o for o in objects(cls.bundle) if o.type.name == "Texture2D"}

    def test_a_nested_bundle_s_resource_of_one_name_is_not_the_own_one(self):
        twin = self.textures["twin"].read()
        # UnityPy's registry answers with the nested bundle's row, registered later (base 0x19)...
        self.assertEqual(twin.image.getpixel((0, 0))[0], 0x19)
        # ...4.3's own-chain procedure with the input's own top-level row (base 0x18).
        self.assertEqual(rd.decode(twin, self.textures["twin"]).getpixel((0, 0))[0], 0x18)

    def test_an_empty_stream_path_is_no_data(self):
        empty = self.textures["empty"].read()
        # UnityPy's `StreamingInfo` is truthy with an empty path, so its `get_image_data` looks one up and raises.
        self.assertTrue(empty.m_StreamData)
        self.assertEqual(empty.m_StreamData.path, "")
        with self.assertRaises(FileNotFoundError):
            empty.image  # noqa: B018
        self.assertEqual(rd.data_of(empty, self.textures["empty"].assets_file), b"")

    def test_a_negative_side_is_negative_size(self):
        negs = self.textures["negs"].read()
        with self.assertRaisesRegex(ValueError, "must be >= 0"):
            negs.image  # noqa: B018
        with self.assertRaises(rd.Failure) as caught:
            rd.decode(negs, self.textures["negs"])
        self.assertEqual((caught.exception.reason, caught.exception.detail), ("decode_error", "negative_size"))


@unittest.skipUnless(ORACLE, "needs UnityPy 1.25.2 (the oracle image)")
class Bounds(unittest.TestCase):
    def test_texture_pixels_count_the_padded_size(self):
        """UnityPy's `get_compressed_image_size` pads as `unityrs_texture`'s plan does (decode.rs:249-252): block
        formats to whole 4 x 4 blocks, RGBA32 and RGB24 not at all."""
        size = Texture2DConverter.get_compressed_image_size
        for fmt in (TextureFormat.DXT1, TextureFormat.DXT5, TextureFormat.BC7):
            self.assertEqual(size(16385, 16381, fmt), (16388, 16384))
            self.assertEqual(size(1, 2, fmt), (4, 4))
        for fmt in (TextureFormat.RGBA32, TextureFormat.RGB24):
            self.assertEqual(size(16385, 16381, fmt), (16385, 16381))
        self.assertLess(16385 * 16381, rd.TEXTURE_PIXELS)
        self.assertGreater(16388 * 16384, rd.TEXTURE_PIXELS)

    def test_the_raw_decoder_s_width_cap(self):
        """Pillow's raw decoder refuses a row past `INT_MAX / bits - 7` pixels with `MemoryError`, after the image
        is allocated; at the cap, too little data is `not enough image data`. The cap is `pillow_memory_error`'s
        only source; any other `MemoryError` propagates."""
        for fmt, cap in ((TextureFormat.RGBA32, 67_108_856), (TextureFormat.RGB24, 89_478_478)):
            with self.subTest(fmt=fmt.name):
                self.assertEqual(rd.RAW_WIDTH_CAP[fmt.name], cap)
                with self.assertRaises(MemoryError):
                    Texture2DConverter.parse_image_data(b"\0" * 4, cap + 1, 1, fmt, (2022, 3, 0, 0), 0)
                with self.assertRaisesRegex(ValueError, "not enough image data"):
                    Texture2DConverter.parse_image_data(b"\0" * 4, cap, 1, fmt, (2022, 3, 0, 0), 0)

    def test_crop_bounds_are_pillow_s(self):
        """`Image._crop` refuses more than twice `MAX_IMAGE_PIXELS` first, then its C crop parses each edge as an
        `int`; the reference decides both from the rectangle."""
        im = Image.new("RGBA", (1, 1))
        self.assertEqual(rd.CROP_PIXELS, 178_956_970)
        with self.assertRaises(Image.DecompressionBombError):
            im.crop((0, 0, 20_000, 10_000))
        self.assertEqual(rd.crop_refusal(0, 0, 20_000, 10_000), "decompression_bomb")
        self.assertEqual(rd.crop_refusal(0, 0, 178_956_970, 1), None)
        self.assertEqual(rd.crop_refusal(0, 0, 178_956_971, 1), "decompression_bomb")
        for box, rect in (
            ((2**31 - 1, 0, 2**31, 1), (2**31 - 1, 0, 1, 1)),
            ((-(2**31) - 1, 0, -(2**31), 1), (-(2**31) - 1, 0, 1, 1)),
        ):
            with self.subTest(box=box), self.assertRaises(OverflowError):
                im.crop(box)
            self.assertEqual(rd.crop_refusal(*rect), "crop_overflow")
        self.assertEqual(im.crop((2**31 - 2, 0, 2**31 - 1, 1)).getpixel((0, 0)), (0, 0, 0, 0))
        self.assertIsNone(rd.crop_refusal(2**31 - 2, 0, 1, 1))

    def test_the_integer_node_types(self):
        """Recognition 2's integer types: `FUNCTION_READ_MAP`'s, less `float`, `double`, `bool`, `string` and
        `TypelessData`."""
        self.assertEqual(
            rd.INTEGER,
            {"SInt8", "UInt8", "char", "short", "SInt16", "unsigned short", "UInt16", "int", "SInt32", "unsigned int",
             "UInt32", "Type*", "long long", "SInt64", "unsigned long long", "UInt64", "FileSize"},
        )  # fmt: skip


if __name__ == "__main__":
    unittest.main()
