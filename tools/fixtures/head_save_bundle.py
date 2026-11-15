"""Record exact HEAD 417d595 format-9 and named-lz4hc bundle saves.

All inputs are synthetic; --check reruns the blob-pinned vendored writer and
reader. Only the documented Tier C encryption and missing-original-flags
corrections patch HEAD's writer. No installed 1.25.2 writer is a reference.
"""
from __future__ import annotations

import argparse
import json
import struct
import sys
from contextlib import contextmanager
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools/parity/tests"))
from head_oracle import HEAD_COMMIT, load
from bundle_save import intern, unpack
from serialized_save import patched_function

DEST = ROOT / "crates/unityrs-core/tests/fixtures/head_save_bundle.json"
PACKERS = ["none", "original", "lz4", "lz4hc", "lzma", (195, 3), (0x2c2, 2), (0x240, 0x40)]


def pack(fmt, *values):
    return struct.pack(">" + fmt, *values)


def noise(size):
    state, data = 0x91e10da5, bytearray()
    for _ in range(size):
        state ^= (state << 13) & 0xffffffff
        state ^= state >> 17
        state ^= (state << 5) & 0xffffffff
        data.append(state & 255)
    return bytes(data)


def source(version, entries, codec=0, engine=b"6000.4.0f1", blocks=True):
    helper, _ = load()
    payload = b"".join(data for _, data, _ in entries)
    stored = helper.COMPRESSION_MAP[codec](payload) if blocks else b""
    info = bytes(16) + pack("i", int(blocks))
    if blocks:
        info += pack("IIH", len(payload), len(stored), codec)
        if version >= 9:
            info += pack("Q", 0)
    info += pack("i", len(entries))
    offset = 0
    for name, data, flags in entries:
        info += pack("qqI", offset, len(data), flags) + name + b"\0"
        offset += len(data)
    header = b"UnityFS\0" + pack("I", version) + b"5.x.x\0" + engine + b"\0"
    header += pack("qIII", 0, len(info), len(info), 0x40)
    header += bytes(-len(header) % 16)
    output = bytearray(header + info + stored)
    size_at = 12 + len(b"5.x.x\0") + len(engine) + 1
    output[size_at:size_at + 8] = pack("q", len(output))
    return bytes(output)


def inputs():
    random = noise(0x40001)
    exact = random[:130549] + bytes(0x20000 - 130549) + b"x"
    threshold = random[:130548] + bytes(0x20000 - 130548) + b"x"
    helper, _ = load()
    assert len(helper.compress_lz4(exact[:-1])) == 0x20000
    assert len(helper.compress_lz4(threshold[:-1])) == 0x1ffff
    payloads = [("short", b"alpha\0\xffbeta"), ("one-byte", b"x"),
                ("random", random), ("zeros", bytes(0x40001)),
                ("mixed", random[:0x10000] + bytes(0x10000) + random[:0x10000] + bytes(0x10000) + b"tail"),
                ("equal-size-chunk", exact), ("padding-threshold", threshold)]
    for version in (8, 9):
        packers = PACKERS if version == 9 else ["lz4hc"]
        for name, payload in payloads:
            entries = [(b"empty.resS", b"", 0), (b"payload.resS", payload, 7)]
            yield f"v{version}-{name}", source(version, entries), packers
        for label, entries in (("no-entries", []), ("empty-entry", [(b"empty.resS", b"", 3)])):
            yield f"v{version}-{label}", source(version, entries, blocks=False), packers
    for codec in (1, 2, 3):
        yield f"v9-original-codec{codec}", source(9, [(b"payload.resS", b"repeated" * 64, 0)], codec), ["original"]
    entries = [(b"same.resS", b"first", 1), (b"keep-\xff.resS", b"middle", 2), (b"same.resS", b"last", 9)]
    yield "v9-duplicate-names", source(9, entries), PACKERS
    yield "v9-no-blocks-declared-data", source(9, [(b"empty.resS", b"discarded", 7)], blocks=False), PACKERS
    for engine, mask in ((b"2020.3.33f1", 0x200), (b"6000.4.0f1", 0x1400)):
        flags = [0x200] if mask == 0x200 else [0x400, 0x1000, 0x1400]
        for flag in flags:
            yield f"v9-clear-encryption-{flag:x}", source(9, [(b"payload.resS", b"encryption cleared", 0)], engine=engine), [
                (0x40 | flag, 0x100 | flag), (0xc3 | flag, 3 | flag)]


@contextmanager
def corrected(cls):
    # HEAD files/BundleFile.py:238-389: clear only encryption bits set,
    # including per-block 0x100. The rest of the vendored function is intact.
    save = patched_function(cls.save_fs,
        "if block_info_flag & self.dataflags.UsesAssetBundleEncryption:\n"
        "        block_info_flag ^= self.dataflags.UsesAssetBundleEncryption\n"
        "    if data_flag & self.dataflags.UsesAssetBundleEncryption:\n"
        "        data_flag ^= self.dataflags.UsesAssetBundleEncryption",
        "block_info_flag &= ~(int(self.dataflags.UsesAssetBundleEncryption) | 0x100)\n"
        "    data_flag &= ~int(self.dataflags.UsesAssetBundleEncryption)")
    with patch.object(cls, "save_fs", save):
        yield


def structure(cls, data):
    from UnityPy.streams import EndianBinaryReader
    rows, blocks = [], []
    real_files, real_block = cls.read_files, cls.decompress_block

    def files(self, reader, directory):
        rows.extend([[row.offset, row.size, row.flags, row.path.encode("utf8", "surrogateescape").hex(),
                      reader.bytes[row.offset:row.offset + row.size].hex()] for row in directory])
        return real_files(self, reader, directory)

    def block(self, index, info, reader, base):
        blocks.append([info.uncompressedSize, info.compressedSize, info.flags, info.offset])
        return real_block(self, index, info, reader, base)

    with patch.object(cls, "read_files", files), patch.object(cls, "decompress_block", block):
        bundle = cls(EndianBinaryReader(data), None)
    return {"flags": int(bundle.dataflags), "blocks": blocks, "directory": rows}


def record(name, data, packer):
    from UnityPy.streams import EndianBinaryReader
    _, module = load()
    cls = module.BundleFile
    bundle = cls(EndianBinaryReader(data), None)
    actual = {}
    try:
        uncorrected = bundle.save(packer=packer)
        try:
            structure(cls, uncorrected)
        except Exception as exc:
            actual["reload_error"] = type(exc).__name__
            actual["hex"] = uncorrected.hex()
    except Exception as exc:
        actual["error"] = type(exc).__name__
    if not hasattr(bundle, "_block_info_flags"):
        bundle._block_info_flags = int(bundle.dataflags)
    with corrected(cls):
        output = bundle.save(packer=packer)
        meta = structure(cls, output)
        back = cls(EndianBinaryReader(output), None)
        if not hasattr(back, "_block_info_flags"):
            back._block_info_flags = int(back.dataflags)
        same = back.save(packer=packer) == output
        assert same or packer == "original", name
        canonical = back.save(packer="none")
        assert cls(EndianBinaryReader(canonical), None).save(packer="none") == canonical, name
    if not actual and output != uncorrected:
        # A per-block encryption flag can survive without making the reader
        # raise; retaining its output still witnesses the Tier C correction.
        assert isinstance(packer, tuple) and packer[1] & 0x100, name
        actual = {"correction": "encryption_bits", "hex": uncorrected.hex()}
    return {"name": name, "input": data.hex(), "output": output.hex(), "packer": packer,
            "actual": actual, "structure": meta, "same_packer_idempotent": same}


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    import UnityPy
    assert UnityPy.__version__ == "1.25.2"
    cases = [record(name, data, packer) for name, data, packers in inputs() for packer in packers]
    cases = json.loads(json.dumps(cases))
    if args.check:
        fixture = json.loads(DEST.read_text())
        assert fixture["head"] == HEAD_COMMIT
        assert unpack(fixture) == cases, "stale head_save_bundle.json; regenerate in pinned oracle"
    else:
        data, values = intern(cases)
        body = json.dumps({"head": HEAD_COMMIT, "data": data, "bytes": values, "cases": cases}, separators=(",", ":")) + "\n"
        assert len(body) < 1_500_000, len(body)
        DEST.write_text(body)
    print(f"head_save_bundle: {len(cases)} HEAD saves")


if __name__ == "__main__":
    main()
