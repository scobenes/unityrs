"""Vorbis fixture tools handle synthetic lacing and external runtime tables.

Each check runs in a child (`--run NAME`, through tools/parity/child.py): the
parity tests share one process, whose in-process dumps refuse a module loaded
from outside the parity tools and the pinned wheels (oracle_env.py's
`LoadedModules`), and `audio_vorbis` lives in tools/fixtures.
"""
import sys
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parents[1]
ROOT = HERE.parents[1]
sys.path.insert(0, str(HERE))

import child  # noqa: E402

CHECKS = (
    "test_ogg_packet_continues_across_pages",
    "test_runtime_conversion_sorts_numeric_keys_and_preserves_packets",
    "test_runtime_conversion_refuses_invalid_base64",
)


def child_checks():
    """The checks, defined only in the child, where importing audio_vorbis is harmless."""
    import base64
    import json
    import struct

    sys.path.insert(0, str(ROOT / "tools/fixtures"))
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

    return VorbisTools


class VorbisToolsInAChild(unittest.TestCase):
    def check(self, name):
        proc = child.run([sys.executable, str(Path(__file__)), "--run", name],
                         cwd=ROOT, capture_output=True, text=True)
        self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)

    def test_ogg_packet_continues_across_pages(self):
        self.check(CHECKS[0])

    def test_runtime_conversion_sorts_numeric_keys_and_preserves_packets(self):
        self.check(CHECKS[1])

    def test_runtime_conversion_refuses_invalid_base64(self):
        self.check(CHECKS[2])


if __name__ == "__main__":
    if sys.argv[1:2] == ["--run"] and sys.argv[2:3] and sys.argv[2] in CHECKS:
        result = unittest.TextTestRunner(verbosity=2).run(child_checks()(sys.argv[2]))
        sys.exit(0 if result.wasSuccessful() else 1)
    unittest.main()
