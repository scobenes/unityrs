"""PARITY.md rows on containers and codecs, run against the oracle: WebFile's gzip and the codec it sniffs,
UnityArchive, LZ4 blocks and the codec helpers, reads past the end, BundleFile.save's flags and encryption mask, and
File's iteration and dict access.

Sections 4 to 6, 12, 13, 18 and 19.2. Every input is built here (`synth.py`), so nothing needs the corpus, and each
test names the rows it pins; a row's test column names the test back. Its class is `Rows`, as in
`test_parity_rows.py`, the module these tests were split from. Needs UnityPy 1.25.2 (the oracle image); skipped
elsewhere.
"""

import gzip
import io
import random
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
    import lz4.block
    import UnityPy
    from UnityPy.enums import ArchiveFlags, ArchiveFlagsOld, ClassIDType, FileType
    from UnityPy.files import BundleFile, WebFile
    from UnityPy.helpers import ImportHelper, ResourceReader
    from UnityPy.helpers.TypeTreeNode import TypeTreeNode
    from UnityPy.streams import EndianBinaryReader


@unittest.skipUnless(ORACLE, "needs UnityPy 1.25.2 (the oracle image)")
class Rows(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="unityrs-rows-"))
        self.addCleanup(lambda: __import__("shutil").rmtree(self.tmp))

    # Sections 4, 5.2, 6: WebFile gzip.
    def test_gzip_webfile_is_detected_and_never_parsed(self):
        plain = S.web_file([(b"a.txt", bytes(range(256)) * 2), (b"b.txt", b"world" * 40)])
        packed = gzip.compress(plain, mtime=0)
        self.assertEqual(list(load(plain)[0].files), ["a.txt", "b.txt"], "a plain WebFile loads")

        # Detection reads the gzip stream through the shared reader and leaves it where it stopped.
        kind, reader = ImportHelper.check_file_type(packed)
        self.assertEqual(kind, FileType.WebFile)
        self.assertNotEqual(reader.Position, 0)
        # So WebFile reads its magic past the start, takes the file for plain, and the signature fails.
        with self.assertRaisesRegex(ValueError, r"^Invalid WebFile signature: '\\x1f\\udc8b\\x08'"):
            load(packed)
        path = self.tmp / "web.data.gz"
        path.write_bytes(packed)
        with self.assertRaisesRegex(ValueError, "Invalid WebFile signature"):
            quietly(UnityPy.load, str(path))
        # Inside a bundle too.
        with self.assertRaisesRegex(ValueError, "Invalid WebFile signature"):
            load(S.fs_bundle([(b"inner.data.gz", packed, 0)]))
        # The decoder itself works on a fresh reader, and agrees with gzip.decompress plus the plain layout.
        self.assertEqual(list(WebFile(EndianBinaryReader(packed), None).files), ["a.txt", "b.txt"])
        self.assertEqual(gzip.decompress(packed), plain)

    # Sections 5.1, 18: UnityArchive.
    def test_unityarchive_loads_as_a_raw_file(self):
        data = S.fs_bundle([(b"CAB-test", text_asset_file(), 4)], signature=b"UnityArchive")
        self.assertEqual(ImportHelper.check_file_type(data)[0], FileType.ResourceFile)
        loaded, env = load(data, name="archive")
        self.assertIsInstance(loaded, EndianBinaryReader)
        self.assertIs(env.get_cab("archive"), loaded)
        with self.assertRaisesRegex(NotImplementedError, "UnityArchive"):
            BundleFile(EndianBinaryReader(data), env)
        # A BundleFile of any other unknown signature raises too (detection never builds one).
        unknown = S.fs_bundle([(b"CAB-test", text_asset_file(), 4)], signature=b"UnityXYZ")
        with self.assertRaisesRegex(NotImplementedError, "^Unknown Bundle signature: UnityXYZ$"):
            BundleFile(EndianBinaryReader(unknown), env)

    # Section 6: LZ4 blocks.
    def test_lz4_stored_size_is_a_capacity(self):
        entries = [(b"CAB-test", text_asset_file(), 4), (b"CAB-test.resS", b"\x11" * 64, 0)]
        size = sum(len(data) for _, data, _ in entries)
        loaded = load(S.fs_bundle(entries, stored_sizes=[size + 100]))[0]
        self.assertEqual(list(loaded.files), ["CAB-test", "CAB-test.resS"], "a block that decodes short loads")
        for stored in (size - 10, 0):
            with self.subTest(stored=stored), self.assertRaises(lz4.block.LZ4BlockError):
                load(S.fs_bundle(entries, stored_sizes=[stored]))
        # A stored size of 0 raises only for output: an empty block, b"\x00" (what lz4 writes for b""), stored as 0
        # decodes to b"", and the bundle loads, the empty block first or last.
        self.assertEqual(lz4.block.compress(b"", store_size=False), b"\x00")
        sf = text_asset_file()
        for blocks in ([sf, b""], [b"", sf]):
            with self.subTest(blocks=[len(block) for block in blocks]):
                loaded = load(S.fs_bundle([(b"CAB-x", sf, 4)], blocks=blocks))[0]
                names = [o.parse_as_dict()["m_Name"] for o in loaded.files["CAB-x"].objects.values()]
                self.assertEqual((list(loaded.files), names), (["CAB-x"], ["hello"]))

    # Sections 5.2, 13: reads past the end.
    def test_reads_past_the_end_return_short_data(self):
        sf = text_asset_file()
        data = sf + b"\x11" * 64
        rows = [(b"CAB-test", 0, len(sf), 4), (b"CAB-test.resS", len(sf), 64 + 1000, 0)]
        loaded = load(S.fs_bundle(rows, blocks=[data], codec=0))[0]
        self.assertEqual(loaded.files["CAB-test.resS"].Length, 64, "the entry is cut to the bytes that exist")
        got = ResourceReader.get_resource_data("CAB-test.resS", loaded.files["CAB-test"], 60, 500)
        self.assertEqual(got, b"\x11" * 4)
        reader = EndianBinaryReader(b"abcd")
        self.assertEqual((reader.read_bytes(10), reader.Position), (b"abcd", 10))
        with self.assertRaises(struct.error):
            reader.read_int()
        # Section 13, negative sizes and positions. The memory reader's Position takes any value, and its reads
        # index the view with it: read(-1) reads to the end, any other size is the slice view[P:P + n].
        data = bytes(range(20))
        reader = EndianBinaryReader(data)

        def read_at(position, size):
            reader.Position = position
            return reader.read(size), reader.Position

        self.assertEqual(read_at(5, 0), (b"", 5))
        self.assertEqual(read_at(5, -1), (data[5:], 20))
        self.assertEqual(read_at(-3, -1), (data[-3:], 20))
        self.assertEqual(read_at(5, -2), (b"", 3))
        self.assertEqual(read_at(2, -5), (data[2:17], -3))
        self.assertEqual(read_at(-3, 2), (data[17:19], -1))
        self.assertEqual(read_at(-2, 4), (b"", 2))
        minus_one = EndianBinaryReader(struct.pack(">i", -1) + data)
        self.assertEqual(minus_one.read_byte_array(), data)
        # A fixed-width primitive at a negative Position counts from the end, and raises when fewer bytes than its
        # size lie from there to the end, or the Position is before the start.
        reader.Position = -4
        self.assertEqual((reader.read_int(), reader.Position), (struct.unpack(">i", data[-4:])[0], 0))
        for position, message in ((-2, "^not enough data to unpack 4 bytes at offset -2$"), (-24, "out of range")):
            reader.Position = position
            with self.subTest(position=position), self.assertRaisesRegex(struct.error, message):
                reader.read_int()
        # The stream reader refuses a negative Position, and its read is the stream's own: -1 reads to the end;
        # below -1 a buffered file raises, while a BytesIO reads to the end.
        stream = EndianBinaryReader(io.BytesIO(data))
        stream.Position = 5
        self.assertEqual((stream.read(-2), stream.Position), (data[5:], 20))
        with self.assertRaisesRegex(ValueError, "^Position cannot be negative$"):
            stream.Position = -1
        path = self.tmp / "data.bin"
        path.write_bytes(data)
        from fsspec.implementations.local import LocalFileSystem

        for opener in (open, LocalFileSystem().open):
            with opener(str(path), "rb") as opened:
                stream = EndianBinaryReader(opened)
                stream.Position = 18
                self.assertEqual(stream.read(-1), data[18:])
                stream.Position = 18
                with self.assertRaisesRegex(ValueError, "^read length must be non-negative or -1$"):
                    stream.read(-7)
        # A file's own signed offsets and sizes reach these rules, at the sites section 13 lists. A bundle's i64
        # directory entries are applied to the blocks' data by the memory reader, from bytes and from a path alike.
        payload = b"0123456789ABCDEF0123456789"
        bundle_path = self.tmp / "negative.bundle"
        for offset, size, expected in (
            (-10, 4, payload[16:20]),
            (2, -1, payload[2:]),
            (2, -5, payload[2:-3]),
            (-30, 4, b""),
            (20, 10, payload[20:]),
        ):
            bundle = S.fs_bundle([(b"x.resS", offset, size, 0)], blocks=[payload])
            bundle_path.write_bytes(bundle)
            for source in (bundle, str(bundle_path)):
                with self.subTest(offset=offset, size=size, path=isinstance(source, str)):
                    entry = quietly(UnityPy.Environment().load_file, source).files["x.resS"]
                    self.assertEqual(bytes(entry.bytes), expected)
        # A WebFile's i32 entries are applied by the file's own reader: slices from bytes, while the stream reader of
        # a path refuses a negative offset and, a buffered file, a size below -1.
        web = S.web_file([(b"a.resS", b"PAYLOAD!")])
        row = len(b"UnityWebData1.0\0") + 4
        web_path = self.tmp / "negative.data"
        for offset, size, from_bytes, from_path in (
            (-4, 4, b"", "^Position cannot be negative$"),
            (None, -1, b"PAYLOAD!", b"PAYLOAD!"),
            (None, -3, b"", "^read length must be non-negative or -1$"),
        ):
            changed = bytearray(web)
            struct.pack_into(
                "<ii", changed, row, struct.unpack_from("<i", web, row)[0] if offset is None else offset, size
            )
            web_path.write_bytes(changed)
            with self.subTest("web", offset=offset, size=size):
                self.assertEqual(bytes(load(bytes(changed))[0].files["a.resS"].bytes), from_bytes)
                if isinstance(from_path, bytes):
                    entry = quietly(UnityPy.Environment().load_file, str(web_path)).files["a.resS"]
                    self.assertEqual(bytes(entry.bytes), from_path)
                else:
                    with self.assertRaisesRegex(ValueError, from_path):
                        quietly(UnityPy.Environment().load_file, str(web_path))
        # An entry's i32 name length is read with the same reader: -1 reads the rest of the data as the name, from
        # bytes and from a path; -12 from a path raises, and from bytes reads b"" and moves the reader back to the
        # entry's start, so the loop adds that entry again and again (stopped here after three).
        offset, size = struct.unpack_from("<ii", web, row)
        changed = bytearray(web)
        struct.pack_into("<i", changed, row + 8, -1)
        web_path.write_bytes(changed)
        for source in (bytes(changed), str(web_path)):
            with self.subTest("web name length -1", path=isinstance(source, str)):
                files = quietly(UnityPy.Environment().load_file, source, name="w.data").files
                self.assertEqual({name: bytes(f.bytes) for name, f in files.items()}, {"a.resSPAYLOAD!": b"PAYLOAD!"})
        struct.pack_into("<i", changed, row + 8, -12)
        web_path.write_bytes(changed)
        with self.assertRaisesRegex(ValueError, "^read length must be non-negative or -1$"):
            quietly(UnityPy.Environment().load_file, str(web_path))
        file_module = sys.modules["UnityPy.files.File"]
        entries = []

        class Again(Exception):
            pass

        def directory_info(*fields):
            entries.append(fields)
            if len(entries) == 3:
                raise Again
            return file_module.DirectoryInfo.__wrapped__(*fields)

        directory_info.__wrapped__ = file_module.DirectoryInfo
        with mock.patch.object(file_module, "DirectoryInfo", directory_info), self.assertRaises(Again):
            load(bytes(changed), "w.data")
        self.assertEqual(entries, [("", offset, size)] * 3)
        # Elsewhere what it gives depends on where the reader lands. In a two-entry WebFile, -2 reads the name b""
        # and moves back 2 bytes, so the next entry is read from there, with a name that runs on into the data; -13
        # raises struct.error. From a path both raise as -12 does.
        two = S.web_file([(b"a.resS", b"PAYLOAD!"), (b"b.resS", b"SECOND")])
        for length in (-2, -13):
            changed = bytearray(two)
            struct.pack_into("<i", changed, row + 8, length)
            web_path.write_bytes(changed)
            with self.subTest("web name length", length=length):
                if length == -2:
                    files = quietly(UnityPy.Environment().load_file, bytes(changed), name="w.data").files
                    self.assertEqual(
                        {name: bytes(f.bytes) for name, f in files.items()},
                        {"": b"PAYLOAD!", "\x06\x00\x00\x00\x06\x00\x00\x00b.resSPAYLOAD!SECOND": b""},
                    )
                else:
                    with self.assertRaisesRegex(struct.error, "^offset -3297 out of range for 70-byte buffer$"):
                        load(bytes(changed), "w.data")
                with self.assertRaisesRegex(ValueError, "^read length must be non-negative or -1$"):
                    quietly(UnityPy.Environment().load_file, str(web_path))
        # Those stream-reader rules are a plain WebFile's from a path. A brotli WebFile is read by the memory reader
        # of its decompressed bytes from any source (WebFile.py:31-34), so from a path too it gives what the same
        # entries give from bytes: a slice for a negative offset or size; for a name length of -2 the moved reader's
        # entries; for -13 struct.error; and for -12 the entry again and again.
        from test_rows_files import unity_brotli

        words = b" ".join(b"word%d" % (i % 37) for i in range(400))  # data brotli compresses (unity_brotli)
        plain = S.web_file([(b"a.resS", words)])
        for fields in ({0: -4, 4: 4}, {4: -3}, {8: -2}):
            changed = bytearray(plain)
            for at, value in fields.items():
                struct.pack_into("<i", changed, row + at, value)
            web_path.write_bytes(unity_brotli(bytes(changed)))
            with self.subTest("brotli web from a path", fields=fields):
                web_file = quietly(UnityPy.Environment().load_file, str(web_path))
                self.assertEqual(web_file.packer, "brotli")
                from_path = {name: bytes(f.bytes) for name, f in web_file.files.items()}
                files = load(bytes(changed), "w.data")[0].files
                self.assertEqual(from_path, {name: bytes(f.bytes) for name, f in files.items()})
                if 8 not in fields:
                    self.assertEqual(from_path, {"a.resS": b""})
                else:
                    self.assertEqual((len(from_path), next(iter(from_path))), (2, ""))  # read from the moved reader
        changed = bytearray(plain)
        struct.pack_into("<i", changed, row + 8, -13)
        web_path.write_bytes(unity_brotli(bytes(changed)))
        with self.assertRaisesRegex(struct.error, f"^offset -3297 out of range for {len(changed)}-byte buffer$"):
            quietly(UnityPy.Environment().load_file, str(web_path))
        offset, size = struct.unpack_from("<ii", plain, row)
        struct.pack_into("<i", changed, row + 8, -12)
        web_path.write_bytes(unity_brotli(bytes(changed)))
        entries.clear()
        with mock.patch.object(file_module, "DirectoryInfo", directory_info), self.assertRaises(Again):
            quietly(UnityPy.Environment().load_file, str(web_path))
        self.assertEqual(entries, [("", offset, size)] * 3)
        # A UnityWeb/UnityRaw level count is an i32, past which read_web_raw skips 8 * (count - 1) bytes: -100 moves
        # Position to -745, before the file's start. From a path the stream reader raises ValueError. From
        # bytes a negative Position counts from the end: below minus the length (a 105-byte bundle) struct.error;
        # otherwise the fields after the level table, the compressed size among them, are read there, and what
        # follows depends on those bytes: in a 4,097-byte bundle an entry of 4,000 b"p" loads, one of 4,000 zero
        # bytes gives a compressed size of 0, and the empty directory raises.
        from test_dump import unityraw

        def level_count(payload):
            raw = unityraw(5, [(b"CAB-x", payload)])
            level = raw.index(bytes(range(16))) + 16 + 16
            self.assertEqual(struct.unpack_from(">i", raw, level), (1,))
            changed = bytearray(raw)
            struct.pack_into(">i", changed, level, -100)
            (self.tmp / "level.bundle").write_bytes(changed)
            return bytes(changed), level + 4 + 8 * (-100 - 1)

        changed, moved = level_count(b"payload!")
        self.assertEqual((len(changed), moved), (105, -745))
        with self.assertRaisesRegex(struct.error, "^offset -745 out of range for 105-byte buffer$"):
            load(changed, "level.bundle")
        with self.assertRaisesRegex(ValueError, "^Position cannot be negative$"):
            quietly(UnityPy.Environment().load_file, str(self.tmp / "level.bundle"))
        changed, moved = level_count(b"p" * 4000)
        self.assertEqual((len(changed), moved), (4097, -745))
        files = load(changed, "level.bundle")[0].files
        self.assertEqual({name: bytes(f.bytes) for name, f in files.items()}, {"CAB-x": b"p" * 4000})
        with self.assertRaisesRegex(ValueError, "^Position cannot be negative$"):
            quietly(UnityPy.Environment().load_file, str(self.tmp / "level.bundle"))
        changed, moved = level_count(bytes(4000))
        self.assertEqual(struct.unpack_from(">I", changed, moved), (0,), "the compressed size read from the end")
        with self.assertRaisesRegex(
            struct.error,
            r"^unpack_from requires a buffer of at least 4 bytes for unpacking 4 bytes at offset 0 "
            r"\(actual buffer size is 0\)$",
        ):
            load(changed, "level.bundle")
        # A blob typetree's node count and string-buffer size each size a read: a negative count reads a slice that
        # iter_unpack refuses unless its length is a multiple of the node size, a count of 0 leaves no root, and a
        # buffer size of -1 takes the rest of the data.
        blob = S.blob(TEXT_ASSET, 22) + b"TAIL"
        count, strings = struct.unpack_from("<ii", blob)

        def parse_blob(count, strings):
            reader = EndianBinaryReader(struct.pack("<ii", count, strings) + blob[8:], endian="<")
            return TypeTreeNode.parse_blob(reader, 22), reader.Position

        self.assertNotEqual((len(blob) - 8 - 24) % 32, 0, "read(-32) at 8 gives view[8:-24], not whole nodes")
        with self.assertRaisesRegex(struct.error, "multiple of 32 bytes"):
            parse_blob(-1, strings)
        with self.assertRaisesRegex(IndexError, "^list index out of range$"):
            parse_blob(0, strings)
        node, end = parse_blob(count, -1)
        self.assertEqual(
            (node.m_Type, [c.m_Name for c in node.m_Children], end), ("TextAsset", ["m_Name", "m_Script"], len(blob))
        )
        # From format 22 an object's data offset is a signed i64: one that lands before the file's start is a
        # negative byte_start, which the memory reader counts from the file's end and a path's stream reader refuses
        # when the object is read (loading reads no object but the first AssetBundle, `test_rows_objects.py`
        # object_container).
        named = text_asset_file("negative")
        obj = load(named)[0].objects[1]
        data_offset = obj.assets_file.header.data_offset
        row_at = named.index(struct.pack("<qIi", obj.byte_start - data_offset, obj.byte_size, 0))
        moved = bytearray(named)
        struct.pack_into("<q", moved, row_at, obj.byte_start - data_offset - len(named))
        (self.tmp / "negative.assets").write_bytes(moved)
        from_bytes = load(bytes(moved))[0].objects[1]
        self.assertEqual(from_bytes.byte_start, obj.byte_start - len(named))
        self.assertEqual(from_bytes.parse_as_dict()["m_Name"], "negative")
        from_path = quietly(UnityPy.Environment().load_file, str(self.tmp / "negative.assets")).objects[1]
        self.assertEqual(from_path.byte_start, obj.byte_start - len(named))
        with self.assertRaisesRegex(ValueError, "^Position cannot be negative$"):
            from_path.parse_as_dict()
        # The memory reader reads the slice view[start:start + size]: for an object that ends at the file's end
        # (no padding after it) that slice ends at index 0 and is empty, which UnityPyBoost refuses.
        flush = text_asset_file("negative", "x" * 100)
        obj = load(flush)[0].objects[1]
        self.assertEqual(obj.byte_start + obj.byte_size, len(flush))
        row_at = flush.index(struct.pack("<qIi", obj.byte_start - data_offset, obj.byte_size, 0))
        moved = bytearray(flush)
        struct.pack_into("<q", moved, row_at, obj.byte_start - data_offset - len(flush))
        with self.assertRaises(EOFError):
            load(bytes(moved))[0].objects[1].parse_as_dict()

    # Section 18: BundleFile.save flags.
    def test_bundle_save_flags(self):
        bundle = load(S.fs_bundle([(b"CAB-test", text_asset_file(), 4)]))[0]
        saved = bundle.save()
        again = load(saved)[0]
        self.assertEqual((int(again.dataflags), again._block_info_flags), (0x40, 0x40))
        empty = load(S.fs_bundle([]))[0]
        self.assertEqual(empty.files, {})
        with self.assertRaisesRegex(AttributeError, "_block_info_flags"):
            empty.save(packer="original")
        # A bundle with no blocks can still list entries: it holds no data, so each reads b"", whatever size the
        # directory states (an entry past the data reads short, section 13), and `original` raises for it too.
        listed = load(S.fs_bundle([(b"e.resS", b"", 0), (b"f.resS", 0, 16, 0)]))[0]
        self.assertEqual({name: bytes(f.bytes) for name, f in listed.files.items()}, {"e.resS": b"", "f.resS": b""})
        with self.assertRaisesRegex(AttributeError, "_block_info_flags"):
            listed.save(packer="original")

    # Sections 12, 18, 19.2: save_fs XORs the flag set's whole encryption mask out when any bit of it is set.
    def test_bundle_save_xors_the_whole_encryption_mask(self):
        def header_flags(data):
            end = data.index(b"\0") + 1 + 4  # signature, format
            end = data.index(b"\0", data.index(b"\0", end) + 1) + 1  # player and engine versions
            return struct.unpack_from(">I", data, end + 16)[0]  # after the size and the two blocks-info sizes

        def saved(engine, extra):
            bundle = load(S.fs_bundle([(b"CAB-test", text_asset_file(), 4)], engine=engine))[0]
            bundle.dataflags = type(bundle.dataflags)(int(bundle.dataflags) | extra)
            return type(bundle.dataflags), bundle.save(packer="original")

        new, old = b"2021.3.2f1", b"2019.4.1f1"
        cases = {
            # ArchiveFlags' mask is 0x1400: one bit of it comes out as the other, which reads back as encrypted.
            (new, 0x1000): (ArchiveFlags, 0x0443, LookupError),
            (new, 0x0400): (ArchiveFlags, 0x1043, LookupError),
            (new, 0x1400): (ArchiveFlags, 0x0043, None),
            (new, 0x0200): (ArchiveFlags, 0x0243, None),  # BlockInfoNeedPaddingAtStart, kept
            # ArchiveFlagsOld's mask is 0x200 alone, which is cleared; its other bits are not encryption.
            (old, 0x0200): (ArchiveFlagsOld, 0x0043, None),
            (old, 0x1000): (ArchiveFlagsOld, 0x1043, None),
        }
        for (engine, extra), (flag_set, flags, error) in cases.items():
            with self.subTest(engine=engine, extra=hex(extra)):
                kind, data = saved(engine, extra)
                self.assertEqual((kind, hex(header_flags(data))), (flag_set, hex(flags)))
                if error is None:
                    self.assertEqual(list(load(data)[0].files), ["CAB-test"])
                else:
                    with self.assertRaisesRegex(error, "^The BundleFile is encrypted, but no key was provided"):
                        load(data)
        # The data blocks' own 0x100 bit is not in the mask, so `original` writes it back.
        bundle = load(S.fs_bundle([(b"CAB-test", text_asset_file(), 4)]))[0]
        bundle._block_info_flags |= 0x100
        again = load(bundle.save(packer="original"))[0]
        self.assertEqual((again._block_info_flags, list(again.files)), (0x102, ["CAB-test"]))

    # Sections 5.2, 19.2: File's iteration and dict-style methods.
    def test_file_iteration_and_dict_access(self):
        def two(name):
            objects = [S.Obj(1, 0, S.s32(name) + S.s32("s")), S.Obj(2, 0, S.s32(f"{name}2") + S.s32("s"))]
            return S.serialized_file([S.Type(49, TEXT_ASSET)], objects)

        bundle = load(S.fs_bundle([(b"CAB-a", two("a"), 4), (b"CAB-a.resS", b"\x11" * 16, 0)]), "b")[0]
        sf, raw = bundle.files["CAB-a"], bundle.files["CAB-a.resS"]
        self.assertIsInstance(raw, EndianBinaryReader)
        text_asset = [ClassIDType.TextAsset]
        # A raw reader is skipped: the bundle's objects are the SerializedFile's two.
        self.assertEqual(list(bundle.get_objects()), list(sf.objects.values()))
        self.assertEqual(list(bundle.get_assets()), [sf])
        self.assertEqual([o.path_id for o in bundle.get_filtered_objects(text_asset)], [1, 2])
        self.assertEqual(list(bundle.get_filtered_objects([ClassIDType.Texture2D])), [])
        # With no types, `return self.get_objects()` ends the generator: nothing.
        self.assertEqual((list(bundle.get_filtered_objects()), list(bundle.get_filtered_objects([]))), ([], []))
        # A SerializedFile's `files` is its objects: get_objects yields them through the ObjectReader branch,
        # get_filtered_objects takes neither branch, and get_assets' `return self` ends its generator.
        self.assertIs(sf.files, sf.objects)
        self.assertEqual(list(sf.get_objects()), list(sf.objects.values()))
        self.assertEqual((list(sf.get_filtered_objects(text_asset)), list(sf.get_assets())), ([], []))
        # A nested container is iterated as `f.objects`, which BundleFile and WebFile lack.
        inner = S.fs_bundle([(b"CAB-i", two("i"), 4)])
        nested = load(S.fs_bundle([(b"inner", inner, 4), (b"CAB-o", two("o"), 4)]), "n")[0]
        web = load(S.web_file([(b"inner", inner)]), "w")[0]
        self.assertIsInstance(web.files["inner"], BundleFile)
        for container in (nested, web):
            for call in (container.get_objects, lambda c=container: c.get_filtered_objects(text_asset)):
                with self.assertRaisesRegex(AttributeError, "'BundleFile' object has no attribute 'objects'"):
                    list(call())
        # get_assets recurses: the reference those raises are checked against.
        self.assertEqual([f.name for f in nested.get_assets()], ["CAB-i", "CAB-o"])
        # `get` is an attribute lookup; keys, items, values and [] are those of `files`.
        self.assertEqual((bundle.get("CAB-a"), bundle.get("nope", 5)), (None, 5))
        self.assertIs(bundle.get("files"), bundle.files)
        self.assertEqual(list(bundle.keys()), ["CAB-a", "CAB-a.resS"])
        self.assertEqual(list(bundle.values()), [sf, raw])
        self.assertEqual(list(bundle.items()), [("CAB-a", sf), ("CAB-a.resS", raw)])
        self.assertIs(bundle["CAB-a"], sf)
        self.assertEqual((list(sf.keys()), sf[2]), ([1, 2], sf.objects[2]))

    # Section 6: the codec helpers and tables.
    def test_compression_helpers(self):
        import lzma

        import brotli
        from UnityPy.enums import CompressionFlags as C
        from UnityPy.helpers import CompressionHelper as H

        data = bytes(range(256)) * 700  # 175 KiB, compressible
        self.assertEqual(sorted(H.DECOMPRESSION_MAP), [C.NONE, C.LZMA, C.LZ4, C.LZ4HC, C.LZHAM])
        self.assertEqual(sorted(H.COMPRESSION_MAP), [C.NONE, C.LZMA, C.LZ4, C.LZ4HC])
        self.assertEqual(H.COMPRESSION_MAP[C.LZ4HC], H.compress_lz4)
        self.assertEqual(H.DECOMPRESSION_MAP[C.LZ4HC], H.decompress_lz4)
        self.assertEqual(
            H.COMPRESSION_CHUNK_SIZE_MAP, {C.NONE: 0xFFFFFFFF, C.LZMA: 0xFFFFFFFF, C.LZ4: 0x20000, C.LZ4HC: 0x20000}
        )
        self.assertEqual((H.GZIP_MAGIC, H.BROTLI_MAGIC), (b"\x1f\x8b", b"brotli"))
        # LZHAM and any other codec value.
        with self.assertRaisesRegex(NotImplementedError, "LZHAM"):
            H.decompress_lzham(b"x", 1)
        bundle = load(S.fs_bundle([(b"CAB-test", text_asset_file(), 4)]))[0]
        with self.assertRaisesRegex(NotImplementedError, "LZHAM"):
            bundle.decompress_data(b"x", 1, 4)
        for codec in (5, 6, 7, 8, 63, 0x40 | 9):
            with self.subTest(codec=codec), self.assertRaisesRegex(ValueError, "^Unknown compression"):
                bundle.decompress_data(b"x", 1, codec)
        # LZMA: props 0x5D, an 8 MiB dictionary, lc 3 lp 0 pb 2, BT4, nice_len 123, an optional u64 size.
        filters = [
            {"id": lzma.FILTER_LZMA1, "dict_size": 1 << 23, "lc": 3, "lp": 0, "pb": 2, "mf": lzma.MF_BT4}
            | {"mode": lzma.MODE_NORMAL, "nice_len": 123}
        ]
        raw = lzma.compress(data, format=lzma.FORMAT_RAW, filters=filters)
        self.assertEqual(H.compress_lzma(data), b"\x5d" + (1 << 23).to_bytes(4, "little") + raw)
        self.assertEqual(H.compress_lzma(data, True), H.compress_lzma(data)[:5] + struct.pack("<Q", len(data)) + raw)
        self.assertEqual(H.decompress_lzma(H.compress_lzma(data)), data)
        self.assertEqual(H.decompress_lzma(H.compress_lzma(data, True), True), data)
        # LZ4: high compression at level 9, no size prefix.
        self.assertEqual(
            H.compress_lz4(data), lz4.block.compress(data, mode="high_compression", compression=9, store_size=False)
        )
        # brotli at the library's default quality, 11; gzip at level 9 with the time as the header's mtime.
        self.assertEqual(H.compress_brotli(data), brotli.compress(data, quality=11))
        with mock.patch("time.time", return_value=1000.0):
            first = H.compress_gzip(data)
        with mock.patch("time.time", return_value=2000.0):
            second = H.compress_gzip(data)
        self.assertEqual((first[4:8], second[4:8]), (struct.pack("<I", 1000), struct.pack("<I", 2000)))
        self.assertEqual((first[8], first[10:], H.decompress_gzip(first)), (2, second[10:], data))  # XFL 2: level 9
        # chunk_based_compress: LZ4 in 128 KiB chunks, LZMA and none in one; a chunk that grows keeps its bytes
        # with the codec bits cleared.
        packed, blocks = H.chunk_based_compress(data, 0x42)
        self.assertEqual([(size, flag) for size, _, flag in blocks], [(0x20000, 0x42), (len(data) - 0x20000, 0x42)])
        self.assertEqual(lz4.block.decompress(packed[: blocks[0][1]], 0x20000), data[:0x20000])
        self.assertEqual([(u, f) for u, _, f in H.chunk_based_compress(data, 1)[1]], [(len(data), 1)])
        self.assertEqual(H.chunk_based_compress(data, 0x40), (data, [(len(data), len(data), 0x40)]))
        noise = random.Random(3).randbytes(1000)
        self.assertEqual(H.chunk_based_compress(noise, 0x42), (noise, [(1000, 1000, 0x40)]))
        # `none` is one block whatever the payload, an empty one included; a compressing codec writes no block for
        # an empty payload. Only a chunk that grows is stored: one that compresses to exactly its own size stays
        # compressed, as a whole chunk and as the last part.
        self.assertEqual(H.chunk_based_compress(b"", 0x40), (b"", [(0, 0, 0x40)]))
        for flag in (1, 2, 3):
            self.assertEqual(H.chunk_based_compress(b"", flag), (b"", []))
        exact = random.Random(5).randbytes(130548) + bytes(0x20000 - 130548)
        self.assertEqual(len(H.compress_lz4(exact)), 0x20000)
        self.assertEqual([f for _, _, f in H.chunk_based_compress(exact, 3)[1]], [3])
        self.assertEqual([f for _, _, f in H.chunk_based_compress(exact + b"x", 3)[1]], [3, 0])
        with self.assertRaisesRegex(NotImplementedError, "No compression function"):
            H.chunk_based_compress(data, 4)

    # Sections 5.2, 6: WebFile sniffs gzip at offset 0 and brotli at 0x20, given a reader at its start.
    def test_webfile_sniffs_its_codec(self):
        from UnityPy.helpers import CompressionHelper as H

        plain = S.web_file([(b"a.txt", b"hello")])
        gz = WebFile(EndianBinaryReader(gzip.compress(plain)), None)
        self.assertEqual((gz.packer, list(gz.files)), ("gzip", ["a.txt"]))
        marked = bytes(0x20) + b"brotli" + bytes(10)
        with mock.patch.object(H, "decompress_brotli", lambda data: plain) as stub:
            br = WebFile(EndianBinaryReader(marked), None)
        self.assertEqual((br.packer, list(br.files), stub(b"")), ("brotli", ["a.txt"], plain))
        self.assertEqual(WebFile(EndianBinaryReader(plain), None).packer, "none")


if __name__ == "__main__":
    unittest.main()
