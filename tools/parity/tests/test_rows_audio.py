"""PARITY.md's FSB5 rows on subsounds too short for fmod_toolkit's WAV writer, run against the oracle.

Sections 16 and 19.2. The banks are built by `export_support.fsb5` from the format's own rules, not by UnityPy, so
nothing needs the corpus; each test names the rows it pins, and a row's test column names the test back. Needs
UnityPy 1.25.2 (the oracle image); skipped elsewhere.
"""

import ctypes
import re
import struct
import sys
import traceback
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest import mock

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE / "tests"))

from export_support import fsb5, wav_header  # noqa: E402
from rows_support import ORACLE  # noqa: E402

if ORACLE:
    import fmod_toolkit  # loads the bundled libfmod, which pyfmodex needs first
    import pyfmodex
    from pyfmodex.exceptions import FmodError
    from UnityPy.export import AudioClipConverter


def too_short(size):
    """fmod_toolkit's raise for WAV data of `size` bytes, under 4."""
    text = "pack_into requires a buffer of at least 44 bytes for packing 8 bytes at offset 36"
    return f"^{re.escape(f'{text} (actual buffer size is {40 + size})')}$"


@unittest.skipUnless(ORACLE, "needs UnityPy 1.25.2 (the oracle image)")
class AudioRows(unittest.TestCase):
    # Sections 16, 19.2: fmod_toolkit allocates the data's length plus 40 bytes and packs the data chunk's header at
    # offset 36, so fewer than 4 data bytes raise.
    def test_short_pcm_subsounds(self):
        pcm = struct.pack("<8h", 1, -1, 2, -2, 3, -3, 4, -4)
        one, four = (pcm[:2], 1), (pcm[:8], 4)
        # Through FMOD: one mono PCM16 sample, alone or beside a subsound of four in either order, raises, and the
        # whole call with it.
        for samples in ((one,), (one, four), (four, one)):
            counts = [count for _, count in samples]
            with self.subTest(samples=counts), self.assertRaisesRegex(struct.error, too_short(2)):
                fmod_toolkit.raw_to_wav(fsb5(2, *samples), "c", 1, 44100)
        # So does UnityPy's export of a clip holding such a bank.
        clip = SimpleNamespace(m_AudioData=fsb5(2, one), m_Resource=None, m_Name="c", m_Channels=1, m_Frequency=44100)
        with self.assertRaisesRegex(struct.error, too_short(2)):
            AudioClipConverter.extract_audioclip_samples(clip)
        # A second subsound of no samples, last or before another, FMOD opens and hands over as 0 bytes, which raise
        # the same way.
        empty = (b"", 0)
        for samples in ((four, empty), (four, empty, four)):
            counts = [count for _, count in samples]
            with self.subTest(samples=counts), self.assertRaisesRegex(struct.error, too_short(0)):
                fmod_toolkit.raw_to_wav(fsb5(2, *samples), "c", 1, 44100)
        # A bank whose first subsound has no samples FMOD refuses to open, whatever follows: the refusal comes from
        # `System.create_sound`, called at `fmod.py` line 61, so fmod_toolkit's sound_to_wav is never reached, and
        # UnityPy's export of the clip raises it.
        for samples in ((empty,), (empty, empty), (empty, one), (empty, four), (empty, four, four)):
            counts = [count for _, count in samples]
            spy = mock.patch.object(fmod_toolkit.fmod, "sound_to_wav", wraps=fmod_toolkit.fmod.sound_to_wav)
            with self.subTest(samples=counts), spy as sound_to_wav:
                try:
                    fmod_toolkit.raw_to_wav(fsb5(2, *samples), "c", 1, 44100)
                except Exception as error:  # noqa: BLE001 - its type is the assertion
                    refused = (type(error), str(error))
                    frames = [
                        (Path(f.filename).name, f.lineno, f.name) for f in traceback.extract_tb(error.__traceback__)
                    ]
                else:
                    self.fail("FMOD opened the bank")
                self.assertEqual(refused, (FmodError, "PLUGIN"))
                self.assertFalse(sound_to_wav.called)
                opened = frames.index(("fmod.py", 61, "raw_to_wav"))
                self.assertEqual(frames[opened + 1][2], "create_sound")
        clip.m_AudioData = fsb5(2, empty, four)
        with self.assertRaisesRegex(FmodError, "^PLUGIN$"):
            AudioClipConverter.extract_audioclip_samples(clip)
        # From 4 bytes the samples, put at offset 44, grow the buffer back: two mono samples, or one stereo sample.
        two = fmod_toolkit.raw_to_wav(fsb5(2, (pcm[:4], 2)), "c", 1, 44100)
        self.assertEqual(two, {"c.wav": wav_header(1, 44100, 16, 4) + pcm[:4]})
        stereo = fmod_toolkit.raw_to_wav(fsb5(2, (pcm[:4], 1), channels=2), "c", 2, 44100)
        self.assertEqual(stereo, {"c.wav": wav_header(2, 44100, 16, 4) + pcm[:4]})
        self.assertEqual(len(stereo["c.wav"]), 48)

        # On a stand-in for FMOD's subsound, which reaches the widths the bundled libfmod refuses in a bank built
        # here: 0 to 3 bytes of PCM8, PCM16, PCM24 and PCM32 raise, and 4 bytes write.
        payload = struct.pack("<3f", 0.5, -0.5, 0.25)
        buffer = ctypes.create_string_buffer(payload, len(payload))

        def subsound(name, bits, size):
            return SimpleNamespace(
                format=SimpleNamespace(format=getattr(pyfmodex.enums.SOUND_FORMAT, name), channels=1, bits=bits),
                default_frequency=8000,
                get_length=lambda unit: size,
                lock=lambda offset, length: [(ctypes.addressof(buffer), ctypes.c_uint(size))],
                unlock=lambda *parts: None,
            )

        for name, bits in (("PCM8", 8), ("PCM16", 16), ("PCM24", 24), ("PCM32", 32)):
            for size in range(4):
                with self.subTest(name, size=size), self.assertRaisesRegex(struct.error, too_short(size)):
                    fmod_toolkit.fmod.subsound_to_wav(subsound(name, bits, size))
            written = fmod_toolkit.fmod.subsound_to_wav(subsound(name, bits, 4))
            self.assertEqual(written, wav_header(1, 8000, bits, 4) + payload[:4])
        # PCM float: converted, the WAV data is half the float bytes, so one float raises and two write 48 bytes;
        # kept as floats, one float writes 48 bytes.
        convert = fmod_toolkit.fmod.subsound_to_wav
        with self.assertRaisesRegex(struct.error, too_short(2)):
            convert(subsound("PCMFLOAT", 32, 4))
        halves = struct.pack("<2h", 16384, -16384)
        self.assertEqual(convert(subsound("PCMFLOAT", 32, 8)), wav_header(1, 8000, 16, 4) + halves)
        self.assertEqual(
            convert(subsound("PCMFLOAT", 32, 4), convert_pcm_float=False),
            wav_header(1, 8000, 32, 4, audio_format=3) + payload[:4],
        )


if __name__ == "__main__":
    unittest.main()
