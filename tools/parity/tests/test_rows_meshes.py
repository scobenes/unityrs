"""PARITY.md rows on meshes, run against the oracle: a 2021.3 Mesh through MeshHandler, its parts by version, compressed
and old meshes, the OBJ text, and renderers with their meshes and materials.

Sections 14.2, 15.3, 15.4 and 19. Every input is built here, so nothing needs the corpus, and each test names the
rows it pins; a row's test column names the test back. Objects are written from the TPK's node for their engine
version (`test_rows_objects.tpk_object`); vertex data and packed bit vectors are laid out from the formats' own rules
(`export_support.py` and the helpers below), not by UnityPy. Its class is `ExportRows`, as in
`test_rows_exports.py`, the module these tests were split from. Needs UnityPy 1.25.2 (the oracle image); skipped
elsewhere.
"""

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

from export_support import element, index_buffer, submeshes, vertex_data  # noqa: E402
from rows_support import ORACLE, load  # noqa: E402
from test_rows_objects import VERSION, pptr, tpk_file, tpk_object  # noqa: E402

if ORACLE:
    from UnityPy.classes import ChannelInfo, StreamInfo
    from UnityPy.enums import ClassIDType
    from UnityPy.export import MeshExporter, MeshRendererExporter
    from UnityPy.helpers import MeshHelper, PackedBitVector, Tpk
    from UnityPy.helpers.UnityVersion import UnityVersion
    from UnityPy.tools import extractor


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


@unittest.skipUnless(ORACLE, "needs UnityPy 1.25.2 (the oracle image)")
class ExportRows(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="unityrs-exports-"))
        self.addCleanup(shutil.rmtree, self.tmp)

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
        # m_Skin is a Mesh field up to 2018.1 and gone from 2018.2; a 2018.1 mesh's skin is read the same way.
        for version, has_skin in (
            ("2017.4.40f1", True),
            ("2018.1.0f2", True),
            ("2018.1.9f2", True),
            ("2018.2.0f2", False),
        ):
            self.assertEqual("m_Skin" in element(43, version=version), has_skin, version)
        late = "2018.1.9f2"
        vd = {**element(43, "m_VertexData", version=late), **vertex_data(4, {0: (0, 0, "f", [p for p, _, _ in rows])})}
        late_skin = [{**s, "weight[0]": 0.25, "weight[1]": 0.75} for s in skin]
        sub = submeshes(4, (0, 6, 0), version=late)
        h = handled(
            late, m_VertexData=vd, m_SubMeshes=sub, m_IndexBuffer=index_buffer([0, 1, 2, 2, 1, 3]), m_Skin=late_skin
        )
        self.assertEqual((h.m_BoneWeights[1], h.m_BoneIndices[2]), ((0.25, 0.75, 0.0, 0.0), (2, 1, 0, 0)))
        # The index width: 16-bit unless m_IndexFormat says 32 (1), which counts from 2017.4, and at 2017.3 only for
        # a mesh whose m_MeshCompression is 0; before 2017.3 the Mesh has no m_IndexFormat.
        triangle = [(0.0, 0.0, 0.0), (1.0, 0.0, 0.0), (0.0, 1.0, 0.0)]
        for version, index_format, compression, sixteen in (
            ("2017.2.5f1", None, 0, True),
            ("2017.3.0f1", 1, 0, False),
            ("2017.3.0f1", 1, 1, True),  # its 32-bit buffer read as 16-bit
            ("2017.4.0f1", 1, 1, False),
            ("2017.4.0f1", 0, 0, True),
        ):
            vd = {**element(43, "m_VertexData", version=version), **vertex_data(3, {0: (0, 0, "f", triangle)})}
            vd["m_Channels"] = vd["m_Channels"][:8]  # eight channels before 2018
            fields = dict(m_VertexData=vd, m_SubMeshes=submeshes(3, (0, 3, 0), version=version))
            fields |= {
                "m_MeshCompression": compression,
                "m_IndexBuffer": index_buffer([0, 1, 2], "HI"[index_format == 1]),
            }
            fields |= {} if index_format is None else {"m_IndexFormat": index_format}
            with self.subTest(version=version, compression=compression):
                h = handled(version, **fields)
                self.assertEqual(h.m_Use16BitIndices, sixteen)
                self.assertEqual(h.m_IndexBuffer, (0, 0, 1, 0, 2, 0) if sixteen and index_format == 1 else (0, 1, 2))
        # 3.4: the Mesh holds its own vertex arrays and no m_VertexData, and process stops at its assert first.
        vertex = {"x": 1.0, "y": 2.0, "z": 3.0}
        with self.assertRaises(AssertionError):
            handled("3.4.0f5", m_Vertices=[vertex])
        # The TPK keeps that layout, m_Use16BitIndices included, up to 3.5.0f4, and gives the Mesh m_VertexData at
        # 3.5.0f5. A 3.5.0f4 Mesh reads its m_Use16BitIndices and fails the assert before the field can decide.
        for version, old in (("3.4.0f5", True), ("3.5.0b1", True), ("3.5.0f4", True), ("3.5.0f5", False)):
            names = {c.m_Name for c in Tpk.get_typetree_node(43, UnityVersion.from_str(version)).m_Children}
            with self.subTest(version):
                self.assertEqual(("m_Use16BitIndices" in names, "m_VertexData" in names), (old, not old))
        mesh_data = tpk_object(43, "3.5.0f4", m_Vertices=[vertex], m_Use16BitIndices=0)
        mesh = load(tpk_file([(1, 43, mesh_data)], unity_version=b"3.5.0f4"))[0].objects[1].parse_as_object()
        self.assertEqual((mesh.m_Use16BitIndices, mesh.m_VertexData), (0, None))
        with self.assertRaises(AssertionError):
            handled("3.5.0f4", m_Vertices=[vertex], m_Use16BitIndices=0)

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
