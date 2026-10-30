"""PARITY.md rows on the export helpers, run against the oracle: what Pillow saves under an extension, the names
`export_obj` gives, an AudioClip with no data and the audio dispatcher, Texture2DConverter's and TextureSwizzler's
helpers, and PackedBitVector's floats.

Sections 15, 16, 19.2 and 23. Every input is built here (`synth.py`), so nothing needs the corpus, and each test
names the rows it pins; a row's test column names the test back. Its class is `Rows`, as in `test_parity_rows.py`,
the module these tests were split from. Needs UnityPy 1.25.2 (the oracle image); skipped elsewhere.
"""

import random
import struct
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest import mock

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE / "tests"))

from rows_support import ORACLE, TEXT_ASSET, load, quietly, text_asset_file, zip_with_a_directory  # noqa: E402

import synth as S  # noqa: E402

if ORACLE:
    import attrs
    import UnityPy
    from PIL import Image
    from UnityPy.classes import PPtr, StreamedResource, Texture2D
    from UnityPy.enums import BuildTarget, TextureFormat
    from UnityPy.export import AudioClipConverter, Texture2DConverter
    from UnityPy.helpers import PackedBitVector, TextureSwizzler
    from UnityPy.tools import extractor


@unittest.skipUnless(ORACLE, "needs UnityPy 1.25.2 (the oracle image)")
class Rows(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="unityrs-rows-"))
        self.addCleanup(lambda: __import__("shutil").rmtree(self.tmp))

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
        # A texture of non-zero width and zero height passes exportTexture2D's `m_Width` test, and its image is the
        # 0×0 RGBA one Texture2D.image gives. Pillow saves that only under SGI's four extensions, `.icns` and `.ico`
        # (a 6-byte header holding no image), and raises under every other extension it has a writer for.
        import pillow_tables

        texture = Texture2D.__new__(Texture2D)
        for field in attrs.fields(Texture2D):
            object.__setattr__(texture, field.name, None)
        texture.object_reader = SimpleNamespace(path_id=1, assets_file=None)  # what exportTexture2D returns reads
        texture.m_PlatformBlob = None
        texture.m_Width, texture.m_Height, texture.m_TextureFormat = 4, 0, TextureFormat.RGBA32
        texture.image_data = b"\x01" * 16
        saved, raised = {}, []
        for extension in [e for extensions in pillow_tables.writer_extensions().values() for e in extensions]:
            try:
                self.assertEqual(extractor.exportTexture2D(texture, str(self.tmp / "zero"), extension), [(None, 1)])
            except (OSError, SystemError, ValueError):
                raised.append(extension)
            else:
                saved[extension] = (self.tmp / f"zero{extension}").read_bytes()
        self.assertEqual(len(saved) + len(raised), 51)
        self.assertEqual(sorted(saved), [".bw", ".icns", ".ico", ".rgb", ".rgba", ".sgi"])
        self.assertEqual({len(saved[e]) for e in (".bw", ".rgb", ".rgba", ".sgi")}, {512})
        self.assertEqual((len(saved[".icns"]), saved[".ico"]), (7470, bytes.fromhex("000001000000")))
        tile = (SystemError, "tile cannot extend outside image")
        for extension, (error, message) in (
            ("", tile),
            (".png", tile),
            (".tga", tile),
            (".bmp", tile),
            (".jpg", (ValueError, "cannot write empty image as JPEG")),
        ):
            with self.subTest("zero height", extension=extension), self.assertRaisesRegex(error, f"^{message}$"):
                extractor.exportTexture2D(texture, str(self.tmp / "zero"), extension)

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
        # A zip's directory entry registers CAB "" (section 3): the empty path reads b"" from it, which FMOD refuses.
        quietly(env.load_file, zip_with_a_directory(), name="z.bin")
        for size in (0, 16):
            with self.subTest(cab="", size=size), self.assertRaises(Exception) as raised:
                AudioClipConverter.extract_audioclip_samples(
                    clip(StreamedResource(m_Source="", m_Offset=0, m_Size=size))
                )
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
        # set_image on a Switch-swizzled texture pads and swizzles through that table too, so a target whose written
        # format the table lacks raises KeyError: the seven ETC targets (section 19.2). BC7 raises its own error.
        raised = {}
        for fmt in TextureFormat:
            try:
                Texture2DConverter.image_to_texture2d(Image.new("RGBA", (8, 8)), fmt, BuildTarget.Switch, blob)
            except KeyError as e:
                raised[fmt.name] = e.args[0].name
            except Exception as e:  # noqa: BLE001 - every other raise is named below
                raised[fmt.name] = type(e).__name__
        written = {
            "ETC_RGB4": "ETC_RGB4",
            "ETC2_RGB": "ETC2_RGB",
            "ETC2_RGBA1": "ETC2_RGBA8",
            "ETC2_RGBA8": "ETC2_RGBA8",
        }
        written |= {"ETC_RGB4_3DS": "ETC_RGB4_3DS", "ETC_RGB4Crunched": "ETC_RGB4Crunched"}
        self.assertEqual(raised, written | {"ETC2_RGBA8Crunched": "ETC2_RGBA8", "BC7": "ValueError"})
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
