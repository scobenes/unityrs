"""Vorbis fixture tools handle synthetic lacing and external runtime tables."""
import base64
import json
import struct
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "tools/fixtures"))
sys.path.insert(0, str(ROOT / "tools/parity"))
import audio_vorbis
import scratch


class VorbisTools(unittest.TestCase):
    def test_ogg_packet_continues_across_pages(self):
        def page(sizes, data):
            return b"OggS" + bytes(22) + bytes([len(sizes)]) + bytes(sizes) + data
        data = page([255], b"x" * 255) + page([2, 0, 1], b"yz!")
        self.assertEqual(audio_vorbis.packets(data), [b"x" * 255 + b"yz", b"", b"!"])

    def test_runtime_conversion_sorts_numeric_keys_and_preserves_packets(self):
        # Entirely invented bytes; no FMOD-derived table data enters a test.
        with scratch.folder("vorbis-table-") as folder:
            path = folder / "own.json"
            path.write_text(json.dumps({str(n): {"headerBytes": base64.b64encode(b"own").decode()}
                                        for n in [100, 20]}))
            self.assertEqual(audio_vorbis.runtime_table(path),
                             b"VST1" + struct.pack("<I", 2) +
                             b"".join(struct.pack("<II", n, 3) + b"own" for n in [20, 100]))

    def test_runtime_conversion_refuses_invalid_base64(self):
        with scratch.folder("vorbis-table-") as folder:
            path = folder / "own.json"
            path.write_text('{"0":{"headerBytes":"!"}}')
            with self.assertRaises(ValueError):
                audio_vorbis.runtime_table(path)


if __name__ == "__main__":
    unittest.main()
