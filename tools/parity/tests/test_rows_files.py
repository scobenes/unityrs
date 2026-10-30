"""PARITY.md rows on files, run against the oracle: loading, bundles, codecs, SerializedFile formats and WebFile.save;
the other writing rows are in `test_rows_writing.py`.

Sections 3 to 7, 12, 18 and 20. Like `test_parity_rows.py`, every input is built here (`synth.py`), so
nothing needs the corpus, and each test names the rows it pins; a row's test column names the test back.
Needs UnityPy 1.25.2 (the oracle image); skipped elsewhere.
"""

import gzip
import ntpath
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

from rows_support import ORACLE, TEXT_ASSET, load, quietly, text_asset_file  # noqa: E402

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
                    # userInformation from 5; below, the attribute is never set (not set to None).
                    self.assertEqual(hasattr(sf, "userInformation"), version >= 5)
                    if version >= 5:
                        self.assertEqual(sf.userInformation, "user")
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
        # Content with no NUL: the memory reader reaches the end of content of at most 32767 bytes and raises a
        # plain Exception, while longer content gives a 32767-byte signature and the ValueError.
        for size, seed, error, message in (
            (200, 5, Exception, "^String not terminated$"),
            (32766, 5, Exception, "^String not terminated$"),
            (32767, 1, Exception, "^String not terminated$"),  # seed 5 at this size is stored, which the trick refuses
            (32768, 5, ValueError, "^Invalid WebFile signature: 'efgh"),
            (40000, 5, ValueError, "^Invalid WebFile signature: 'efgh"),
        ):
            nul_free = bytes(random.Random(seed).choices(b"abcdefgh", k=size))
            with self.subTest(size=size), self.assertRaisesRegex(error, message) as caught:
                load(unity_brotli(nul_free))
            self.assertIs(type(caught.exception), error)
        # The boundary is the memory reader's: no NUL in 32767 bytes raises, and from 32768 it returns 32767.
        with self.assertRaisesRegex(Exception, "^String not terminated$"):
            EndianBinaryReader(b"B" * 32767).read_string_to_null()
        self.assertEqual(EndianBinaryReader(b"B" * 32768).read_string_to_null(), "B" * 32767)
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
        # Anything else goes through load_file. A stream with a `name` is keyed by that name and registered as a
        # CAB under its basename, so a PPtr to that CAB resolves.
        with open(d / "a.assets", "rb") as named:
            e = quietly(UnityPy.Environment, named)
        self.assertEqual((list(e.files), list(e.cabs)), ([str(d / "a.assets")], ["a.assets"]))
        # A stream whose `name` is not a str is keyed by that value too: a TemporaryFile's file descriptor (an
        # int), a SpooledTemporaryFile's None. A bundle or WebFile loads under it; a SerializedFile raises
        # AttributeError (parse_file's `name.endswith`) and a raw file TypeError (register_cab's basename). With
        # `name=` each loads.
        bundle, serialized = S.fs_bundle([(b"CAB-a", text_asset_file("in bundle"), 4)]), text_asset_file("t")
        web = S.web_file([(b"cab-w", text_asset_file("in web"))])
        raw = b"not a unity file, " * 8

        def stream(kind, data):
            made = tempfile.TemporaryFile() if kind == "int" else tempfile.SpooledTemporaryFile()  # noqa: SIM115
            self.addCleanup(made.close)
            made.write(data)
            made.seek(0)
            return made

        for kind, type_name in (("int", "int"), ("None", "NoneType")):
            with self.subTest(name=kind):
                for payload, cab in ((bundle, "cab-a"), (web, "cab-w")):
                    f = stream(kind, payload)
                    e = quietly(UnityPy.Environment, f)
                    self.assertEqual((list(e.files), list(e.cabs)), ([f.name], [cab]))
                    # File.name keeps the basename of a str name only: any other is "".
                    self.assertEqual(e.files[f.name].name, "")
                self.assertIsInstance(f.name, int if kind == "int" else type(None))
                with self.assertRaisesRegex(AttributeError, f"^'{type_name}' object has no attribute 'endswith'$"):
                    quietly(UnityPy.Environment, stream(kind, serialized))
                with self.assertRaisesRegex(TypeError, f"^expected str, bytes or os.PathLike object, not {type_name}$"):
                    quietly(UnityPy.Environment, stream(kind, raw))
                kinds = ((serialized, "SerializedFile"), (raw, "EndianBinaryReader"), (bundle, "BundleFile"))
                for payload, loaded in kinds:
                    e = UnityPy.Environment()
                    quietly(e.load_file, stream(kind, payload), name="given")
                    self.assertEqual(list(e.files), ["given"])
                    self.assertTrue(type(e.files["given"]).__name__.startswith(loaded))
        # A stream opened from a bytes path has a bytes `name`. A bundle, a WebFile and a raw file load under it, the
        # raw file registered under its bytes basename lower-cased (ntpath.basename takes bytes); a SerializedFile
        # raises TypeError, from bytes.endswith given str suffixes.
        named_by_bytes = {}
        for payload, file_name in ((bundle, "B.bundle"), (web, "W.data"), (raw, "Raw.Bin"), (serialized, "S.assets")):
            path = d / "bytes" / file_name
            path.parent.mkdir(exist_ok=True)
            path.write_bytes(payload)
            with open(os.fsencode(path), "rb") as f:
                try:
                    e = quietly(UnityPy.Environment, f)
                except TypeError as error:
                    named_by_bytes[file_name] = str(error)
                    continue
            self.assertEqual(list(e.files), [os.fsencode(path)])
            loaded = e.files[os.fsencode(path)]
            named_by_bytes[file_name] = (list(e.cabs), type(loaded).__name__, getattr(loaded, "name", None))
        self.assertEqual(
            named_by_bytes,
            {
                "B.bundle": (["cab-a"], "BundleFile", ""),
                "W.data": (["cab-w"], "WebFile", ""),
                "Raw.Bin": ([b"raw.bin"], "EndianBinaryReader_Streamable", None),
                "S.assets": "a bytes-like object is required, not 'str'",
            },
        )
        # With a str name, File.name is os.path.basename (posix): a backslash is kept.
        for given, name in (("dir/X.bundle", "X.bundle"), ("a\\b\\C.bundle", "a\\b\\C.bundle")):
            self.assertEqual(quietly(UnityPy.Environment().load_file, bundle, name=given).name, name)
        # A file keyed by such a name breaks two later calls (section 19.2). PPtr.deref from a SerializedFile
        # loaded on its own compares `key.lower()` for each Environment key in order until one matches, so an int or
        # None key raises AttributeError unless a matching key comes first: one after the match is never reached,
        # but with no key matching (files loaded by path, keyed by their full paths) every key is reached. A bytes
        # key (bytes have `lower`) never matches. Environment.save of such a file, once changed, raises TypeError:
        # ntpath.basename of an int or None, or joining out_path with a bytes basename.
        from UnityPy.classes import PPtr

        main = S.serialized_file(
            [S.Type(49, TEXT_ASSET)], [S.Obj(1, 0, S.s32("main") + S.s32("s"))], externals=[S.External(b"other.assets")]
        )

        def deref_after(first=None, last=None):
            e = UnityPy.Environment()
            if first is not None:
                quietly(e.load_file, first)
            quietly(e.load_file, text_asset_file("other"), name="other.assets")
            sf = quietly(e.load_file, main, name="main.assets")
            if last is not None:
                quietly(e.load_file, last)
            return PPtr(m_FileID=1, m_PathID=1, assetsfile=sf).deref().parse_as_dict()["m_Name"]

        for kind, type_name in (("int", "int"), ("None", "NoneType")):
            with self.subTest(deref=kind):
                with self.assertRaisesRegex(AttributeError, f"^'{type_name}' object has no attribute 'lower'$"):
                    deref_after(first=stream(kind, bundle))
                self.assertEqual(deref_after(last=stream(kind, bundle)), "other")
        with open(os.fsencode(d / "bytes" / "B.bundle"), "rb") as f:
            self.assertEqual(deref_after(first=f), "other")
        # No key matching: files loaded by path are keyed by their full paths, so the loop reaches every key before
        # the registry finds `other.assets`, and an int or None key raises though it is the last one.
        by_path = d / "by-path"
        by_path.mkdir()
        (by_path / "main.assets").write_bytes(main)
        (by_path / "other.assets").write_bytes(text_asset_file("other"))

        def deref_by_path(last=None):
            e = UnityPy.Environment(path=str(by_path))
            sf = quietly(e.load_file, str(by_path / "main.assets"))
            quietly(e.load_file, str(by_path / "other.assets"))
            if last is not None:
                quietly(e.load_file, last)
            self.assertEqual(list(e.files)[:2], [str(by_path / "main.assets"), str(by_path / "other.assets")])
            return PPtr(m_FileID=1, m_PathID=1, assetsfile=sf).deref().parse_as_dict()["m_Name"]

        self.assertEqual(deref_by_path(), "other")
        for kind, type_name in (("int", "int"), ("None", "NoneType")):
            with (
                self.subTest(deref_by_path=kind),
                self.assertRaisesRegex(AttributeError, f"^'{type_name}' object has no attribute 'lower'$"),
            ):
                deref_by_path(last=stream(kind, bundle))
        out = d / "out"
        out.mkdir()
        for kind, message in (
            ("int", "^expected str, bytes or os.PathLike object, not int$"),
            ("None", "^expected str, bytes or os.PathLike object, not NoneType$"),
            ("bytes", "^sequence item 1: expected str instance, bytes found$"),
        ):
            with self.subTest(save=kind):
                e = UnityPy.Environment()
                if kind == "bytes":
                    with open(os.fsencode(d / "bytes" / "B.bundle"), "rb") as f:
                        quietly(e.load_file, f).mark_changed()
                else:
                    quietly(e.load_file, stream(kind, bundle)).mark_changed()
                (key,) = e.files
                self.assertIsInstance(key, {"int": int, "None": type(None), "bytes": bytes}[kind])
                with self.assertRaisesRegex(TypeError, message):
                    e.save(out_path=str(out))
                # The Tier C (writing) reference: the same bytes loaded with name= the key's str(), or os.fsdecode()
                # of bytes, which UnityPy saves under that string's basename.
                name = os.fsdecode(key) if isinstance(key, bytes) else str(key)
                e = UnityPy.Environment()
                quietly(e.load_file, bundle, name=name).mark_changed()
                (out / kind).mkdir()
                e.save(out_path=str(out / kind))
                self.assertEqual([p.name for p in (out / kind).iterdir()], [ntpath.basename(name)])
                saved = (out / kind / ntpath.basename(name)).read_bytes()
                self.assertEqual(sorted(load(saved)[0].files), sorted(load(bundle)[0].files))
        self.assertEqual(sorted(p.name for p in (out / "None").iterdir()), ["None"])
        self.assertEqual(sorted(p.name for p in (out / "bytes").iterdir()), ["B.bundle"])
        # Anything else is keyed by str(obj.__hash__()) (section 19.2): bytes and a read-only memoryview by the
        # bytes' hash, a nameless stream by its own id-based hash.
        data = text_asset_file("q")
        stream = io.BytesIO(data)
        reader = EndianBinaryReader(data)
        for arg, key in (
            (data, hash(data)),
            (memoryview(data), hash(data)),
            (stream, hash(stream)),
            (reader, hash(reader)),
        ):
            with self.subTest(type(arg).__name__):
                e = quietly(UnityPy.Environment, arg)
                self.assertEqual((list(e.files), list(e.cabs)), ([str(key)], [str(key)]))
        self.assertNotEqual(str(hash(stream)), str(hash(io.BytesIO(data))), "equal streams, two keys")
        self.assertEqual(len(quietly(UnityPy.Environment, data, memoryview(data)).files), 1, "one key, one entry")
        # A bytearray, which UnityPy's own FileSourceType lists, raises: the eagerly evaluated default key calls
        # its __hash__, which is None (Tier B, section 19.2). A writable memoryview cannot be hashed, and a Path
        # is no type the reader takes.
        for arg, error, message in (
            (bytearray(data), TypeError, "^'NoneType' object is not callable$"),
            (memoryview(bytearray(data)), ValueError, "^cannot hash writable memoryview object$"),
            (d / "a.assets", TypeError, "^Unsupported type for EndianBinaryReader"),
        ):
            with self.subTest(type(arg).__name__), self.assertRaisesRegex(error, message):
                quietly(UnityPy.Environment, arg)
        # load_file's `name=` keys a stream or bytes, a bytearray included, which then loads; a path string is
        # keyed by the path as given, or the base path of a `.splitN` name, whatever `name=` says.
        e = UnityPy.Environment()
        quietly(e.load_file, data, name="custom")
        quietly(e.load_file, bytearray(text_asset_file("r")), name="from a bytearray")
        quietly(e.load_file, memoryview(bytearray(text_asset_file("w"))), name="from a writable memoryview")
        with mock.patch("builtins.open", tracked):
            quietly(e.load_file, str(d / "a.assets"), name="ignored")
            quietly(e.load_file, str(d / "sp" / "s.assets.split0"), name="ignored too")
        self.assertEqual(
            {k.replace(str(d), "D"): type(f).__name__ for k, f in e.files.items()},
            {
                "custom": "SerializedFile",
                "from a bytearray": "SerializedFile",
                "from a writable memoryview": "SerializedFile",
                "D/a.assets": "SerializedFile",
                "D/sp/s.assets": "SerializedFile",
            },
        )
        # The bytes key is the process's string hash, which PYTHONHASHSEED randomises: two processes key one
        # input differently, two under one seed alike, and in each the key is str(hash(data)), which the binding
        # computes in the caller's process (Tier A, section 19.2).
        import subprocess

        (d / "q.bin").write_bytes(data)
        code = "import sys, UnityPy; b = open(sys.argv[1], 'rb').read(); print(*UnityPy.Environment(b).files, hash(b))"

        def key_under(seed):
            run = subprocess.run(
                [sys.executable, "-c", code, str(d / "q.bin")],
                env=dict(os.environ, PYTHONHASHSEED=seed),
                capture_output=True,
                text=True,
                check=True,
            )
            key, hashed = run.stdout.split()
            self.assertEqual(key, hashed)
            return key

        self.assertNotEqual(key_under("1"), key_under("2"))
        self.assertEqual(key_under("7"), key_under("7"))

    def chdir(self, path):
        import os

        cwd = os.getcwd()
        os.chdir(path)
        self.addCleanup(os.chdir, cwd)

    # Section 12: UnityCN, on a bundle encrypted here with the inverse of UnityPy's own decryption.
    def test_unitycn_decryption(self):
        import hashlib

        import lz4.block
        from Crypto.Cipher import AES
        from UnityPy.helpers import ArchiveStorageManager as M

        key = b"0123456789abcdef"
        previous = M.DECRYPT_KEY
        self.addCleanup(setattr, M, "DECRYPT_KEY", previous)
        # set_assetbundle_decrypt_key: 16 bytes, a str encoded with surrogateescape; any other length raises.
        M.set_assetbundle_decrypt_key("\udcff" + "k" * 15)
        self.assertEqual(M.DECRYPT_KEY, b"\xff" + b"k" * 15)
        with self.assertRaisesRegex(ValueError, "should be 16 bytes and now is 15 bytes"):
            M.set_assetbundle_decrypt_key(b"k" * 15)
        # decrypt_key: AES-ECB encrypts the key vector with the key, then XORs the data vector.
        aes = AES.new(key, AES.MODE_ECB)
        self.assertEqual(M.decrypt_key(b"v" * 16, bytes(16), key), aes.encrypt(b"v" * 16))

        # A header whose vectors decrypt to the signature and to nibbles whose first 16 are a permutation, so
        # each decrypted byte has exactly one encrypted form.
        table = bytes([7, 3, 12, 0, 15, 9, 1, 14, 4, 11, 6, 2, 13, 8, 10, 5])
        nibbles = table + bytes(i & 0xF for i in range(5, 21))
        plain_key = bytes((a << 4 | (b & 0xF)) for a, b in zip(nibbles[::2], nibbles[1::2], strict=True))
        key_vec, sig_key_vec = b"K" * 16, b"S" * 16
        vectors = (bytes(a ^ b for a, b in zip(aes.encrypt(key_vec), plain_key, strict=True)), key_vec)
        signature = (
            bytes(a ^ b for a, b in zip(aes.encrypt(sig_key_vec), M.UNITY3D_SIGNATURE, strict=True)),
            sig_key_vec,
        )
        header = struct.pack(">I", 0x11223344) + vectors[0] + vectors[1] + b"\0" + signature[0] + signature[1] + b"\0"

        def decryptor(data=header):
            return M.ArchiveStorageDecryptor(EndianBinaryReader(data))

        M.DECRYPT_KEY = None
        with self.assertRaisesRegex(LookupError, "no key was provided"):
            decryptor()
        M.DECRYPT_KEY = b"another key 16 b"
        with self.assertRaisesRegex(Exception, "^Invalid signature"):
            decryptor()
        # decrypt_key imports pycryptodome's AES when it runs: without pycryptodome no UnityCN bundle loads.
        no_crypto = mock.patch.dict(sys.modules, {"Crypto": None, "Crypto.Cipher": None})
        with no_crypto, self.assertRaisesRegex(ModuleNotFoundError, "^import of Crypto"):
            decryptor()
        M.DECRYPT_KEY = key
        # read_vector: 16 data bytes, 16 key bytes, one skipped byte; the header is 4 + 33 + 33 bytes.
        reader = EndianBinaryReader(header)
        dec = M.ArchiveStorageDecryptor(reader)
        self.assertEqual((reader.Position, dec.unknown_1), (70, 0x11223344))
        self.assertEqual(
            (dec.index, dec.substitute), (table, bytes(nibbles[16 + i * 4 + j] for j in range(4) for i in range(4)))
        )

        def encrypt(plain, index):
            """The bytes decrypt_block turns into `plain`: its own walk, each byte replaced by its preimage."""
            enc = M.ArchiveStorageDecryptor.__new__(M.ArchiveStorageDecryptor)
            enc.index, enc.substitute = dec.index, dec.substitute

            def decrypt_byte(view, offset, idx):
                want = view[offset]
                for candidate in range(256):
                    probe = bytearray([candidate])
                    M.ArchiveStorageDecryptor.decrypt_byte(dec, probe, 0, idx)
                    if probe[0] == want:
                        view[offset] = candidate
                        return want, offset + 1, idx + 1
                raise AssertionError("no preimage")

            enc.decrypt_byte = decrypt_byte
            with mock.patch.object(M, "UnityPyBoost", None):
                return bytes(enc.decrypt_block(plain, index))

        # decrypt_block: UnityPyBoost's C loop and the Python loop agree, and undo `encrypt`.
        payload = text_asset_file("unity cn", "x" * 3000)
        blocks = [lz4.block.compress(part, store_size=False) for part in (payload[:2000], payload[2000:])]
        for index, block in enumerate(blocks):
            cipher = encrypt(block, index)
            self.assertNotEqual(cipher, block)
            self.assertEqual(bytes(dec.decrypt_block(cipher, index)), block)
            with mock.patch.object(M, "UnityPyBoost", None):
                self.assertEqual(bytes(dec.decrypt_block(cipher, index)), block)
        noise = random.Random(9).randbytes(4096)
        with mock.patch.object(M, "UnityPyBoost", None):
            python = bytes(dec.decrypt_block(noise, 3))
        self.assertEqual(bytes(dec.decrypt_block(noise, 3)), python)
        self.assertEqual(hashlib.sha256(python).hexdigest()[:16], "669156c9d687bd03", "the bytes the loop gives")

        # A bundle: the decryptor header follows the flags, and each LZ4 block flagged 0x100 is decrypted with
        # its block index; a stored block with that flag is read as it is. `info_index` encrypts the compressed
        # blocks info under that index.
        def bundle(block_rows, flags=0x1043, info_index=None):
            info = bytes(16) + struct.pack(">i", len(block_rows))
            data = b""
            for stored, packed, block_flags in block_rows:
                info += struct.pack(">IIH", stored, len(packed), block_flags)
                data += packed
            info += struct.pack(">i", 1) + struct.pack(">qqI", 0, len(payload), 4) + b"CAB-cn\0"
            packed_info = lz4.block.compress(info, store_size=False)
            if info_index is not None:
                packed_info = encrypt(packed_info, info_index)
            head = b"UnityFS\0" + struct.pack(">I", 8) + b"5.x.x\0" + b"2021.3.2f1\0"
            out = bytearray(head + struct.pack(">qIII", 0, len(packed_info), len(info), flags) + header)
            out += bytes(-len(out) % 16)
            out += packed_info + data
            struct.pack_into(">q", out, len(head), len(out))
            return bytes(out)

        rows = [(2000, encrypt(blocks[0], 0), 0x102), (len(payload) - 2000, encrypt(blocks[1], 1), 0x102)]
        loaded = load(bundle(rows))[0]
        self.assertEqual([o.parse_as_dict()["m_Name"] for o in loaded.files["CAB-cn"].objects.values()], ["unity cn"])
        stored = load(bundle([(len(payload), payload, 0x100)]))[0]
        self.assertEqual(list(stored.files["CAB-cn"].objects), [1])
        # Encrypted under one index and read under another, the block does not decompress.
        swapped = [(2000, encrypt(blocks[0], 1), 0x102), rows[1]]
        with self.assertRaises(lz4.block.LZ4BlockError):
            load(bundle(swapped))
        # With 0x100 in the header's own flags too, the compressed blocks info is decrypted before it is
        # decompressed, at index 0: left plain, or encrypted under another index, it does not decompress.
        loaded = load(bundle(rows, flags=0x1143, info_index=0))[0]
        self.assertEqual([o.parse_as_dict()["m_Name"] for o in loaded.files["CAB-cn"].objects.values()], ["unity cn"])
        for info_index in (None, 1):
            with self.subTest(info_index=info_index), self.assertRaises(lz4.block.LZ4BlockError):
                load(bundle(rows, flags=0x1143, info_index=info_index))
        with self.assertRaises(lz4.block.LZ4BlockError):
            load(bundle(rows, info_index=0))  # without the flag, the encrypted info is read as it is

    # Sections 7.1, 20: what 1.25.2 does with formats it does not know.
    def test_formats_past_22_read_as_22(self):
        for version in (23, 24, 25, 26, 27, 100):
            with self.subTest(version=version):
                sf = load(
                    S.serialized_file([S.Type(49, TEXT_ASSET)], [S.Obj(1, 0, S.s32("n") + S.s32("s"))], version=version)
                )[0]
                self.assertEqual(
                    (sf.header.version, sf.objects[1].parse_as_dict()), (version, {"m_Name": "n", "m_Script": "s"})
                )
        # Above 100 the sanity check fails, and the file is a raw reader.
        raw = load(S.serialized_file([S.Type(49, TEXT_ASSET)], [S.Obj(1, 0, S.s32("n") + S.s32("s"))], version=101))[0]
        self.assertIsInstance(raw, EndianBinaryReader)
        # Bundle format 9 is read with format 8's layout.
        nine = bytearray(S.fs_bundle([(b"CAB-a", text_asset_file("nine"), 4)]))
        struct.pack_into(">I", nine, 8, 9)
        b = load(bytes(nine))[0]
        self.assertEqual(
            (b.version, [o.parse_as_dict()["m_Name"] for o in b.files["CAB-a"].objects.values()]), (9, ["nine"])
        )

    # Sections 7.2, 19.2: SerializedFile.load_dependencies.
    def test_load_dependencies(self):
        d = self.tmp
        (d / "dep.assets").write_bytes(text_asset_file("dep"))
        (d / "extra.assets").write_bytes(text_asset_file("extra"))
        externals = [S.External(b"dep.assets"), S.External(b"archive:/CAB-missing/CAB-missing")]
        main = S.serialized_file(
            [S.Type(49, TEXT_ASSET)], [S.Obj(1, 0, S.s32("main") + S.s32("s"))], externals=externals
        )
        env = UnityPy.Environment(path=str(d))
        sf = load(main, "main.assets", env)[0]
        # Every external by its path, then the extra names; a miss is None, with no error.
        self.assertIsNone(quietly(sf.load_dependencies, ["extra.assets", "nope.assets"]))
        self.assertEqual(list(env.files), ["main.assets", "dep.assets", "extra.assets"])
        # load_file(path, True) passes True as `parent`, which load_file never hands on (parse_file gets the
        # Environment), so what it loads is no dependency: it joins Environment.objects and the CAB registry.
        dep = env.files["dep.assets"]
        self.assertEqual((dep.parent, dep.is_dependency), (env, False))
        self.assertEqual([o.parse_as_dict()["m_Name"] for o in env.objects], ["main", "dep", "extra"])
        self.assertEqual(list(env.cabs), ["main.assets", "dep.assets", "extra.assets"])
        # Only the extras are guarded: an external whose lookup raises (a `path` the disk cannot list) escapes.
        lone = load(main, "main.assets", UnityPy.Environment(path=""))[0]
        with self.assertRaises(FileNotFoundError):
            quietly(lone.load_dependencies)
        # An external's `name` is ntpath.basename of its path: it splits at a backslash too, where File.name keeps
        # one (section 5.2).
        paths = (b"Library\\unity default resources", b"a/b\\C.assets", b"archive:/CAB-x/CAB-x", b"x/")
        named = S.serialized_file(
            [S.Type(49, TEXT_ASSET)], [S.Obj(1, 0, S.s32("n") + S.s32("s"))], externals=[S.External(p) for p in paths]
        )
        externals = load(named, "named.assets", UnityPy.Environment())[0].externals
        self.assertEqual([e.path for e in externals], [p.decode() for p in paths])
        self.assertEqual([e.name for e in externals], ["unity default resources", "C.assets", "CAB-x", ""])

    # Section 5.1: BlocksInfoAtTheEnd (0x80).
    # Section 5.1: the flag set a bundle's flags are read with, by engine version, each boundary from both sides.
    def test_archive_flags_by_engine_version(self):
        old = ["2019.4.40f1", "2020.1.0f1", "2020.3.33f1", "2021.2.0f1", "2021.3.1f1", "2022.1.0f1"]
        new = ["2020.3.34f1", "2021.3.2f1", "2022.1.1f1", "2022.2.0f1", "2023.1.0f1", "6000.0.0f1"]
        for engine in old + new:
            bundle = load(S.fs_bundle([(b"CAB-a", text_asset_file(), 4)], engine=engine.encode()))[0]
            with self.subTest(engine):
                self.assertEqual(
                    type(bundle.dataflags).__name__, "ArchiveFlagsOld" if engine in old else "ArchiveFlags"
                )

    def test_blocks_info_at_the_end(self):
        import lz4.block

        entry = text_asset_file("at the end")
        packed = lz4.block.compress(entry, store_size=False)
        info = bytes(16) + struct.pack(">i", 1) + struct.pack(">IIH", len(entry), len(packed), 2)
        info += struct.pack(">i", 1) + struct.pack(">qqI", 0, len(entry), 4) + b"CAB-end\0"
        packed_info = lz4.block.compress(info, store_size=False)
        head = b"UnityFS\0" + struct.pack(">I", 8) + b"5.x.x\0" + b"2021.3.2f1\0"

        def bundle(trailer=b""):
            out = bytearray(head + struct.pack(">qIII", 0, len(packed_info), len(info), 0x80 | 0x40 | 2))
            out += bytes(-len(out) % 16)
            out += packed + packed_info  # the data right after the header, the blocks info last
            struct.pack_into(">q", out, len(head), len(out))
            return bytes(out) + trailer

        loaded = load(bundle())[0]
        self.assertEqual(
            [o.parse_as_dict()["m_Name"] for o in loaded.files["CAB-end"].objects.values()], ["at the end"]
        )
        # The info is read from the reader's end, whatever the header's size says: bytes after it break it.
        with self.assertRaises(lz4.block.LZ4BlockError):
            load(bundle(b"\0\0\0\0"))
