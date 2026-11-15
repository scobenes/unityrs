"""Record synthetic L8 AudioClip exports with UnityPy 1.25.2/fmod_toolkit 0.1.3.

Run this generator in the oracle container; --check compares recorded bytes.
FMOD is a test oracle only. Every case runs in a child, so malformed banks
cannot kill the recorder. Refused PCM widths remain native WAV-rule tests
until owner-gated Editor fixtures (G8) are available.
"""
import argparse
import json
import random
import struct
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools/parity"))
sys.path.insert(0, str(ROOT / "tools/parity/tests"))


def bank(mode, entries, version=1, names=None):
    """FSB5 format facts: (payload, frames, channels, rate index, chunks)."""
    headers, data = bytearray(), bytearray()
    for payload, frames, channels, rate, chunks in entries:
        data += bytes(-len(data) % 32)
        if channels not in (1, 2, 6, 8):
            chunks = [(1, bytes([channels])), *chunks]
        channel_bits = {1: 0, 2: 1, 6: 2, 8: 3}.get(channels, 0)
        raw = bool(chunks) | rate << 1 | channel_bits << 5 | (len(data) // 32) << 7 | frames << 34
        headers += struct.pack("<Q", raw)
        for i, (kind, value) in enumerate(chunks):
            headers += struct.pack("<I", kind << 25 | len(value) << 1 | (i + 1 < len(chunks)))
            headers += value
        data += payload
    name_table = bytearray()
    if names is not None:
        strings = bytearray()
        for name in names:
            name_table += struct.pack("<I", len(names) * 4 + len(strings))
            strings += name + b"\0"
        name_table += strings
    head = b"FSB5" + struct.pack("<6I", version, len(entries), len(headers), len(name_table), len(data), mode)
    return head + bytes((64 if version == 0 else 60) - len(head)) + headers + name_table + data


def ima_frame(channels, rng, step=None, bad_channel=0):
    """Xbox headers/4-byte interleave through stereo; FSB/2-byte above it."""
    histories = [rng.randrange(-32768, 32768) for _ in range(channels)]
    steps = [rng.randrange(89) for _ in range(channels)]
    if step is not None:
        steps[bad_channel] = step
    if channels <= 2:
        header = b"".join(struct.pack("<hBB", h, s, 0x55) for h, s in zip(histories, steps))
    else:
        header = struct.pack(f"<{channels}h", *histories) + b"".join(bytes([s, 0x55]) for s in steps)
    return header + rng.randbytes(32 * channels)


def fadpcm_frame(channels, rng, coef=None, shift=None):
    """Each channel has its own histories and eight groups of 32 nibbles."""
    frames = []
    for _ in range(channels):
        coefs = sum((rng.randrange(5) if coef is None else coef) << (4 * i) for i in range(8))
        shifts = sum((rng.randrange(16) if shift is None else shift) << (4 * i) for i in range(8))
        frames.append(struct.pack("<IIhh", coefs, shifts, rng.randrange(-32768, 32768),
                                  rng.randrange(-32768, 32768)) + rng.randbytes(128))
    return b"".join(frames)


def adpcm_cases(add):
    rng = random.Random(0xADFC5)
    for mode, label, frame, samples, size in [(7, "ima", ima_frame, 64, 0x24),
                                              (16, "fadpcm", fadpcm_frame, 256, 0x8c)]:
        for channels in [1, 2, 3, 4, 5, 6, 8]:
            payload = b"".join(frame(channels, rng) for _ in range(3))
            add(f"{label}_c{channels}_frames3", bank(mode, [(payload, samples * 3, channels, 8, [])]))
            for missing in [1, size - 1]:
                add(f"{label}_c{channels}_short_last_{missing}",
                    bank(mode, [(payload[:-missing], samples * 3, channels, 8, [])]))
            add(f"{label}_c{channels}_short_first",
                bank(mode, [(payload[:size * channels - 1], samples, channels, 8, [])]), "error")
        payload = frame(1, rng) + frame(1, rng)
        for count in [samples - 1, samples, samples + 1, samples * 2 - 1, samples * 2,
                      samples * 2 + 1, samples * 3]:
            add(f"{label}_count_{count}", bank(mode, [(payload, count, 1, 8, [])]))
        # The first frame straddles the next subsound's aligned start.
        add(f"{label}_first_crosses_next", bank(mode, [(payload[:16], samples, 1, 8, []),
                                                      (payload, samples, 1, 8, [])]))
        # Over-claimed samples consume padding, then the next subsound.
        add(f"{label}_reads_next", bank(mode, [(payload, samples * 4, 1, 8, []),
                                             (payload, samples * 2, 1, 8, [])]))
        add(f"{label}_reads_trailing", bank(mode, [(payload, samples * 3, 1, 8, [])]) + frame(1, rng))
        add(f"{label}_first_completed_by_trailing",
            bank(mode, [(payload[:16], samples, 1, 8, [])]) + payload[16:size])
        add(f"{label}_later_short_first", bank(mode, [(payload, samples, 1, 8, []),
                                                     (payload[:size - 1], samples, 1, 8, [])]), "error")
        add(f"{label}_short_mono", bank(mode, [(payload, 1, 1, 8, [])]), tier_c=True)
        add(f"{label}_later_empty", bank(mode, [(payload, samples, 1, 8, []),
                                               (b"", 0, 1, 8, [])]), tier_c=True)
        add(f"{label}_empty_first", bank(mode, [(payload, 0, 1, 8, [])]), "emptyfirst")
    for channels in [1, 2, 6]:
        for step in [88, 89, 90, 100, 127, 128, 240, 255]:
            for bad_channel in ([0, 1] if channels == 2 else [min(channels - 1, 3)]):
                payload = (ima_frame(channels, rng) + ima_frame(channels, rng, step, bad_channel)
                           + ima_frame(channels, rng))
                add(f"ima_c{channels}_step_{step}_channel{bad_channel}",
                    bank(7, [(payload, 192, channels, 8, [])]))
    payload = bytearray(ima_frame(1, rng))
    for last in [0, 15]:
        payload[-1] = (payload[-1] & 15) | last << 4
        add(f"ima_ignored_last_nibble_{last}", bank(7, [(payload, 64, 1, 8, [])]))
    for coef in range(16):
        # Same histories/body distinguish every coefficient, including 5-7;
        # 8-15 must replay exactly like 0-7 on FMOD 2.00.22 Linux x86_64.
        add(f"fadpcm_coef_{coef}", bank(16, [(fadpcm_frame(1, random.Random(55), coef, 4),
                                             256, 1, 8, [])]))
    for shift in range(16):
        add(f"fadpcm_shift_{shift}", bank(16, [(fadpcm_frame(1, random.Random(66), 2, shift),
                                               256, 1, 8, [])]))
    clipped = struct.pack("<IIhh", 0x22222222, 0xcccccccc, 32000, -32000) + b"\x77" * 128
    add("fadpcm_clipping", bank(16, [(clipped, 256, 1, 8, [])]))


def cases():
    out = []

    def add(name, data, rust="ok", tier_c=False, **fields):
        out.append(dict(case=name, data=data.hex(), name="clip", rust=rust, tier_c=tier_c,
                        compression=fields.pop("compression", 0), **fields))

    for label, data in [("ogg", b"OggSsynthetic"), ("riff", b"RIFFsynthetic"),
                        ("m4a", b"\0\0\0\x10ftypM4A synthetic"), ("riff_before_m4a", b"RIFFftyp")]:
        add(label, data)
    add("legacy_ogg", b"OggSlegacy", legacy_type=14, compression=None)
    add("legacy_non_passthrough", b"FORMsynthetic", "unsupported", legacy_type=2, compression=None)
    pcm = struct.pack("<8h", -32768, -16384, -1, 0, 1, 1234, 16384, 32767)
    for channels in [1, 2]:
        for rate in range(11):
            add(f"pcm16_c{channels}_r{rate}", bank(2, [(pcm, 8 // channels, channels, rate, [])]))
    add("version0", bank(2, [(pcm, 8, 1, 8, [])], version=0))
    add("named_multi", bank(2, [(pcm, 8, 1, 8, []), (pcm[:8], 2, 2, 9, [])],
                             names=[b"first", b"second"]))
    add("overrides", bank(2, [(pcm, 8, 1, 0, [(1, b"\1"), (2, struct.pack("<I", 12345))])]))
    # pyfmodex 0.7.2 sound.py:169-184 returns c_float GetDefaults;
    # fmod_toolkit 0.1.3 fmod.py:117 truncates it with int(), then :164-165
    # write sample_rate and sample_rate * channels * bits // 8.
    for frequency in [44100, 16_777_215, 16_777_216, 16_777_217, 16_777_219]:
        add(f"frequency_override_{frequency}",
            bank(2, [(pcm, 8, 1, 8, [(2, struct.pack("<I", frequency))])]))
    add("short_mono", bank(2, [(pcm[:2], 1, 1, 8, [])]), tier_c=True)
    add("later_empty", bank(2, [(pcm, 8, 1, 8, []), (b"", 0, 1, 8, [])]), tier_c=True)
    add("empty_first", bank(2, [(b"", 0, 1, 8, []), (pcm, 8, 1, 8, [])]), "emptyfirst")
    for mode in [1, 3, 4, 5]:
        add(f"refused_pcm{mode}", bank(mode, [(bytes(24), 24 // {1: 1, 3: 3, 4: 4, 5: 4}[mode], 1, 8, [])]))
    adpcm_cases(add)
    for mode in [11, 15, 17]:
        add(f"deferred_codec{mode}", bank(mode, [(bytes(64), 32, 1, 8, [])]), "unsupported")
    good = bank(2, [(pcm, 8, 1, 8, [])])
    for field, offset, value in [("count", 8, 0xffffffff), ("headers", 12, 0xffffffff),
                                 ("names", 16, 0xffffffff), ("data", 20, 0xffffffff), ("version", 4, 9)]:
        hostile = bytearray(good)
        struct.pack_into("<I", hostile, offset, value)
        add(f"hostile_{field}", hostile, "error")
    add("truncated", good[:-1], "error")
    add("bad_name", bank(2, [(pcm, 8, 1, 8, [])], names=[b"broken"])[:-17], "error")
    review_cases(add)
    return out


def review_cases(add):
    """Round-1 boundaries; independent of the original cases' random stream."""
    for mode, label, size, samples in [(7, "ima", 36, 64), (16, "fadpcm", 140, 256)]:
        for channels in [2, 3, 6, 8]:
            if mode == 16:
                frame = b"".join(struct.pack("<IIhh", 0, 0, 0, 0)
                                 + bytes([(c + 1) * 17]) * 128 for c in range(channels))
            elif channels == 2:
                frame = b"".join(struct.pack("<hBB", 100 + c, 0, 0) for c in range(channels))
                frame += bytes(32 * channels)
            else:
                frame = struct.pack(f"<{channels}h", *range(100, 100 + channels))
                frame += bytes(34 * channels)
            for count in [257, 259, 511, 512]:
                payload = frame * ((count + samples - 1) // samples)
                for shape, missing in [("full", 0), ("short1", 1), ("short_frame", size)]:
                    add(f"{label}_c{channels}_count{count}_{shape}",
                        bank(mode, [(payload[:len(payload) - missing], count, channels, 8, [])]))
    for mode, label, size, samples in [(1, "refused_pcm1", 1, 4), (2, "pcm16", 2, 4),
                                       (3, "refused_pcm3", 3, 4), (4, "refused_pcm4", 4, 4),
                                       (5, "refused_pcm5", 4, 4), (7, "ima", 36, 64),
                                       (16, "fadpcm", 140, 256)]:
        for channels in [32, 33, 64]:
            payload = bytes(size * channels * (1 if mode in [7, 16] else samples))
            add(f"{label}_c{channels}_limit", bank(mode, [(payload, samples, channels, 8, [])]),
                "ok" if channels == 32 else "channels", channels=channels)


def record_extensions():
    """Evaluate the property, including the two enums' colliding integer keys.

    UnityPy 1.25.2: classes/legacy_patch/AudioClip.py:5-6 and enums/Audio.py:33-56.
    The final dict alone loses the overwritten AudioType keys' identities.
    """
    from types import SimpleNamespace
    from UnityPy.classes import AudioClip
    from UnityPy.enums import AudioCompressionFormat, AudioType

    inputs = [(f"{enum.__name__}.{value.name}", int(value))
              for enum in (AudioCompressionFormat, AudioType) for value in enum]
    inputs += [("none", None), ("negative", -1), ("unknown", 999)]
    return [{"case": label, "compression": value,
             "extension": AudioClip.extension.fget(SimpleNamespace(m_CompressionFormat=value))}
            for label, value in inputs]


def source_cases():
    """Source selection and actual resource ranges, not a replacement fetcher."""
    def case(label, inline, source="audio.resS", offset=3, size=8, resources=None, rust="ok", **extra):
        return dict(kind="source", case=label, name="clip", data=None if inline is None else inline.hex(),
                    resource=None if source is None else dict(source=source, offset=offset, size=size),
                    resources=resources if resources is not None else {"audio.resS": b"preRIFFdatapost".hex()},
                    rust=rust, **extra)

    return [
        case("inline_precedes_resource", b"OggSinline"),
        case("inline_precedes_missing_resource", b"OggSinline", source="missing.resS"),
        case("legacy_array_precedes_resource", b"OggSinline", legacy_array=True),
        case("empty_inline_streamed_range", b""),
        case("absent_inline_streamed_range", None),
        case("streamed_ogg_range", b"", resources={"audio.resS": b"preOggSdatapost".hex()}),
        case("missing_resource", b"", source="missing.resS", rust="missing"),
        case("neither_source", b"", source=None, rust="neither"),
        case("empty_existing_resource", b"", offset=0, size=0, rust="empty"),
        case("empty_missing_resource", b"", source="missing.resS", offset=0, size=0, rust="empty"),
    ]


def oracle_files(clip):
    from UnityPy.export.AudioClipConverter import extract_audioclip_samples

    try:
        files = extract_audioclip_samples(clip)
        return {"files": {name: value.hex() for name, value in files.items()}}
    except Exception as error:  # noqa: BLE001 - recording the oracle's exception is intentional
        return {"error": type(error).__name__, "message": str(error)}


def record_source(case):
    import UnityPy
    import scratch
    import synth
    from types import SimpleNamespace
    from UnityPy.classes import StreamedResource

    # A real SerializedFile/Environment runs helpers/ResourceReader.py:10-38,
    # including dependency lookup, isolated from the recorder's working folder.
    with scratch.folder("audio-source-") as folder:
        env = UnityPy.Environment(path=str(folder))
        assets = env.load_file(synth.serialized_file([], []), name="audio.assets")
        for name, data in case["resources"].items():
            env.load_file(bytes.fromhex(data), name=name)
        resource = case["resource"]
        if resource is not None:
            resource = StreamedResource(m_Source=resource["source"], m_Offset=resource["offset"],
                                        m_Size=resource["size"])
        data = bytes.fromhex(case["data"]) if case["data"] is not None else None
        if case.get("legacy_array"):
            data = list(data)
        clip = SimpleNamespace(m_Name=case["name"], m_AudioData=data, m_Resource=resource,
                               object_reader=SimpleNamespace(assets_file=assets),
                               m_Channels=2, m_Frequency=44100)
        return {**case, "oracle": oracle_files(clip)}


def record_case(case):
    import UnityPy
    import fmod_toolkit
    from types import SimpleNamespace
    from UnityPy.classes import AudioClip

    assert UnityPy.__version__ == "1.25.2"
    assert fmod_toolkit.__version__ == "0.1.3"
    if case.get("kind") == "source":
        return record_source(case)
    clip = SimpleNamespace(m_Name=case["name"], m_AudioData=bytes.fromhex(case["data"]),
                           m_Resource=None, m_Channels=2, m_Frequency=44100,
                           m_CompressionFormat=case["compression"], m_Type=case.get("legacy_type"))
    extension = AudioClip.extension.fget(clip)
    return {**case, "extension": extension, "oracle": oracle_files(clip)}


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--check", action="store_true")
    parser.add_argument("--record-case", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.record_case:
        print(json.dumps(record_case(json.load(sys.stdin))))
        return 0
    import child
    import UnityPy
    import fmod_toolkit

    assert UnityPy.__version__ == "1.25.2"
    assert fmod_toolkit.__version__ == "0.1.3"
    def record(inputs):
        recorded = []
        for case in inputs:
            # The shared child runner handles externally killed children and timeout cleanup.
            # One case costs seconds (interpreter start, UnityPy and FMOD set-up) on a native
            # host but can take tens of seconds on an emulated or busy one; the bound is for
            # a child that hangs, not for a slow one.
            proc = child.run([sys.executable, str(Path(__file__).resolve()), "--record-case"],
                             input=json.dumps(case), capture_output=True, text=True, timeout=180)
            if proc.returncode:
                if case["rust"] != "error":
                    raise RuntimeError(f"audio case {case['case']}: {proc.returncode}: {proc.stderr}")
                recorded.append({**case, "oracle": {"process_exit": proc.returncode}})
            else:
                recorded.append(json.loads(proc.stdout))
        return recorded

    from fmod_toolkit import get_pyfmodex_system_instance
    from pyfmodex.flags import INIT_FLAGS
    system, _ = get_pyfmodex_system_instance(2, INIT_FLAGS.NORMAL)
    assert system.version == 0x20022
    result = {"reference": "UnityPy 1.25.2 / fmod_toolkit 0.1.3 / FMOD 2.00.22 Linux x86_64", "level": "L8",
              "cases": record(cases()), "extensions": record_extensions(), "sources": record(source_cases())}
    body = json.dumps(result, indent=2) + "\n"
    path = ROOT / "crates/unityrs-audio/tests/fixtures/audio.json"
    if args.check:
        if not path.exists() or path.read_text() != body:
            parser.exit(1, "stale or missing audio.json; regenerate in the pinned oracle\n")
    else:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(body)
    return 0


if __name__ == "__main__":
    sys.exit(main())
