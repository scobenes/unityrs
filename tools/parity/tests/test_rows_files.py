"""PARITY.md rows on files, run against the oracle: loading, bundles, codecs, SerializedFile formats and writing.

Sections 3 to 7, 12, 18 and 20. Like `test_parity_rows.py`, every input is built here (`synth.py`), so
nothing needs the corpus, and each test names the rows it pins; a row's test column names the test back.
Needs UnityPy 1.25.2 (the oracle image); skipped elsewhere.
"""

import gzip
import random
import shutil
import struct
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE / "tests"))

from test_parity_rows import ORACLE, TEXT_ASSET, load, quietly, text_asset_file  # noqa: E402

import synth as S  # noqa: E402

if ORACLE:
    import UnityPy
    from UnityPy.enums import BuildTarget
    from UnityPy.streams import EndianBinaryReader


def two_ints(version, endian="<", **kw):
    """A SerializedFile of `version` holding one object (path id 7) of class 49 with two ints, a and b."""
    tree = S.Node("TextAsset", "Base", [S.Node("int", "a", byte_size=4), S.Node("int", "b", byte_size=4)], byte_size=8)
    path_id = kw.pop("path_id", 7)
    obj = S.Obj(path_id, 49, struct.pack(endian + "ii", 11, 22), is_destroyed=kw.pop("is_destroyed", 0))
    return S.serialized_file([S.Type(49, tree)], [obj], version=version, endian=endian, **kw)


def unity_brotli(data, comment=b"UnityWeb Compressed Content (brotli)"):
    """A brotli stream that opens with a metadata meta-block holding `comment`, as Unity's WebGL builds write,
    placed so that `brotli` sits at offset 0x20, where UnityPy looks for it (RFC 7932 sections 9.1, 9.2).

    The compressed stream's meta-blocks move one bit, which only data that brotli compresses survives (the
    padding of an uncompressed meta-block would move), so the result is checked.
    """
    import brotli

    packed = brotli.compress(data, lgwin=16)  # WBITS 16: the stream header is the single bit 0
    rest = int.from_bytes(packed, "little") >> 1  # the meta-blocks without that bit, from the next byte
    rest = rest.to_bytes((len(packed) * 8 - 1 + 7) // 8, "little")
    meta = bytes(0x20 - 2 - comment.index(b"brotli")) + comment
    # Bits, least significant first: WBITS 0; ISLAST 0; MNIBBLES 3 (a metadata block); reserved 0;
    # MSKIPBYTES 1; MSKIPLEN - 1 in 8 bits; zero padding to the byte.
    head = 3 << 2 | 1 << 5 | (len(meta) - 1) << 7
    out = head.to_bytes(2, "little") + meta + rest
    assert brotli.decompress(out) == data, "data brotli stores uncompressed"
    return out


@unittest.skipUnless(ORACLE, "needs UnityPy 1.25.2 (the oracle image)")
class FileRows(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="unityrs-file-rows-"))
        self.addCleanup(shutil.rmtree, self.tmp)

    # Section 7.1: the gates below format 14, and the script types and externals of every format.
    def test_old_format_gates(self):
        external = S.External(b"library/x", b"tmp", bytes(range(16)), 2)
        for version in range(1, 16):
            for endian in "<>":
                with self.subTest(version=version, endian=endian):
                    sf = load(
                        two_ints(
                            version,
                            endian,
                            unity_version=b"4.7.2f1",
                            script_types=[(3, 0x55)],
                            externals=[external],
                            user_information=b"user",
                            is_destroyed=3,
                        )
                    )[0]
                    # Below 9 the endian byte is read at file_size - metadata_size, the metadata's first byte.
                    self.assertEqual(sf.header.endian, endian)
                    obj = sf.objects[7]
                    self.assertEqual(obj.parse_as_dict(), {"a": 11, "b": 22})
                    # Below 11 a u16 is_destroyed follows the class id; from 11 to 16 a script index does.
                    self.assertEqual(obj.is_destroyed, 3 if version < 11 else None)
                    # Script types from 11, each identifier an i32 below 14.
                    self.assertEqual(
                        [(s.local_serialized_file_index, s.local_identifier_in_file) for s in sf.script_types],
                        [(3, 0x55)] if version >= 11 else [],
                    )
                    # Externals: temp_empty from 6, guid and type from 5; below that the slots are unset.
                    ext = sf.externals[0]
                    self.assertEqual(ext.path, "library/x")
                    present = {name: hasattr(ext, name) for name in ("temp_empty", "guid", "type")}
                    self.assertEqual(present, {"temp_empty": version >= 6, "guid": version >= 5, "type": version >= 5})
                    if version >= 6:
                        self.assertEqual((ext.temp_empty, ext.guid, ext.type), ("tmp", bytes(range(16)), 2))
                    if version < 6:
                        with self.assertRaises(AttributeError):
                            ext.temp_empty  # noqa: B018
                    self.assertEqual(getattr(sf, "userInformation", None), "user" if version >= 5 else None)
                    # The platform from 8, the tree flag from 13; below, their defaults.
                    self.assertEqual(
                        sf.target_platform, BuildTarget(19) if version >= 8 else BuildTarget.UnknownPlatform
                    )
                    self.assertEqual(hasattr(sf, "_m_target_platform"), version >= 8)
                    self.assertTrue(sf._enable_type_tree)
                    # The version string from 7. Below it `version` is never set, and ObjectReader.version's
                    # AttributeError falls to ObjectReader.__getattr__, which returns None.
                    self.assertEqual(sf.unity_version, "4.7.2f1" if version >= 7 else "2.5.0f5")
                    self.assertEqual(hasattr(sf, "version"), version >= 7)
                    if version < 7:
                        self.assertIsNone(obj.version)
                    # Old trees are read field by field below 10 and at 11: variable_count only at 2, index and
                    # meta flag everywhere but 3. Blob trees (10, 12 on) give no variable_count.
                    nodes = list(sf.types[0].node.traverse())
                    self.assertEqual(
                        [(n.m_Index, n.m_MetaFlag, n.m_VariableCount) for n in nodes],
                        [(None, None, None)] * 3
                        if version == 3
                        else [(i, 0, 0 if version == 2 else None) for i in range(3)],
                    )

        # big_id_enabled is an i32 from 7 to 13; with it a path id below 14 is an i64, else an i32.
        for version in range(7, 14):
            with self.subTest(version=version, big_id=True):
                sf = load(two_ints(version, big_id_enabled=1, path_id=0x1_0000_0007))[0]
                self.assertEqual((sf.big_id_enabled, list(sf.objects)), (1, [0x1_0000_0007]))
        self.assertEqual(load(two_ints(13))[0].big_id_enabled, 0)
        self.assertEqual(load(two_ints(14))[0].big_id_enabled, 0, "not read from 14")

        # Below 7 an object that falls back to the TPK hands it the None version, which it cannot compare.
        no_tree = S.serialized_file([S.Type(1, TEXT_ASSET)], [S.Obj(7, 49, S.s32("n") + S.s32("s"))], version=6)
        obj = load(no_tree)[0].objects[7]
        self.assertIsNone(obj.serialized_type)
        with self.assertRaisesRegex(NotImplementedError, "^Unsupported comparison$"):
            quietly(obj.parse_as_dict)
        self.assertEqual(
            quietly(
                load(S.serialized_file([S.Type(1, TEXT_ASSET)], [S.Obj(7, 49, S.s32("n") + S.s32("s"))], version=7))[0]
                .objects[7]
                .parse_as_dict
            ),
            {"m_Name": "n", "m_Script": "s"},
        )

    # Sections 4, 6: a brotli WebFile, its stream opening with the metadata block Unity writes.
    def test_brotli_webfile_loads(self):
        import brotli

        web = S.web_file([(b"a.bin", b"hello" * 30), (b"s.assets", text_asset_file("in brotli"))])
        packed = unity_brotli(web)
        self.assertEqual((packed[0x20:0x26], brotli.decompress(packed)), (b"brotli", web))
        wf = load(packed)[0]
        self.assertEqual((type(wf).__name__, wf.packer, list(wf.files)), ("WebFile", "brotli", ["a.bin", "s.assets"]))
        self.assertEqual([o.parse_as_dict()["m_Name"] for o in wf.files["s.assets"].objects.values()], ["in brotli"])
        # Detection looks only at the marker: a brotli stream without it is a raw file, and with it any
        # content is taken for a WebFile, so content that is not one raises.
        self.assertIsInstance(load(brotli.compress(web))[0], EndianBinaryReader)
        text = b"not a web file\0" + bytes(random.Random(5).choices(b"abcdefgh", k=2000))
        with self.assertRaisesRegex(ValueError, "^Invalid WebFile signature: 'not a web file'"):
            load(unity_brotli(text))
        with self.assertRaisesRegex(brotli.error, "decoder failed"):
            load(packed[:0x30] + bytes(200))
        # decompress_brotli and decompress_gzip are the libraries' own.
        from UnityPy.helpers import CompressionHelper as H

        self.assertEqual((H.decompress_brotli(packed), H.decompress_gzip(gzip.compress(web))), (web, web))

    # Sections 18, 19.2: WebFile.save.
    def test_webfile_save(self):
        import brotli

        web = S.web_file([(b"a.bin", b"hello"), (b"s.assets", text_asset_file())], signature=b"TuanjieWebData1.0")
        wf, env = load(web)
        plain = wf.save()
        # The table: the signature given (the default, whatever the file's was), then offset, size and path rows
        # with the data offset first, then each entry's bytes; a SerializedFile entry is saved again.
        resaved = wf.files["s.assets"].save()
        self.assertEqual(plain, S.web_file([(b"a.bin", b"hello"), (b"s.assets", resaved)]))
        self.assertEqual(
            wf.save(signature="X"), S.web_file([(b"a.bin", b"hello"), (b"s.assets", resaved)], signature=b"X")
        )
        # gzip and brotli compress; any other packer writes the table uncompressed, lz4 and lzma included.
        self.assertEqual(gzip.decompress(wf.save(packer="gzip")), plain)
        self.assertEqual(wf.save(packer="brotli"), brotli.compress(plain))
        for packer in ("none", "lz4", "lzma", "LZMA", "original", "anything"):
            with self.subTest(packer=packer):
                self.assertEqual(wf.save(packer=packer), plain)
        # The default "none" never reuses the file's own packer; None or "" does.
        br = load(unity_brotli(web))[0]
        self.assertEqual((br.packer, br.save()), ("brotli", plain))
        self.assertEqual({br.save(packer=None), br.save(packer="")}, {brotli.compress(plain)})
        # Environment.save passes its `pack` on, so pack="lz4" writes a changed WebFile uncompressed.
        wf.mark_changed()
        out = self.tmp / "out"
        out.mkdir()
        env.save(pack="lz4", out_path=str(out))
        self.assertEqual((out / "f.assets").read_bytes(), plain)
        # An EndianBinaryWriter entry (from get_writeable_cab) has no save, so the whole save raises.
        wf.get_writeable_cab("extra.resS").write(b"abc")
        with self.assertRaisesRegex(AttributeError, "'EndianBinaryWriter' object has no attribute 'save'"):
            wf.save()

    # Sections 18, 19.2: File.get_writeable_cab, and SerializedFile.get_writeable_cab, which never returns a writer.
    def test_writeable_cabs(self):
        from UnityPy.streams import EndianBinaryWriter

        bundle = load(S.fs_bundle([(b"CAB-a", text_asset_file(), 4), (b"CAB-a.resS", b"\x11" * 8, 0)]))[0]
        # File.get_writeable_cab: the default name is `cab_file`; flags and endian come from the first .resS.
        writer = bundle.get_writeable_cab()
        self.assertIsInstance(writer, EndianBinaryWriter)
        self.assertEqual((writer.name, writer.flags, writer.endian), ("CAB-UnityPy_Mod.resS", 0, ">"))
        self.assertIs(bundle.files["CAB-UnityPy_Mod.resS"], writer)
        self.assertIs(bundle.get_writeable_cab("CAB-UnityPy_Mod.resS"), writer, "an existing writer is returned")
        with self.assertRaisesRegex(ValueError, "isn't an EndianBinaryWriter"):
            bundle.get_writeable_cab("CAB-a")
        bundle.files["CAB-a.resS"].flags, bundle.files["CAB-a.resS"].endian = 7, "<"
        self.assertEqual((bundle.get_writeable_cab("b.resS").flags, bundle.files["b.resS"].endian), (7, "<"))
        lone = load(S.fs_bundle([(b"CAB-x", text_asset_file(), 4)]))[0]
        self.assertEqual((lone.get_writeable_cab("w").flags, lone.files["w"].endian), (0, ">"), "no .resS: flags 0")
        lone.cab_file = ""
        self.assertIsNone(lone.get_writeable_cab(), "no name and an empty cab_file")

        # SerializedFile.get_writeable_cab: from a bundle or a WebFile the parent's writer is made and its path
        # set, then the external's `__class__` assignment raises TypeError (FileIdentifier is slotted), so no
        # external is registered and nothing is returned; every later call does the same.
        for container in (
            S.fs_bundle([(b"CAB-a", text_asset_file(), 4)]),
            S.web_file([(b"CAB-a", text_asset_file())]),
        ):
            parent = load(container)[0]
            inner = parent.files["CAB-a"]
            for attempt in (1, 2):
                with (
                    self.subTest(type(parent).__name__, attempt=attempt),
                    self.assertRaisesRegex(
                        TypeError,
                        "__class__ assignment: 'FileIdentifier' object layout differs from 'FileIdentifierFake'",
                    ),
                ):
                    inner.get_writeable_cab()
            self.assertEqual(list(parent.files), ["CAB-a", "CAB-UnityPy_Mod.resS"])
            self.assertEqual(parent.files["CAB-UnityPy_Mod.resS"].path, "archive:/CAB-a/CAB-UnityPy_Mod.resS")
            self.assertEqual(inner.externals, [])
        # Under any other parent (an Environment) it returns None and makes nothing.
        self.assertIsNone(load(text_asset_file())[0].get_writeable_cab())

    # Section 3: what the Environment constructor does with each argument.
    def test_environment_constructor_arguments(self):
        import io
        import os
        import zipfile

        d = self.tmp
        (d / "a.assets").write_bytes(text_asset_file("a"))
        (d / "sub").mkdir()
        (d / "sub" / "b.assets").write_bytes(text_asset_file("b"))
        buffer = io.BytesIO()
        with zipfile.ZipFile(buffer, "w") as archive:
            archive.writestr("c.assets", text_asset_file("c"))
        for name in ("x.zip", "y.ZIP", "z.apk"):
            (d / name).write_bytes(buffer.getvalue())
        (d / "sp").mkdir()
        whole = text_asset_file("split")
        (d / "sp" / "s.assets.split0").write_bytes(whole[:50])
        (d / "sp" / "s.assets.split1").write_bytes(whole[50:])

        real_open = open  # load_files opens with the builtin open and never closes: close them here

        def tracked(path, mode="r", *args, **kw):
            stream = real_open(path, mode, *args, **kw)  # noqa: SIM115 - UnityPy reads it after the call
            self.addCleanup(stream.close)
            return stream

        def env(*args, **kw):
            with mock.patch("builtins.open", tracked):
                e = quietly(UnityPy.Environment, *args, **kw)
            return e, {k.replace(str(d), "D"): type(f).__name__ for k, f in e.files.items()}

        cwd = os.getcwd()
        self.assertEqual(env()[0].path, cwd, "a local fs defaults `path` to the cwd")
        # A file sets `path` to its folder and loads it; a `.splitN` name merges its parts through load_files.
        e, files = env(str(d / "a.assets"))
        self.assertEqual((e.path, files, hasattr(e, "file")), (str(d), {"D/a.assets": "SerializedFile"}, True))
        for part in ("s.assets.split0", "s.assets.split1"):
            e, files = env(str(d / "sp" / part))
            self.assertEqual((e.path, files), (str(d / "sp"), {"D/sp/s.assets": "SerializedFile"}))
        # Each file sets `path` again, so the last one wins.
        e, files = env(str(d / "a.assets"), str(d / "sub" / "b.assets"))
        self.assertEqual((e.path, len(files), hasattr(e, "file")), (str(d / "sub"), 2, False))
        # A `.zip` or `.apk` (case-sensitive) goes to load_zip_file, its members keyed by their own names, and
        # leaves `path` alone; `.ZIP` is a file like any other, and a real zip's fifth byte keeps it raw (section 4).
        for name in ("x.zip", "z.apk"):
            e, files = env(str(d / name))
            self.assertEqual((e.path, files), (cwd, {"c.assets": "SerializedFile"}))
        e, files = env(str(d / "y.ZIP"))
        self.assertEqual((e.path, list(files)), (str(d), ["D/y.ZIP"]))
        self.assertIsInstance(e.files[str(d / "y.ZIP")], EndianBinaryReader)
        # A folder sets `path` and loads everything under it, zips as raw files.
        e, files = env(str(d))
        self.assertEqual(e.path, str(d))
        self.assertEqual(
            sorted(files), ["D/a.assets", "D/sp/s.assets", "D/sub/b.assets", "D/x.zip", "D/y.ZIP", "D/z.apk"]
        )
        # A bare file name in the cwd: its folder is "".
        self.chdir(d)
        self.assertEqual(env("a.assets")[0].path, ".")
        # A string that is neither file nor folder is ignored, with no error.
        e, files = env(str(d / "missing"))
        self.assertEqual((e.path, files, hasattr(e, "file")), (os.getcwd(), {}, False))
        # Anything else loads as a stream, keyed by its hash.
        e, files = env(text_asset_file("q"))
        self.assertEqual(list(files.values()), ["SerializedFile"])
        self.assertTrue(next(iter(files)).lstrip("-").isdigit())

    def chdir(self, path):
        import os

        cwd = os.getcwd()
        os.chdir(path)
        self.addCleanup(os.chdir, cwd)
