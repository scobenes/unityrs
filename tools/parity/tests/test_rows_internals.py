"""PARITY.md rows on UnityPy's internals, run against the oracle: module aliases, special methods given other
operands, the UnityCN block decryption, objects after `patch`, version strings, `read_typetree`'s defaults and
UnityPyBoost's node finalizer.

Sections 7 to 12 and 23. Like `test_parity_rows.py`, every input is built here (`synth.py`), so nothing needs the
corpus, and each test names the rows it pins; a row's test column names the test back. Needs UnityPy 1.25.2 (the
oracle image); skipped elsewhere.
"""

import importlib
import io
import json
import operator
import resource
import struct
import subprocess
import sys
import unittest
from pathlib import Path
from unittest import mock

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE / "tests"))

from test_parity_rows import ORACLE, load, quietly, text_asset_file  # noqa: E402

import synth as S  # noqa: E402

if ORACLE:
    import lz4.block
    import UnityPy
    from UnityPy import UnityPyBoost, config
    from UnityPy.enums import ClassIDType, TextureFormat
    from UnityPy.export import Texture2DConverter
    from UnityPy.helpers import ArchiveStorageManager, CompressionHelper, TypeTreeHelper
    from UnityPy.helpers.Tpk import TpkUnityNode
    from UnityPy.helpers.TypeTreeNode import TypeTreeNode
    from UnityPy.helpers.UnityVersion import UnityVersion
    from UnityPy.streams import EndianBinaryReader

KEY = b"0123456789abcdef"


def decryptor_header(nibbles, key=KEY):
    """The 70 bytes of a UnityCN decryptor header (section 12) whose key vector decrypts to `nibbles`, the index table
    then the substitute table, and whose signature vector decrypts to UnityPy's signature."""
    from Crypto.Cipher import AES

    aes = AES.new(key, AES.MODE_ECB)
    plain_key = bytes(a << 4 | b for a, b in zip(nibbles[::2], nibbles[1::2], strict=True))
    key_vector, signature_vector = b"K" * 16, b"S" * 16
    data = bytes(a ^ b for a, b in zip(aes.encrypt(key_vector), plain_key, strict=True))
    signature = bytes(
        a ^ b for a, b in zip(aes.encrypt(signature_vector), ArchiveStorageManager.UNITY3D_SIGNATURE, strict=True)
    )
    return struct.pack(">I", 0) + data + key_vector + b"\0" + signature + signature_vector + b"\0"


def walk_end(block):
    """The last index the decryptor's LZ4-token walk visits (`helpers/ArchiveStorageManager.py:100-143`), given
    tables that leave every byte as it is; an index of `len(block)` is the byte just past the block."""
    last = -1

    def visit(i):
        nonlocal last
        last = max(last, i)
        return block[i] if i < len(block) else 0

    offset = 0
    while offset < len(block):
        at = offset
        token = visit(at)
        at += 1
        literals = token >> 4
        if literals == 0xF:
            more = 0xFF
            while more == 0xFF:
                more = visit(at)
                at += 1
                literals += more
        at += literals
        if at < len(block):
            visit(at)
            visit(at + 1)
            at += 2
            if token & 0xF == 0xF:
                more = 0xFF
                while more == 0xFF:
                    more = visit(at)
                    at += 1
        offset = at
    return last


@unittest.skipUnless(ORACLE, "needs UnityPy 1.25.2 (the oracle image)")
class InternalRows(unittest.TestCase):
    # Section 23: a module's own name for a class or function defined elsewhere (section 21, group `aliases`).
    def test_module_aliases(self):
        self.assertIs(importlib.import_module("UnityPy.classes.ClassIDTypeToClassMap").CIT, ClassIDType)
        self.assertIs(Texture2DConverter.TF, TextureFormat)
        self.assertIs(TypeTreeHelper.read_typetree_boost, UnityPyBoost.read_typetree)
        self.assertIs(importlib.import_module("UnityPy.helpers.TypeTreeNode").TypeTreeNodeC, UnityPyBoost.TypeTreeNode)
        self.assertEqual(TypeTreeNode.__bases__, (UnityPyBoost.TypeTreeNode,))

    # Sections 9.3, 10 and 11: `UnityVersion`'s comparisons, `TypeTreeNode.__eq__`, `TpkUnityNode.__eq__` and
    # `TpkUnityNode.__hash__`, given operands they do not expect.
    def test_special_methods_on_other_operands(self):
        version = UnityVersion.from_str("2019.4.15f1")
        compare = ("eq", "ne", "lt", "le", "gt", "ge")
        for other in (int(version), version.as_tuple(), UnityVersion.from_str("2019.4.15f1")):
            with self.subTest(other=other):
                self.assertEqual([getattr(operator, op)(version, other) for op in compare], [1, 0, 0, 1, 0, 1])
        for other in (None, 1.0, "x", b"x", [2019]):
            for op in compare:
                with (
                    self.subTest(other=other, op=op),
                    self.assertRaisesRegex(NotImplementedError, "^Unsupported comparison$"),
                ):
                    getattr(operator, op)(version, other)
        for call in (lambda: None == version, lambda: version in [None]):  # noqa: E711, SIM300 (the reflected call)
            with self.assertRaisesRegex(NotImplementedError, "^Unsupported comparison$"):
                call()
        # TypeTreeNode.__eq__ compares `to_dict()` and the children, so another operand raises reading its to_dict;
        # with __eq__ and no __hash__ of its own, the class is unhashable.
        node = TypeTreeNode(0, "int", "a", 4, 1)
        self.assertEqual(
            (node == TypeTreeNode(0, "int", "a", 4, 1), node == TypeTreeNode(0, "int", "b", 4, 1)), (True, False)
        )
        for call, kind in (
            (lambda: node == 5, "int"),
            (lambda: node != 5, "int"),
            (lambda: node == None, "NoneType"),  # noqa: E711
            (lambda: None == node, "NoneType"),  # noqa: E711, SIM300
            (lambda: node in [None], "NoneType"),
        ):
            with (
                self.subTest(kind=kind),
                self.assertRaisesRegex(AttributeError, f"^'{kind}' object has no attribute 'to_dict'$"),
            ):
                call()
        with self.assertRaisesRegex(TypeError, "^unhashable type: 'TypeTreeNode'$"):
            hash(node)

        # TpkUnityNode is slotted, with no __dict__, which its __eq__ and __hash__ read.
        def tpk_node():
            return TpkUnityNode(io.BytesIO(struct.pack("<HHihbIH", 1, 2, 4, 1, 0, 0, 0)))

        a, b = tpk_node(), tpk_node()
        self.assertFalse(hasattr(a, "__dict__"))
        for call in (lambda: a == b, lambda: a == a, lambda: a != b, lambda: hash(a), lambda: {a}):
            with (
                self.subTest(call=call),
                self.assertRaisesRegex(AttributeError, "^'TpkUnityNode' object has no attribute '__dict__'$"),
            ):
                call()
        self.assertEqual((a == 5, a != 5, a in [a]), (False, True, True))

    # Section 12: `UnityPyBoost.decrypt_block` and the Python loop where the LZ4-token walk runs past the block.
    def test_decrypt_block_past_the_block_end(self):
        manager = ArchiveStorageManager
        previous = manager.DECRYPT_KEY
        self.addCleanup(setattr, manager, "DECRYPT_KEY", previous)
        manager.DECRYPT_KEY = KEY
        identity = bytes(range(16)) + bytes(16)  # index 0..15, substitute 0: every byte decrypts to itself
        decryptor = manager.ArchiveStorageDecryptor(EndianBinaryReader(decryptor_header(identity)))
        self.assertEqual((decryptor.index, decryptor.substitute), (bytes(range(16)), bytes(16)))
        no_boost = mock.patch.object(manager, "UnityPyBoost", None)
        # A walk inside the block: both loops give the same bytes.
        self.assertEqual(walk_end(b"\0\0\0"), 2)
        with no_boost:
            self.assertEqual(bytes(decryptor.decrypt_block(b"\0\0\0", 0)), b"\0\0\0")
        self.assertEqual(bytes(decryptor.decrypt_block(b"\0\0\0", 0)), b"\0\0\0")
        # A walk one byte past the end: a match offset's second byte (`00 00`), a literal-length run (`f0`).
        for block in (b"\0\0", b"\xf0"):
            with self.subTest(block=block.hex()):
                self.assertEqual(walk_end(block), len(block))
                self.assertEqual(UnityPyBoost.decrypt_block(decryptor.index, decryptor.substitute, block, 0), block)
                self.assertEqual(bytes(decryptor.decrypt_block(block, 0)), block)
                with no_boost, self.assertRaisesRegex(IndexError, "^index out of bounds on dimension 1$"):
                    decryptor.decrypt_block(block, 0)
        # Through loading: an LZMA block flagged 0x101 whose walk does the same loads with UnityPyBoost and raises
        # without it. compress_lzma's output depends on liblzma, so the payload is found, not fixed.
        for size in range(400):
            payload = text_asset_file("cn", "y" * size)
            packed = CompressionHelper.compress_lzma(payload)
            if walk_end(packed) == len(packed):
                break
        else:
            self.fail("no payload whose LZMA block's walk ends one byte past it")
        info = bytes(16) + struct.pack(">i", 1) + struct.pack(">IIH", len(payload), len(packed), 0x101)
        info += struct.pack(">i", 1) + struct.pack(">qqI", 0, len(payload), 4) + b"CAB-cn\0"
        packed_info = lz4.block.compress(info, store_size=False)
        head = b"UnityFS\0" + struct.pack(">I", 8) + b"5.x.x\0" + b"2021.3.2f1\0"
        bundle = bytearray(head + struct.pack(">qIII", 0, len(packed_info), len(info), 0x1043))
        bundle += decryptor_header(identity)
        bundle += bytes(-len(bundle) % 16) + packed_info + packed
        struct.pack_into(">q", bundle, len(head), len(bundle))
        loaded = load(bytes(bundle))[0]
        self.assertEqual([o.parse_as_dict()["m_Name"] for o in loaded.files["CAB-cn"].objects.values()], ["cn"])
        with no_boost, self.assertRaisesRegex(IndexError, "^index out of bounds on dimension 1$"):
            load(bytes(bundle))

    # Section 8: after `patch` or `set_raw_data`, every read still reads the file's bytes.
    def test_patched_objects_read_the_original_bytes(self):
        new = S.s32("new") + S.s32("zz")
        for how in ("patch", "set_raw_data"):
            with self.subTest(how):
                sf = load(text_asset_file("old", "yy"), "p.assets")[0]
                obj = sf.objects[1]
                raw = obj.get_raw_data()
                if how == "patch":
                    self.assertEqual(obj.patch({"m_Name": "new", "m_Script": "zz"}), new)
                else:
                    obj.set_raw_data(new)
                self.assertEqual((obj.data, sf.is_changed), (new, True))
                reads = (
                    obj.parse_as_dict()["m_Name"],
                    obj.read().m_Name,
                    obj.parse_as_object().m_Name,
                    obj.peek_name(),
                )
                self.assertEqual(reads, ("old",) * 4)
                self.assertEqual((obj.get_raw_data(), obj.byte_size), (raw, 16))
                saved = load(sf.save(), "r.assets")[0]
                self.assertEqual(saved.objects[1].parse_as_dict()["m_Name"], "new")

    # Sections 7.2 and 11: a version string `UnityVersion.VersionPattern` does not match.
    def test_an_unmatched_version_string_raises(self):
        for version in ("2017.4", "abc"):
            with self.subTest(version), self.assertRaisesRegex(ValueError, f"^Invalid version string: {version}$"):
                UnityVersion.from_str(version)
            for fallback, fmt in ((None, 9), (None, 15), (None, 22), ("2020.1.1f1", 22)):
                data = text_asset_file(unity_version=version.encode(), version=fmt)
                fallback_set = mock.patch.object(config, "FALLBACK_UNITY_VERSION", fallback)
                with self.subTest(version, fallback=fallback, format=fmt), fallback_set:
                    with self.assertRaisesRegex(ValueError, f"^Invalid version string: {version}$"):
                        load(data, "v.assets")
                    with self.assertRaisesRegex(ValueError, f"^Invalid version string: {version}$"):
                        quietly(UnityPy.load, data)
        # The reference: the same file with a version the pattern matches.
        self.assertEqual(
            load(text_asset_file(unity_version=b"5.6.7"), "v.assets")[0].version.as_tuple(), (5, 6, 7, 3, 0)
        )

    # Section 9.1: `TypeTreeHelper.read_typetree` called with its own defaults.
    def test_read_typetree_with_its_own_defaults_raises(self):
        obj = load(text_asset_file("old", "yy"), "t.assets")[0].objects[1]
        node = obj._get_typetree_node()

        def reader():
            return EndianBinaryReader(obj.get_raw_data(), endian="<")

        r = reader()
        with self.assertRaisesRegex(ValueError, "^Expected to read None bytes, but only read 16 bytes$"):
            TypeTreeHelper.read_typetree(node, r)
        self.assertEqual(r.Position, 16)
        with self.assertRaisesRegex(ValueError, "^Expected to read 0 bytes, but only read 16 bytes$"):
            TypeTreeHelper.read_typetree(node, reader(), byte_size=0)
        for kw in ({"byte_size": 16}, {"check_read": False}):
            with self.subTest(**kw):
                self.assertEqual(
                    TypeTreeHelper.read_typetree(node, reader(), **kw), {"m_Name": "old", "m_Script": "yy"}
                )

    # Section 9.1: `UnityPyBoost.TypeTreeNode.__del__`, the C type's finalizer, called by hand.
    def test_boost_node_finalizer_releases_its_fields(self):
        # In a child process, which ends with os._exit: what the process does after the call is not characterised.
        code = (
            "import json, os, sys\n"
            "from UnityPy import UnityPyBoost\n"
            "from UnityPy.helpers.TypeTreeNode import TypeTreeNode\n"
            "cls = {'subclass': TypeTreeNode, 'boost': UnityPyBoost.TypeTreeNode}[sys.argv[1]]\n"
            "name = ''.join(['node ', 'name'])\n"
            "node = cls(0, 'int', name, 4, 1)\n"
            "before = sys.getrefcount(name)\n"
            "node.__del__()\n"
            "print(json.dumps([before - sys.getrefcount(name), repr(node)]), flush=True)\n"
            "os._exit(0)\n"
        )

        def no_core_dump():
            resource.setrlimit(resource.RLIMIT_CORE, (0, 0))

        for which in ("subclass", "boost"):
            with self.subTest(which):
                child = subprocess.run(
                    [sys.executable, "-c", code, which], capture_output=True, text=True, preexec_fn=no_core_dump
                )
                self.assertEqual(child.returncode, 0, child.stderr)
                released, shown = json.loads(child.stdout)
                self.assertEqual(released, 1)
                self.assertEqual(shown, "TypeTreeNode(m_Level=0, m_Type='int', m_Name='node name', m_MetaFlag=None)")


if __name__ == "__main__":
    unittest.main()
