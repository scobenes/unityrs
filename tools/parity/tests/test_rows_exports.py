"""PARITY.md rows on exports, run against the oracle: audio, textures, sprites, meshes, renderers, shaders and the
extractor.

Sections 14.2, 15, 16 and 19. Like `test_parity_rows.py`, every input is built here, so nothing needs the corpus,
and each test names the rows it pins; a row's test column names the test back. Objects are written from the TPK's
node for their engine version (`test_rows_objects.tpk_object`); vertex data, packed bit vectors and FSB5 banks are
laid out by the helpers below from the formats' own rules, not by UnityPy. Needs UnityPy 1.25.2 (the oracle
image); skipped elsewhere.
"""

import ctypes
import json
import math
import shutil
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

from test_parity_rows import ORACLE, load  # noqa: E402
from test_rows_objects import VERSION, default_value, pptr, tpk_file, tpk_object  # noqa: E402

import synth as S  # noqa: E402

if ORACLE:
    import fmod_toolkit  # loads the bundled libfmod, which pyfmodex needs first
    import pyfmodex
    from PIL import Image
    from pyfmodex.exceptions import FmodError
    from UnityPy.classes import ChannelInfo, StreamInfo
    from UnityPy.enums import BuildTarget, ClassIDType, TextureFormat
    from UnityPy.export import (
        AudioClipConverter,
        MeshExporter,
        MeshRendererExporter,
        SpriteHelper,
        Texture2DConverter,
    )
    from UnityPy.helpers import MeshHelper, PackedBitVector, Tpk
    from UnityPy.helpers.UnityVersion import UnityVersion
    from UnityPy.tools import extractor


def element(class_id, *path, version=VERSION):
    """The default value of the field at `path` in `class_id`'s TPK node, an array's element for an array:
    `element(43, "m_SubMeshes")` is one SubMesh."""
    node = Tpk.get_typetree_node(class_id, UnityVersion.from_str(version))
    for name in path:
        node = next(child for child in node.m_Children if child.m_Name == name)
        if node.m_Children and node.m_Children[0].m_Type == "Array":
            node = node.m_Children[0].m_Children[1]
    return default_value(node)


def vertex_data(count, columns, endian="<"):
    """A 2019-and-later `m_VertexData` of `count` vertices. `columns` maps a channel to (stream, format, struct code,
    one tuple per vertex); a stream holds its channels interleaved in channel order and starts at a multiple of 16,
    the layout Unity writes and `MeshHandler.get_streams` derives."""
    channels = [{"stream": 0, "offset": 0, "format": 0, "dimension": 0} for _ in range(14)]
    data = b""
    for stream in sorted({column[0] for column in columns.values()}):
        own = sorted((chn, column) for chn, column in columns.items() if column[0] == stream)
        offset = 0
        for chn, (_, fmt, code, rows) in own:
            channels[chn] = {"stream": stream, "offset": offset, "format": fmt, "dimension": len(rows[0])}
            offset += struct.calcsize(code) * len(rows[0])
        for v in range(count):
            data += b"".join(struct.pack(f"{endian}{len(rows[v])}{code}", *rows[v]) for _, (_, _, code, rows) in own)
        data += bytes(-len(data) % 16)
    return {"m_VertexCount": count, "m_Channels": channels, "m_DataSize": data}


def submeshes(count, *spans, class_id=43, version=VERSION):
    """SubMesh values: one (first byte, index count, topology) per span."""
    out = []
    for first, n, topology in spans:
        sub = element(class_id, *(("m_RD",) if class_id == 213 else ()), "m_SubMeshes", version=version)
        sub.update(
            firstByte=first, indexCount=n, vertexCount=count, **({"topology": topology} if "topology" in sub else {})
        )
        out.append(sub)
    return out


def index_buffer(indices, code="H"):
    return list(struct.pack(f"<{len(indices)}{code}", *indices))


def packed_ints(values, bits):
    """A PackedBitVector of integers: `bits` bits an item, least significant first."""
    total = sum(v << (i * bits) for i, v in enumerate(values))
    return {
        "m_NumItems": len(values),
        "m_Data": list(total.to_bytes((len(values) * bits + 7) // 8, "little")),
        "m_BitSize": bits,
    }


def packed_floats(quanta, bits, start=0.0, step=1.0):
    """A PackedBitVector of floats: item i is `start + quanta[i] * step`, with step = range / (2**bits - 1)."""
    return {**packed_ints(quanta, bits), "m_Range": step * ((1 << bits) - 1), "m_Start": start}


def fsb5(mode, *samples, channels=1):
    """An FSB5 (version 1) bank of `mode`'s codec: one (data, sample count) per subsound, each at 44100 Hz (rate
    index 8), the sample headers' data offsets in 32-byte units."""
    headers, data = b"", b""
    for payload, count in samples:
        data += bytes(-len(data) % 32)
        headers += struct.pack("<Q", 8 << 1 | (channels - 1) << 5 | (len(data) // 32) << 7 | count << 34)
        data += payload
    head = struct.pack("<6I", 1, len(samples), len(headers), 0, len(data), mode)
    return b"FSB5" + head + bytes(32) + headers + data


def wav_header(channels, rate, bits, length, audio_format=1):
    """fmod_toolkit's 44-byte RIFF header."""
    block = channels * bits // 8
    return struct.pack(
        "<4sI4s4sIHHIIHH4sI",
        b"RIFF", length + 36, b"WAVE", b"fmt ", 16, audio_format, channels, rate, rate * block, block, bits,
        b"data", length,
    )  # fmt: skip


@unittest.skipUnless(ORACLE, "needs UnityPy 1.25.2 (the oracle image)")
class ExportRows(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="unityrs-exports-"))
        self.addCleanup(shutil.rmtree, self.tmp)

    # Section 16: what FMOD and fmod_toolkit make of each codec, on banks built here.
    def test_audio_codecs(self):
        pcm = struct.pack("<8h", 1, -1, 2, -2, 3, -3, 4, -4)
        wavs = fmod_toolkit.raw_to_wav(fsb5(2, (pcm, 4), (pcm[:8], 2), channels=2), "c", 2, 44100)
        # PCM16: the samples as they are behind the 44-byte header; a second subsound is `{name}-1.wav`.
        self.assertEqual(
            wavs, {"c.wav": wav_header(2, 44100, 16, 16) + pcm, "c-1.wav": wav_header(2, 44100, 16, 8) + pcm[:8]}
        )
        # IMA ADPCM (36 bytes per 64 samples), FADPCM (0x8C per 256) and MPEG (silent 128 kbps frames, 1152
        # samples each) decode to PCM16.
        frame = bytes.fromhex("fffb9004") + bytes(413)
        for mode, data, count in ((7, bytes(72), 128), (16, bytes(0x118), 512), (11, frame * 5, 5760)):
            with self.subTest(mode=mode):
                wav = fmod_toolkit.raw_to_wav(fsb5(mode, (data, count)), "c", 1, 44100)["c.wav"]
                self.assertEqual(wav, wav_header(1, 44100, 16, 2 * count) + bytes(2 * count))
        # The bundled libfmod refuses the other codecs in a bank built here: PCM8, PCM24, PCM32, PCM float, GC
        # ADPCM, VAG, HEVAG, CELT, XWMA and Vorbis with VERSION; XMA, ATRAC9 and Opus with UNSUPPORTED.
        refused = {1: "VERSION", 3: "VERSION", 4: "VERSION", 5: "VERSION", 6: "VERSION", 8: "VERSION", 9: "VERSION"}
        refused |= {
            12: "VERSION",
            14: "VERSION",
            15: "VERSION",
            10: "UNSUPPORTED",
            13: "UNSUPPORTED",
            17: "UNSUPPORTED",
        }
        for mode, error in refused.items():
            with self.subTest(mode=mode), self.assertRaisesRegex(FmodError, f"^{error}$"):
                fmod_toolkit.raw_to_wav(fsb5(mode, (bytes(64), 32)), "c", 1, 44100)
        # Not a bank: FMOD opens an AIFF, a MOD or an MP3 as one sound with no subsounds, and fmod_toolkit exports
        # subsounds only, so nothing comes back.
        samples = struct.pack(">4h", 0, 1000, -1000, 32767)
        rate = struct.pack(">HQ", 16383 + 15, 44100 << 48)  # 44100 as an 80-bit extended float
        comm = b"COMM" + struct.pack(">IhIh", 18, 1, 4, 16) + rate
        ssnd = b"SSND" + struct.pack(">III", 8 + len(samples), 0, 0) + samples
        aiff = b"FORM" + struct.pack(">I", 4 + len(comm) + len(ssnd)) + b"AIFF" + comm + ssnd
        mod = bytes(20) + bytes(30) * 31 + bytes([1, 0]) + bytes(128) + b"M.K." + bytes(1024)
        for name, data in (("aiff", aiff), ("mod", mod), ("mp3", frame * 5)):
            with self.subTest(name):
                self.assertEqual(fmod_toolkit.raw_to_wav(data, "c", 1, 44100), {})

        # fmod_toolkit's own rules, on a stand-in subsound: PCM float becomes int16 as `(v * 32768)` clipped and
        # truncated toward zero, or stays float (format 3); any other format raises TypeError, its message joining
        # a str and the enum.
        floats = struct.pack("<7f", 0.5, -0.5, 1.0, -1.0, 2.0, 0.99999, -0.0000305)
        self.assertEqual(
            struct.unpack("<7h", fmod_toolkit.fmod.convert_pcm_float_to_pcm_int16(floats)),
            (16384, -16384, 32767, -32768, 32767, 32767, 0),
        )
        buffer = ctypes.create_string_buffer(floats, len(floats))

        def subsound(fmt, bits=32):
            return SimpleNamespace(
                format=SimpleNamespace(format=fmt, channels=1, bits=bits),
                default_frequency=8000,
                get_length=lambda unit: len(floats),
                lock=lambda offset, length: [(ctypes.addressof(buffer), ctypes.c_uint(len(floats)))],
                unlock=lambda *parts: None,
            )

        float_format = pyfmodex.enums.SOUND_FORMAT.PCMFLOAT
        as_int = fmod_toolkit.fmod.subsound_to_wav(subsound(float_format))
        self.assertEqual(as_int[:44], wav_header(1, 8000, 16, 14))
        self.assertEqual(struct.unpack("<7h", as_int[44:]), (16384, -16384, 32767, -32768, 32767, 32767, 0))
        as_float = fmod_toolkit.fmod.subsound_to_wav(subsound(float_format), convert_pcm_float=False)
        self.assertEqual(as_float, wav_header(1, 8000, 32, 28, audio_format=3) + floats)
        with self.assertRaisesRegex(TypeError, "^can only concatenate str"):
            fmod_toolkit.fmod.subsound_to_wav(subsound(pyfmodex.enums.SOUND_FORMAT.BITSTREAM, 0))
        # PCM8, PCM24 and PCM32: the data as it is, behind a header of that width.
        for name, bits in (("PCM8", 8), ("PCM24", 24), ("PCM32", 32)):
            with self.subTest(name):
                got = fmod_toolkit.fmod.subsound_to_wav(subsound(getattr(pyfmodex.enums.SOUND_FORMAT, name), bits))
                self.assertEqual(got, wav_header(1, 8000, bits, 28) + floats)

        # AudioClip.samples is extract_audioclip_samples(self).
        from UnityPy.classes import AudioClip

        clip = object.__new__(AudioClip)
        with mock.patch.object(AudioClipConverter, "extract_audioclip_samples", return_value={"x": b""}) as call:
            self.assertEqual(clip.samples, {"x": b""})
        call.assert_called_once_with(clip)
        # The enums.
        from UnityPy.enums import AudioCompressionFormat, AudioType

        self.assertEqual(
            {e.name: e.value for e in AudioType},
            {
                "UNKNOWN": 0, "ACC": 1, "AIFF": 2, "IT": 10, "MOD": 12, "MPEG": 13, "OGGVORBIS": 14, "S3M": 17,
                "WAV": 20, "XM": 21, "XMA": 22, "VAG": 23, "AUDIOQUEUE": 24,
            },
        )  # fmt: skip
        self.assertEqual(
            {e.name: e.value for e in AudioCompressionFormat},
            {
                "PCM": 0, "Vorbis": 1, "ADPCM": 2, "MP3": 3, "VAG": 4, "HEVAG": 5, "XMA": 6, "AAC": 7, "GCADPCM": 8,
                "ATRAC9": 9,
            },
        )  # fmt: skip

    # Section 15.1: get_image_from_texture2d's arguments; what image_to_texture2d and compress_astc do on the way.
    def test_texture_conversion(self):
        calls = []
        with mock.patch.object(Texture2DConverter, "parse_image_data", lambda *a: calls.append(a)):
            texture = SimpleNamespace(get_image_data=lambda: b"d", m_Width=2, m_Height=3, m_TextureFormat=4)
            Texture2DConverter.get_image_from_texture2d(SimpleNamespace(**vars(texture), object_reader=None))
            reader = SimpleNamespace(version=(2021, 3, 2, 1), platform=BuildTarget.Switch)
            Texture2DConverter.get_image_from_texture2d(
                SimpleNamespace(**vars(texture), object_reader=reader, m_PlatformBlob=[1]), flip=False
            )
        self.assertEqual(
            calls,
            [
                (b"d", 2, 3, 4, (0, 0, 0, 0), BuildTarget.UnknownPlatform, None, True),
                (b"d", 2, 3, 4, (2021, 3, 2, 1), BuildTarget.Switch, [1], False),
            ],
        )

        # image_to_texture2d flips first: the bottom row comes first.
        image = Image.new("RGBA", (1, 2))
        image.putdata([(1, 2, 3, 4), (5, 6, 7, 8)])
        self.assertEqual(
            Texture2DConverter.image_to_texture2d(image, TextureFormat.RGBA32),
            (bytes(range(5, 9)) + bytes(range(1, 5)), TextureFormat.RGBA32),
        )
        self.assertEqual(
            Texture2DConverter.image_to_texture2d(image, TextureFormat.RGBA32, flip=False)[0][:4], bytes(range(1, 5))
        )
        # A block format pads to whole blocks by repeating the last column and row: a 2x2 image is one DXT1 block.
        padded = Texture2DConverter.pad_image(image, 3, 3)
        self.assertEqual(list(padded.getdata()), [(1, 2, 3, 4)] * 3 + [(5, 6, 7, 8)] * 6)
        self.assertEqual(
            len(Texture2DConverter.image_to_texture2d(Image.new("RGBA", (2, 2)), TextureFormat.DXT1)[0]), 8
        )
        # Switch: padded to whole GOB blocks (the edge repeated) and swizzled; RGB24 is packed as RGBA (four bytes a
        # pixel) and still tagged RGB24.
        blob = list(bytes(8) + (1).to_bytes(4, "little"))  # two GOBs a block
        data, written = Texture2DConverter.image_to_texture2d(
            Image.new("RGB", (4, 4), (9, 8, 7)), TextureFormat.RGB24, BuildTarget.Switch, blob
        )
        self.assertEqual((len(data), written), (16 * 16 * 4, TextureFormat.RGB24))
        self.assertEqual(set(data), {9, 8, 7, 255})
        # The image is packed in its own mode, unconverted: an RGB image has no band A for Alpha8.
        with self.assertRaises(ValueError):
            Texture2DConverter.image_to_texture2d(Image.new("RGB", (4, 4)), TextureFormat.Alpha8)

        # compress_astc: astcenc with an LDR context, cached per thread and block size.
        self.assertIs(Texture2DConverter.get_astc_context((4, 4)), Texture2DConverter.get_astc_context((4, 4)))
        made = []
        with (
            mock.patch.object(Texture2DConverter.astc_encoder, "ASTCConfig", lambda *a, **k: made.append((a, k))),
            mock.patch.object(Texture2DConverter.astc_encoder, "ASTCContext"),
        ):
            Texture2DConverter._get_astc_context.__wrapped__(0, (6, 6))
        flags = Texture2DConverter.astc_encoder.ASTCConfigFlags.USE_DECODE_UNORM8
        self.assertEqual(
            made,
            [((Texture2DConverter.astc_encoder.ASTCProfile.LDR, 6, 6), {"block_z": 1, "quality": 100, "flags": flags})],
        )
        solid = Image.new("RGBA", (4, 4), (10, 200, 30, 255))
        data, written = Texture2DConverter.image_to_texture2d(solid, TextureFormat.ASTC_RGBA_4x4)
        self.assertEqual((len(data), written), (16, TextureFormat.ASTC_RGBA_4x4))
        decoded = Texture2DConverter.parse_image_data(data, 4, 4, written, (2021, 3, 2, 1), 0, None)
        self.assertEqual(set(decoded.getdata()), {(10, 200, 30, 255)})

    # Sections 15.3, 19.1: a 2021.3 Mesh through MeshHandler, from the file.
    def test_mesh_vertex_data(self):
        positions = [(0.0, 0.0, 0.0), (1.0, 0.0, 0.0), (0.0, 1.0, 0.0), (1.0, 1.0, 0.0), (2.0, 2.0, 0.5)]
        colors = [(v * 50, 128, 0, 255) for v in range(5)]  # UNorm8
        uv0 = [(x, y) for x, y, _ in positions]
        tangents = [(-32767, 0, 32767, v) for v in range(5)]  # SNorm16, in stream 1
        uv1 = [(0.25 * v, 0.5) for v in range(5)]
        columns = {0: (0, 0, "f", positions), 3: (0, 2, "B", colors), 4: (0, 0, "f", uv0)}
        columns |= {2: (1, 5, "h", tangents), 5: (1, 0, "f", uv1)}
        indices = [0, 1, 2, 2, 1, 3] + [0, 1, 3, 2] + [0, 1, 2, 3, 3]  # triangles, a quad, a strip
        data = tpk_object(
            43,
            m_Name="m",
            m_SubMeshes=submeshes(5, (0, 6, 0), (12, 4, 2), (20, 5, 1)),
            m_IndexBuffer=index_buffer(indices),
            m_VertexData=vertex_data(5, columns),
        )
        sf = load(tpk_file([(1, 43, data)]))[0]
        mesh = sf.objects[1].parse_as_object()
        handler = MeshHelper.MeshHandler(mesh)
        self.assertEqual((handler.version, handler.endianess), (sf.objects[1].version, "<"))
        handler.process()
        # UNorm8 colours and SNorm16 tangents come out as the raw integers.
        got = (
            handler.m_Vertices,
            handler.m_Colors,
            handler.m_UV0,
            handler.m_Tangents,
            handler.m_UV1,
            handler.m_Normals,
        )
        self.assertEqual(got, (positions, colors, uv0, tangents, uv1, None))
        self.assertEqual((handler.m_VertexCount, list(handler.m_IndexBuffer)), (5, indices))
        # Stream 1 starts at 5 * 24 = 120 rounded up to 128.
        streams = handler.get_streams(mesh.m_VertexData.m_Channels, 5)
        self.assertEqual(
            [(s.channelMask, s.offset, s.stride) for s in streams], [(0b11001, 0, 24), (0b100100, 128, 16)]
        )
        # Triangles; a quad as two; a strip skipping the degenerate (2, 3, 3), every other one reversed, and its
        # indexCount rewritten from 5 to 6.
        self.assertEqual(
            handler.get_triangles(), [[(0, 1, 2), (2, 1, 3)], [(0, 1, 3), (0, 3, 2)], [(0, 1, 2), (2, 1, 3)]]
        )
        self.assertEqual(mesh.m_SubMeshes[2].indexCount, 6)
        for topology in (3, 4, 5):  # lines, line strip, points
            mesh.m_SubMeshes[0].topology = topology
            with self.subTest(topology=topology), self.assertRaisesRegex(ValueError, "lines or points"):
                handler.get_triangles()

        # 32-bit indices from m_IndexFormat 1: a submesh's firstByte counts 4 bytes an index.
        data = tpk_object(
            43,
            m_SubMeshes=submeshes(4, (0, 3, 0), (12, 3, 0)),
            m_IndexFormat=1,
            m_IndexBuffer=index_buffer([0, 1, 2, 2, 1, 3], "I"),
            m_VertexData=vertex_data(4, {0: (0, 0, "f", positions[:4])}),
        )
        handler = MeshHelper.MeshHandler(load(tpk_file([(1, 43, data)]))[0].objects[1].parse_as_object())
        handler.process()
        self.assertEqual((handler.m_Use16BitIndices, handler.get_triangles()), (False, [[(0, 1, 2)], [(2, 1, 3)]]))

        # m_StreamData: the vertex data comes through get_resource_data.
        stored = vertex_data(3, {0: (0, 0, "f", positions[:3])})
        data = tpk_object(
            43,
            m_VertexData={**stored, "m_DataSize": b""},
            m_StreamData={"offset": 7, "size": len(stored["m_DataSize"]), "path": "archive:/CAB-x/CAB-x.resS"},
        )
        sf = load(tpk_file([(1, 43, data)]))[0]
        with mock.patch.object(MeshHelper, "get_resource_data", return_value=stored["m_DataSize"]) as read:
            handler = MeshHelper.MeshHandler(sf.objects[1].parse_as_object())
            handler.process()
        read.assert_called_once_with("archive:/CAB-x/CAB-x.resS", sf, 7, len(stored["m_DataSize"]))
        self.assertEqual(handler.m_Vertices, positions[:3])

        # A SpriteRenderData, or a Mesh without a reader, needs the version passed.
        from UnityPy.classes import Mesh, SpriteRenderData

        for src in (object.__new__(SpriteRenderData), object.__new__(Mesh)):
            with self.assertRaisesRegex(ValueError, "^No version provided and no object reader found$"):
                MeshHelper.MeshHandler(src)

        # A big-endian file: MeshHandler still swaps as for little-endian, and Mesh.export passes no endianness.
        def swapped(row):
            return struct.unpack("<3f", struct.pack(">3f", *row))

        data = tpk_object(
            43,
            endian=">",
            m_Name="be",
            m_SubMeshes=submeshes(3, (0, 3, 0)),
            m_IndexBuffer=index_buffer([0, 1, 2]),
            m_VertexData=vertex_data(3, {0: (0, 0, "f", positions[:3])}, endian=">"),
        )
        mesh = load(tpk_file([(1, 43, data)], endian=">"))[0].objects[1].parse_as_object()
        handler = MeshHelper.MeshHandler(mesh)
        handler.process()
        self.assertEqual(handler.m_Vertices, [swapped(row) for row in positions[:3]])
        handler = MeshHelper.MeshHandler(mesh, endianess=">")
        handler.process()
        self.assertEqual(handler.m_Vertices, positions[:3])
        x, y, z = swapped(positions[1])
        self.assertIn(f"v {-x:.9G} {y:.9G} {z:.9G}\n", mesh.export())

    # Section 15.3: MeshHandler's parts, each by version.
    def test_mesh_handler_parts(self):
        def handler(version):
            return MeshHelper.MeshHandler(None, version=version)

        def channel(fmt=0, dimension=1, stream=0, offset=0):
            return ChannelInfo(stream=stream, offset=offset, format=fmt, dimension=dimension)

        # The struct code of each format: VertexChannelFormat below 2017, VertexFormat2017 below 2019,
        # VertexFormat after; an unknown format raises.
        codes = {
            (5, 6, 7, 1): "f e B B I",
            (2017, 4, 40, 1): "f e B B b H h B b H h I i",
            (2019, 1, 0, 1): "f e B b H h B b H h I i",
        }
        for version, expected in codes.items():
            h = handler(version)
            got = []
            for fmt in range(len(expected.split())):
                got.append(h.get_channel_dtype(channel(fmt)))
                self.assertEqual(h.get_channel_component_size(channel(fmt)), struct.calcsize(got[-1]))
            self.assertEqual(" ".join(got), expected)
            with self.subTest(version=version), self.assertRaises(ValueError):
                h.get_channel_dtype(channel(len(got)))
        from UnityPy.enums import VertexFormat as VF

        self.assertEqual(
            [
                list(m)
                for m in (
                    VF.VERTEX_CHANNEL_FORMAT_STRUCT_TYPE_MAP,
                    VF.VERTEX_FORMAT_2017_STRUCT_TYPE_MAP,
                    VF.VERTEX_FORMAT_STRUCT_TYPE_MAP,
                )
            ],
            [list(VF.VertexChannelFormat), list(VF.VertexFormat2017), list(VF.VertexFormat)],
        )
        from UnityPy.enums.GfxPrimitiveType import GfxPrimitiveType
        from UnityPy.enums.MeshTopology import MeshTopology

        names = ["Triangles", "TriangleStrip", "Quads", "Lines", "LineStrip", "Points"]
        self.assertEqual([(e.name, e.value) for e in MeshTopology], [(n, i) for i, n in enumerate(names)])
        self.assertEqual(
            [(e.name, e.value) for e in GfxPrimitiveType], [(f"kPrimitive{n}", i) for i, n in enumerate(names)]
        )

        # get_streams: streams up to the highest a channel names; a channel's dimension masked to its low four
        # bits; each stream's start rounded up to 16.
        h = handler((2021, 3, 2, 1))
        streams = h.get_streams([channel(0, 3), channel(2, 2, stream=2), channel(0, 0), channel(2, 0x14)], 3)
        self.assertEqual(
            [(s.channelMask, s.offset, s.stride) for s in streams], [(0b1001, 0, 16), (0, 48, 0), (0b10, 48, 2)]
        )

        # get_channels: the fixed layout before 4, per stream in channel order: vertex and normal float3, colour
        # four bytes, two UVs float2, tangent float4.
        def stream(mask):
            return StreamInfo(channelMask=mask, offset=0, stride=0, dividerOp=0, frequency=0)

        got = handler((3, 5, 7, 1)).get_channels([stream(0b100111), stream(0b011000)])
        self.assertEqual(
            [(c.stream, c.offset, c.format, c.dimension) for c in got],
            [(0, 0, 0, 3), (0, 12, 0, 3), (0, 24, 2, 4), (1, 0, 0, 2), (1, 8, 0, 2), (0, 28, 0, 4)],
        )

        # read_vertex_data: before 2018, channel 2 in the colour format reads four components whatever its
        # dimension; from 2018 channel 2 is the tangent and keeps it. The C gather and the Python loop agree.
        data = struct.pack("<3f4B3f4B", 1, 2, 3, 10, 20, 30, 40, 4, 5, 6, 50, 60, 70, 80)
        src = SimpleNamespace(m_VertexData=SimpleNamespace(m_VertexCount=2, m_DataSize=data))
        layout = (
            [channel(0, 3), channel(0, 0), channel(2, 1, offset=12)],
            [StreamInfo(channelMask=0b101, offset=0, stride=16, dividerOp=0, frequency=0)],
        )
        for boost in (MeshHelper.UnityPyBoost, None):
            with self.subTest(boost=bool(boost)), mock.patch.object(MeshHelper, "UnityPyBoost", boost):
                old = MeshHelper.MeshHandler(src, version=(2017, 4, 40, 1))
                old.read_vertex_data(*layout)
                self.assertEqual(
                    (old.m_Vertices, old.m_Colors), ([(1, 2, 3), (4, 5, 6)], [(10, 20, 30, 40), (50, 60, 70, 80)])
                )
                new = MeshHelper.MeshHandler(src, version=(2018, 1, 0, 1))
                new.read_vertex_data(*layout)
                self.assertEqual(
                    (new.m_Vertices, new.m_Tangents, new.m_Colors), ([(1, 2, 3), (4, 5, 6)], [(10,), (50,)], None)
                )
        # unpack_vertexdata(data, component size, vertices, stream offset, stride, channel offset, dimension, swap).
        boost = MeshHelper.UnityPyBoost.unpack_vertexdata
        self.assertEqual(bytes(boost(bytes(range(8)), 2, 2, 0, 4, 0, 2, True)), bytes([1, 0, 3, 2, 5, 4, 7, 6]))
        self.assertEqual(bytes(boost(bytes(range(8)), 1, 2, 0, 4, 2, 1, False)), bytes([2, 6]))

        # assign_channel_vertex_data: each channel's attribute, from 2018 and before; an unknown channel raises.
        attributes = ["m_Vertices", "m_Normals", "m_Tangents", "m_Colors", *(f"m_UV{i}" for i in range(8))]
        attributes += ["m_BoneWeights", "m_BoneIndices"]

        def assigned(version, chn):
            h = handler(version)
            h.assign_channel_vertex_data(chn, [chn])
            return [a for a in attributes if getattr(h, a) == [chn]]

        self.assertEqual([assigned((2018, 1, 0, 1), c) for c in range(14)], [[a] for a in attributes])
        old = ["m_Vertices", "m_Normals", "m_Colors", "m_UV0", "m_UV1", "m_UV2", "m_UV3", "m_Tangents"]
        self.assertEqual([assigned((5, 6, 7, 1), c) for c in range(8)], [[a] for a in old])
        self.assertEqual(assigned((4, 7, 2, 1), 5), ["m_Tangents"])
        for version, chn in (((2018, 1, 0, 1), 14), ((2017, 4, 40, 1), 8)):
            with self.subTest(version=version), self.assertRaisesRegex(ValueError, f"^Unknown channel {chn}$"):
                handler(version).assign_channel_vertex_data(chn, [])

        # normalize multiplies by the reciprocal; at a length of 1e-5 or less it returns integer zeros.
        self.assertEqual(MeshHelper.normalize(3, 4, 0), (0.6000000000000001, 0.8, 0.0))
        self.assertEqual(MeshHelper.normalize(2e-5, 0), (0.9999999999999999, 0.0))
        zero = MeshHelper.normalize(1e-5, 0, 0)
        self.assertEqual((zero, [type(v) for v in zero]), ((0, 0, 0), [int] * 3))

        # PackedBitVector: a little-endian bit stream, `m_BitSize` bits an item from `start`; floats as
        # start + item * range / (2**bits - 1), or `m_Start` alone at bit size 0.
        packed = SimpleNamespace(**packed_floats([5, 3, 7, 1, 6], 3, start=1.0, step=0.5))
        self.assertEqual(PackedBitVector.unpack_ints(packed), [5, 3, 7, 1, 6])
        self.assertEqual(PackedBitVector.unpack_ints(packed, 1, 2), [3, 7])
        self.assertEqual(PackedBitVector.unpack_ints(packed, 0, 4, shape=(2,)), [[5, 3], [7, 1]])
        self.assertEqual(PackedBitVector.unpack_floats(packed, 2, 2), [4.5, 1.5])
        packed.m_BitSize = 0
        self.assertEqual(PackedBitVector.unpack_floats(packed, 0, 2), [1.0, 1.0])

    # Sections 15.3, 19.1: compressed meshes, and meshes from 3.4, 3.5 and 4.7.
    def test_compressed_and_old_meshes(self):
        def compressed(version=VERSION, **fields):
            value = element(43, "m_CompressedMesh", version=version)
            for name, packed in fields.items():
                self.assertIn(name, value)
                value[name] = {**value[name], **packed} if isinstance(packed, dict) else packed
            return value

        def handled(version, **fields):
            data = tpk_object(43, version, **fields)
            sf = load(tpk_file([(1, 43, data)], unity_version=version.encode()))[0]
            handler = MeshHelper.MeshHandler(sf.objects[1].parse_as_object())
            handler.process()
            return handler

        no_vertices = vertex_data(0, {})  # fourteen channels of dimension 0
        cm = compressed(
            m_Vertices=packed_floats([1, 2, 3, 4, 5, 6], 8),
            m_UV=packed_floats(list(range(10, 22)), 8),
            m_UVInfo=0x555,  # UV channels 0, 1 and 2, each of two dimensions
            m_Normals=packed_floats([2, 2, 4, 4], 8, step=0.25),
            m_NormalSigns=packed_ints([0, 1], 1),
            m_Tangents=packed_floats([2, 2, 4, 4], 8, step=0.25),
            m_TangentSigns=packed_ints([1, 0, 0, 1], 1),
            m_FloatColors=packed_floats(list(range(8)), 8),
            m_Weights=packed_ints([31, 10, 10, 10], 5),
            m_BoneIndices=packed_ints([7, 1, 2, 3, 4], 4),
            m_Triangles=packed_ints([1, 0, 1], 2),
        )
        h = handled(VERSION, m_CompressedMesh=cm, m_VertexData=no_vertices, m_SubMeshes=submeshes(2, (0, 3, 0)))
        self.assertEqual((h.m_VertexCount, h.m_Vertices), (2, [[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]]))
        # The third UV channel starts where the second did: `uvSrcOffset = uvDim * m_VertexCount` assigns.
        self.assertEqual(
            (h.m_UV0, h.m_UV1, h.m_UV2), ([[10, 11], [12, 13]], [[14, 15], [16, 17]], [[14, 15], [16, 17]])
        )
        # Normals: z from the unit length, negated for sign 0; a too-long (x, y) is normalised with z = 0.
        r = 1.0 * (1.0 / math.sqrt(2.0))
        self.assertEqual(h.m_Normals, [(0.5, 0.5, -math.sqrt(0.5)), (r, r, 0.0)])
        self.assertEqual(h.m_Tangents, [(0.5, 0.5, math.sqrt(0.5), -1.0), (r, r, -0.0, 1.0)])
        self.assertEqual(h.m_Colors, [[0, 1, 2, 3], [4, 5, 6, 7]])
        # Weights in 31sts; a vertex whose three weights stop short of 31 gets `1 - sum`, the sum still in 31sts.
        self.assertEqual(h.m_BoneWeights, [(1.0, 0, 0, 0), (10 / 31, 10 / 31, 10 / 31, -29)])
        self.assertEqual((h.m_BoneIndices, h.m_IndexBuffer), ([(7, 0, 0, 0), (1, 2, 3, 4)], [1, 0, 1]))
        # Without m_UVInfo: UV0, then UV1 when the vector holds four floats a vertex.
        for count, uv1 in ((8, [[14, 15], [16, 17]]), (6, None)):
            cm_plain = compressed(m_Vertices=packed_floats(range(6), 8), m_UV=packed_floats(range(10, 10 + count), 8))
            h = handled(VERSION, m_CompressedMesh=cm_plain, m_VertexData=no_vertices)
            self.assertEqual((h.m_UV0, h.m_UV1), ([[10, 11], [12, 13]], uv1))
        # An m_Channels list with no entries fails in get_streams before anything is read.
        with self.assertRaisesRegex(ValueError, "max"):
            handled(VERSION, m_VertexData={**no_vertices, "m_Channels": []})

        # 4.7: bind poses, and packed RGBA colours divided by 255 (the vertex data's colours stay integers).
        old = "4.7.2f1"
        cm = compressed(
            old,
            m_Vertices=packed_floats(range(6), 8),
            m_BindPoses=packed_floats(range(16), 8),
            m_Colors=packed_ints([0x11223344, 0xFF00FF00], 32),
        )
        h = handled(old, m_CompressedMesh=cm)
        self.assertEqual(h.m_BindPose, [[[0, 1, 2, 3], [4, 5, 6, 7], [8, 9, 10, 11], [12, 13, 14, 15]]])
        self.assertEqual(h.m_Colors, [(0x11 / 255, 0x22 / 255, 0x33 / 255, 0x44 / 255), (1.0, 0.0, 1.0, 0.0)])

        # 4.7 vertex data: the file's own streams and channels, channel 5 the tangent.
        rows = [((float(v), 0.0, 1.0), (0.5, float(v)), (1.0, 2.0, 3.0, float(v))) for v in range(4)]
        channels = [{"stream": 0, "offset": 0, "format": 0, "dimension": 0} for _ in range(6)]
        for chn, offset, dimension in ((0, 0, 3), (3, 12, 2), (5, 20, 4)):
            channels[chn] = {"stream": 0, "offset": offset, "format": 0, "dimension": dimension}
        stream = {**element(43, "m_VertexData", "m_Streams", version=old), "channelMask": 0b101001, "stride": 36}
        vd = {
            **element(43, "m_VertexData", version=old),
            "m_VertexCount": 4,
            "m_Channels": channels,
            "m_Streams": [stream],
        }
        vd["m_DataSize"] = b"".join(struct.pack("<9f", *p, *uv, *t) for p, uv, t in rows)
        sub = submeshes(4, (0, 6, 0), version=old)
        h = handled(old, m_VertexData=vd, m_SubMeshes=sub, m_IndexBuffer=index_buffer([0, 1, 2, 2, 1, 3]))
        self.assertEqual((h.m_Vertices, h.m_UV0, h.m_Tangents), tuple([r[i] for r in rows] for i in range(3)))
        self.assertEqual(h.get_triangles(), [[(0, 1, 2), (2, 1, 3)]])

        # 3.5: the four fixed streams through get_channels. Its SubMesh has `isTriStrip` and no `topology`, so
        # get_triangles reads every 3.x submesh as a strip, a triangle list included; the skin comes from m_Skin.
        old = "3.5.7f6"
        vd = element(43, "m_VertexData", version=old)
        vd.update({"m_VertexCount": 4, "m_DataSize": b"".join(struct.pack("<5f", *p, *uv) for p, uv, _ in rows)})
        vd["m_Streams[0]"] = {**vd["m_Streams[0]"], "channelMask": 0b1001, "stride": 20}
        sub = submeshes(4, (0, 6, None), version=old)
        skin = [
            {
                "weight[0]": 0.5,
                "weight[1]": 0.5,
                "weight[2]": 0.0,
                "weight[3]": 0.0,
                "boneIndex[0]": v,
                "boneIndex[1]": 1,
                "boneIndex[2]": 0,
                "boneIndex[3]": 0,
            }
            for v in range(4)
        ]
        self.assertEqual(set(skin[0]), set(element(43, "m_Skin", version=old)))
        h = handled(old, m_VertexData=vd, m_SubMeshes=sub, m_IndexBuffer=index_buffer([0, 1, 2, 2, 1, 3]), m_Skin=skin)
        self.assertEqual((h.m_Vertices, h.m_UV0), tuple([r[i] for r in rows] for i in range(2)))
        self.assertEqual(h.get_triangles(), [[(0, 1, 2), (1, 2, 3)]])
        self.assertEqual((h.m_BoneWeights[0], h.m_BoneIndices[3]), ((0.5, 0.5, 0.0, 0.0), (3, 1, 0, 0)))
        # 3.4: the Mesh holds its own vertex arrays and no m_VertexData, and process stops at its assert first.
        vertex = {"x": 1.0, "y": 2.0, "z": 3.0}
        with self.assertRaises(AssertionError):
            handled("3.4.0f5", m_Vertices=[vertex])

    # Sections 15.4, 19.2: the OBJ text, NaN and infinity included; no vertices.
    def test_mesh_obj_export(self):
        nan, inf = float("nan"), float("inf")
        positions = [(0.0, 1e-10, nan), (inf, -1.5, 0.1), (1.0, 2.0, 3.0)]
        uv0 = [(0.5, nan), (0.25, 1.0), (0.0, 0.0)]
        normals = [(1.0, 0.0, 0.0), (0.0, 1.0, 0.0), (nan, 0.0, -1.0)]
        columns = {0: (0, 0, "f", positions), 1: (0, 0, "f", normals), 4: (0, 0, "f", uv0)}
        data = tpk_object(
            43,
            m_Name="m",
            m_SubMeshes=submeshes(3, (0, 3, 0), (6, 3, 0)),
            m_IndexBuffer=index_buffer([0, 1, 2, 2, 1, 0]),
            m_VertexData=vertex_data(3, columns),
        )
        mesh = load(tpk_file([(1, 43, data)]))[0].objects[1].parse_as_object()
        body = (
            "v -0 1.00000001E-10 NAN\nv -INF -1.5 0.100000001\nv -1 2 3\n"
            "vt 0.5 NAN\nvt 0.25 1\nvt 0 0\n"
            "vn -1 0 0\nvn -0 1 0\nvn NAN 0 -1\n"
        )
        self.assertEqual(mesh.export(), "g m\n" + body + "g m_0\nf 3/3/3 2/2/2 1/1/1\ng m_1\nf 1/1/1 2/2/2 3/3/3\n")
        self.assertEqual(
            MeshExporter.export_mesh_obj(mesh, ["mat", None]),
            "g m\nmtllib m.mtl\n" + body + "g m_0\nusemtl mat\nf 3/3/3 2/2/2 1/1/1\ng m_1\nf 1/1/1 2/2/2 3/3/3\n",
        )
        with self.assertRaisesRegex(NotImplementedError, "^Export format fbx not implemented$"):
            mesh.export("fbx")
        with mock.patch.object(MeshExporter, "export_mesh", return_value="text") as export:
            self.assertEqual(mesh.export("x"), "text")
        export.assert_called_once_with(mesh, "x")
        # No vertices, or vertices without positions: False, not text.
        for vd in (vertex_data(0, {}), vertex_data(3, {4: (0, 0, "f", uv0)})):
            data = tpk_object(43, m_Name="e", m_VertexData=vd)
            self.assertIs(
                MeshExporter.export_mesh_obj(load(tpk_file([(1, 43, data)]))[0].objects[1].parse_as_object()), False
            )

    # Sections 14.2, 15.4, 19.1: renderers, their meshes and their materials.
    def test_mesh_renderer_export(self):
        def material(name, **props):
            saved = element(21, "m_SavedProperties")
            return tpk_object(21, m_Name=name, m_SavedProperties={**saved, **props})

        tex_env = element(21, "m_SavedProperties", "m_TexEnvs")[1]
        renderer = element(23, "m_StaticBatchInfo")
        triangle = vertex_data(3, {0: (0, 0, "f", [(0.0, 0.0, 0.0), (1.0, 0.0, 0.0), (0.0, 1.0, 0.0)])})
        objects = [
            (1, 1, tpk_object(1, m_Name="go", m_Component=[{"component": pptr(2)}, {"component": pptr(3)}])),
            (2, 33, tpk_object(33, m_GameObject=pptr(1), m_Mesh=pptr(4))),
            # Batched from submesh 1; the second material slot empty.
            (
                3,
                23,
                tpk_object(
                    23,
                    m_GameObject=pptr(1),
                    m_Materials=[pptr(5), pptr(0)],
                    m_StaticBatchInfo={**renderer, "firstSubMesh": 1, "subMeshCount": 2},
                ),
            ),
            (
                4,
                43,
                tpk_object(
                    43,
                    m_Name="m",
                    m_SubMeshes=submeshes(3, (0, 3, 0), (6, 3, 0), (12, 3, 0)),
                    m_IndexBuffer=index_buffer([0, 1, 2] * 3),
                    m_VertexData=triangle,
                ),
            ),
            (5, 21, material("mat", m_Floats=[("_Shininess", 5.0), ("_Transparency", 0.5)])),
            (6, 21, material("colored", m_Colors=[("_Color", {"r": 1.0, "g": 0.0, "b": 0.0, "a": 1.0})])),
            (7, 21, material("textured", m_TexEnvs=[("_MainTex", tex_env)])),
            (8, 21, material("ab", m_TexEnvs=[("ab", tex_env)])),
            (9, 137, tpk_object(137, m_Mesh=pptr(4))),
            (10, 23, tpk_object(23, m_GameObject=pptr(1), m_Materials=[pptr(5)])),  # not batched
            (11, 1, tpk_object(1, m_Name="bare")),
            (12, 23, tpk_object(23, m_GameObject=pptr(11))),
        ]
        sf = load(tpk_file(objects))[0]
        parsed = {path_id: sf.objects[path_id].parse_as_object() for path_id, _, _ in objects}
        # get_mesh: the SkinnedMeshRenderer's own mesh, or the GameObject's MeshFilter mesh; none without one.
        for path_id in (3, 9):
            self.assertEqual(MeshRendererExporter.get_mesh(parsed[path_id]).object_reader.path_id, 4)
        self.assertIsNone(MeshRendererExporter.get_mesh(parsed[12]))
        # The GameObject's accessors: the first component of the type, else None.
        go = parsed[1]
        self.assertEqual([go.m_MeshFilter.m_PathID, go.m_MeshRenderer.m_PathID], [2, 3])
        self.assertEqual([go.m_Transform, go.m_Animator, go.m_Animation, go.m_SkinnedMeshRenderer], [None] * 4)
        # An empty component list raises: `m_Components` reads `m_Component[0]` to tell pairs from ComponentPairs.
        with self.assertRaises(IndexError):
            _ = parsed[11].m_Transform

        # export_mesh_renderer: a material per submesh from `firstSubMesh` on, but the names are listed without
        # the skipped submeshes, so submesh 0 gets submesh 1's material; the .obj and the .mtl.
        out = self.tmp / "out"
        parsed[3].export(str(out))
        mtl = "newmtl mat\nKa 0.2000 0.2000 0.2000\nKd 0.8000 0.8000 0.8000\nKs 0.2000 0.2000 0.2000\n"
        mtl += "Tr 0.5000\nNs 5.0000"
        self.assertEqual((out / "m.mtl").read_text(), mtl)
        obj = (out / "m.obj").read_text()
        self.assertIn("g m_0\nusemtl mat\nf", obj)
        self.assertEqual(obj.count("usemtl"), 1)
        self.assertEqual(sorted(p.name for p in out.iterdir()), ["m.mtl", "m.obj"])
        # No mesh: the folder and nothing in it.
        parsed[12].export(str(self.tmp / "none"))
        self.assertEqual(list((self.tmp / "none").iterdir()), [])
        # Not batched: `m_SubsetIndices` is None on the merged class, and min(None) raises.
        with self.assertRaisesRegex(TypeError, "not iterable"):
            parsed[10].export(str(self.tmp / "unbatched"))
        # export_material: any colour raises (`color.R` on a ColorRGBA); a texture property raises while the dict
        # of them is unpacked as pairs (one of two characters unpacks, then has no m_Texture).
        with self.assertRaisesRegex(AttributeError, "'ColorRGBA' object has no attribute 'R'"):
            MeshRendererExporter.export_material(parsed[6])
        with self.assertRaisesRegex(ValueError, "too many values to unpack"):
            MeshRendererExporter.export_material(parsed[7])
        with self.assertRaisesRegex(AttributeError, "'str' object has no attribute 'm_Texture'"):
            MeshRendererExporter.export_material(parsed[8])
        with mock.patch.object(MeshRendererExporter, "export_mesh_renderer") as export:
            parsed[3].export("d")
        export.assert_called_once_with(parsed[3], "d")

        # 5.4: no m_StaticBatchInfo, so it reads None.subMeshCount; the GameObject's components are (class id,
        # PPtr) pairs, and its accessors read `m_GameObject` from the PPtr.
        old = "5.4.6f3"
        pair = element(1, "m_Component", version=old)
        self.assertIsInstance(pair, tuple)
        objects = [
            (1, 1, tpk_object(1, old, m_Name="go", m_Component=[(33, pptr(2))])),
            (2, 33, tpk_object(33, old, m_GameObject=pptr(1), m_Mesh=pptr(4))),
            (3, 23, tpk_object(23, old, m_GameObject=pptr(1), m_SubsetIndices=[0])),
            (4, 43, tpk_object(43, old, m_Name="m")),
        ]
        sf = load(tpk_file(objects, unity_version=old.encode()))[0]
        with self.assertRaisesRegex(AttributeError, "'NoneType' object has no attribute 'subMeshCount'"):
            sf.objects[3].parse_as_object().export(str(self.tmp / "old"))
        with self.assertRaisesRegex(AttributeError, "m_GameObject"):
            _ = sf.objects[1].parse_as_object().m_MeshFilter

        # VideoClip: the dict only, with no exporter and nothing patched on.
        from UnityPy.classes import VideoClip

        self.assertNotIn(ClassIDType.VideoClip, extractor.EXPORT_TYPES)
        self.assertEqual([a for a in ("export", "image", "samples", "save_image") if hasattr(VideoClip, a)], [])
        data = tpk_object(329, m_Name="v", m_FrameCount=3)
        reader = load(tpk_file([(1, 329, data)]))[0].objects[1]
        self.assertEqual((reader.parse_as_dict()["m_FrameCount"], type(reader.parse_as_object())), (3, VideoClip))

    # Sections 7.2, 14.2, 15.2, 19.1: sprites, from the file to the image.
    def test_sprites(self):
        def tex(x, y):
            return (x * 60, y * 60, 100, 255)

        def texture(pixels, version=VERSION, size=4):
            data = bytes(b for y in range(size) for x in range(size) for b in pixels(x, y))
            fields = dict(m_Width=size, m_Height=size, m_CompleteImageSize=len(data), m_TextureFormat=4)
            return tpk_object(
                28, version, m_Name="t", m_ImageCount=1, m_TextureDimension=2, **fields, **{"image data": data}
            )

        def rect(x, y, width, height):
            return {"x": x, "y": y, "width": width, "height": height}

        def render_data(box, settings, texture=None, alpha=None, version=VERSION, **fields):
            rd = element(213, "m_RD", version=version)
            texture, alpha = texture or pptr(1), alpha or pptr(0)
            return {
                **rd,
                "texture": texture,
                "alphaTexture": alpha,
                "textureRect": rect(*box),
                "settingsRaw": settings,
                **fields,
            }

        def sprite(box=(1, 0, 2, 3), settings=2, **fields):  # 2: rectangle packing, not packed
            rd = render_data(box, settings, **fields.pop("rd", {}))
            return tpk_object(213, m_Name="s", m_PixelsToUnits=1.0, m_RD=rd, **fields)

        def pixels(image):
            return [[image.getpixel((x, y)) for x in range(image.width)] for y in range(image.height)]

        def grid(width, height, at):
            return [[at(x, y) for x in range(width)] for y in range(height)]

        guid = {f"data[{i}]": i + 1 for i in range(4)}
        atlas_data = element(687078895, "m_RenderDataMap")[1]
        atlas_map = [
            ((guid, 5), {**atlas_data, **render_data((0, 1, 2, 2), 2)}),
            ((guid, 7), {**atlas_data, **render_data((0, 0, 4, 4), 0)}),  # tight
        ]
        quad = vertex_data(4, {0: (0, 0, "f", [(0.0, 0.0, 0.0), (2.0, 0.0, 0.0), (0.0, 2.0, 0.0), (2.0, 2.0, 0.0)])}
                           | {4: (0, 0, "f", [(0.0, 0.0), (0.5, 0.0), (0.0, 0.5), (0.5, 0.5)])})  # fmt: skip
        mesh = {
            "m_VertexData": quad,
            "m_IndexBuffer": index_buffer([0, 1, 2, 2, 1, 3]),
            "m_SubMeshes": submeshes(4, (0, 6, 0), (0, 3, 0), class_id=213),
        }
        objects = [
            (1, 28, texture(tex)),
            (2, 28, texture(lambda x, y: (10 * x + y, 0, 0, 77))),
            (3, 213, sprite()),
            *((20 + rot, 213, sprite(settings=3 | rot << 2)) for rot in range(5)),  # packed, each rotation
            (30, 213, sprite((0.5, 0, 2, 1))),
            (31, 213, sprite((1.5, 0, 2, 1))),
            (5, 213, sprite((0, 0, 2, 2), rd={"alpha": pptr(2)})),
            (6, 687078895, tpk_object(687078895, m_Name="atl", m_RenderDataMap=atlas_map)),
            (7, 213, sprite(m_SpriteAtlas=pptr(6), m_RenderDataKey=(guid, 5))),
            (8, 213, sprite(m_AtlasTags=["atl"], m_RenderDataKey=(guid, 5))),
            (9, 213, sprite(m_SpriteAtlas=pptr(6), m_RenderDataKey=(guid, 6))),
            (10, 213, sprite(m_AtlasTags=["other"], m_RenderDataKey=(guid, 5))),
            (11, 213, sprite((0, 0, 4, 4), 0, rd=mesh)),  # tight, with UVs
            (12, 213, sprite(m_SpriteAtlas=pptr(6), m_RenderDataKey=(guid, 7), rd=mesh)),
            (13, 213, sprite(rd={"texture": pptr(1, file_id=1)})),  # its texture in another file
        ]
        data = tpk_file(objects, externals=[S.External(b"archive:/CAB-o/CAB-o")])
        sf = load(data)[0]
        self.assertEqual(sf._cache, {})

        def image(path_id):
            return pixels(sf.objects[path_id].parse_as_object().image)

        # The crop of textureRect, then a vertical flip.
        expected = grid(2, 3, lambda x, y: tex(1 + x, 2 - y))
        self.assertEqual(image(3), expected)
        # Packed: flip left-right, flip top-bottom, rotate 180 or rotate 90 clockwise before the final flip.
        turns = [
            expected,
            grid(2, 3, lambda x, y: tex(2 - x, 2 - y)),
            grid(2, 3, lambda x, y: tex(1 + x, y)),
            grid(2, 3, lambda x, y: tex(2 - x, y)),
            grid(3, 2, lambda x, y: tex(2 - y, 2 - x)),
        ]
        self.assertEqual([image(20 + rot) for rot in range(5)], turns)
        # Pillow rounds the box half to even: x 0.5 crops from 0, x 1.5 from 2.
        self.assertEqual([image(30), image(31)], [[[tex(0, 0), tex(1, 0)]], [[tex(2, 0), tex(3, 0)]]])
        # An alpha texture's first band becomes the alpha.
        self.assertEqual(image(5), grid(2, 2, lambda x, y: (*tex(x, 1 - y)[:3], 10 * x + 1 - y)))
        # The cache: the unflipped decode, by texture path id, or the (texture, alpha) pair.
        self.assertEqual(set(sf._cache), {1, (1, 2)})
        self.assertEqual(sf._cache[1].getpixel((0, 0)), tex(0, 0))
        # A texture of the same path id in another file is the cached one: the key leaves out the FileID.
        self.assertEqual(image(13), expected)
        fresh = load(data)[0]
        with self.assertRaises(FileNotFoundError):
            _ = fresh.objects[13].parse_as_object().image

        # Atlases: by pointer, or by the first m_AtlasTags name a SpriteAtlas in the file peeks as; then the
        # render data under m_RenderDataKey, a key the atlas lacks raising StopIteration.
        by_atlas = grid(2, 2, lambda x, y: tex(x, 2 - y))
        self.assertEqual([image(7), image(8)], [by_atlas, by_atlas])
        with self.assertRaises(StopIteration):
            image(9)
        self.assertEqual(image(10), expected)
        # Tight: the mesh is the sprite's own m_RD's, with or without an atlas; the triangles of the first
        # submesh only are copied.
        seen = []

        def handler(rd, version):
            seen.append(rd)
            raise LookupError

        for path_id in (11, 12):
            sprite_obj = sf.objects[path_id].parse_as_object()
            with mock.patch.object(SpriteHelper, "MeshHandler", handler), self.assertRaises(LookupError):
                _ = sprite_obj.image
            self.assertIs(seen[-1], sprite_obj.m_RD)
        with mock.patch.object(SpriteHelper, "copy_triangle", wraps=SpriteHelper.copy_triangle) as copy:
            self.assertEqual(image(11), grid(2, 2, lambda x, y: tex(x, 1 - y)))
        self.assertEqual([c.args[1] for c in copy.call_args_list], [((0, 0), (2, 0), (0, 2)), ((0, 2), (2, 0), (2, 2))])
        with mock.patch.object(SpriteHelper, "get_image_from_sprite", return_value="image") as call:
            sprite_obj = sf.objects[3].parse_as_object()
            self.assertEqual(sprite_obj.image, "image")
        call.assert_called_once_with(sprite_obj)

        # 4.7: the render data holds `vertices` and no m_VertexData, so a tight sprite stops at process's assert.
        old = "4.7.2f1"
        vertex = element(213, "m_RD", "vertices", version=old)
        rd = render_data((0, 0, 2, 2), 0, version=old, vertices=[vertex] * 3, indices=[0, 1, 2])
        objects = [(1, 28, texture(tex, old)), (2, 213, tpk_object(213, old, m_Name="s", m_PixelsToUnits=1.0, m_RD=rd))]
        old_sf = load(tpk_file(objects, unity_version=old.encode()))[0]
        old_sprite = old_sf.objects[2].parse_as_object()
        self.assertEqual(SpriteHelper.get_image(old_sprite, old_sprite.m_RD.texture, None).size, (4, 4))
        with self.assertRaises(AssertionError):
            _ = old_sprite.image
        with self.assertRaises(AssertionError):
            MeshHelper.MeshHandler(old_sprite.m_RD, old_sprite.object_reader.version).process()

        # SpriteSettings: packed bit 0, packing mode bit 1, rotation bits 2-5, mesh type bit 6; rotations 5 to 15
        # are not in the enum.
        from UnityPy.enums import SpriteMeshType, SpritePackingMode, SpritePackingRotation

        got = SpriteHelper.SpriteSettings(1 | 1 << 1 | 3 << 2 | 1 << 6 | 0xFF00)
        self.assertEqual(
            (got.packed, got.packingMode, got.packingRotation, got.meshType),
            (
                True,
                SpritePackingMode.kSPMRectangle,
                SpritePackingRotation.kSPRRotate180,
                SpriteMeshType.kSpriteMeshTypeTight,
            ),
        )
        for rotation in range(5, 16):
            with self.subTest(rotation=rotation), self.assertRaises(ValueError):
                SpriteHelper.SpriteSettings(rotation << 2)
        self.assertEqual([(e.name, e.value) for e in SpritePackingMode], [("kSPMTight", 0), ("kSPMRectangle", 1)])
        rotations = ["kSPRNone", "kSPRFlipHorizontal", "kSPRFlipVertical", "kSPRRotate180", "kSPRRotate90"]
        self.assertEqual([(e.name, e.value) for e in SpritePackingRotation], [(n, i) for i, n in enumerate(rotations)])
        self.assertEqual([e.name for e in SpriteMeshType], ["kSpriteMeshTypeFullRect", "kSpriteMeshTypeTight"])

    # Section 15.2: render_sprite_mesh and copy_triangle on their own.
    def test_sprite_mesh_rendering(self):
        texture = Image.new("RGBA", (4, 4))
        texture.putdata([(x * 60, y * 60, 100, 255) for y in range(4) for x in range(4)])

        def render(vertices, uvs=((0.0, 0.0), (0.625, 0.0), (0.0, 0.875)), triangles=((0, 1, 2),)):
            mesh = SimpleNamespace(get_triangles=lambda: [list(triangles)], m_Vertices=list(vertices), m_UV0=list(uvs))
            with mock.patch.object(SpriteHelper, "copy_triangle") as copy:
                image = SpriteHelper.render_sprite_mesh(SimpleNamespace(m_PixelsToUnits=1.0), mesh, texture)
            return image.size, [(c.args[1], c.args[3]) for c in copy.call_args_list]

        # The constant axis is dropped (z first, then y, then x); positions and UVs round half to even: x 2.5 and
        # 1.5 to 2, u 0.625 * 4 to 2, v 0.875 * 4 to 4.
        size, calls = render([(0.0, 0.0, 7.0), (2.5, 0.0, 7.0), (1.5, 3.0, 7.0)])
        self.assertEqual(size, (2, 3))
        self.assertEqual(calls, [(((0, 0), (2, 0), (0, 4)), ((0, 0), (2, 0), (2, 3)))])
        size, calls = render([(0.0, 5.0, 0.0), (2.0, 5.0, 0.0), (0.0, 5.0, 2.0)])
        self.assertEqual((size, calls[0][1]), ((2, 2), ((0, 0), (2, 0), (0, 2))))
        with self.assertRaisesRegex(ValueError, "^Can't process 3d sprites!$"):
            render([(0.0, 0.0, 0.0), (1.0, 1.0, 1.0), (2.0, 0.0, 2.0)])
        with self.assertRaisesRegex(ValueError, "^No UV coordinates found in sprite mesh!$"):
            render([(0.0, 0.0, 0.0)] * 3, uvs=())
        for vertices, triangles in (([(0.0, 0.0, 0.0)] * 3, None), ([], ((0, 1, 2),))):
            mesh = SimpleNamespace(
                get_triangles=lambda t=triangles: [] if t is None else [list(t)], m_Vertices=vertices, m_UV0=[(0, 0)]
            )
            with self.assertRaisesRegex(ValueError, "^No triangles found in mesh!$"):
                SpriteHelper.render_sprite_mesh(SimpleNamespace(m_PixelsToUnits=1.0), mesh, texture)

        # copy_triangle: a triangle of the same shape is cropped to its box, the box's far edge left out, and
        # pasted through a polygon mask; any other goes through an affine transform numpy solves.
        dst = Image.new("RGBA", (4, 4))
        SpriteHelper.copy_triangle(texture, ((0, 0), (2, 0), (0, 2)), dst, ((1, 1), (3, 1), (1, 3)))
        self.assertEqual(
            [dst.getpixel(p) for p in ((1, 1), (2, 1), (1, 2), (2, 2))],
            [texture.getpixel(p) for p in ((0, 0), (1, 0), (0, 1), (1, 1))],
        )
        self.assertEqual({dst.getpixel(p) for p in ((3, 1), (1, 3), (0, 0), (3, 3))}, {(0, 0, 0, 0)})
        dst = Image.new("RGBA", (4, 4))
        solved, real = [], SpriteHelper.np.linalg.solve
        with mock.patch.object(
            SpriteHelper.np.linalg, "solve", lambda m, y: solved.append((m, y, real(m, y))) or solved[-1][2]
        ):
            SpriteHelper.copy_triangle(texture, ((0, 0), (2, 0), (0, 2)), dst, ((0, 0), (4, 0), (0, 4)))
        ((m, y, a),) = solved
        self.assertEqual((m[1], m[5], y), ([4, 0, 1, 0, 0, 0], [0, 0, 0, 0, 4, 1], [0, 2, 0, 0, 0, 2]))
        self.assertEqual([round(float(v), 12) for v in a], [0.5, 0, 0, 0, 0.5, 0])
        self.assertEqual(dst.getpixel((3, 3)), (0, 0, 0, 0))
        self.assertEqual(dst.getpixel((0, 0)), texture.getpixel((0, 0)))

    # Sections 14.2, 15.7, 19.2: the extractor, from a file and function by function.
    def test_extractor_tree(self):
        from UnityPy.classes import PPtr

        def info(path_id):
            return {"preloadIndex": 0, "preloadSize": 0, "asset": pptr(path_id)}

        def bundle(*entries):
            return (100, 142, tpk_object(142, m_Name="ab", m_Container=[(path, info(p)) for path, p in entries]))

        def files(root):
            return {str(p.relative_to(root)): p.read_bytes() for p in sorted(root.rglob("*")) if p.is_file()}

        rgba = bytes(b for i in range(16) for b in (i, 2 * i, 3 * i, 255))
        texture = tpk_object(
            28, m_Name="t", m_Width=4, m_Height=4, m_TextureFormat=4, m_ImageCount=1, **{"image data": rgba}
        )
        rd = {
            **element(213, "m_RD"),
            "texture": pptr(1),
            "textureRect": {"x": 1, "y": 0, "width": 2, "height": 3},
            "settingsRaw": 2,
        }
        objects = [
            (1, 28, texture),
            (3, 213, tpk_object(213, m_Name="s.png", m_PixelsToUnits=1.0, m_RD=rd)),
            (4, 49, tpk_object(49, m_Name="n", m_Script="ok\udcff")),
            (5, 128, tpk_object(128, m_Name="f", m_FontData=list(b"OTTO\0\1"))),
            (6, 128, tpk_object(128, m_Name="g", m_FontData=[0, 1])),
            (7, 128, tpk_object(128, m_Name="h")),
        ]
        # By container: Sprite before Font before TextAsset before Texture2D; the first directory dropped; the
        # texture after its sprite at one path, so the PNG is the texture's; the container's extension kept,
        # except a font's: always `.ttf`, because m_FontData is a list and `[0:4] == b"OTTO"` never holds; a font
        # without data writes nothing.
        entries = [("Assets/A/x.png", 1), ("Assets/A/x.png", 3), ("Assets/A/t.bytes", 4), ("Assets/A/f.ttf", 5)]
        entries += [("Assets/A/g", 6), ("Assets/A/h", 7)]
        data = tpk_file([bundle(*entries), *objects])
        dst = self.tmp / "container"
        exported = extractor.extract_assets(data, dst, ignore_first_container_dirs=1)
        out = files(dst)
        self.assertEqual(sorted(out), ["A/f.ttf", "A/g.ttf", "A/t.bytes", "A/x.png"])
        self.assertEqual((out["A/t.bytes"], out["A/f.ttf"], out["A/g.ttf"]), (b"ok\xff", b"OTTO\0\1", b"\0\1"))
        with Image.open(dst / "A/x.png") as png:
            self.assertEqual(png.size, (4, 4))
        self.assertEqual([path_id for _, path_id in exported], [3, 1, 5, 6, 7, 4, 1])
        # Every object by name, into a folder that must exist already; the sprite named `s.png` returns its
        # texture, which is then skipped.
        dst = self.tmp / "objects"
        with self.assertRaises(FileNotFoundError):
            extractor.extract_assets(tpk_file(objects), dst, use_container=False)
        dst.mkdir()
        exported = extractor.extract_assets(tpk_file(objects), dst, use_container=False)
        self.assertEqual(sorted(files(dst)), ["f.ttf", "g.ttf", "n.txt", "s.png"])
        self.assertEqual([path_id for _, path_id in exported], [3, 1, 5, 6, 7, 4])
        # Nothing catches a failure: a mesh without vertices writes `False` and raises after opening its file,
        # and the TextAsset after it is never written. A GameObject sorts first and always raises.
        mesh = tpk_object(43, m_Name="m", m_VertexData=vertex_data(0, {}))
        for path_id, class_id, obj, error in (
            (2, 43, mesh, TypeError),
            (2, 1, tpk_object(1, m_Name="go"), AttributeError),
        ):
            dst = self.tmp / f"failed{class_id}"
            with self.subTest(class_id=class_id), self.assertRaises(error):
                extractor.extract_assets(
                    tpk_file([bundle(("a/o.asset", 2), ("a/t.txt", 4)), (path_id, class_id, obj), objects[2]]), dst
                )
            self.assertEqual(files(dst), {"a/o.asset": b""} if class_id == 43 else {})
        # exportSprite has no default extension: Pillow refuses a path without one. It returns the sprite and its
        # texture, never the alpha texture (it reads `assets_file` from a PPtr, whose attribute is `assetsfile`).
        sprite = load(tpk_file(objects))[0].objects[3]
        with self.assertRaisesRegex(ValueError, "unknown file extension"):
            extractor.exportSprite(sprite, str(self.tmp / "bare"), "")
        with_alpha = tpk_object(213, m_Name="a", m_PixelsToUnits=1.0, m_RD={**rd, "alphaTexture": pptr(2)})
        sf = load(tpk_file([(1, 28, texture), (2, 28, texture), (8, 213, with_alpha)]))[0]
        self.assertEqual(extractor.exportSprite(sf.objects[8], str(self.tmp / "alpha"), ".png"), [(sf, 8), (sf, 1)])
        # exportMesh writes obj.export() to `{fp}.obj`.
        triangle = vertex_data(3, {0: (0, 0, "f", [(0.0, 0.0, 0.0)] * 3)})
        mesh = tpk_object(
            43,
            m_Name="m",
            m_SubMeshes=submeshes(3, (0, 3, 0)),
            m_IndexBuffer=index_buffer([0, 1, 2]),
            m_VertexData=triangle,
        )
        reader = load(tpk_file([(2, 43, mesh)]))[0].objects[2]
        extractor.exportMesh(reader, str(self.tmp / "m"), "")
        self.assertEqual((self.tmp / "m.obj").read_text(), reader.parse_as_object().export())

        # exportShader: obj.export() as UTF-8 text, `.txt` without an extension.
        shader = SimpleNamespace(export=lambda: "é", assets_file="sf", object_reader=SimpleNamespace(path_id=3))
        self.assertEqual(extractor.exportShader(shader, str(self.tmp / "sh"), ""), [("sf", 3)])
        self.assertEqual((self.tmp / "sh.txt").read_bytes(), "é".encode())

        # exportAudioClip: one sample as `{fp}.wav` whatever it holds; several in a folder, `.wav` added to names
        # that end in `.wav`; none, nothing.
        def clip(samples):
            return SimpleNamespace(samples=samples, object_reader=SimpleNamespace(assets_file="sf", path_id=8))

        self.assertEqual(extractor.exportAudioClip(clip({"c.ogg": b"OggS"}), str(self.tmp / "one")), [("sf", 8)])
        extractor.exportAudioClip(clip({"c.wav": b"1", "c-1.wav": b"2"}), str(self.tmp / "two"))
        extractor.exportAudioClip(clip({}), str(self.tmp / "none"))
        self.assertEqual((self.tmp / "one.wav").read_bytes(), b"OggS")
        self.assertEqual(files(self.tmp / "two"), {"c-1.wav.wav": b"2", "c.wav.wav": b"1"})
        self.assertFalse(any(p.name.startswith("none") for p in self.tmp.iterdir()))

        # exportMonoBehaviour: the dict as JSON, indent 4, non-ASCII kept; raw bytes as `.bin` when it does not
        # parse. Through export_obj a MonoBehaviour raises after writing (`obj.path_id`), and another type with
        # export_unknown_as_typetree raises before.
        mono = tpk_object(114, m_Name="é")
        sf = load(tpk_file([(9, 114, mono), (10, 21, tpk_object(21, m_Name="mat"))]))[0]
        reader = sf.objects[9]
        self.assertEqual(extractor.exportMonoBehaviour(reader, str(self.tmp / "mb")), [(sf, 9)])
        text = (self.tmp / "mb.json").read_bytes()
        self.assertEqual(text, json.dumps(reader.parse_as_dict(), indent=4, ensure_ascii=False).encode())
        self.assertIn("é".encode(), text)
        broken = SimpleNamespace(parse_as_dict=lambda: 1 / 0, get_raw_data=lambda: b"raw", assets_file="sf", path_id=9)
        extractor.exportMonoBehaviour(broken, str(self.tmp / "raw"))
        self.assertEqual((self.tmp / "raw.bin").read_bytes(), b"raw")
        with self.assertRaisesRegex(AttributeError, "path_id"):
            extractor.export_obj(reader, str(self.tmp / "via"))
        self.assertTrue((self.tmp / "via.json").exists())
        with self.assertRaisesRegex(AttributeError, "get_raw_data"):
            extractor.export_obj(sf.objects[10], str(self.tmp / "unknown"), export_unknown_as_typetree=True)
        self.assertEqual(extractor.export_obj(sf.objects[10], str(self.tmp / "unknown")), [])
        self.assertFalse(any(p.name.startswith("unknown") for p in self.tmp.iterdir()))

        # crawl_obj: a parsed object raises at `obj.m_PathID`; a null PPtr at `obj.m_Index`; another PPtr is kept
        # by its path id alone. flatten opens lists, tuples and dicts.
        with self.assertRaisesRegex(AttributeError, "m_PathID"):
            extractor.crawl_obj(sf.objects[10].parse_as_object())
        with self.assertRaisesRegex(AttributeError, "m_Index"):
            extractor.crawl_obj(PPtr(m_FileID=0, m_PathID=0, assetsfile=sf))
        ptr = PPtr(m_FileID=0, m_PathID=10, assetsfile=sf)
        self.assertEqual(extractor.crawl_obj(ptr), {10: ptr})
        self.assertEqual(list(extractor.flatten([1, (2, [3]), {"a": 4, "b": {"c": (5,)}}])), [1, 2, 3, 4, 5])

    # Sections 14.2, 15.5, 19.1: a 2021.3 Shader's text, the tables the converter reads, and older shaders.
    def test_shader_text(self):
        import lz4.block
        from UnityPy.export import ShaderConverter as SC

        def subprogram(version, kind, code, keywords=(), local=None, endian="<"):
            out = struct.pack(f"{endian}ii", version, kind) + bytes(12) + (bytes(4) if version >= 201608170 else b"")
            for words in (keywords, *(() if local is None else (local,))):
                out += struct.pack(f"{endian}i", len(words)) + b"".join(S.s32(w, endian) for w in words)
            return out + struct.pack(f"{endian}i", len(code)) + code + bytes(-len(code) % 4)

        def program(subprograms, entry=12, endian="<"):
            head = 4 + entry * len(subprograms)
            out, table = b"", b""
            for sub in subprograms:
                table += struct.pack(f"{endian}ii", head + len(out), len(sub)) + bytes(entry - 8)
                out += sub
            return struct.pack(f"{endian}i", len(subprograms)) + table + out

        def value(v):
            return {"val": v, "name": ""}

        def prop(name, kind, defaults=(0, 0, 0, 0), attributes=(), texture=("", 2)):
            p = element(48, "m_ParsedForm", "m_PropInfo", "m_Props")
            p.update(m_Name=name, m_Description=name.strip("_"), m_Type=kind, m_Attributes=list(attributes))
            p.update({f"m_DefValue[{i}]": float(v) for i, v in enumerate(defaults)})
            return {**p, "m_DefTexture": {**p["m_DefTexture"], "m_DefaultName": texture[0], "m_TexDim": texture[1]}}

        def sub(blob, kind, tier=0):
            return {
                **element(48, "m_ParsedForm", "m_SubShaders", "m_Passes", "progVertex", "m_SubPrograms"),
                "m_BlobIndex": blob,
                "m_GpuProgramType": kind,
                "m_ShaderHardwareTier": tier,
            }

        passes = element(48, "m_ParsedForm", "m_SubShaders", "m_Passes")
        state = {
            **passes["m_State"],
            "m_Name": "P",
            "m_Tags": {"tags": [("LightMode", "Always")]},
            "gpuProgramID": 7,
            "lighting": True,
        }
        state |= {
            k: value(v)
            for k, v in dict(
                zTest=2, zWrite=0, culling=0, offsetFactor=1, offsetUnits=2, alphaToMask=1, zClip=0
            ).items()
        }
        normal = {**passes, "m_Type": 0, "m_State": state}
        normal["progVertex"] = {**normal["progVertex"], "m_SubPrograms": [sub(0, 4, 1), sub(0, 4, 2), sub(1, 15)]}
        subshader = element(48, "m_ParsedForm", "m_SubShaders")
        subshader |= {"m_LOD": 100, "m_Tags": {"tags": [("RenderType", "Opaque")]}}
        subshader["m_Passes"] = [
            normal,
            {**passes, "m_Type": 1, "m_UseName": "Other/PASS"},
            {**passes, "m_Type": 2, "m_TextureName": "_Grab"},
        ]
        form = element(48, "m_ParsedForm")
        form |= {
            "m_Name": "Test/S",
            "m_FallbackName": "Diffuse",
            "m_CustomEditorName": "Ed",
            "m_SubShaders": [subshader],
        }
        form["m_PropInfo"] = {"m_Props": [
            prop("_Color", 0, (1, 0.5, 0, 1), ["HDR"]), prop("_V", 1), prop("_F", 2, (0.25,)),
            prop("_R", 3, (0.5, 0, 2)), prop("_MainTex", 4, texture=("white", 2)), prop("_Any", 4, texture=("", 1)),
            prop("_None", 4, texture=("", 0)),
        ]}  # fmt: skip
        # Platform D3D11 then GLES3, each its own LZ4 slice; a third platform has no lengths and is never read.
        glsl = subprogram(201806140, 4, b"void main(){}", ["A", "B"], ["L"])
        programs = [
            program([subprogram(202012090, 0, b"x"), subprogram(202012090, 15, b"DXBC")]),
            program([glsl, glsl]),
        ]
        packed = [lz4.block.compress(p, store_size=False) for p in programs]
        shader = tpk_object(
            48,
            m_Name="Test/S",
            m_ParsedForm=form,
            platforms=[4, 9, 18],
            offsets=[[0], [len(packed[0])]],
            compressedLengths=[[len(p)] for p in packed],
            decompressedLengths=[[len(p)] for p in programs],
            compressedBlob=list(b"".join(packed)),
        )
        parsed = load(tpk_file([(1, 48, shader)]))[0].objects[1].parse_as_object()
        props = [
            '[HDR] _Color ("Color", Color) = (1,0.5,0,1)\n', '_V ("V", Vector) = (0,0,0,0)\n',
            '_F ("F", Float) = 0.25\n', '_R ("R", Range(0, 2)) = 0.5\n', '_MainTex ("MainTex", 2D) = "white" { }\n',
            '_Any ("Any", any) = "" { }\n', '_None ("None", ) = "" { }\n',
        ]  # fmt: skip
        glsl_text = 'Keywords { "A" "B" }\nLocal Keywords { "L" }\n"void main(){}"'
        expected = (
            SC.HEADER + 'Shader "Test/S" {\n' + "\n".join(["Properties {\n", *props, "}\n"])
            + 'SubShader {\n LOD 100\n Tags { "RenderType" = "Opaque" }\n'
            + ' Pass {\n Name "P"\n  Tags { "LightMode" = "Always" }\n AlphaToMask On\n ZClip Off\n ZTest Less\n'
            + " ZWrite Off\n Cull Off\n Offset 1.0, 2.0\n  Lighting On\n  GpuProgramID 7\n"
            + 'Program "vp" {\n'
            + f'SubProgram "gles3 hw_tier01 " {{\n{glsl_text}\n}}\nSubProgram "gles3 hw_tier02 " {{\n{glsl_text}\n}}\n'
            + 'SubProgram "d3d11 " {\n"// shader disassembly not supported on DXBC"\n}\n}\n}\n'
            + ' UsePass "Other/PASS"\n GrabPass {\n "_Grab"\n}\n}\n'
            + 'Fall back "Diffuse"\nCustomEditor "Ed"\n}'
        )  # fmt: skip
        self.assertEqual(parsed.export(), expected)
        with mock.patch.object(SC, "export_shader", return_value="t") as export:
            self.assertEqual(parsed.export(), "t")
        export.assert_called_once_with(parsed)
        # A property of type 5 (Int, from 2021.1) raises once its name is written.
        int_prop = SimpleNamespace(m_Attributes=[], m_Name="_I", m_Description="I", m_Type=5)
        with self.assertRaisesRegex(ValueError, "^5$"):
            SC.ConvertSerializedProperty(int_prop)

        # ShaderSubProgram: each program type's text; Metal skips its entry name, behind the 0xF00DCAFE header or,
        # without it, after dropping four bytes; an unknown type raises.
        def exported(kind, code, version=202012090):
            return SC.ShaderSubProgram(EndianBinaryReader(subprogram(version, kind, code), endian="<")).Export()

        from UnityPy.streams import EndianBinaryReader

        texts = {kind: exported(kind, b"src") for kind in range(32) if kind not in (23, 24)}
        self.assertEqual({k for k, t in texts.items() if t == '"src"'}, {1, 2, 3, 4, 5, 6, 7, 8, 26, 27, 28, 29, 30})
        self.assertEqual(
            {k for k, t in texts.items() if t == '"// shader disassembly not supported on DXBC"'}, set(range(9, 23))
        )
        self.assertEqual(
            (texts[25], texts[0], texts[31]),
            ('""', '"//shader disassembly not supported on 0"', '"//shader disassembly not supported on 31"'),
        )
        self.assertEqual(exported(23, struct.pack("<II", 0xF00DCAFE, 8) + b"main\0metal"), '"metal"')
        self.assertEqual(exported(24, b"1234main\0metal"), '"metal"')
        with self.assertRaises(ValueError):
            exported(32, b"")
        # Local keywords are read only for versions in [201806140, 202012090); the reserved word from 201608170.
        for version, local in ((201806140, ["L"]), (202012089, ["L"]), (202012090, None), (201802150, None)):
            reader = EndianBinaryReader(subprogram(version, 4, b"c", ["K"], local) + b"\xff" * 4, endian="<")
            got = SC.ShaderSubProgram(reader)
            self.assertEqual((got.m_Keywords, got.m_LocalKeywords, bytes(got.m_ProgramCode)), (["K"], local, b"c"))
        got = SC.ShaderSubProgram(EndianBinaryReader(subprogram(201510240, 4, b"c", ["K"]), endian="<"))
        self.assertEqual((got.m_Keywords, bytes(got.m_ProgramCode)), (["K"], b"c"))
        # ShaderProgram: 12-byte table entries from 2019.3, 8 before.
        for version, entry in (((2019, 3, 0, 1), 12), ((2019, 2, 9, 1), 8)):
            got = SC.ShaderProgram(EndianBinaryReader(program([glsl, glsl], entry), endian="<"), version)
            self.assertEqual([s.m_Keywords for s in got.m_SubPrograms], [["A", "B"]] * 2)

        # The tables: the program types each platform can use; NaCl, Flash, PSM and every platform the table lacks
        # raise; the platform names.
        usable = {}
        for platform in range(-1, 22):
            try:
                usable[platform] = [t for t in range(32) if SC.CheckGpuProgramUsable(platform, t)]
            except NotImplementedError:
                usable[platform] = None
        consoles = [26, 27, 28, 29, 30]
        self.assertEqual(usable, {
            -1: None, 0: [1], 1: [9, 10, 11, 12], 2: consoles, 3: None, 4: list(range(15, 23)), 5: [5], 6: None,
            7: None, 8: [13, 14], 9: [2, 3, 4], 10: consoles, 11: consoles, 12: consoles, 13: None, 14: [23],
            15: [6, 7, 8], 16: consoles, 17: consoles, 18: [25], 19: consoles, 20: consoles, 21: None,
        })  # fmt: skip
        names = "openGL d3d9 xbox360 ps3 d3d11 gles glesdesktop flash d3d11_9x gles3 psp2 ps4 xboxone psm metal glcore"
        names += " n3ds wiiu vulkan switch xboxone+_d3d12"
        self.assertEqual([SC.GetPlatformString(p) for p in range(-1, 22)], ["unknown", *names.split(), "unknown"])
        from UnityPy.enums import (
            PassType,
            SerializedPropertyType,
            ShaderCompilerPlatform,
            ShaderGpuProgramType,
            TextureDimension,
        )

        self.assertEqual([e.value for e in ShaderCompilerPlatform], list(range(-1, 21)))
        self.assertEqual([e.value for e in ShaderGpuProgramType], list(range(32)))
        self.assertEqual([e.name for e in PassType], ["kPassTypeNormal", "kPassTypeUse", "kPassTypeGrab"])
        self.assertEqual(
            [e.name for e in SerializedPropertyType], ["kColor", "kVector", "kFloat", "kRange", "kTexture"]
        )
        self.assertEqual([e.value for e in TextureDimension], [-1, 0, 1, 2, 3, 4, 5, 6, 2147483647])

        # Before 5.5: m_Script is a string, and `bytes(m_Script)` raises TypeError on the blob path (5.3-5.4) and
        # on the plain path alike.
        blob = program([subprogram(201510240, 4, b"code", endian=">")], 8, endian=">")
        for version, fields in (
            (
                "5.4.6f3",
                {"m_SubProgramBlob": list(lz4.block.compress(blob, store_size=False)), "decompressedSize": len(blob)},
            ),
            ("5.4.6f3", {}),
            ("4.7.2f1", {}),
        ):
            data = tpk_object(48, version, m_Name="old", m_Script="Shader { GpuProgramIndex 0 }", **fields)
            old = load(tpk_file([(1, 48, data)], unity_version=version.encode()))[0].objects[1].parse_as_object()
            with (
                self.subTest(version=version, blob=bool(fields)),
                self.assertRaisesRegex(TypeError, "without an encoding"),
            ):
                old.export()
        # The 5.4 blob itself reads big-endian and fills `GpuProgramIndex N`, N being the rest of the line.
        got = SC.ShaderProgram(EndianBinaryReader(blob), (5, 4, 6, 1))
        self.assertEqual(got.Export("Shader {\nGpuProgramIndex 0\n}"), 'Shader {\n"code"\n}')
        with self.assertRaisesRegex(ValueError, "'0 }'"):
            got.Export("Shader { GpuProgramIndex 0 }")

    # Section 15.7: the bytes of Pillow's PNG writer with its defaults, rebuilt here from the PNG specification.
    def test_png_bytes(self):
        import io
        import random
        import zlib

        def paeth(a, b, c):
            p = a + b - c
            pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
            return a if pa <= pb and pa <= pc else b if pb <= pc else c

        def chunk(kind, data):
            return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

        def encode(image):
            """Each row filtered by the type whose bytes, as signed, have the least absolute sum, tried in the order
            none, up, sub, Paeth with the first kept on a tie (average only with `optimize`); zlib level 6, memLevel
            9, Z_FILTERED; IDAT chunks of 65536 bytes."""
            bpp, color = {"L": (1, 0), "LA": (2, 4), "RGB": (3, 2), "RGBA": (4, 6)}[image.mode]
            width, height = image.size
            stride, pixels = width * bpp, image.tobytes()
            prev, raw = bytes(stride), bytearray()
            for y in range(height):
                row, best = pixels[y * stride : (y + 1) * stride], None
                for kind in (0, 2, 1, 4):
                    line = bytearray()
                    for i in range(stride):
                        a, b, c = row[i - bpp] if i >= bpp else 0, prev[i], prev[i - bpp] if i >= bpp else 0
                        line.append((row[i] - (0, a, b, (a + b) // 2, paeth(a, b, c))[kind]) & 0xFF)
                    score = sum(min(v, 256 - v) for v in line)
                    if best is None or score < best[0]:
                        best = (score, kind, line)
                raw += bytes([best[1]]) + best[2]
                prev = row
            packer = zlib.compressobj(6, zlib.DEFLATED, 15, 9, zlib.Z_FILTERED)
            stream = packer.compress(bytes(raw)) + packer.flush()
            idat = b"".join(chunk(b"IDAT", stream[i : i + 65536]) for i in range(0, len(stream), 65536))
            head = chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, color, 0, 0, 0))
            return b"\x89PNG\r\n\x1a\n" + head + idat + chunk(b"IEND", b"")

        rnd = random.Random(5)
        cases = [("RGBA", 24, 16, 4), ("RGB", 40, 20, 4), ("L", 30, 30, 4), ("LA", 33, 17, 4), ("RGBA", 130, 130, 256)]
        for mode, width, height, noise in cases:
            image = Image.new(mode, (width, height))
            image.frombytes(
                bytes(
                    (x * 3 + y * 5 + k * 40 + rnd.randrange(noise)) % 256
                    for y in range(height)
                    for x in range(width)
                    for k in range(len(mode))
                )
            )
            saved = io.BytesIO()
            image.save(saved, "PNG")
            with self.subTest(mode=mode, size=(width, height)):
                self.assertEqual(saved.getvalue(), encode(image))
        self.assertEqual(saved.getvalue().count(b"IDAT"), 2)  # the noisy 130x130 needs two chunks
