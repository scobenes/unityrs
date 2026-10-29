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


VERSION = "2021.3.2f1"  # synth.serialized_file's default engine version


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
        for size, error, message in (
            (200, Exception, "^String not terminated$"),
            (32766, Exception, "^String not terminated$"),
            (32768, ValueError, "^Invalid WebFile signature: 'efgh"),
            (40000, ValueError, "^Invalid WebFile signature: 'efgh"),
        ):
            nul_free = bytes(random.Random(5).choices(b"abcdefgh", k=size))
            with self.subTest(size=size), self.assertRaisesRegex(error, message) as caught:
                load(unity_brotli(nul_free))
            self.assertIs(type(caught.exception), error)
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
        # Anything else goes through load_file. A stream with a `name` is keyed by that name and registered as a
        # CAB under its basename, so a PPtr to that CAB resolves.
        with open(d / "a.assets", "rb") as named:
            e = quietly(UnityPy.Environment, named)
        self.assertEqual((list(e.files), list(e.cabs)), ([str(d / "a.assets")], ["a.assets"]))
        # Anything else is keyed by str(obj.__hash__()) (section 19.2): bytes and a read-only memoryview by the
        # bytes' hash, a nameless stream by its own id-based hash.
        data = text_asset_file("q")
        stream = io.BytesIO(data)
        for arg, key in ((data, hash(data)), (memoryview(data), hash(data)), (stream, hash(stream))):
            with self.subTest(type(arg).__name__):
                e = quietly(UnityPy.Environment, arg)
                self.assertEqual((list(e.files), list(e.cabs)), ([str(key)], [str(key)]))
        self.assertNotEqual(str(hash(stream)), str(hash(io.BytesIO(data))), "equal streams, two keys")
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
        with mock.patch("builtins.open", tracked):
            quietly(e.load_file, str(d / "a.assets"), name="ignored")
            quietly(e.load_file, str(d / "sp" / "s.assets.split0"), name="ignored too")
        self.assertEqual(
            {k.replace(str(d), "D"): type(f).__name__ for k, f in e.files.items()},
            {
                "custom": "SerializedFile",
                "from a bytearray": "SerializedFile",
                "D/a.assets": "SerializedFile",
                "D/sp/s.assets": "SerializedFile",
            },
        )
        # The bytes key is the process's string hash, which PYTHONHASHSEED randomises: two processes key one
        # input differently, so no port can reproduce it.
        import subprocess

        (d / "q.bin").write_bytes(data)
        code = "import sys, UnityPy; b = open(sys.argv[1], 'rb').read(); print(*UnityPy.Environment(b).files, hash(b))"
        keys = set()
        for seed in ("1", "2"):
            run = subprocess.run(
                [sys.executable, "-c", code, str(d / "q.bin")],
                env=dict(os.environ, PYTHONHASHSEED=seed),
                capture_output=True,
                text=True,
                check=True,
            )
            key, hashed = run.stdout.split()
            self.assertEqual(key, hashed)
            keys.add(key)
        self.assertEqual(len(keys), 2)

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

    # Sections 7.2, 18, 19.1, 19.2: SerializedFile.save by format, its header, and SERIALIZED_FILE_PARSE_TYPETREE.
    def test_serialized_file_save(self):
        from UnityPy import config

        def source(version, **kw):
            type_id = 0 if version >= 16 else 49
            objects = [S.Obj(n, type_id, S.s32(f"o{n}") + S.s32("s" * n)) for n in (9, 2)]
            return S.serialized_file(
                [S.Type(49, TEXT_ASSET, deps=[3])],
                objects,
                version=version,
                script_types=[(0, 5)],
                externals=[S.External(b"lib/x", b"t", bytes(range(16)), 2)],
                user_information=b"u",
                **kw,
            )

        def flag(value):
            config.SERIALIZED_FILE_PARSE_TYPETREE = value
            self.addCleanup(setattr, config, "SERIALIZED_FILE_PARSE_TYPETREE", True)

        expected = [(2, {"m_Name": "o2", "m_Script": "ss"}), (9, {"m_Name": "o9", "m_Script": "s" * 9})]
        # Every save of a format below 10, or of 11, raises: SerializedType.write calls serialized_file.dump, which
        # does not exist. 10 and 12 on write the tree as a blob, and the file reloads with every member.
        for version in range(1, 23):
            with self.subTest(version=version, tree_flag=True):
                sf = load(source(version))[0]
                if version < 10 or version == 11:
                    with self.assertRaisesRegex(AttributeError, "'SerializedFile' object has no attribute 'dump'"):
                        sf.save()
                    continue
                back = load(sf.save())[0]
                # Objects sorted by path id; each member a format reads is written back.
                self.assertEqual([(o.path_id, o.parse_as_dict()) for o in back.objects.values()], expected)
                self.assertEqual((back.externals[0].path, back.externals[0].guid), ("lib/x", bytes(range(16))))
                self.assertEqual(len(back.script_types), 1 if version >= 11 else 0)
                self.assertEqual(back.types[0].type_dependencies, (3,) if version >= 21 else None)
                self.assertEqual((back.userInformation, back.unity_version), ("u", VERSION))
                self.assertEqual(sf.save(packer="lz4"), sf.save(), "packer is ignored")
        # Format 22's header: zeros in the legacy metadata size, file size and data offset, the version kept;
        # then the endian byte, reserved bytes, u32 metadata size, i64 file size and data offset (aligned to 16),
        # and the file's own unknown i64. Earlier formats keep the four u32 fields.
        raw = bytearray(source(22))
        struct.pack_into(">q", raw, 40, 0x1122334455667788)
        out = load(bytes(raw))[0].save()
        meta_size, file_size, data_offset, unknown = struct.unpack_from(">Iqqq", out, 20)
        self.assertEqual(struct.unpack_from(">IIII", out), (0, 0, 22, 0))
        self.assertEqual((file_size, data_offset % 16, unknown, out[16]), (len(out), 0, 0x1122334455667788, 0))
        meta, size, version, offset = struct.unpack_from(">IIII", load(source(17))[0].save())
        self.assertEqual((version, offset % 16), (17, 0))
        # Each object's data starts on an 8-byte boundary from the data offset.
        back = load(out)[0]
        self.assertEqual([o.byte_start - back.header.data_offset for o in back.objects.values()], [0, 16])

        # SERIALIZED_FILE_PARSE_TYPETREE false: the trees are still read, and objects parse with them; save then
        # writes the tree flag false and no trees, so from format 13 the file reloads through the TPK.
        flag(False)
        sf = load(source(22))[0]
        self.assertIsNotNone(sf.types[0].node)
        self.assertEqual(sf.objects[2].parse_as_dict(), expected[0][1])
        out = sf.save()
        flag(True)
        back = load(out)[0]
        self.assertEqual((back._enable_type_tree, back.types[0].node), (False, None))
        self.assertEqual([(o.path_id, o.parse_as_dict()) for o in back.objects.values()], expected)
        # Below 13 there is no flag: the save drops the trees all the same, and the file, whose reader expects
        # them, does not reload (from 9 on its header is written as usual).
        for version in (10, 12):
            with self.subTest(version=version, tree_flag=False):
                flag(False)
                out = load(source(version))[0].save()
                flag(True)
                with self.assertRaises(IndexError):
                    load(out)
        # Below 9 the header names data offset 32, while the data follows the 16-byte header at 16; then come
        # the endian byte and the metadata. UnityPy does not read it back: below 5 the file loads as a raw reader.
        # (From 5 the load misreads sizes and allocates a gigabyte or more before it fails, so it is not run here.)
        for version, reload in ((4, True), (8, False)):
            with self.subTest(version=version, tree_flag=False):
                flag(False)
                out = load(source(version))[0].save()
                flag(True)
                meta, size, version_field, offset = struct.unpack_from(">IIII", out)
                self.assertEqual((version_field, offset, size, out[16:24]), (version, 32, len(out), S.s32("o2")))
                self.assertEqual(out[size - meta], 0, "the endian byte opens the metadata")
                if reload:
                    self.assertIsInstance(load(out)[0], EndianBinaryReader)

    # Section 18: BundleFile.save's packer tuple, its compressed packers, and save_web_raw.
    def test_bundle_save_packers(self):
        import lz4.block

        payload = text_asset_file("p", "q" * 300)
        bundle = load(S.fs_bundle([(b"CAB-a", payload, 4)]))[0]

        def flags(data):
            head = data.index(b"2021.3.2f1\0") + 11
            return struct.unpack_from(">I", data, head + 16)[0]

        def block_flags(saved):
            back = load(saved)[0]
            return (
                back.dataflags,
                back._block_info_flags,
                [o.parse_as_dict()["m_Name"] for o in back.files["CAB-a"].objects.values()],
            )

        # A tuple is (data_flag, block_info_flag): the header's flags first, the blocks' codec second.
        saved = bundle.save(packer=(0x43, 2))
        self.assertEqual((flags(saved), block_flags(saved)[1:]), (0x43, (2, ["p"])))
        self.assertEqual(flags(bundle.save(packer=(0x41, 3))), 0x41)
        # lz4 is (194, 2): the blocks info LZ4 at the end, LZ4 blocks of liblz4 HC level 9; lzma (65, 1).
        saved = bundle.save(packer="lz4")
        self.assertEqual((flags(saved), block_flags(saved)[1:]), (0xC2, (2, ["p"])))
        entry = bundle.files["CAB-a"].save()  # every SerializedFile entry is saved again
        self.assertIn(lz4.block.compress(entry, mode="high_compression", compression=9, store_size=False), saved)
        self.assertEqual(flags(bundle.save(packer="lzma")), 0x41)
        # lz4hc, the HEAD packer, is unknown here. HEAD writes it as save_fs(195, 3), which 1.25.2 writes for the
        # tuple (195, 3): header flags 0xC3, blocks flagged 3, compressed with liblz4 HC level 9 as for lz4.
        with self.assertRaisesRegex(NotImplementedError, "lz4hc"):
            bundle.save(packer="lz4hc")
        saved = bundle.save(packer=(195, 3))
        self.assertEqual((flags(saved), block_flags(saved)[1:]), (0xC3, (3, ["p"])))
        self.assertIn(lz4.block.compress(entry, mode="high_compression", compression=9, store_size=False), saved)
        # save_web_raw: UnityWeb and UnityRaw other than format 6 save through it, formats 1 to 3 only.
        from test_dump import unityraw

        def old_raw(fmt, name, data):
            """UnityRaw formats 1 to 3: no hash or crc; sizes, then the complete size from 2, the info size from 3."""
            head = b"UnityRaw\0" + struct.pack(">I", fmt) + b"5.x.x\0" + b"2.6.1f1\0"
            block = struct.pack(">i", 1) + name + b"\0" + struct.pack(">II", 4 + len(name) + 9, len(data)) + data
            fields = 24 + 4 * (fmt >= 2) + 4 * (fmt >= 3)
            size = len(head) + fields
            out = head + struct.pack(">IIIiII", size + len(block), size, 1, 1, len(block), len(block))
            out += (struct.pack(">I", size + len(block)) if fmt >= 2 else b"") + (
                struct.pack(">I", 0) if fmt >= 3 else b""
            )
            return out + block

        for fmt in (1, 2, 3, 4, 5, 7):
            with self.subTest(format=fmt):
                raw = load(old_raw(fmt, b"CAB-r", payload) if fmt <= 3 else unityraw(fmt, [(b"CAB-r", payload)]))[0]
                if fmt > 3:
                    with self.assertRaisesRegex(NotImplementedError, "version > 3 is not supported"):
                        raw.save()
                    continue
                back = load(raw.save())[0]
                self.assertEqual([o.parse_as_dict()["m_Name"] for o in back.files["CAB-r"].objects.values()], ["p"])

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

    # Section 5.1: BlocksInfoAtTheEnd (0x80).
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
