"""bundle_nodes.py reads UnityFS nodes from stored, LZ4 and LZMA blocks, and refuses invalid LZ4 matches."""

import lzma
import struct
import sys
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))

import scratch  # noqa: E402
from bundle_nodes import bundle_nodes, decompress, lz4_block  # noqa: E402


def require(condition, message):
    if not condition:
        raise ValueError(message)


def literal_lz4(raw):
    """One LZ4 sequence holding only literals (the format's last sequence)."""
    n = len(raw)
    if n < 15:
        return bytes([n << 4]) + raw
    extra, rest = n - 15, b""
    while extra >= 255:
        rest += b"\xff"
        extra -= 255
    return b"\xf0" + rest + bytes([extra]) + raw


def lzma_block(raw):
    lc, lp, pb, size = 3, 0, 2, 1 << 16
    filters = [dict(id=lzma.FILTER_LZMA1, lc=lc, lp=lp, pb=pb, dict_size=size)]
    return bytes([(pb * 5 + lp) * 9 + lc]) + size.to_bytes(4, "little") + lzma.compress(
        raw, format=lzma.FORMAT_RAW, filters=filters)


class BundleNodeTests(unittest.TestCase):
    def test_lz4_copies_overlapping_matches_and_long_literals(self):
        block = bytes([0x35]) + b"abc" + b"\x03\x00" + bytes([0x10]) + b"X"
        self.assertEqual(lz4_block(block, 13, require), b"abcabcabcabcX")
        raw = bytes(i % 256 for i in range(300))
        self.assertEqual(lz4_block(literal_lz4(raw), 300, require), raw)

    def test_lz4_refuses_a_match_before_the_output(self):
        with self.assertRaisesRegex(ValueError, "invalid LZ4 match"):
            lz4_block(bytes([0x15]) + b"a" + b"\x02\x00" + bytes([0x10]) + b"X", 11, require)

    def test_lzma_block_round_trips(self):
        raw = b"unityrs fixture texels " * 40
        self.assertEqual(decompress(lzma_block(raw), len(raw), 1, require), raw)

    def test_lz4_bundle_nodes_resolve_by_name(self):
        nodes = [("CAB-a", b"serialized"), ("CAB-a.resS", bytes(range(1, 200)))]
        payload = b"".join(data for _, data in nodes)
        directory, offset = b"", 0
        for name, data in nodes:
            directory += struct.pack(">qqI", offset, len(data), 0) + name.encode() + b"\0"
            offset += len(data)
        block = literal_lz4(payload)
        info = bytes(16) + struct.pack(">iIIH", 1, len(payload), len(block), 2)
        info += struct.pack(">i", len(nodes)) + directory
        stored_info = literal_lz4(info)
        head = b"UnityFS\0" + struct.pack(">I", 8) + b"5.x.x\0" + b"2022.3.62f3\0"
        padding = bytes(-(len(head) + 20) % 16)
        body = stored_info + block
        data = head + struct.pack(">qIII", len(head) + 20 + len(padding) + len(body),
                                  len(stored_info), len(info), 0x42) + padding + body
        path = scratch.for_case(self) / "bundle"
        path.write_bytes(data)
        self.assertEqual(bundle_nodes(path, require), dict(nodes))


if __name__ == "__main__":
    unittest.main()
