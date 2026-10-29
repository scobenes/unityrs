"""PARITY.md rows on textures, audio and export, run against the oracle.

Texture2D and Texture2DArray decoding, `set_image`, Pillow's save, the
texture and audio helpers, `export_obj`'s names and PackedBitVector, each on
the smallest input that shows it, built here (`synth.py`). Each test names the
PARITY.md rows it keeps honest, and each row's test column names the test back
(test_parity_rows.py checks both). Needs UnityPy 1.25.2 (the oracle image);
skipped elsewhere.
"""

import random
import struct
import sys
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest import mock

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE / "tests"))

import synth as S  # noqa: E402
from parity_rows import ORACLE, TEXT_ASSET, RowsCase, load, text_asset_file  # noqa: E402

try:
    import attrs
    import etcpak
    import UnityPy
    from PIL import Image
    from UnityPy.classes import PPtr, StreamedResource, StreamingInfo, Texture2D, Texture2DArray
    from UnityPy.enums import BuildTarget, TextureFormat
    from UnityPy.enums.GraphicsFormat import GraphicsFormat
    from UnityPy.export import AudioClipConverter, Texture2DConverter
    from UnityPy.helpers import PackedBitVector, ResourceReader, TextureSwizzler
    from UnityPy.tools import extractor
except ImportError:  # pragma: no cover - outside the oracle image (ORACLE is False)
    pass


@unittest.skipUnless(ORACLE, "needs UnityPy 1.25.2 (the oracle image)")
class MediaRows(RowsCase):
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
        # A 0x0 texture fetches its data first too, so it never reaches the empty-image shortcut.
        fake = texture(StreamingInfo(offset=0, size=0, path=""), width=0)
        with self.assertRaises(IsADirectoryError):
            Texture2DConverter.get_image_from_texture2d(fake)
        # Only a texture with no m_StreamData at all (pre-5.3 trees) gets the ValueError.
        fake = texture(None)
        with self.assertRaisesRegex(ValueError, "^No image data found$"):
            Texture2D.get_image_data(fake)
        image = Texture2DConverter.parse_image_data(b"", 0, 0, 4, (2021, 3, 2, 0), 0)
        self.assertEqual((image.mode, image.size), ("RGBA", (0, 0)), "parse_image_data's own shortcut")

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
        # R8G8B8_UInt decodes as RGB48, Pillow's 5-6-5 `RGB;16`: not the stored bytes.
        data = bytes(range(48))
        image = Texture2DArray.images.fget(self.texture_array(GraphicsFormat.R8G8B8_UInt, data, 4, 1))[0]
        self.assertEqual(image.mode, "RGB")
        self.assertNotEqual(
            image.tobytes(), Image.frombytes("RGB", (4, 4), data).transpose(Image.Transpose.FLIP_TOP_BOTTOM).tobytes()
        )

    # Sections 15.7, 19.2: what Pillow saves under the container's extension.
    def test_pillow_saves_by_extension_and_mode(self):
        image = Image.new("RGBA", (4, 4), (1, 2, 3, 4))
        cases = [
            (".png", "RGBA", None),
            (".tga", "RGBA", None),
            (".jpg", "RGB", None),
            (".jpg", "RGBA", OSError),
            (".jpeg", "LA", OSError),
            (".psd", "RGBA", KeyError),
            (".xyz", "RGBA", ValueError),
            ("", "RGBA", ValueError),
        ]
        for extension, mode, error in cases:
            with self.subTest(extension=extension, mode=mode):
                path = self.tmp / f"image{extension}"
                if error is None:
                    image.convert(mode).save(path)
                    self.assertTrue(path.exists())
                else:
                    with self.assertRaises(error):
                        image.convert(mode).save(path)

    # Sections 15.7, 19.2: the names `export_obj(ref, fp, True, True)` gives, as exportGameObject means to call it.
    def test_export_obj_appends_none_for_the_path_id(self):
        sf = load(text_asset_file("hello", "world"), env=UnityPy.Environment(path=str(self.tmp)))[0]
        (self.tmp / "go").mkdir()  # exportGameObject makes its folder first
        exported = extractor.export_obj(PPtr(m_FileID=0, m_PathID=1, assetsfile=sf), str(self.tmp / "go"), True, True)
        self.assertEqual(exported, [(sf, 1)])
        self.assertEqual((self.tmp / "go" / "hello_None.txt").read_bytes(), b"world")

    # Section 19.2: with `_None` in place of the path id, objects of one name share one file in the crawl's folder.
    def test_export_obj_names_collide_without_the_path_id(self):
        objects = [S.Obj(1, 0, S.s32("") + S.s32("first")), S.Obj(2, 0, S.s32("") + S.s32("second"))]
        data = S.serialized_file([S.Type(49, TEXT_ASSET)], objects)
        sf = load(data, env=UnityPy.Environment(path=str(self.tmp)))[0]
        folder = self.tmp / "go"
        folder.mkdir()
        for path_id in (1, 2):  # two components with an empty m_Name, as most MonoBehaviours have
            pointer = PPtr(m_FileID=0, m_PathID=path_id, assetsfile=sf)
            self.assertEqual(extractor.export_obj(pointer, str(folder), True, True), [(sf, path_id)])
        # Both land on `{fp}/_None.txt`; the later overwrites the earlier.
        self.assertEqual(sorted(p.name for p in folder.iterdir()), ["_None.txt"])
        self.assertEqual((folder / "_None.txt").read_bytes(), b"second")
        # The names exportGameObject means to give them, with the path id appended, are distinct.
        self.assertEqual(len({f"_{reader.path_id}.txt" for reader in sf.objects.values()}), 2)

    # Sections 16, 19.2: an AudioClip with no data.
    def test_an_empty_audio_clip_follows_an_empty_resource_path(self):
        env = UnityPy.Environment(path=str(self.tmp))
        bundle = load(S.fs_bundle([(b"CAB-a", text_asset_file(), 4), (b"CAB-a.resS", b"\x11" * 16, 0)]), env=env)[0]
        sf = bundle.files["CAB-a"]

        def clip(resource, audio_data=b""):
            return SimpleNamespace(
                m_AudioData=audio_data,
                m_Resource=resource,
                object_reader=SimpleNamespace(assets_file=sf),
                m_Name="clip",
                m_Channels=1,
                m_Frequency=44100,
            )

        empty = StreamedResource(m_Source="", m_Offset=0, m_Size=0)
        self.assertTrue(empty, "a StreamedResource is truthy, even empty")
        # The empty path is looked up, misses, is loaded relative to env.path, and opens the directory.
        for size in (0, 16):
            with self.subTest(size=size), self.assertRaises(IsADirectoryError):
                AudioClipConverter.extract_audioclip_samples(
                    clip(StreamedResource(m_Source="", m_Offset=0, m_Size=size))
                )
        # A real resource with no bytes reaches FMOD, which refuses it.
        source = StreamedResource(m_Source="archive:/CAB-a/CAB-a.resS", m_Offset=0, m_Size=0)
        with self.assertRaises(Exception) as raised:
            AudioClipConverter.extract_audioclip_samples(clip(source))
        self.assertEqual(type(raised.exception).__name__, "FmodError")
        # Only a clip with no m_Resource at all gets the ValueError.
        with self.assertRaisesRegex(ValueError, "^AudioClip with neither m_AudioData nor m_Resource$"):
            AudioClipConverter.extract_audioclip_samples(clip(None))
        ogg = b"OggS" + bytes(12)
        self.assertEqual(AudioClipConverter.extract_audioclip_samples(clip(None, ogg)), {"clip.ogg": ogg})

    # Sections 15.1, 15.6, 23: texture helpers of Texture2DConverter and TextureSwizzler.
    def test_texture_helpers(self):
        import threading

        size = Texture2DConverter.get_compressed_image_size
        for fmt, padded in (
            (TextureFormat.DXT1, (8, 4)),
            (TextureFormat.BC7, (8, 4)),
            (TextureFormat.ETC2_RGBA8, (8, 4)),
            (TextureFormat.EAC_R, (8, 4)),
            (TextureFormat.PVRTC_RGB2, (8, 4)),
            (TextureFormat.PVRTC_RGBA4, (8, 4)),
            (TextureFormat.ASTC_RGB_6x6, (6, 6)),
            (TextureFormat.ASTC_RGBA_10x10, (10, 10)),
            (TextureFormat.ATC_RGBA8, (5, 3)),
            (TextureFormat.RGBA32, (5, 3)),
        ):
            with self.subTest(fmt.name):
                self.assertEqual(size(5, 3, fmt), padded)
        self.assertEqual(size(9, 3, TextureFormat.PVRTC_RGB2), (16, 4), "PVRTC 2 bpp: 8x4 blocks")
        # parse_image_data's checks before any format: a zero side, no data, a number outside the enum.
        parse, version = Texture2DConverter.parse_image_data, (2021, 3, 2, 0)
        for width, height in ((0, 4), (4, 0)):
            empty = parse(b"\x01" * 64, width, height, TextureFormat.RGBA32, version, 0)
            self.assertEqual((empty.mode, empty.size), ("RGBA", (0, 0)))
        with self.assertRaisesRegex(ValueError, "^Texture2D has no image data$"):
            parse(b"", 4, 4, TextureFormat.RGBA32, version, 0)
        with self.assertRaisesRegex(ValueError, "is not a valid TextureFormat"):
            parse(b"\x01" * 64, 4, 4, 999, version, 0)
        # pad_image: the last column and row, then the corner pixel.
        image = Image.new("L", (2, 2))
        image.putdata([1, 2, 3, 4])
        self.assertEqual(
            list(Texture2DConverter.pad_image(image, 4, 3).getdata()), [1, 2, 2, 2, 3, 4, 4, 4, 3, 4, 4, 4]
        )
        self.assertIs(Texture2DConverter.pad_image(image, 2, 2), image)
        # The Xbox swap: byte pairs, a last odd byte kept.
        self.assertEqual(
            Texture2DConverter.swap_bytes_for_xbox(b"\x01\x02\x03\x04\x05"), bytearray(b"\x02\x01\x04\x03\x05")
        )
        self.assertEqual(
            Texture2DConverter.XBOX_SWAP_FORMATS,
            [
                TextureFormat.RGB565,
                TextureFormat.DXT1,
                TextureFormat.DXT1Crunched,
                TextureFormat.DXT5,
                TextureFormat.DXT5Crunched,
            ],
        )
        # ASTC: 16 bytes a block; data short of the blocks raises; one LDR context per thread and block size.
        self.assertEqual(Texture2DConverter.calculate_astc_compressed_size(9, 5, (4, 4)), 3 * 2 * 16)
        with self.assertRaisesRegex(ValueError, "Invalid ASTC data size"):
            Texture2DConverter.astc(bytes(15), 4, 4, (4, 4))
        context = Texture2DConverter.get_astc_context((4, 4))
        self.assertIs(Texture2DConverter.get_astc_context((4, 4)), context)
        self.assertIsNot(Texture2DConverter.get_astc_context((6, 6)), context)
        other = []
        thread = threading.Thread(target=lambda: other.append(Texture2DConverter.get_astc_context((4, 4))))
        thread.start()
        thread.join()
        self.assertIsNot(other[0], context)
        import astc_encoder

        self.assertEqual((context.config.block_x, context.config.block_y), (4, 4))
        self.assertEqual(context.config.profile, astc_encoder.ASTCProfile.LDR)
        self.assertEqual(context.config.flags, astc_encoder.ASTCConfigFlags.USE_DECODE_UNORM8)
        made = []
        with (
            mock.patch.object(astc_encoder, "ASTCConfig", lambda *a, **kw: made.append((a, kw)) or a[0]),
            mock.patch.object(astc_encoder, "ASTCContext", lambda config: config),
        ):
            Texture2DConverter._get_astc_context.__wrapped__(0, (5, 5))
        self.assertEqual(made[0][1]["quality"], 100)
        # TextureSwizzler: the GOB, the blob, the padded size and the walk.
        self.assertEqual(
            (TextureSwizzler.GOB_X_TEXEL_COUNT, TextureSwizzler.GOB_Y_TEXEL_COUNT, TextureSwizzler.TEXEL_BYTE_SIZE),
            (4, 8, 16),
        )
        self.assertEqual(
            (TextureSwizzler.TEXELS_IN_GOB, sorted(TextureSwizzler.GOB_MAP)),
            (32, [(x, y) for x in range(4) for y in range(8)]),
        )
        blob = [0] * 8 + list((3).to_bytes(4, "little"))
        self.assertEqual(TextureSwizzler.get_switch_gobs_per_block(blob), 8)
        self.assertTrue(TextureSwizzler.is_switch_swizzled(BuildTarget.Switch, blob))
        self.assertFalse(TextureSwizzler.is_switch_swizzled(BuildTarget.Switch, [0] * 12), "one GOB per block")
        self.assertFalse(TextureSwizzler.is_switch_swizzled(BuildTarget.Switch, blob[:11]), "under 12 bytes")
        self.assertFalse(TextureSwizzler.is_switch_swizzled(BuildTarget.Android, blob))
        self.assertEqual(TextureSwizzler.get_padded_texture_size(5, 9, 4, 4, 2), (16, 64))
        self.assertEqual(len(TextureSwizzler.TEXTURE_FORMAT_BLOCK_SIZE_MAP), 29)
        data = bytes(range(256)) * 4  # one block of 2 GOBs of RGBA32: 16 x 16 pixels, 1024 bytes
        swizzled = TextureSwizzler.swizzle(data, 16, 16, 4, 1, 2)
        self.assertNotEqual(swizzled, data)
        self.assertEqual(TextureSwizzler.deswizzle(swizzled, 16, 16, 4, 1, 2), data)
        # Only whole blocks of GOBs are walked: the rest of the output stays zero.
        self.assertEqual(TextureSwizzler.deswizzle(data, 16, 8, 4, 1, 2), bytearray(len(data)))

    # Section 16: what the dispatcher does before FMOD.
    def test_audio_dispatch(self):
        def clip(data, channels=0, frequency=0):
            return SimpleNamespace(
                m_AudioData=data,
                m_Resource=None,
                m_Name="c",
                m_Channels=channels,
                m_Frequency=frequency,
                object_reader=None,
            )

        for data, name in (
            (b"OggS" + bytes(8), "c.ogg"),
            (b"RIFF" + bytes(8), "c.wav"),
            (b"\0\0\0\x20ftypM4A ", "c.m4a"),
        ):
            with self.subTest(name):
                self.assertEqual(AudioClipConverter.extract_audioclip_samples(clip(data)), {name: data})
        calls = []
        with mock.patch.object(
            AudioClipConverter.fmod_toolkit, "raw_to_wav", lambda *a, **kw: calls.append((a, kw)) or {"done": b""}
        ):
            self.assertEqual(AudioClipConverter.extract_audioclip_samples(clip(b"FSB5" + bytes(8))), {"done": b""})
            AudioClipConverter.extract_audioclip_samples(clip(b"FSB5", 1, 22050), convert_pcm_float=False)
        self.assertEqual(calls[0], ((b"FSB5" + bytes(8), "c", 2, 44100), {"convert_pcm_float": True}))
        self.assertEqual(calls[1], ((b"FSB5", "c", 1, 22050), {"convert_pcm_float": False}))
        from UnityPy.enums.Audio import AUDIO_TYPE_EXTEMSION, AudioCompressionFormat, AudioType

        # 22 entries written, 20 kept: the two IntEnums share values 1 and 2, and the later entry wins.
        self.assertEqual(len(AUDIO_TYPE_EXTEMSION), 20)
        self.assertEqual(
            (AUDIO_TYPE_EXTEMSION[AudioType.MPEG], AUDIO_TYPE_EXTEMSION[AudioCompressionFormat.MP3]), (".mp3", ".fsb")
        )
        from UnityPy.classes import AudioClip

        extension = {
            fmt.name: AudioClip.extension.fget(SimpleNamespace(m_CompressionFormat=fmt))
            for fmt in AudioCompressionFormat
        }
        self.assertEqual(
            extension,
            {
                "PCM": ".fsb",
                "Vorbis": ".fsb",
                "ADPCM": ".fsb",
                "MP3": ".fsb",
                "VAG": ".vag",
                "HEVAG": ".vag",
                "XMA": ".wav",
                "AAC": ".m4a",
                "GCADPCM": ".fsb",
                "ATRAC9": ".at9",
            },
        )
        self.assertEqual(AudioClip.extension.fget(SimpleNamespace(m_CompressionFormat=11)), ".audioclip")

    # Section 15.3: PackedBitVector.unpack_floats divides before it multiplies.
    def test_unpack_floats_divides_first(self):
        def f32(value):
            return struct.unpack("<f", struct.pack("<f", value))[0]

        divided = multiplied = 0
        rng = random.Random(7)
        for _ in range(50):
            bits = rng.choice([8, 10, 11, 12, 16, 20, 24])
            values = [rng.randrange(1 << bits) for _ in range(32)]
            packed = sum(v << (bits * i) for i, v in enumerate(values)).to_bytes((bits * 32 + 7) // 8, "little")
            vector = SimpleNamespace(
                m_NumItems=32,
                m_BitSize=bits,
                m_Range=f32(rng.uniform(0.01, 50)),
                m_Start=f32(rng.uniform(-20, 20)),
                m_Data=list(packed),
            )
            got = PackedBitVector.unpack_floats(vector)
            scale = vector.m_Range / ((1 << bits) - 1)
            self.assertEqual(got, [x * scale + vector.m_Start for x in values])
            left_to_right = [x * vector.m_Range / ((1 << bits) - 1) + vector.m_Start for x in values]
            divided += len(values)
            multiplied += sum(a != b for a, b in zip(got, left_to_right, strict=True))
        # Multiplying first rounds differently for a fair share of values.
        self.assertGreater(multiplied, divided // 20)
        # Bit size 0 repeats the start.
        empty = SimpleNamespace(m_NumItems=3, m_BitSize=0, m_Range=1.0, m_Start=2.5, m_Data=[])
        self.assertEqual(PackedBitVector.unpack_floats(empty), [2.5, 2.5, 2.5])


if __name__ == "__main__":
    unittest.main()
