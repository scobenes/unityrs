"""Synthetic Vorbis from SoundFile 0.13.1 / libsndfile 1.2.2, never from FMOD's table.

Use the pinned oracle image with SoundFile installed for this test only.
--check rebuilds encoder packets and independent float32-to-int16 samples
in the channel order observed through FMOD on ordinary Ogg Vorbis streams.
No setup-table data, its digests or derived outputs are recorded. The
--runtime-table mode reads an external Fmod5Sharp JSON table, emits VST1 to
stdout, and never writes a file; Rust's runtime-only test consumes the pipe.
"""
import argparse
import io
import json
import struct
import sys
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools/parity"))


def packets(data):
    """Ogg lacing, including packets continued across pages (encoder output)."""
    pos, pending, result = 0, bytearray(), []
    while pos < len(data):
        assert data[pos:pos + 4] == b"OggS"
        count = data[pos + 26]
        sizes = data[pos + 27:pos + 27 + count]
        pos += 27 + count
        for size in sizes:
            pending += data[pos:pos + size]
            pos += size
            if size < 255:
                result.append(bytes(pending))
                pending.clear()
    assert not pending
    return result


def runtime_table(path):
    # This output and anything using it must remain in memory, never a fixture.
    table = json.loads(Path(path).read_text())
    import base64
    result = bytearray(b"VST1" + struct.pack("<I", len(table)))
    for key in sorted(table, key=int):
        packet = base64.b64decode(table[key]["headerBytes"], validate=True)
        result += struct.pack("<II", int(key), len(packet)) + packet
    return result


def fmod_channel_order(ogg, decoded):
    """Observe output-to-stream indices; retain no FMOD PCM or digest."""
    import numpy as np
    from fmod_toolkit import get_pyfmodex_system_instance
    from fmod_toolkit.fmod import subsound_to_wav
    from pyfmodex.flags import INIT_FLAGS, MODE
    from pyfmodex.structure_declarations import CREATESOUNDEXINFO

    channels = decoded.shape[1]
    system, _ = get_pyfmodex_system_instance(32, INIT_FLAGS.NORMAL)
    assert system.version == 0x20022
    # raw_to_wav iterates subsounds; an ordinary Ogg is the sound itself.
    sound = system.create_sound(ogg, MODE.OPENMEMORY,
                                exinfo=CREATESOUNDEXINFO(length=len(ogg)))
    try:
        wav = subsound_to_wav(sound)
    finally:
        sound.release()
    actual = np.frombuffer(wav[44:], dtype="<i2").reshape(-1, channels)
    reference = (decoded * 32768).clip(-32768, 32767).astype("<i2")
    assert actual.shape == reference.shape
    errors = np.array([[np.max(np.abs(actual[:, c].astype(int) - reference[:, v]))
                        for v in range(channels)] for c in range(channels)])
    order = errors.argmin(axis=1).tolist()
    assert sorted(order) == list(range(channels))
    assert max(errors[range(channels), order]) <= 2
    return order


def record():
    import numpy as np
    import soundfile as sf
    from audio import bank
    import child
    import UnityPy
    import fmod_toolkit

    assert UnityPy.__version__ == "1.25.2"
    assert fmod_toolkit.__version__ == "0.1.3"
    assert sf.__version__ == "0.13.1" and sf.__libsndfile_version__ == "1.2.2"
    cases, orders = [], {}
    selected = {1, 2, 3, 4, 5, 6, 7, 8, 9, 16, 32}
    configs = [(1, 44100, 2048), (2, 44100, 4097), (2, 48000, 8192),
               (6, 44100, 2048), (8, 48000, 2048)]
    configs += [(c, 44100, 512) for c in range(1, 33) if c not in (1, 2, 6, 8)]
    for channels, frequency, frames in configs:
        t = np.arange(frames, dtype=np.float64) / frequency
        signal = np.array([0.7 * np.sin(t * 2 * np.pi * (440 + c * 233)) for c in range(channels)]).T
        stream = io.BytesIO()
        sf.write(stream, signal, frequency, format="OGG", subtype="VORBIS")
        ogg = stream.getvalue()
        p = packets(ogg)
        decoded, rate = sf.read(io.BytesIO(ogg), dtype="float32", always_2d=True)
        assert decoded.shape == (frames, channels) and rate == frequency
        order = fmod_channel_order(ogg, decoded)
        orders[str(channels)] = order
        if channels not in selected:
            continue
        assert p[0][28:] == b"\xb8\x01", "FSB5 uses 256/2048 blocks"
        setup = p[2]  # From our encoder, independent of any FMOD table.
        crc = zlib.crc32(setup)
        data = b"".join(struct.pack("<H", len(packet)) + packet for packet in p[3:]) + b"\0\0"
        subs = [(data, frames, channels, {44100: 8, 48000: 9}[frequency], [(11, struct.pack("<I", crc))])]
        encoded = bank(15, subs)
        pcm = (decoded[:, order] * 32768).clip(-32768, 32767).astype("<i2").tobytes()
        request = {"data": encoded.hex(), "name": "tone", "compression": 1}
        proc = child.run([sys.executable, str(ROOT / "tools/fixtures/audio.py"), "--record-case"],
                         input=json.dumps(request), capture_output=True, text=True, timeout=180, check=True)
        oracle = json.loads(proc.stdout)["oracle"]
        cases.append({"case": f"vorbis_c{channels}_{frequency}_{frames}", "bank": encoded.hex(),
                      "setup": setup.hex(), "crc": crc, "channels": channels, "frequency": frequency,
                      "frames": frames, "independent_fmod_order_pcm16": pcm.hex(), "oracle": oracle})
    return {"reference": "UnityPy 1.25.2 / fmod_toolkit 0.1.3 / FMOD 2.00.22 Linux x86_64",
            "encoder_decoder": "SoundFile 0.13.1 / libsndfile 1.2.2 (bundled libvorbis)",
            "provenance": "All setup packets and bank payloads are independently encoder-generated; no FMOD table used.",
            "ogg_fmod_channel_order": {str(c): orders[str(c)] for c in range(1, 33)},
            "cases": cases}


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--check", action="store_true")
    parser.add_argument("--runtime-table", type=Path)
    args = parser.parse_args()
    if args.runtime_table:
        sys.stdout.buffer.write(runtime_table(args.runtime_table))
        return
    body = json.dumps(record(), indent=2) + "\n"
    path = ROOT / "crates/unityrs-audio/tests/fixtures/vorbis.json"
    if args.check:
        if path.read_text() != body:
            parser.exit(1, "stale Vorbis fixture; regenerate in the pinned oracle image\n")
    else:
        path.write_text(body)


if __name__ == "__main__":
    main()
