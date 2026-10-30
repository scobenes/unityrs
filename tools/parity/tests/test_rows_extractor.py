"""PARITY.md rows on getting images out, run against the oracle: texture conversion, sprites from the file to the image
and their mesh rendering, the extractor, and the bytes of Pillow's PNG writer.

Sections 7.2, 14.2, 15.1, 15.2, 15.7 and 19. Every input is built here, so nothing needs the corpus, and each test
names the rows it pins; a row's test column names the test back. Objects are written from the TPK's node for their
engine version (`test_rows_objects.tpk_object`); vertex data is laid out from the format's own rules
(`export_support.py`), not by UnityPy. Its class is `ExportRows`, as in `test_rows_exports.py`, the module these
tests were split from. Needs UnityPy 1.25.2 (the oracle image); skipped elsewhere.
"""

import json
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

import synth as S  # noqa: E402

if ORACLE:
    from PIL import Image
    from UnityPy.enums import BuildTarget, TextureFormat
    from UnityPy.export import SpriteHelper, Texture2DConverter
    from UnityPy.helpers import MeshHelper
    from UnityPy.tools import extractor


@unittest.skipUnless(ORACLE, "needs UnityPy 1.25.2 (the oracle image)")
class ExportRows(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="unityrs-exports-"))
        self.addCleanup(shutil.rmtree, self.tmp)

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

        # Before 5.6 the render data holds `vertices` and `indices` and no m_VertexData, so a tight sprite stops at
        # process's assert; from 5.6 it holds m_VertexData, and the sprite renders.
        def tight(version, **rd_fields):
            rd = render_data((0, 0, 2, 2), 0, version=version, **rd_fields)
            sprite_data = tpk_object(213, version, m_Name="s", m_PixelsToUnits=1.0, m_RD=rd)
            objects = [(1, 28, texture(tex, version)), (2, 213, sprite_data)]
            return load(tpk_file(objects, unity_version=version.encode()))[0].objects[2].parse_as_object()

        for old in ("4.7.2f1", "5.0.0f4", "5.5.6f1"):
            with self.subTest(old):
                vertex = element(213, "m_RD", "vertices", version=old)
                old_sprite = tight(old, vertices=[vertex] * 3, indices=[0, 1, 2])
                self.assertIsNone(old_sprite.m_RD.m_VertexData)
                self.assertEqual(SpriteHelper.get_image(old_sprite, old_sprite.m_RD.texture, None).size, (4, 4))
                with self.assertRaises(AssertionError):
                    _ = old_sprite.image
                with self.assertRaises(AssertionError):
                    MeshHelper.MeshHandler(old_sprite.m_RD, old_sprite.object_reader.version).process()
        new = "5.6.0f1"
        new_rd = element(213, "m_RD", version=new)
        self.assertNotIn("vertices", new_rd)
        quad_56 = {**new_rd["m_VertexData"], **quad}
        mesh_56 = {**mesh, "m_VertexData": quad_56, "m_SubMeshes": submeshes(4, (0, 6, 0), class_id=213, version=new)}
        self.assertEqual(tight(new, **mesh_56).image.size, (2, 2))

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
        # "The same shape" is the same two edge offsets: a mirrored copy of the same size takes the affine path.
        with mock.patch.object(SpriteHelper.np.linalg, "solve", wraps=SpriteHelper.np.linalg.solve) as solve:
            SpriteHelper.copy_triangle(
                texture, ((0, 0), (2, 0), (0, 2)), Image.new("RGBA", (4, 4)), ((2, 0), (0, 0), (2, 2))
            )
        self.assertEqual(solve.call_count, 1)
        # A destination whose corners are collinear makes the affine matrix singular, and the error escapes, from
        # copy_triangle and from a mesh whose positions round onto a line while its UVs do not (section 19.2).
        for flat in (((0, 0), (1, 0), (2, 0)), ((0, 0), (1, 1), (2, 2))):
            with self.subTest(flat), self.assertRaisesRegex(SpriteHelper.np.linalg.LinAlgError, "^Singular matrix$"):
                SpriteHelper.copy_triangle(texture, ((0, 0), (0, 2), (2, 0)), Image.new("RGBA", (4, 4)), flat)
        mesh = SimpleNamespace(
            get_triangles=lambda: [[(0, 1, 2)]],
            m_Vertices=[(0.0, 0.0, 0.0), (1.0, 0.4, 0.0), (2.0, 0.4, 0.0)],
            m_UV0=[(0.0, 0.0), (0.5, 0.0), (0.0, 0.5)],
        )
        with self.assertRaisesRegex(SpriteHelper.np.linalg.LinAlgError, "^Singular matrix$"):
            SpriteHelper.render_sprite_mesh(SimpleNamespace(m_PixelsToUnits=1.0), mesh, texture)
        # With the source's own offsets a collinear triangle takes the translation path: a flat box copies
        # nothing, and a diagonal one the pixels Pillow's mask of it covers.
        for flat, covered in ((((0, 0), (1, 0), (2, 0)), []), (((0, 0), (1, 1), (2, 2)), [(0, 0), (1, 0), (1, 1)])):
            dst = Image.new("RGBA", (4, 4))
            SpriteHelper.copy_triangle(texture, flat, dst, flat)
            self.assertEqual([(x, y) for y in range(4) for x in range(4) if dst.getpixel((x, y))[3]], covered)
        # So one zero-area destination gets pixels from a source with its offsets and raises from a mirrored one:
        # Pillow's mask of it covers pixels, but no transform defines them (section 19.2's convention).
        big = Image.new("RGBA", (8, 8), (9, 9, 9, 255))
        dst = Image.new("RGBA", (8, 8))
        SpriteHelper.copy_triangle(big, ((0, 0), (2, 2), (5, 5)), dst, ((1, 1), (3, 3), (6, 6)))
        self.assertEqual(sum(1 for y in range(8) for x in range(8) if dst.getpixel((x, y))[3]), 6)
        with self.assertRaisesRegex(SpriteHelper.np.linalg.LinAlgError, "^Singular matrix$"):
            SpriteHelper.copy_triangle(
                big, ((0, 0), (2, 2), (5, 5)), Image.new("RGBA", (8, 8)), ((6, 6), (4, 4), (1, 1))
            )

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
        # There, a sprite whose m_Name has an extension is saved by that extension's Pillow writer (section 15.8);
        # only one with no extension raises.
        for name, error in (("a.bmp", None), ("plain", "unknown file extension")):
            dst = self.tmp / f"sprite-{name}"
            dst.mkdir()
            named = tpk_file([(1, 28, texture), (3, 213, tpk_object(213, m_Name=name, m_PixelsToUnits=1.0, m_RD=rd))])
            with self.subTest(name):
                if error:
                    with self.assertRaisesRegex(ValueError, error):
                        extractor.extract_assets(named, dst, use_container=False)
                    self.assertEqual(files(dst), {})
                else:
                    extractor.extract_assets(named, dst, use_container=False)
                    out = files(dst)
                    self.assertEqual((sorted(out), out[name][:2]), ([name], b"BM"))
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
