"""Record and check WebFile saves against UnityPy 1.25.2 in both directions.

Plain saves are byte equal. Compressed saves have equal decompressed bytes;
Rust's deterministic stored streams retain a WebFile-readable brotli marker.
UnityPy's raw brotli saves lack this marker and fail to reload (Tier C).
"""
from __future__ import annotations

import argparse
import json
import os
import sys
import struct
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools/parity"))


def load(data):
    from UnityPy.files.WebFile import WebFile
    from UnityPy.streams import EndianBinaryReader
    # A fresh direct reader bypasses the already documented loading detector's
    # gzip cursor bug, without patching the WebFile reader itself.
    return WebFile(EndianBinaryReader(data), None)


def unpack(data, packer):
    from UnityPy.helpers import CompressionHelper
    if packer == "gzip":
        return CompressionHelper.decompress_gzip(data)
    if packer == "brotli":
        return CompressionHelper.decompress_brotli(data)
    return data


def record(case):
    source = load(bytes.fromhex(case["input"]))
    if case["mutation"] != "none":
        obj = source.files["inner"].files["asset"].objects[7]
        obj.set_raw_data(b"" if case["mutation"] == "empty" else b"edited\0\xff")
    assert source.save() == source.save(packer="none", signature="UnityWebData1.0")
    output = source.save(packer=case["packer"], signature=case["signature"])
    plain = unpack(output, case["packer"])
    rust = bytes.fromhex(case["rust"])
    assert unpack(rust, case["packer"]) == plain, case["name"]
    if case["packer"] == "none":
        assert rust == output
    # The actual pinned WebFile reader, including its compression sniff,
    # reads every Rust-produced stream and then emits the exact plain save.
    read_rust = load(rust)
    assert read_rust.packer == case["packer"]
    assert read_rust.save(signature=case["signature"]) == plain
    oracle_reload_error = None
    try:
        load(output)
    except Exception as exc:
        oracle_reload_error = type(exc).__name__
    assert (oracle_reload_error is not None) == (case["packer"] == "brotli")
    # Record reloadable reference bytes for Rust to replay; oracle compressor
    # timestamps and version-dependent choices do not enter the fixture.
    return {k: v for k, v in case.items() if k != "rust"} | {
        "plain": plain.hex(), "oracle_reload_error": oracle_reload_error}


def intern(cases):
    data, index = [], {}
    for case in cases:
        for key in ("input", "plain"):
            value = case[key]
            if value not in index:
                index[value] = len(data)
                data.append(value)
            case[key] = index[value]
    return {"reference": "UnityPy 1.25.2", "data": data, "cases": cases}


def collision():
    # A Tuanjie signature puts this first member at offset 34, so the input
    # loads. The default Unity signature shifts it to the detector's offset 32.
    signature = b"TuanjieWebData1.0\0"
    offset = len(signature) + 4 + 12 + 6
    source = signature + struct.pack("<iiii", offset, offset, 4, 6) + b"brotlidata"
    output = load(source).save()
    assert output[32:38] == b"brotli"
    try:
        load(output)
    except Exception as exc:
        error = type(exc).__name__
    else:
        raise AssertionError("plain save must collide with brotli sniff")
    return {"input": source.hex(), "output": output.hex(), "error": error}


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--check", action="store_true")
    parser.add_argument("--inputs", type=Path)
    args = parser.parse_args()
    import UnityPy
    import child
    assert UnityPy.__version__ == "1.25.2"
    if args.inputs:
        inputs = json.loads(args.inputs.read_text())
    else:
        proc = child.run(["cargo", "run", "--quiet", "--locked", "-p", "unityrs-core",
                          "--example", "webfile_save_inputs"], cwd=ROOT,
                         env=dict(os.environ, CARGO_ENCODED_RUSTFLAGS=""),
                         check=True, capture_output=True, text=True)
        inputs = json.loads(proc.stdout)
    fixture = intern([record(case) for case in inputs])
    fixture["collision"] = collision()
    path = ROOT / "crates/unityrs-core/tests/fixtures/webfile_save.json"
    body = json.dumps(fixture, separators=(",", ":")) + "\n"
    assert len(body) < 1_500_000, len(body)
    if args.check:
        assert json.loads(path.read_text()) == fixture, "stale webfile_save.json"
    else:
        path.write_text(body)
    print(f"webfile_save: {len(inputs)} bidirectional saves, {len(fixture['data'])} shared byte strings")


if __name__ == "__main__":
    main()
