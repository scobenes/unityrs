"""The NGUI reference's reads (`ngui_read.py`): its Environment sealed to the view, the bounds it decides before
Pillow does, and UnityPy's own behaviour where the reference departs from it, each checked on the oracle.

Needs UnityPy 1.25.2 and Pillow 9.5.0 (the oracle image), skipped elsewhere; the scenes are cargo's
(`ngui_scenes.py`).
"""

import json
import os
import struct
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
    from UnityPy.files import BundleFile, SerializedFile

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
# A child that runs UnityPy's own resource lookup for `gone` on the reference's sealed Environment, in its working
# directory: `get_resource_data` (helpers/ResourceReader.py:10-33), and `load_dependencies` then `get_cab`.
SEALED = textwrap.dedent("""
    import sys
    sys.path.insert(0, sys.argv[1])
    import ngui_read as rd
    from UnityPy.files import SerializedFile
    from UnityPy.helpers.ResourceReader import get_resource_data
    bundle = rd.load({b: open(b, "rb").read() for b in ("d.bundle", "x.bundle")}, "x.bundle")
    sf = next(f for f in bundle.files.values() if isinstance(f, SerializedFile))
    try:
        print(list(get_resource_data("archive:/CAB-x16/CAB-missing.resS", sf, 0, 4)))
    except FileNotFoundError:
        print("not found")
    sf.load_dependencies(["CAB-missing.resS"])
    print(sf.environment.get_cab("CAB-missing.resS"))
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
        # UnityPy's own lookup, on the sealed Environment in the same directory, finds nothing there.
        proc = child.run([sys.executable, "-c", SEALED, str(HERE)], cwd=tmp, capture_output=True, text=True,
                         check=True)  # fmt: skip
        self.assertEqual(proc.stdout.splitlines(), ["not found", "None"])
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
        # Four externals: CAB-gone, CAB-d5, the raw `CAB-x5.resS` row, etc. `m_FileID - 1` indexes them as a Python
        # list does (classes/PPtr.py:56-60), a negative index from the end.
        cases = [
            ((0, 0), "bad_pointer"),  # null
            ((-4, 1), "bad_pointer"),  # index -5, below the start
            ((5, 2), "bad_pointer"),  # index 4, past the externals
            ((0, 98), "bad_pointer"),  # a missing object
            ((3, 1), "bad_pointer"),  # the raw `.resS` row
            ((-1, 2), "bad_pointer"),  # index -2: the raw row again
            ((1, 99), "outside_view(CAB-gone)"),
            ((-3, 1), "outside_view(CAB-gone)"),  # index -4: the first external
            ((4, 1), "outside_view(etc)"),
        ]
        for (file_id, path_id), reason in cases:
            with self.subTest(pointer=(file_id, path_id)), self.assertRaises(rd.Failure) as caught:
                rd.deref(file_id, path_id, sf)
            self.assertEqual(caught.exception.reason, reason)
        self.assertEqual(rd.deref(2, 1, sf).peek_name(), "elsewhere")  # the dependency's CAB
        self.assertEqual(rd.deref(-2, 1, sf).peek_name(), "elsewhere")  # index -3: the same external
        with self.assertRaises(IndexError):  # UnityPy's own `FileIdBelowStart`
            UnityPy.classes.PPtr(m_FileID=-4, m_PathID=1, assetsfile=sf).deref()
        # A pointer whose external names a container row (`streamed_sheets`' nested bundle `inner`).
        streamed = rd.load(self.scenes["streamed_sheets"].view("x.bundle"), "x.bundle")
        self.assertEqual([e.path for e in streamed.files["CAB-x16p"].externals], ["archive:/inner/inner"])
        with self.assertRaises(rd.Failure) as caught:
            rd.deref(1, 1, streamed.files["CAB-x16p"])
        self.assertEqual(caught.exception.reason, "bad_pointer")


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

    def test_the_own_chain_walks_the_directory_s_rows(self):
        """UnityPy's `files` dict keeps one object per path, a repeated path's last in its first row's place; 4.3
        walks the rows, which the reference keeps at the load."""
        scene = ngui_scenes.scenes()["repeated_rows"]
        bundle = rd.load(scene.view("x.bundle"), "x.bundle")
        self.assertEqual(list(bundle.files), ["CAB-x20", "sheet.resS", "SHEET.resS", "CAB-n.resS"])
        paths = [path for path, _ in rd.rows(bundle)]
        self.assertEqual(paths, ["CAB-x20", "sheet.resS", "SHEET.resS", "sheet.resS", "CAB-n.resS", "CAB-n.resS"])
        first = [bytes(rd._get_resource_data(f, 0, 1)) for _, f in rd.rows(bundle)[1:5]]
        self.assertEqual(first, [b"\x51", b"\x52", b"\x53", b"\x54"])
        # The dict, reversed, meets `SHEET.resS` (0x52) before `sheet.resS`, which holds the third row (0x53).
        self.assertEqual(bytes(rd._get_resource_data(bundle.files["sheet.resS"], 0, 1)), b"\x53")
        textures = {o.peek_name(): o for o in objects(bundle) if o.type.name == "Texture2D"}
        for name, own, registry in (("rep", 0x53, 0x53), ("nest", 0x54, 0x55)):
            with self.subTest(texture=name):
                texture = textures[name].read()
                self.assertEqual(rd.decode(texture, textures[name]).getpixel((0, 0))[0], own)
                # UnityPy's registry answers `nest` with the nested bundle's row, registered after the raw one.
                self.assertEqual(texture.image.getpixel((0, 0))[0], registry)

    def test_a_check_read_refusal_is_told_by_the_check(self):
        """UnityPy's `check_read` raises a `ValueError` when a read ends before the object does
        (helpers/TypeTreeHelper.py:155-156); without the check the same read gives its value."""
        scene = ngui_scenes.scenes()["failed_reads"]
        bundle = rd.load(scene.view("x.bundle"), "x.bundle")
        by_id = {(o.assets_file.name, o.path_id): o for o in objects(bundle)}
        tail, ttail, ui = by_id[("CAB-x15", 10)], by_id[("CAB-x15", 11)], by_id[("CAB-x15", 2)]
        for obj, method in ((tail, tail.parse_as_dict), (ttail, ttail.read)):
            with self.subTest(path_id=obj.path_id):
                with self.assertRaisesRegex(ValueError, "^Expected to read"):
                    method()
                method(check_read=False)
                with self.assertRaises(rd.Failure) as caught:
                    rd.checked(method)
                self.assertEqual(caught.exception.detail, "asset_bundle")
        # A read that runs out fails with the check or without it: `read`.
        with self.assertRaises(rd.Failure) as caught:
            rd.checked(ui.read)
        self.assertEqual(caught.exception.detail, "read")

    def test_a_value_error_not_the_check_s_is_a_read(self):
        """`cb` is cut after `m_ForcedFallbackFormat`: UnityPyBoost's `read_bool` for `m_DownscaleFallback` raises a
        `ValueError` that is not the check's, with the check and without it, so `checked` gives `read`. (Taking every
        `ValueError` for the check's would give `asset_bundle` here.)"""
        scene = ngui_scenes.scenes()["failed_reads"]
        bundle = rd.load(scene.view("x.bundle"), "x.bundle")
        cb = next(o for o in objects(bundle) if o.assets_file.name == "CAB-x15" and o.path_id == 14)
        with self.assertRaises(ValueError) as checked:
            cb.read()
        self.assertNotRegex(str(checked.exception), "^Expected to read")
        with self.assertRaises(ValueError):
            cb.read(check_read=False)
        with self.assertRaises(rd.Failure) as caught:
            rd.checked(cb.read)
        self.assertEqual((caught.exception.reason, caught.exception.detail), ("decode_error", "read"))

    def test_an_object_of_no_bytes_is_read_by_unitypy_s_own_reader(self):
        """An object of no bytes skips UnityPyBoost (helpers/TypeTreeHelper.py:146-153); UnityPy's own reader, at the
        file's end, raises `struct.error`: a read failure, the texture's name peek and the MonoBehaviour's head."""
        scene = ngui_scenes.scenes()["failed_reads"]
        bundle = rd.load(scene.view("x.bundle"), "x.bundle")
        by_file = {o.assets_file.name: o for o in objects(bundle) if o.assets_file.name != "CAB-x15"}
        for cab, method in (("CAB-x15b", by_file["CAB-x15b"].peek_name),
                            ("CAB-x15c", by_file["CAB-x15c"].parse_monobehaviour_head)):  # fmt: skip
            with self.subTest(cab=cab):
                with self.assertRaises(struct.error):
                    method()
                with self.assertRaises(rd.Failure) as caught:
                    rd.read(method)
                self.assertEqual(caught.exception.detail, "read")
        self.assertEqual(rd.recognise(by_file["CAB-x15c"], ("NGUIAtlas",)), ("unresolved", None))

    def test_a_texture_whose_typetree_lacks_a_field_is_a_read_failure(self):
        """UnityPy reads a Texture2D whose typetree has `m_Name` alone as an `UnknownObject`."""
        scene = ngui_scenes.scenes()["failed_reads"]
        bundle = rd.load(scene.view("x.bundle"), "x.bundle")
        stub = next(o for o in objects(bundle) if o.assets_file.name == "CAB-x15" and o.path_id == 13)
        self.assertEqual(type(stub.read()).__name__, "UnknownObject")
        with self.assertRaises(rd.Failure) as caught:
            rd.texture_of(stub)
        self.assertEqual((caught.exception.reason, caught.exception.detail), ("decode_error", "read"))

    def test_a_texture_s_read_value_by_4_4_step_2_s_rule(self):
        """UnityPy's `Texture2D` requires 11 fields (classes/generated.py:5153-5164): one lacking
        `m_LightmapFormat` is an `UnknownObject`, a read failure; one with a field past them is a `Texture2D`, that
        field an attribute (helpers/TypeTreeHelper.py:241-258), and exported. `image_data` must be bytes and
        `m_PlatformBlob` a list or absent."""
        scene = ngui_scenes.scenes()["failed_reads"]
        bundle = rd.load(scene.view("x.bundle"), "x.bundle")
        by_id = {o.path_id: o for o in objects(bundle) if o.assets_file.name == "CAB-x15"}
        self.assertEqual(type(by_id[15].read()).__name__, "UnknownObject")
        with self.assertRaises(rd.Failure) as caught:
            rd.texture_of(by_id[15])
        self.assertEqual(caught.exception.detail, "read")
        extra = rd.texture_of(by_id[16])
        self.assertEqual((type(extra).__name__, extra.m_Extra), ("Texture2D", 7))

        class Stream:
            """An `m_StreamData` of these values."""

            def __init__(self, path, offset, size):
                self.path, self.offset, self.size = path, offset, size

        class Changed:
            """`obj`, its read's value with `changes` set on it."""

            def __init__(self, obj, **changes):
                self.obj, self.changes, self.serialized_type = obj, changes, obj.serialized_type

            def read(self, check_read=True):
                value = self.obj.read(check_read=check_read)
                for name, v in self.changes.items():
                    setattr(value, name, v)
                return value

        for changes, ok in (
            ({"image_data": "x"}, False),
            ({"image_data": 4}, False),
            ({"m_PlatformBlob": 3}, False),
            ({"m_PlatformBlob": []}, True),
            ({"m_PlatformBlob": None}, True),
            ({"m_Width": 1.0}, False),
            ({"m_Height": True}, False),
            ({"m_TextureFormat": 1.0}, False),
            ({"m_StreamData": Stream(3, 0, 4)}, False),
            ({"m_StreamData": Stream("", "0", 4)}, False),
            ({"m_StreamData": Stream("", 0, None)}, False),
            ({"m_StreamData": Stream("", 0, 4)}, True),
        ):
            with self.subTest(changes=changes):
                if ok:
                    rd.texture_of(Changed(by_id[16], **changes))
                    continue
                with self.assertRaises(rd.Failure) as caught:
                    rd.texture_of(Changed(by_id[16], **changes))
                self.assertEqual(caught.exception.detail, "read")

    def test_an_object_with_no_typetree_is_not_read(self):
        """Version 1 configures no typetree provider: `layouts`' CAB-x11c stores no typetrees, and UnityPy reads its
        MonoScript by the TPK, where the core's read is `NoTypeTree`. So its MonoBehaviour's `m_Script` read fails:
        unresolved, counted."""
        scene = ngui_scenes.scenes()["layouts"]
        bundle = rd.load(scene.view("x.bundle"), "x.bundle")
        by_id = {o.path_id: o for o in objects(bundle) if o.assets_file.name == "CAB-x11c"}
        self.assertEqual(by_id[1].read().m_ClassName, "NGUIAtlas")
        with self.assertRaises(rd.Failure) as caught:
            rd.typed(by_id[1])
        self.assertEqual((caught.exception.reason, caught.exception.detail), ("decode_error", "asset_bundle"))
        self.assertEqual(rd.recognise(by_id[2], ("NGUIAtlas",)), ("unresolved", None))

    def test_a_name_read_of_no_typetree_and_of_no_name_field(self):
        """The core reads a name through the object's typetree, so a texture with none owns no name, where UnityPy
        peeks it by the TPK; a Material with none is not read, where UnityPy reads its `_MainTex` by the TPK. A type
        with no name field is a read failure, decided from its own nodes."""
        scene = ngui_scenes.scenes()["failed_reads"]
        bundle = rd.load(scene.view("x.bundle"), "x.bundle")
        by_id = {(o.assets_file.name, o.path_id): o for o in objects(bundle)}
        stripped, tlmat = by_id[("CAB-x15d", 1)], by_id[("CAB-x15d", 2)]
        self.assertEqual(stripped.peek_name(), "tl")
        with self.assertRaises(rd.Failure) as caught:
            rd.name_of(stripped)
        self.assertEqual((caught.exception.reason, caught.exception.detail), ("decode_error", "asset_bundle"))
        envs = tlmat.read().m_SavedProperties.m_TexEnvs
        self.assertEqual([(name, env.m_Texture.m_PathID) for name, env in envs], [("_MainTex", 3)])
        atlas = by_id[("CAB-x15e", 6)]
        self.assertIsNone(rd.maintex_name(rd.atlas_tree(atlas), atlas))
        nameless = by_id[("CAB-x15", 29)]
        self.assertIsNone(nameless.peek_name())
        with self.assertRaises(rd.Failure) as caught:
            rd.name_of(nameless)
        self.assertEqual(caught.exception.detail, "read")

    def test_a_pointer_s_registry_lookup_finds_no_container(self):
        """`fallback_rows`' `CAB-dm` and `CAB-dn`: UnityPy's `get_cab` answers with the SerializedFile, which a
        container row of the same name, later in its bundle or in a later dependency, takes the key of in 4.3's
        registry, as in the core's: `find_file` gives None, and the pointer is `outside_view(<cab>)`."""
        scene = ngui_scenes.scenes()["fallback_rows"]
        bundle = rd.load(scene.view("x.bundle"), "x.bundle")
        env = next(o for o in objects(bundle)).assets_file.environment
        by_id = {o.path_id: o for o in objects(bundle) if o.assets_file.name == "CAB-x22"}
        for cab, atlas, script in (("CAB-dm", 6, 7), ("CAB-dn", 8, 9)):
            with self.subTest(cab=cab):
                self.assertIsInstance(env.get_cab(cab), SerializedFile)
                self.assertIsInstance(getattr(env, rd.REGISTRY)[cab.lower()], BundleFile)
                self.assertIsNone(env.find_file(cab))
                tree = rd.atlas_tree(by_id[atlas])
                with self.assertRaises(rd.Failure) as caught:
                    rd.deref_value(tree["material"], by_id[atlas].assets_file)
                self.assertEqual(caught.exception.reason, f"outside_view({cab})")
                self.assertIsNone(rd.maintex_name(tree, by_id[atlas]))
                self.assertEqual(rd.recognise(by_id[script], ("NGUIAtlas",)), ("unresolved", None))
        # A container two levels down is no key; a raw row is returned as is (`bad_pointer` at its `objects`).
        keys = getattr(env, rd.REGISTRY)
        self.assertNotIsInstance(keys["w.ress"], (BundleFile, SerializedFile))
        self.assertIs(env.find_file("w.resS"), keys["w.ress"])

    def test_the_view_s_registry_keys_a_container_by_its_name(self):
        """`fallback_rows`: UnityPy's registry gives a container no key, so it answers `r.resS` with the
        dependency's earlier raw row; 4.3's registry, as the core keeps it, holds the container under the key, and
        the read moves on to the next candidate. A container's own entries register after its key."""
        scene = ngui_scenes.scenes()["fallback_rows"]
        bundle = rd.load(scene.view("x.bundle"), "x.bundle")
        env = next(o for o in objects(bundle)).assets_file.environment
        self.assertEqual(bytes(rd._get_resource_data(env.get_cab("r.resS"), 0, 1)), b"\x81")
        keys = getattr(env, rd.REGISTRY)
        self.assertIsInstance(keys["r.ress"], BundleFile)
        self.assertEqual(bytes(rd._get_resource_data(keys["n.ress"], 0, 1)), b"\x83")
        dr = next(o for o in objects(bundle) if o.path_id == 2)
        with self.assertRaises(rd.Failure) as caught:
            rd.decode(dr.read(), dr)
        self.assertEqual(caught.exception.reason, "outside_view(archive:/CAB-x22/r.resS)")

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
        # An area past 64 bits: 2^33 x 2^31 = 2^64, which a 64-bit product wraps to 0; Pillow refuses it first.
        with self.assertRaises(Image.DecompressionBombError):
            im.crop((0, 0, 2**33, 2**31))
        self.assertEqual(rd.crop_refusal(0, 0, 2**33, 2**31), "decompression_bomb")
        for box, rect in (
            ((2**31 - 1, 0, 2**31, 1), (2**31 - 1, 0, 1, 1)),
            ((-(2**31) - 1, 0, -(2**31), 1), (-(2**31) - 1, 0, 1, 1)),
            ((0, 2**31 - 1, 1, 2**31), (0, 2**31 - 1, 1, 1)),
            ((0, -(2**31) - 1, 1, -(2**31)), (0, -(2**31) - 1, 1, 1)),
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
