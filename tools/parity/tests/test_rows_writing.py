"""PARITY.md rows on writing, run against the oracle: the writeable CABs, SerializedFile.save by format,
BundleFile.save's packers and save_web_raw, and HEAD's own bundle writer and reader (UnityPy git 417d595, run from
head_417d595/ by head_oracle.py).

Sections 7.2, 18, 19 and 20. Every input is built here (`synth.py`), so nothing needs the corpus, and each test
names the rows it pins; a row's test column names the test back. Its class is `FileRows`, as in
`test_rows_files.py`, the module these tests were split from. Needs UnityPy 1.25.2 (the oracle image); skipped
elsewhere.
"""

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
    from UnityPy.streams import EndianBinaryReader


VERSION = "2021.3.2f1"  # synth.serialized_file's default engine version


@unittest.skipUnless(ORACLE, "needs UnityPy 1.25.2 (the oracle image)")
class FileRows(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="unityrs-file-rows-"))
        self.addCleanup(shutil.rmtree, self.tmp)

    # Sections 18, 19.2: File.get_writeable_cab, and SerializedFile.get_writeable_cab, which never returns a writer.
    def test_writeable_cabs(self):
        from UnityPy.streams import EndianBinaryWriter

        bundle, env = load(S.fs_bundle([(b"CAB-a", text_asset_file(), 4), (b"CAB-a.resS", b"\x11" * 8, 0)]))
        # File.get_writeable_cab: the default name is `cab_file`; flags and endian come from the first .resS. The
        # writer goes into `files` and is not registered as a CAB.
        cabs = dict(env.cabs)
        writer = bundle.get_writeable_cab()
        self.assertEqual(env.cabs, cabs)
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
        import lzma

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

        def hc(data):
            return lz4.block.compress(data, mode="high_compression", compression=9, store_size=False)

        self.assertIn(hc(entry), saved)
        self.assertEqual(flags(bundle.save(packer="lzma")), 0x41)
        # A block is the output for a 128 KiB chunk of the entries concatenated, not for an entry: with two small
        # entries, neither one's own output is in the file, and the concatenation's is.
        two = load(S.fs_bundle([(b"CAB-a", payload, 4), (b"CAB-b", text_asset_file("b", "r" * 300), 4)]))[0]
        saved = two.save(packer="lz4")
        entries = [two.files[name].save() for name in ("CAB-a", "CAB-b")]
        self.assertEqual([hc(e) in saved for e in entries] + [hc(b"".join(entries)) in saved], [False, False, True])
        # lz4hc, the HEAD packer, is unknown here. The tuple (195, 3) writes header flags 0xC3 and 1.25.2's blocks,
        # liblz4 HC level 9 as for lz4, which are not HEAD's (section 20).
        with self.assertRaisesRegex(NotImplementedError, "lz4hc"):
            bundle.save(packer="lz4hc")
        saved = bundle.save(packer=(195, 3))
        self.assertEqual((flags(saved), block_flags(saved)[1:]), (0xC3, (3, ["p"])))
        self.assertIn(hc(entry), saved)
        # Entry names are read surrogate-escaped and written back with write_string_to_null, which encodes with
        # surrogateescape: a name holding the byte 0xFF saves and reloads.
        odd = load(S.fs_bundle([(b"CAB-\xff", payload, 4), (b"r\xff.resS", b"\x11" * 8, 0)]))[0]
        self.assertEqual(list(odd.files), ["CAB-\udcff", "r\udcff.resS"])
        for packer in ("none", "lz4"):
            with self.subTest(packer=packer):
                self.assertEqual(list(load(odd.save(packer=packer))[0].files), ["CAB-\udcff", "r\udcff.resS"])
        # On several chunks 1.25.2 stores a chunk only when compressing makes it larger, whatever came before;
        # HEAD compresses the first chunk always and stores every chunk once 0x20000 compressed bytes are out.
        from UnityPy.helpers import CompressionHelper as H

        def blocks(resource):
            raw_bundle = load(S.fs_bundle([(b"CAB-a", payload, 4), (b"CAB-a.resS", resource, 0)]))[0]
            seen = []
            real = H.chunk_based_compress

            def spy(data, flag):
                out = real(data, flag)
                seen.append(out[1])
                return out

            with mock.patch.object(H, "chunk_based_compress", spy):
                saved = raw_bundle.save(packer=(195, 3))
            self.assertEqual(bytes(load(saved)[0].files["CAB-a.resS"].bytes), resource)
            return seen[0]

        chunk = 0x20000
        halves = b"".join(random.Random(i).randbytes(chunk // 2) + bytes(chunk // 2) for i in range(3))
        got = blocks(halves)
        # The entries come first, so the three chunks of the resource span four of the data.
        self.assertEqual([f for _, _, f in got], [3, 3, 3, 3])
        self.assertGreaterEqual(got[0][1] + got[1][1], chunk, "HEAD would store the last two")
        got = blocks(random.Random(7).randbytes(chunk + chunk // 2))
        self.assertEqual([(f, size == stored) for stored, size, f in got], [(0, True), (0, True)])
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

        # UnityWeb and UnityRaw format 6 save through save_fs(64, 64): header flags 0x40, and they reload.
        for signature in (b"UnityRaw", b"UnityWeb"):
            with self.subTest(signature=signature, format=6):
                six = load(signature + unityraw(6, [(b"CAB-r", payload)])[len(b"UnityRaw") :])[0]
                self.assertEqual((six.signature, six.version), (signature.decode(), 6))
                saved = six.save()
                head = saved.index(b"5.6.7f1\0") + 8
                self.assertEqual(struct.unpack_from(">I", saved, head + 16)[0], 0x40)
                back = load(saved)[0]
                self.assertEqual([o.parse_as_dict()["m_Name"] for o in back.files["CAB-r"].objects.values()], ["p"])
        for fmt in (1, 2, 3, 4, 5, 7):
            with self.subTest(format=fmt):
                raw = load(old_raw(fmt, b"CAB-r", payload) if fmt <= 3 else unityraw(fmt, [(b"CAB-r", payload)]))[0]
                if fmt > 3:
                    with self.assertRaisesRegex(
                        NotImplementedError, "^Saving Unity Web bundles with version > 3 is not supported$"
                    ):
                        raw.save()
                    continue
                back = load(raw.save())[0]
                self.assertEqual([o.parse_as_dict()["m_Name"] for o in back.files["CAB-r"].objects.values()], ["p"])
                # save never reads the packer here, so every one, an unknown one included, writes the same bytes.
                # UnityRaw stores the directory and the data as they are; UnityWeb stores what compress_lzma(..., True)
                # makes of them: 0x5D, the dictionary size 0x800000, the size as 8 bytes, then raw LZMA1.
                contents = {}
                for signature in ("UnityRaw", "UnityWeb"):
                    raw.signature = signature
                    packers = (None, "none", "lz4", "lzma", "original", "bogus", (194, 2))
                    saves = [raw.save(packer=packer) for packer in packers]
                    self.assertEqual(saves, [saves[0]] * len(packers), signature)
                    strings = saves[0].index(b"2.6.1f1\0") + len(b"2.6.1f1\0")
                    _, header_size, _, _, packed, size = struct.unpack_from(">IIIiII", saves[0], strings)
                    contents[signature] = saves[0][header_size:]
                    self.assertEqual(len(contents[signature]), packed)
                    back = load(saves[0])[0]
                    self.assertEqual(
                        (back.signature, [o.parse_as_dict()["m_Name"] for o in back.files["CAB-r"].objects.values()]),
                        (signature, ["p"]),
                    )
                plain, web = contents["UnityRaw"], contents["UnityWeb"]
                self.assertEqual((plain[:10], len(plain), size), (struct.pack(">i", 1) + b"CAB-r\0", size, size))
                self.assertEqual(web, H.compress_lzma(plain, True))
                self.assertEqual(web[:13], bytes.fromhex("5d00008000") + struct.pack("<Q", len(plain)))
                settings = {"id": lzma.FILTER_LZMA1, "dict_size": 0x800000, "lc": 3, "lp": 0, "pb": 2}
                settings.update(mode=lzma.MODE_NORMAL, mf=lzma.MF_BT4, nice_len=123)
                encoder = lzma.LZMACompressor(format=lzma.FORMAT_RAW, filters=[settings])
                self.assertEqual(web[13:], encoder.compress(plain) + encoder.flush())
                raw.signature = "UnityRaw"
        # Above format 3 the raise is the only thing in the way: UnityPy's own save_web_raw run with its version
        # check removed (the Tier C (writing) reference) writes the file's hash and CRC after the header's strings,
        # and saves UnityRaw and UnityWeb bundles that reload with the same entries, hash and CRC.
        import inspect
        import re
        import textwrap

        bundle_module = sys.modules["UnityPy.files.BundleFile"]
        source = textwrap.dedent(inspect.getsource(bundle_module.BundleFile.save_web_raw))
        check = re.compile(r"\n *if self\.version > 3:\n *raise NotImplementedError\([^)]*\)\n")
        self.assertEqual(len(check.findall(source)), 1)
        namespace = {}
        exec(compile(check.sub("\n", source), "save_web_raw", "exec"), vars(bundle_module), namespace)
        for signature in ("UnityRaw", "UnityWeb"):
            for fmt in (4, 5, 7):
                with self.subTest("past the check", signature=signature, format=fmt):
                    raw = load(unityraw(fmt, [(b"CAB-r", payload), (b"CAB-r.resS", b"res")]))[0]
                    raw.signature = signature
                    with mock.patch.object(bundle_module.BundleFile, "save_web_raw", namespace["save_web_raw"]):
                        saved = raw.save()
                    strings = saved.index(b"5.6.7f1\0") + len(b"5.6.7f1\0")
                    self.assertEqual(saved[strings : strings + 20], bytes(raw._hash) + struct.pack(">I", raw.crc))
                    back = load(saved)[0]
                    self.assertEqual((back.signature, back.version), (signature, fmt))
                    self.assertEqual(
                        (
                            [o.parse_as_dict()["m_Name"] for o in back.files["CAB-r"].objects.values()],
                            back.files["CAB-r.resS"].bytes,
                        ),
                        (["p"], b"res"),
                    )
                    self.assertEqual((bytes(back._hash), back.crc), (bytes(raw._hash), raw.crc))

    # Sections 18, 20: HEAD's own bundle writer and reader (UnityPy git 417d595, run from head_417d595/ by
    # head_oracle.py): its `lz4hc` rule on several chunks at formats 8 and 9, and a format-9 save with every packer,
    # against the rule section 20 states and against what 1.25.2 writes.
    def test_head_saves(self):
        import lz4.block
        from UnityPy.helpers import CompressionHelper as H

        import head_oracle

        helper, head = head_oracle.load()
        chunk = 0x20000

        def rule(data, flag, version):
            """Section 20's rule: (uncompressed, compressed, flags, offset) per block, and the bytes written."""
            switch = flag & 0x3F
            if switch == 0:
                return [(len(data), len(data), flag, 0 if version >= 9 else None)], len(data)
            size = chunk if switch in (2, 3) else 0xFFFFFFFF  # LZ4 and LZ4HC in 128 KiB chunks, LZMA in one
            blocks, written = [], 0
            for start in range(0, len(data), size):
                piece = data[start : start + size]
                if written >= size:  # the bytes written before it, padding included
                    packed, flags = piece, flag ^ switch
                elif switch == 1:
                    packed, flags = H.compress_lzma(piece), flag
                else:
                    packed = lz4.block.compress(piece, mode="high_compression", compression=9, store_size=False)
                    flags = flag
                blocks.append((len(piece), len(packed), flags, written if version >= 9 else None))
                written += len(packed)
                written += -written % 16 if version >= 9 else 0
            return blocks, written

        halves = b"".join(random.Random(i).randbytes(chunk // 2) + bytes(chunk // 2) for i in range(3))
        inputs = {
            "compressible": b"".join(bytes([i]) * chunk for i in range(5)),
            "random": random.Random(1).randbytes(2 * chunk),
            "half-compressible": halves,
            "a chunk and a half": random.Random(2).randbytes(chunk + chunk // 2),
            "short": b"q" * 300,
            "empty": b"",  # `none` one block of size 0; a compressing packer none
        }
        # LZMA in chunks of 0xFFFFFFFF bytes, as 1.25.2 (one block under 4 GiB); LZ4 and LZ4HC in 128 KiB.
        self.assertEqual(
            {int(k): v for k, v in helper.COMPRESSION_CHUNK_SIZE_MAP.items()},
            {0: 0xFFFFFFFF, 1: 0xFFFFFFFF, 2: chunk, 3: chunk},
        )
        got = {}
        for name, data in inputs.items():
            for version in (8, 9):
                for flag in (0x40, 1, 2, 3):
                    with self.subTest(name, version=version, flag=flag):
                        out, infos = helper.chunk_based_compress(data, flag, version)
                        blocks = [(b.uncompressedSize, b.compressedSize, b.flags, b.offset) for b in infos]
                        self.assertEqual((blocks, len(out)), rule(data, flag, version))
                        got[name, version, flag] = [f for _, _, f, _ in blocks]
        # Where HEAD's lz4hc blocks differ from 1.25.2's tuple packer: the first chunk is compressed even when it
        # grows, and a chunk that shrinks is stored once 0x20000 bytes are out.
        old = {name: [f for _, _, f in H.chunk_based_compress(data, 3)[1]] for name, data in inputs.items()}
        self.assertEqual((got["random", 8, 3], old["random"]), ([3, 0], [0, 0]))
        self.assertEqual((got["half-compressible", 9, 3], old["half-compressible"]), ([3, 3, 0], [3, 3, 3]))
        self.assertEqual(got["compressible", 9, 3], [3] * 5)
        self.assertEqual([got["empty", 9, flag] for flag in (0x40, 1, 2, 3)], [[0x40], [], [], []])
        # HEAD's LZMA is one chunk, compressed although it grows; 1.25.2 stores a chunk that grows.
        self.assertEqual(
            (got["random", 9, 1], [f for _, _, f in H.chunk_based_compress(inputs["random"], 1)[1]]), ([1], [0])
        )

        # The save path: HEAD's `save` hands chunk_based_compress the entries concatenated, the blocks' codec and
        # the bundle's format, and its reader, seeking each block by its offset from format 9, reads the entries
        # back. A format-9 bundle is made by HEAD's own save of a format-8 one.
        def entries(bundle):
            return {
                k: [o.parse_as_dict()["m_Name"] for o in f.objects.values()] if hasattr(f, "objects") else f.bytes
                for k, f in bundle.files.items()
            }

        def head_bundle(data):
            return quietly(head.BundleFile, EndianBinaryReader(data), UnityPy.Environment(), name="h")

        def header(saved):
            """The format, and the blocks info's uncompressed size and the flags, from a UnityFS header."""
            engine_end = saved.index(b"\0", saved.index(b"\0", 12) + 1)  # after the signature, format and player
            info_size, flags = struct.unpack_from(">II", saved, engine_end + 1 + 12)
            return {"format": struct.unpack_from(">I", saved, 8)[0], "info": info_size, "flags": flags}

        def payload_of(bundle):
            """The entries concatenated as `save_fs` writes them: a raw reader's bytes, any other entry saved."""
            return b"".join(
                bytes(f.bytes) if isinstance(f, EndianBinaryReader) else f.save() for f in bundle.files.values()
            )

        payload = text_asset_file("p", "q" * 300)
        source = S.fs_bundle([(b"CAB-a", payload, 4), (b"CAB-a.resS", halves, 0)])
        eight = head_bundle(source)
        eight.version = 9
        nine_bytes = eight.save()
        eight.version = 8
        nine = head_bundle(nine_bytes)
        self.assertEqual((nine.version, entries(nine)), (9, entries(eight)))
        directory = sum(8 + 8 + 4 + len(name) + 1 for name in nine.files)
        self.assertEqual(header(nine_bytes), {"format": 9, "info": 16 + 4 + 18 + 4 + directory, "flags": 0x40})
        real = helper.chunk_based_compress
        for bundle, version in ((eight, 8), (nine, 9)):
            packers = [
                ("none", 0x40, 0x40),
                ("lz4", 0xC2, 2),
                ("lz4hc", 0xC3, 3),
                ("lzma", 0x41, 1),
                ((0x43, 3), 0x43, 3),
                ("original", bundle.dataflags, bundle._block_info_flags),
            ]
            for packer, data_flag, block_flag in packers:
                with self.subTest(version=version, packer=packer):
                    seen = []

                    def spy(data, flag, version, seen=seen):
                        out = real(data, flag, version)
                        seen.append((bytes(data), flag, version, out))
                        return out

                    with mock.patch.object(helper, "chunk_based_compress", spy):
                        saved = bundle.save(packer=packer)
                    (data, flag, at, (out, infos)) = seen[0]
                    self.assertEqual((len(seen), flag, at), (1, block_flag, version))
                    self.assertEqual((header(saved)["format"], header(saved)["flags"]), (version, int(data_flag)))
                    self.assertEqual(data, payload_of(bundle))
                    blocks = [(b.uncompressedSize, b.compressedSize, b.flags, b.offset) for b in infos]
                    self.assertEqual((blocks, len(out)), rule(data, flag, version))
                    self.assertEqual(entries(head_bundle(saved)), entries(bundle))
        # HEAD fails to write where 1.25.2 does, so these take the Tier C (writing) rows: `original` on a bundle
        # that had no block raises, and a header holding a bit of the encryption mask is saved with the whole mask
        # XORed out (0x1000 -> 0x400, 0x400 -> 0x1000), which HEAD's own reader reads back as encrypted.
        no_blocks = head_bundle(S.fs_bundle([(b"e.resS", b"", 0)]))
        no_blocks.version = 9
        self.assertFalse(hasattr(no_blocks, "_block_info_flags"))
        with self.assertRaisesRegex(AttributeError, "'BundleFile' object has no attribute '_block_info_flags'"):
            no_blocks.save(packer="original")
        for bit, written in ((0x1000, 0x440), (0x400, 0x1040)):
            with self.subTest(encryption_bit=bit):
                encrypted = head_bundle(nine_bytes)
                encrypted.dataflags = type(encrypted.dataflags)(int(encrypted.dataflags) | bit)
                saved = encrypted.save(packer="original")
                self.assertEqual(header(saved)["flags"], written)
                with self.assertRaisesRegex(LookupError, "^The BundleFile is encrypted, but no key was provided!"):
                    head_bundle(saved)
        # 1.25.2 reads a format-9 bundle with format 8's layout and saves it so, under version 9: 10-byte block
        # entries with no offset. HEAD's reader takes each for 18 bytes: in a save of one block it seeks the block
        # past the data (the offset it reads is the directory's count and half of the first entry's offset) and reads
        # a directory count of 0, so a stored block gives a bundle with no entries and a compressed one fails to
        # decompress; a save of more blocks has an outcome that depends on the bytes, and one with no block it reads.
        laid_out_as_8 = bytearray(source)
        struct.pack_into(">I", laid_out_as_8, 8, 9)
        old_nine = load(bytes(laid_out_as_8))[0]
        saved = old_nine.save(packer="none")
        self.assertEqual(header(saved), {"format": 9, "info": 16 + 4 + 10 + 4 + directory, "flags": 0x40})
        self.assertEqual(entries(load(saved)[0]), entries(nine))
        self.assertEqual(head_bundle(saved).files, {})

        def old_nine_of(data):
            laid_out = bytearray(S.fs_bundle([(b"CAB-a", payload, 4), (b"CAB-a.resS", data, 0)]))
            struct.pack_into(">I", laid_out, 8, 9)
            return load(bytes(laid_out))[0]

        def old_flags(bundle, packer):
            """The flags of each block 1.25.2's own chunk rule writes for a save of `bundle`."""
            flag = {"lz4": 2, "lzma": 1}[packer]
            return [int(b[2]) for b in H.chunk_based_compress(payload_of(bundle), flag)[1]]

        stored = old_nine_of(random.Random(1).randbytes(0x1F000))  # one chunk that grows under compression
        packed = old_nine_of(bytes(0x9000))
        for bundle, packer, flags, error in (
            (stored, "lz4", [0], None),
            (stored, "lzma", [0], None),
            (packed, "lz4", [2], lz4.block.LZ4BlockError),
            (packed, "lzma", [1], struct.error),
        ):
            with self.subTest("HEAD reads 1.25.2's", packer=packer, flags=flags):
                self.assertEqual(old_flags(bundle, packer), flags)
                saved = bundle.save(packer=packer)
                if error is None:
                    self.assertEqual(head_bundle(saved).files, {})
                else:
                    with self.assertRaises(error):
                        head_bundle(saved)
        for size, error, message in (
            (0x29000, Exception, "^String not terminated$"),
            (0x40000, struct.error, "^unpack_from requires"),
        ):
            bundle = old_nine_of(bytes(size))
            with self.subTest("HEAD reads 1.25.2's", blocks=len(old_flags(bundle, "lz4"))):
                self.assertEqual(len(old_flags(bundle, "lz4")), 2 if size < 0x40000 else 3)
                with self.assertRaisesRegex(error, message):
                    head_bundle(bundle.save(packer="lz4"))
        # The outcome of more blocks depends on the bytes: behind an empty first entry, two blocks of random data,
        # stored, load with no entries, and two of zeros, compressed, fail to decompress. A save with no block (an
        # empty payload under a compressing packer) HEAD reads whole.

        def old_nine_entries(listed):
            laid_out = bytearray(S.fs_bundle([(name, data, 0) for name, data in listed]))
            struct.pack_into(">I", laid_out, 8, 9)
            return load(bytes(laid_out))[0]

        for data, flags, error in (
            (random.Random(2).randbytes(0x29000), [0, 0], None),
            (bytes(0x29000), [2, 2], lz4.block.LZ4BlockError),
        ):
            bundle = old_nine_entries([(b"e.resS", b""), (b"CAB-a.resS", data)])
            with self.subTest("HEAD reads 1.25.2's, behind an empty entry", flags=flags):
                self.assertEqual(old_flags(bundle, "lz4"), flags)
                if error is None:
                    self.assertEqual(head_bundle(bundle.save(packer="lz4")).files, {})
                else:
                    with self.assertRaises(error):
                        head_bundle(bundle.save(packer="lz4"))
        empty = old_nine_entries([(b"e.resS", b""), (b"f.resS", b"")])
        for packer in ("lz4", "lzma"):
            with self.subTest("HEAD reads 1.25.2's with no block", packer=packer):
                self.assertEqual(old_flags(empty, packer), [])
                read = head_bundle(empty.save(packer=packer))
                self.assertEqual({k: bytes(f.bytes) for k, f in read.files.items()}, {"e.resS": b"", "f.resS": b""})

        # 1.25.2 reads each of HEAD's 18-byte entries as 10 bytes. A save of one block it reads as a bundle with no
        # entries (the directory's count is the high half of the block's offset, 0). In a save of two blocks the
        # second entry it reads is a block of size 0 whose flags are the high half of the second block's
        # uncompressed size, and the directory count's high half is that size's low half: it loads, with no entries,
        # only for a second block of 0x8000 to 0xFFFF bytes. One of three or more blocks it fails to load, by the
        # bytes it lands on; one with no block it reads whole.
        def blocks_of(bundle, packer):
            flag = {"none": 0x40, "lz4": 2, "lz4hc": 3, "lzma": 1}[packer]
            return len(helper.chunk_based_compress(payload_of(bundle), flag, 9)[1])

        self.assertEqual((blocks_of(nine, "none"), load(nine_bytes)[0].files), (1, {}))
        self.assertEqual((blocks_of(nine, "lzma"), load(nine.save(packer="lzma"))[0].files), (1, {}))
        self.assertEqual(blocks_of(nine, "lz4"), 4)
        with self.assertRaises(struct.error):
            load(nine.save(packer="lz4"))

        def head_nine_of(data):
            bundle = head_bundle(S.fs_bundle([(b"CAB-a", payload, 4), (b"CAB-a.resS", data, 0)]))
            bundle.version = 9
            return bundle

        base = len(payload_of(head_nine_of(b"")))
        for second, error, message in (
            (1, struct.error, "^unpack_from requires"),
            (0x7FFF, struct.error, "^unpack_from requires"),
            (0x8000, None, None),
            (0xC000, None, None),
            (0xFFFF, None, None),
            (0x10000, struct.error, "^unpack requires a buffer of 5 bytes$"),
            (0x17FFF, struct.error, "^unpack_from requires"),
            (0x18000, struct.error, "^unpack requires a buffer of 5 bytes$"),
            (0x20000, lz4.block.LZ4BlockError, ""),
        ):
            for kind, packer in (("random", "lz4"), ("zeros", "lz4hc")):
                size = chunk + second - base
                data = random.Random(second).randbytes(size) if kind == "random" else bytes(size)
                bundle = head_nine_of(data)
                with self.subTest("1.25.2 reads HEAD's", second=hex(second), packer=packer):
                    self.assertEqual((len(payload_of(bundle)), blocks_of(bundle, packer)), (chunk + second, 2))
                    saved = bundle.save(packer=packer)
                    if error is None:
                        self.assertEqual(load(saved)[0].files, {})
                    else:
                        with self.assertRaisesRegex(error, message):
                            load(saved)
        for chunks, error in ((1, None), (3, lz4.block.LZ4BlockError)):
            with self.subTest(blocks=chunks):
                resource_data = random.Random(chunks).randbytes(chunks * chunk - 1000)
                bundle = head_nine_of(resource_data)
                self.assertEqual(blocks_of(bundle, "lz4"), chunks)
                saved = bundle.save(packer="lz4")
                if error is None:
                    self.assertEqual(load(saved)[0].files, {})
                else:
                    with self.assertRaises(error):
                        load(saved)
        self.assertEqual(blocks_of(no_blocks, "lz4"), 0)
        self.assertEqual(entries(load(no_blocks.save(packer="lz4"))[0]), {"e.resS": b""})
