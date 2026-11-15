"""Record B2a bundle saves from synthetic Rust builders with UnityPy 1.25.2.

--check rebuilds inputs, compares uncompressed and tuple-HC saves byte for byte,
and compares structure and decompressed entries for other compressed saves.
Tier C references patch only UnityPy's encryption-bit removal, missing original
block flags, old SerializedFile writing bugs, and the legacy version guard.
"""
from __future__ import annotations

import argparse
import json
import os
import sys
from contextlib import contextmanager
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools/parity"))
from serialized_save import corrected as corrected_serialized, patched_function


@contextmanager
def corrected():
    from UnityPy.files.BundleFile import BundleFile

    # files/BundleFile.py:238-393 and :395-487, whole functions. The reader
    # knows all these legacy versions; only the writer's guard is removed.
    fs = patched_function(BundleFile.save_fs,
        "if block_info_flag & self.dataflags.UsesAssetBundleEncryption:\n"
        "        block_info_flag ^= self.dataflags.UsesAssetBundleEncryption\n"
        "    if data_flag & self.dataflags.UsesAssetBundleEncryption:\n"
        "        data_flag ^= self.dataflags.UsesAssetBundleEncryption",
        "block_info_flag &= ~(int(self.dataflags.UsesAssetBundleEncryption) | 0x100)\n"
        "    data_flag &= ~int(self.dataflags.UsesAssetBundleEncryption)")
    legacy = patched_function(BundleFile.save_web_raw,
        'if self.version > 3:\n        raise NotImplementedError("Saving Unity Web bundles with version > 3 is not supported")',
        "if False:\n        pass")
    with (patch.object(BundleFile, "save_fs", fs),
          patch.object(BundleFile, "save_web_raw", legacy), corrected_serialized(True)):
        yield


def load(data):
    from UnityPy.files.BundleFile import BundleFile
    from UnityPy.streams import EndianBinaryReader
    return BundleFile(EndianBinaryReader(data), None)


def structure(data):
    """Read metadata with the oracle's reader; retain no compressor sizes."""
    from UnityPy.files.BundleFile import BundleFile
    rows = []
    blocks = []
    real_files = BundleFile.read_files
    real_decode = BundleFile.decompress_data

    def files(self, reader, directory):
        rows.extend([[row.offset, row.size, getattr(row, "flags", None),
                      row.path.encode("utf8", "surrogateescape").hex(),
                      reader.bytes[row.offset:row.offset + row.size].hex()]
                     for row in directory])
        return real_files(self, reader, directory)

    def decode(self, compressed_data, uncompressed_size, flags, index=0):
        result = real_decode(self, compressed_data, uncompressed_size, flags, index)
        blocks.append([uncompressed_size, int(flags)])
        return result

    with patch.object(BundleFile, "read_files", files), patch.object(BundleFile, "decompress_data", decode):
        bundle = load(data)
    fs = bundle.signature == "UnityFS" or bundle.version == 6
    return {"signature": bundle.signature, "format": bundle.version,
            "player": bundle.version_player, "engine": bundle.version_engine,
            "flags": int(bundle.dataflags) if fs else None,
            "alignment": bundle._uses_block_alignment,
            "blocks": blocks[1:] if fs else [], "directory": rows}


def install_mutation(bundle, mutation):
    if mutation == "none":
        return
    sf = bundle.files["CAB-file"]
    obj = next(iter(sf.objects.values()))
    before = obj.get_raw_data()
    if mutation == "raw":
        obj.set_raw_data(b"replacement-\0\xff")
    elif mutation == "empty":
        obj.set_raw_data(b"")
    elif mutation == "typetree":
        value = obj.parse_as_dict()
        value["value"] = -123456
        obj.save_typetree(value)
    else:
        raise AssertionError(mutation)
    assert obj.get_raw_data() == before


def record(source, packer):
    bundle = load(bytes.fromhex(source["input"]))
    install_mutation(bundle, source["mutation"])
    actual = {}
    try:
        uncorrected = bundle.save(packer=packer)
        try:
            structure(uncorrected)
        except Exception as exc:
            actual["reload_error"] = type(exc).__name__
            actual["hex"] = uncorrected.hex()
    except Exception as exc:
        actual["error"] = type(exc).__name__
    # UnityPy sets this field only when the input has at least one block.
    if not hasattr(bundle, "_block_info_flags") and hasattr(bundle, "dataflags"):
        bundle._block_info_flags = int(bundle.dataflags)
    with corrected():
        for file in bundle.files.values():
            if hasattr(file, "_enable_type_tree") and file.header.version < 13:
                file._enable_type_tree = True
        output = bundle.save(packer=packer)
        meta = structure(output)
        back = load(output)
        if not hasattr(back, "_block_info_flags") and hasattr(back, "dataflags"):
            back._block_info_flags = int(back.dataflags)
        same_packer_idempotent = back.save(packer=packer) == output
        canonical = back.save(packer="none")
        assert load(canonical).save(packer="none") == canonical, source["name"]
        assert same_packer_idempotent or packer == "original", source["name"]
    return {"name": source["name"], "input": source["input"], "mutation": source["mutation"],
            "packer": packer, "actual": actual, "output": output.hex(), "structure": meta,
            "same_packer_idempotent": same_packer_idempotent}


def intern(records):
    # Content boundaries share random resource bytes even when a packer's
    # header moves them. Whole strings and fixed chunks would repeat them.
    data, chunks, values, indexes = [], {}, [], {}

    def one(value):
        if value not in indexes:
            indexes[value] = len(values)
            raw = bytes.fromhex(value)
            pieces = []
            start = 0
            for i, byte in enumerate(raw, 1):
                if (i-start >= 512 and byte == 0) or i-start >= 2048 or i == len(raw):
                    chunk = raw[start:i].hex()
                    if chunk not in chunks:
                        chunks[chunk] = len(data)
                        data.append(chunk)
                    pieces.append(chunks[chunk])
                    start = i
            values.append(pieces)
        return indexes[value]

    for case in records:
        case["input"] = one(case["input"])
        case["output"] = one(case["output"])
        if "hex" in case["actual"]:
            case["actual"]["hex"] = one(case["actual"]["hex"])
        for row in case["structure"]["directory"]:
            row[4] = one(row[4])
    return data, values


def unpack(fixture):
    values = ["".join(fixture["data"][at] for at in pieces) for pieces in fixture["bytes"]]
    for case in fixture["cases"]:
        for key in ("input", "output"):
            case[key] = values[case[key]]
        for row in case["structure"]["directory"]:
            row[4] = values[row[4]]
        if "hex" in case["actual"]:
            case["actual"]["hex"] = values[case["actual"]["hex"]]
    return fixture["cases"]


def check(old, fresh):
    assert len(old) == len(fresh)
    for before, after in zip(old, fresh):
        # Web/Raw ignores its packer, so all their saves use the exact rule.
        packer = after["packer"]
        hc = isinstance(packer, list) and packer[1] & 0x3f == 3
        exact = packer == "none" or hc or after["structure"]["signature"] != "UnityFS"
        if not exact:
            before = {k: v for k, v in before.items() if k != "output"}
            after = {k: v for k, v in after.items() if k != "output"}
        assert before == after, after["name"]


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
                          "--example", "bundle_save_inputs"], cwd=ROOT,
                         env=dict(os.environ, CARGO_ENCODED_RUSTFLAGS=""),
                         check=True, capture_output=True, text=True)
        inputs = json.loads(proc.stdout)
    cases = [record(source, tuple(packer) if isinstance(packer, list) else packer)
             for source in inputs for packer in source["packers"]]
    # JSON normalizes tuple packers to arrays on disk and in --check.
    cases = json.loads(json.dumps(cases))
    path = ROOT / "crates/unityrs-core/tests/fixtures/bundle_save.json"
    if args.check:
        try:
            check(unpack(json.loads(path.read_text())), cases)
        except (AssertionError, FileNotFoundError) as exc:
            parser.exit(1, f"stale bundle_save.json: {exc}; regenerate in pinned oracle\n")
    else:
        data, values = intern(cases)
        body = '{"reference":"UnityPy 1.25.2","data":[\n'
        body += ",\n".join(json.dumps(value) for value in data) + '],"bytes":[\n'
        body += ",\n".join(json.dumps(value, separators=(",", ":")) for value in values) + '],"cases":[\n'
        body += ",\n".join(json.dumps(case, separators=(",", ":")) for case in cases) + "]}\n"
        path.write_text(body)
    print(f"bundle_save: {len(inputs)} synthetic inputs, {len(cases)} saves")


if __name__ == "__main__":
    main()
