"""PARITY.md rows on textures, run against the oracle: a texture with no pixel data, a zero-size image, what `set_image`
writes (BC7, swizzled and byte-swapped platforms, the modes Pillow packs), and Texture2DArray.

Sections 5.2, 14.2, 15.1, 17, 18, 18.1, 19.1 and 19.2. Every input is built here (`synth.py`), so nothing needs the
corpus, and each test names the rows it pins; a row's test column names the test back. Its class is `Rows`, as in
`test_parity_rows.py`, the module these tests were split from. Needs UnityPy 1.25.2 (the oracle image); skipped
elsewhere.
"""

import os
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest import mock

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE / "tests"))

from rows_support import ORACLE, load, quietly, text_asset_file, zip_with_a_directory  # noqa: E402

if ORACLE:
    import attrs
    import etcpak
    import UnityPy
    from PIL import Image
    from UnityPy.classes import StreamingInfo, Texture2D, Texture2DArray
    from UnityPy.enums import BuildTarget, TextureFormat
    from UnityPy.enums.GraphicsFormat import GraphicsFormat
    from UnityPy.export import Texture2DConverter
    from UnityPy.helpers import ResourceReader, TextureSwizzler


@unittest.skipUnless(ORACLE, "needs UnityPy 1.25.2 (the oracle image)")
class Rows(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="unityrs-rows-"))
        self.addCleanup(lambda: __import__("shutil").rmtree(self.tmp))

    # Sections 5.2, 14.2, 17: a texture with no pixel data.
    def test_an_empty_texture_follows_an_empty_stream_path(self):
        env = UnityPy.Environment(path=str(self.tmp))
        sf = load(text_asset_file(), env=env)[0]

        def texture(stream, width=4):
            return SimpleNamespace(
                image_data=b"",
                m_StreamData=stream,
                object_reader=SimpleNamespace(assets_file=sf, version=(2021, 3, 2, 0), platform=0),
                m_Width=width,
                m_Height=width,
                m_TextureFormat=4,
                m_PlatformBlob=None,
                get_image_data=lambda: Texture2D.get_image_data(fake),
            )

        fake = texture(StreamingInfo(offset=0, size=0, path=""))
        self.assertTrue(fake.m_StreamData, "a StreamingInfo is truthy, even empty")
        # The empty path is looked up, misses, is loaded relative to env.path, and opens the directory.
        with self.assertRaises(IsADirectoryError):
            Texture2D.get_image_data(fake)
        with self.assertRaises(IsADirectoryError):
            ResourceReader.get_resource_data("", sf, 0, 0)
        # The names tried, in order: the basename, then `{name}.resource`, `{name}.assets.resS`, `{name}.resS`; a
        # miss after load_dependencies raises FileNotFoundError.
        names = ["x.bin", "x.resource", "x.assets.resS", "x.resS"]
        for first in range(4):
            own = UnityPy.Environment(path=str(self.tmp))
            asking = load(text_asset_file(), env=own)[0]
            for n, name in enumerate(names[first:]):
                load(bytes([65 + first + n]) * 10, name=name, env=own)
            with self.subTest(names[first]):
                self.assertEqual(
                    ResourceReader.get_resource_data("archive:/CAB/x.bin", asking, 2, 3), bytes([65 + first]) * 3
                )
        with self.assertRaisesRegex(FileNotFoundError, "^Resource file x.bin not found$"):
            ResourceReader.get_resource_data("archive:/CAB/x.bin", sf, 0, 1)
        # A 0x0 texture fetches its data first too, so where that fetch raises it never reaches the empty-image
        # shortcut. Where the fetch succeeds it does, before the data is looked at: a 0x4 or a 4x0 texture with
        # image_data gives the (0, 0) image.
        fake = texture(StreamingInfo(offset=0, size=0, path=""), width=0)
        with self.assertRaises(IsADirectoryError):
            Texture2DConverter.get_image_from_texture2d(fake)
        for width, height in ((0, 4), (4, 0)):
            fake = texture(None)
            fake.image_data, fake.m_Width, fake.m_Height = b"\xff" * 64, width, height
            with self.subTest(width=width, height=height):
                image = Texture2DConverter.get_image_from_texture2d(fake)
                self.assertEqual((image.mode, image.size), ("RGBA", (0, 0)))
        # Only a texture with no m_StreamData at all (pre-5.3 trees) gets the ValueError.
        fake = texture(None)
        with self.assertRaisesRegex(ValueError, "^No image data found$"):
            Texture2D.get_image_data(fake)
        image = Texture2DConverter.parse_image_data(b"", 0, 0, 4, (2021, 3, 2, 0), 0)
        self.assertEqual((image.mode, image.size), ("RGBA", (0, 0)), "parse_image_data's own shortcut")
        # A zip's directory entry registers CAB "" (section 3), which answers the empty path first: it reads b"", so
        # a 0x0 texture returns the empty image and one of non-zero size raises ValueError.
        quietly(env.load_file, zip_with_a_directory(), name="z.bin")
        self.assertEqual(ResourceReader.get_resource_data("", sf, 0, 8), b"")
        fake = texture(StreamingInfo(offset=0, size=0, path=""), width=0)  # `texture` reads `fake` when called
        image = Texture2DConverter.get_image_from_texture2d(fake)
        self.assertEqual((image.mode, image.size), ("RGBA", (0, 0)))
        fake = texture(StreamingInfo(offset=0, size=0, path=""))
        with self.assertRaisesRegex(ValueError, "^Texture2D has no image data$"):
            Texture2DConverter.get_image_from_texture2d(fake)

    # Sections 18, 19.1: a zero-size image through `image_to_texture2d` and `set_image`.
    def test_zero_size_images_crash_etcpak(self):
        # Each call that reaches etcpak runs in a forked child, which etcpak 0.9.14 kills with a fatal signal on empty
        # data: SIGBUS or SIGSEGV, not the same in every run. The child is made non-dumpable first (PR_SET_DUMPABLE,
        # 0), so the crash writes no core dump anywhere.
        import ctypes
        import signal

        # Section 19.1 states this for a texture that is not Switch-swizzled: the cases run at platform 0, at
        # StandaloneWindows64 and XBOX360 with no platform blob, and at Switch with a blob of one GOB per block. A
        # Switch-swizzled texture is not characterised.
        configs = ((0, None), (BuildTarget.StandaloneWindows64, None), (BuildTarget.XBOX360, None))
        configs += ((BuildTarget.Switch, [0] * 12),)
        self.assertEqual([TextureSwizzler.is_switch_swizzled(*config) for config in configs], [False] * 4)

        def convert(target, size, config):
            return Texture2DConverter.image_to_texture2d(Image.new("RGBA", size), TextureFormat[target], *config)

        def crashes(target, width, height, config):
            pid = os.fork()
            if pid == 0:  # the child: the one call, then out without running the parent's cleanups
                try:
                    ctypes.CDLL(None, use_errno=True).prctl(4, 0, 0, 0, 0)
                    os.dup2(os.open(os.devnull, os.O_WRONLY), 2)
                    convert(target, (width, height), config)
                finally:
                    os._exit(0)
            status = os.waitpid(pid, 0)[1]
            return os.WIFSIGNALED(status) and os.WTERMSIG(status) in (signal.SIGBUS, signal.SIGSEGV)

        def write(target, size, config):
            data, written = convert(target, size, config)
            return data, written.name

        etcpak_targets = ["DXT1", "DXT1Crunched", "DXT5", "DXT5Crunched", "BC4", "BC5", "ETC_RGB4", "ETC_RGB4Crunched"]
        etcpak_targets += ["ETC_RGB4_3DS", "ETC2_RGB", "ETC2_RGBA1", "ETC2_RGBA8", "ETC2_RGBA8Crunched"]
        sizes = ((0, 0), (0, 4), (4, 0))
        astc = [f"ASTC_{kind}_{n}x{n}" for kind in ("RGB", "RGBA", "HDR") for n in (4, 5, 6, 8, 10, 12)]
        self.assertEqual(sorted(astc), sorted(f.name for f in TextureFormat if f.name.startswith("ASTC")))
        raising = {(0, 4): (6, 8, 10, 12), (4, 0): (6, 8, 10, 12), (0, 5): (4, 8, 10, 12), (5, 0): (4, 8, 10, 12)}
        bc7 = "^params must be an instance of BC7CompressBlockParams$"
        padded = "^height and width must be > 0$"
        for config in configs:
            cases = [(target, *size, config) for target in etcpak_targets for size in sizes]
            self.assertEqual([case for case in cases if not crashes(*case)], [])
            # BC7 raises before etcpak reads the data; where padding to whole blocks resizes an empty strip, Pillow
            # raises first, for every one of these targets.
            for size in sizes:
                with self.subTest("BC7", size=size, config=config), self.assertRaisesRegex(ValueError, bc7):
                    convert("BC7", size, config)
            for target in [*etcpak_targets, "BC7"]:
                for size in ((0, 5), (5, 0)):
                    with (
                        self.subTest(target=target, size=size, config=config),
                        self.assertRaisesRegex(ValueError, padded),
                    ):
                        convert(target, size, config)

            # Every ASTC target pads the same way: Pillow raises where the padding stretches the empty one-row (or
            # one-column) strip it repeats to more than one (0×4 at blocks of 6 and up, 0×5 at 4 and at 8 and up; a
            # resize to the same size returns a copy), and the rest write 0 bytes.
            writing = {}
            for target in astc:
                block = int(target.rsplit("x", 1)[1])
                for size in (*sizes, (0, 5), (5, 0)):
                    with self.subTest(target=target, size=size, config=config):
                        if block in raising.get(size, ()):
                            with self.assertRaisesRegex(ValueError, padded):
                                write(target, size, config)
                        else:
                            writing[target, size] = write(target, size, config)
            self.assertEqual(len(writing), 18 * 5 - 3 * 4 * 4)
            # The other targets tested write 0 bytes for these three sizes. Each target that writes does so under the
            # format image_to_texture2d maps it to for a non-empty image.
            for target in ("RGBA32", "ARGB32", "Alpha8", "R16", "RGB565"):
                for size in sizes:
                    writing[target, size] = write(target, size, config)
            for (target, size), (data, tag) in writing.items():
                with self.subTest(target=target, size=size, config=config):
                    self.assertEqual((data, tag), (b"", write(target, (4, 4), config)[1]))
            mapped = {"ARGB32": "RGBA32", "R16": "R8", "RGB565": "RGB24", "ASTC_HDR_4x4": "ASTC_RGBA_4x4"}
            self.assertEqual({target: writing[target, (0, 0)][1] for target in mapped}, mapped)
        # Texture2D.image of a texture with a zero side is that 0×0 image, and set_image to RGBA32 writes it with
        # the image's size.
        texture = Texture2D.__new__(Texture2D)
        for field in attrs.fields(Texture2D):
            object.__setattr__(texture, field.name, None)
        texture.object_reader, texture.m_PlatformBlob = None, None
        texture.m_Width, texture.m_Height, texture.m_TextureFormat = 4, 0, TextureFormat.RGBA32
        texture.image_data = b"\x01" * 16
        self.assertEqual((texture.image.mode, texture.image.size), ("RGBA", (0, 0)))
        texture.set_image(Image.new("RGBA", (4, 0)), TextureFormat.RGBA32)
        self.assertEqual((texture.image_data, texture.m_Width, texture.m_Height), (b"", 4, 0))

    # Sections 15.1, 18, 19.2: the BC7 bytes `set_image` should write.
    def test_bc7_set_image_reference_is_etcpak_default(self):
        image = Image.new("RGBA", (6, 5))
        image.putdata([(x * 40, y * 50, (x * y * 13) % 256, 255 - x * 30) for y in range(5) for x in range(6)])
        with self.assertRaisesRegex(ValueError, "^params must be an instance of BC7CompressBlockParams$"):
            Texture2DConverter.image_to_texture2d(image, TextureFormat.BC7)
        # The call UnityPy means to make: default parameters for its None.
        compress = etcpak.compress_bc7

        def with_defaults(data, width, height, params):
            self.assertIsNone(params)
            return compress(data, width, height, etcpak.BC7CompressBlockParams())

        with mock.patch.object(etcpak, "compress_bc7", with_defaults):
            written, tag = Texture2DConverter.image_to_texture2d(image, TextureFormat.BC7)
        self.assertEqual(tag, TextureFormat.BC7)
        # The data it hands etcpak: the image flipped, padded to whole 4x4 blocks, as RGBA.
        padded = Texture2DConverter.pad_image(image.transpose(Image.Transpose.FLIP_TOP_BOTTOM), 8, 8)
        reference = compress(padded.tobytes("raw", "RGBA"), 8, 8, etcpak.BC7CompressBlockParams())
        self.assertEqual(written, reference)
        self.assertEqual(compress(padded.tobytes("raw", "RGBA"), 8, 8, etcpak.BC7CompressBlockParams()), reference)
        decoded = Texture2DConverter.parse_image_data(written, 6, 5, TextureFormat.BC7, (2021, 3, 1, 3), 0)
        self.assertEqual((decoded.mode, decoded.size), ("RGBA", (6, 5)))
        # On a Switch-swizzled texture the image is padded to the swizzle's size, not to whole 4x4 blocks,
        # compressed, and the compressed bytes swizzled with BC7's 4x4 block and the blob's GOBs.
        blob = [0] * 8 + list((4).to_bytes(4, "little"))  # 1 << 4 = 16 GOBs per block
        with mock.patch.object(etcpak, "compress_bc7", with_defaults):
            switch, tag = Texture2DConverter.image_to_texture2d(image, TextureFormat.BC7, BuildTarget.Switch, blob)
        self.assertEqual(tag, TextureFormat.BC7)
        self.assertEqual(TextureSwizzler.TEXTURE_FORMAT_BLOCK_SIZE_MAP[TextureFormat.BC7], (4, 4))
        width, height = TextureSwizzler.get_padded_texture_size(6, 5, 4, 4, 16)
        self.assertEqual((width, height), (16, 512))
        flipped = image.transpose(Image.Transpose.FLIP_TOP_BOTTOM)
        linear = compress(
            Texture2DConverter.pad_image(flipped, width, height).tobytes("raw", "RGBA"),
            width,
            height,
            etcpak.BC7CompressBlockParams(),
        )
        self.assertEqual(switch, bytes(TextureSwizzler.swizzle(linear, width, height, 4, 4, 16)))
        self.assertEqual((len(switch), len(written)), (8192, 64))
        # The reader deswizzles it back: the same pixels as the linear write (its blocks hold the same texels).
        version = (2021, 3, 1, 3)
        back = Texture2DConverter.parse_image_data(switch, 6, 5, TextureFormat.BC7, version, BuildTarget.Switch, blob)
        self.assertEqual(list(back.getdata()), list(decoded.getdata()))

    # Sections 18, 19.2: `set_image` on a swizzled or byte-swapped platform.
    def test_set_image_writes_linear_mips_and_never_swaps_for_xbox(self):
        image = Image.new("RGBA", (64, 64))
        image.putdata([(x * 4, y * 4, 128, 255) for y in range(64) for x in range(64)])
        blob = [0] * 8 + list((4).to_bytes(4, "little"))  # 16 GOBs per block

        def texture(platform, blob=None):
            return SimpleNamespace(
                m_TextureFormat=TextureFormat.RGBA32,
                object_reader=SimpleNamespace(platform=platform),
                m_PlatformBlob=blob,
                m_MipMap=False,
                m_MipCount=1,
                m_StreamData=None,
            )

        switch = texture(BuildTarget.Switch, blob)
        Texture2D.set_image(switch, image, TextureFormat.RGBA32, 2)
        top, _ = Texture2DConverter.image_to_texture2d(image, TextureFormat.RGBA32, BuildTarget.Switch, blob)
        mip, _ = Texture2DConverter.image_to_texture2d(
            image.resize((32, 32), Image.Resampling.BICUBIC), TextureFormat.RGBA32
        )
        self.assertNotEqual(top, Texture2DConverter.image_to_texture2d(image, TextureFormat.RGBA32)[0], "swizzled")
        self.assertEqual(switch.image_data, top + mip, "a swizzled top level, then a linear mip")
        self.assertEqual((switch.m_MipCount, switch.m_MipMap), (2, True))
        # A swizzled mip would be padded to whole GOB blocks: four times the linear one here.
        swizzled_mip, _ = Texture2DConverter.image_to_texture2d(
            image.resize((32, 32), Image.Resampling.BICUBIC), TextureFormat.RGBA32, BuildTarget.Switch, blob
        )
        self.assertEqual((len(mip), len(swizzled_mip)), (4096, 16384))
        self.assertTrue(TextureSwizzler.is_switch_swizzled(BuildTarget.Switch, blob))
        # XBOX360: the reader swaps DXT1's byte pairs, the writer never does, so a written texture reads back wrong.
        xbox = texture(BuildTarget.XBOX360)
        Texture2D.set_image(xbox, image, TextureFormat.DXT1)
        self.assertEqual(xbox.image_data, Texture2DConverter.image_to_texture2d(image, TextureFormat.DXT1)[0])
        args = (xbox.image_data, 64, 64, TextureFormat.DXT1, (2021, 3, 1, 3))
        plain = Texture2DConverter.parse_image_data(*args, BuildTarget.StandaloneWindows64)
        swapped = Texture2DConverter.parse_image_data(*args, BuildTarget.XBOX360)
        self.assertNotEqual(list(swapped.getdata()), list(plain.getdata()))

    # Sections 15.1, 18, 18.1, 19.2: `set_image` packs the image in the mode it opened in.
    def test_set_image_packs_only_what_pillow_packs(self):
        image = Image.new("RGBA", (8, 8))
        image.putdata([(x * 30, y * 30, 7, 255 - x) for y in range(8) for x in range(8)])

        def texture():
            return SimpleNamespace(
                m_TextureFormat=TextureFormat.DXT1,
                object_reader=None,
                m_PlatformBlob=None,
                m_MipMap=False,
                m_MipCount=1,
                m_StreamData=None,
            )

        targets = {
            TextureFormat.DXT1: "RGBA",
            TextureFormat.RGB24: "RGB",
            TextureFormat.R8: "R",
            TextureFormat.Alpha8: "A",
        }
        # An indexed and a grey PNG, set from a path, keep the mode Image.open gives and raise for every target.
        for mode in ("P", "L"):
            path = self.tmp / f"{mode}.png"
            image.convert(mode).save(path)
            with Image.open(path) as opened:
                self.assertEqual(opened.mode, mode)
            for fmt, raw in targets.items():
                with self.subTest(mode, target=fmt.name):
                    with self.assertRaisesRegex(ValueError, f"^No packer found from {mode} to {raw}$"):
                        Texture2D.set_image(texture(), str(path), fmt)
                    # The reference: UnityPy's set_image of the image converted to RGBA, which packs.
                    converted = texture()
                    Texture2D.set_image(converted, Image.open(path).convert("RGBA"), fmt)
                    reference = Texture2DConverter.image_to_texture2d(Image.open(path).convert("RGBA"), fmt)[0]
                    self.assertEqual(converted.image_data, reference)
        # RGB raises for Alpha8 alone; converted, its alpha is 255.
        rgb = image.convert("RGB")
        with self.assertRaisesRegex(ValueError, "^No packer found from RGB to A$"):
            Texture2DConverter.image_to_texture2d(rgb, TextureFormat.Alpha8)
        for fmt in set(targets) - {TextureFormat.Alpha8}:
            Texture2DConverter.image_to_texture2d(rgb, fmt)
        self.assertEqual(
            Texture2DConverter.image_to_texture2d(rgb.convert("RGBA"), TextureFormat.Alpha8)[0], b"\xff" * 64
        )
        # LAB packs into Alpha8: Pillow's `A` band of LAB is its a* channel.
        lab = Image.new("LAB", (8, 8), (10, 20, 30))
        self.assertEqual(Texture2DConverter.image_to_texture2d(lab, TextureFormat.Alpha8)[0], bytes([20]) * 64)
        # Modes Pillow cannot convert to RGBA either: Tier U.
        for mode in ("BGR;15", "BGR;16", "BGR;24", "La"):
            with self.subTest(mode), self.assertRaisesRegex(ValueError, "^conversion from .* not supported$"):
                Image.new(mode, (2, 2)).convert("RGBA")

    # Sections 14.2, 19.2: Texture2DArray decodes every slice with platform 0 and no blob.
    def test_texture2darray_ignores_the_platform(self):
        self.assertIn("m_PlatformBlob", attrs.fields_dict(Texture2D))
        self.assertNotIn("m_PlatformBlob", attrs.fields_dict(Texture2DArray))
        data = bytes(range(64)) * 2  # two 4x4 RGBA32 slices
        seen = []
        parse = Texture2DConverter.parse_image_data

        def record(image_data, width, height, fmt, version, platform, blob=None, flip=True):
            seen.append((platform, blob))
            return parse(image_data, width, height, fmt, version, platform, blob, flip)

        for platform in (BuildTarget.Switch, BuildTarget.XBOX360):
            array = self.texture_array(GraphicsFormat.R8G8B8A8_UNorm, data, 4, 2, platform)
            with mock.patch.object(Texture2DConverter, "parse_image_data", record):
                images = Texture2DArray.images.fget(array)
            self.assertEqual(
                [image.tobytes() for image in images], [self.flipped_rgba(data[:64]), self.flipped_rgba(data[64:])]
            )
        self.assertEqual(seen, [(0, None)] * 4)

    @staticmethod
    def flipped_rgba(data):
        return Image.frombytes("RGBA", (4, 4), data).transpose(Image.Transpose.FLIP_TOP_BOTTOM).tobytes()

    def texture_array(self, graphics_format, data, width, depth, platform=0):
        return SimpleNamespace(
            m_Format=int(graphics_format),
            image_data=data,
            m_StreamData=None,
            m_DataSize=len(data),
            m_Depth=depth,
            m_Width=width,
            m_Height=width,
            object_reader=SimpleNamespace(version=(2021, 3, 2, 0), platform=platform, assets_file=None),
        )

    # Sections 14.2, 19.2: a streamed array's data is never read, and the reference is the same bytes held inline.
    def test_streamed_texture2darray_never_reads_its_resource(self):
        data = bytes(range(64)) * 2  # two 4x4 RGBA32 slices, as the .resS would hold them
        reads = []

        def get_resource_data(path, assets_file, offset, size):
            reads.append((path, offset, size))
            return data[offset : offset + size]

        streamed = self.texture_array(GraphicsFormat.R8G8B8A8_UNorm, b"", 4, 2)  # a parsed array's empty data
        streamed.m_StreamData = StreamingInfo(path="archive:/CAB-x/CAB-x.resS", offset=0, size=len(data))
        streamed.m_DataSize = len(data)
        with mock.patch.object(ResourceReader, "get_resource_data", get_resource_data):
            with self.assertRaisesRegex(ValueError, "^Texture2D has no image data$"):
                Texture2DArray.images.fget(streamed)
            self.assertEqual(reads, [], "`image_data` is b'', not None, so the .resS is never read")
            # The reference: the bytes the .resS holds, decoded as an array holding them inline decodes them.
            inline = Texture2DArray.images.fget(self.texture_array(GraphicsFormat.R8G8B8A8_UNorm, data, 4, 2))
            streamed.image_data = None  # what the fallback needs to fire
            read = Texture2DArray.images.fget(streamed)
        self.assertEqual(reads, [("archive:/CAB-x/CAB-x.resS", 0, len(data))])
        self.assertEqual([image.tobytes() for image in read], [image.tobytes() for image in inline])
        self.assertEqual(
            [image.tobytes() for image in inline], [self.flipped_rgba(data[:64]), self.flipped_rgba(data[64:])]
        )

    # Sections 14.2, 19.2: the 8-bit integer graphics formats map to 16-bit texture formats.
    def test_texture2darray_8bit_integer_formats(self):
        raises = {
            "R8_UInt": ValueError,  # R16
            "R8G8_UInt": KeyError,  # RG32
            "R8G8B8A8_UInt": ValueError,  # RGBA64
            "R8_SInt": ValueError,  # R16_SIGNED
            "R8G8_SInt": KeyError,  # RG32_SIGNED
            "R8G8B8_SInt": ValueError,  # RGB48_SIGNED
            "R8G8B8A8_SInt": ValueError,  # RGBA64_SIGNED
        }
        for name, error in raises.items():
            channels = name.count("8")
            with self.subTest(name), self.assertRaises(error):
                Texture2DArray.images.fget(self.texture_array(GraphicsFormat[name], bytes(16 * channels), 4, 1))
        # A value GraphicsFormat lacks raises ValueError; a graphics format the map lacks, NotImplementedError.
        from UnityPy.enums.GraphicsFormat import GRAPHICS_TO_TEXTURE_MAP

        unmapped = next(g for g in GraphicsFormat if g not in GRAPHICS_TO_TEXTURE_MAP)
        for value, error in ((99999, ValueError), (unmapped, NotImplementedError)):
            with self.subTest(value=value), self.assertRaises(error):
                Texture2DArray.images.fget(self.texture_array(value, bytes(64), 4, 1))
        # R8G8B8_UInt decodes as RGB48, Pillow's 5-6-5 `RGB;16`: not the stored bytes.
        data = bytes(range(48))
        image = Texture2DArray.images.fget(self.texture_array(GraphicsFormat.R8G8B8_UInt, data, 4, 1))[0]
        self.assertEqual(image.mode, "RGB")
        self.assertNotEqual(
            image.tobytes(), Image.frombytes("RGB", (4, 4), data).transpose(Image.Transpose.FLIP_TOP_BOTTOM).tobytes()
        )


if __name__ == "__main__":
    unittest.main()
