"""Record synthetic UnityCN answers with UnityPy 1.25.2 + UnityPyBoost.

    scripts/dev python tools/fixtures/unitycn.py
    scripts/dev python tools/fixtures/unitycn.py --check

Inputs come from the Rust test-only encryptor, never a real key or bundle.
The nested-distinct-tables input reads through both encrypted bundle levels.
The Tier U case runs only the bounded Python implementation: running Boost
on it would read/write outside its bytes object. Rust must refuse it.
The JSON is replayed by unityrs-core's unitycn_oracle integration test.
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools" / "parity"))
import child  # noqa: E402

import Crypto  # noqa: E402
import UnityPy  # noqa: E402
from UnityPy.files.BundleFile import BundleFile  # noqa: E402
from UnityPy.helpers import ArchiveStorageManager as M  # noqa: E402
from UnityPy.streams import EndianBinaryReader  # noqa: E402

DEST = ROOT / "crates/unityrs-core/tests/fixtures/unitycn/oracle.json"


def answer(fn):
    try:
        value = fn()
        return {"value": value.hex() if isinstance(value, (bytes, bytearray)) else value}
    except Exception as exc:
        return {"error": type(exc).__name__}


def record(inputs):
    assert UnityPy.__version__ == "1.25.2", UnityPy.__version__
    assert Crypto.__version__ == "3.23.0", Crypto.__version__
    assert M.UnityPyBoost is not None, "UnityPyBoost is required"
    key = bytes.fromhex(inputs["key"])
    header = bytes.fromhex(inputs["header"])
    previous, boost = M.DECRYPT_KEY, M.UnityPyBoost
    out = {"generator": "tools/fixtures/unitycn.py", "unitypy": UnityPy.__version__,
           "inputs": inputs, "bundles": [], "headers": [], "blocks": [], "setters": [], "vectors": []}
    try:
        for length in (0, 15, 16, 17, 32):
            out["setters"].append({"length": length, "result": answer(lambda: M.set_assetbundle_decrypt_key(bytes(length)))})
        for mode, supplied in (("correct", key), ("missing", None), ("wrong", b"synthetic-key-02")):
            M.DECRYPT_KEY = supplied
            for case in inputs["bundles"]:
                def load(case=case):
                    bundle = BundleFile(EndianBinaryReader(bytes.fromhex(case["hex"])), None, name="synthetic.bundle")
                    leaf = bundle
                    for name in case.get("path", ["synthetic.resS"]):
                        leaf = leaf.files[name]
                    return leaf.bytes
                out["bundles"].append({"name": case["name"], "key": mode, "result": answer(load)})
            for length in inputs["header_lengths"]:
                reader = EndianBinaryReader(header[:length])
                def parse():
                    M.ArchiveStorageDecryptor(reader)
                out["headers"].append({"length": length, "key": mode, "result": answer(parse), "position": reader.Position})
        M.DECRYPT_KEY = key
        reader = EndianBinaryReader(header)
        reader.read_u_int()
        for _ in range(2):
            data, vector = M.read_vector(reader)
            out["vectors"].append({"data": data.hex(), "key": vector.hex(), "position": reader.Position,
                                   "decrypted": M.decrypt_key(vector, data, key).hex()})
        for case in inputs["blocks"]:
            decryptor = M.ArchiveStorageDecryptor.__new__(M.ArchiveStorageDecryptor)
            decryptor.index = bytes.fromhex(case["index"])
            decryptor.substitute = bytes.fromhex(case["substitute"])
            data = bytes.fromhex(case["hex"])
            index = case["block_index"]
            M.UnityPyBoost = None
            python = answer(lambda: bytes(decryptor.decrypt_block(data, index)))
            M.UnityPyBoost = boost
            native = None if case["tier_u"] else answer(lambda: bytes(decryptor.decrypt_block(data, index)))
            out["blocks"].append({"name": case["name"], "python": python, "boost": native})
    finally:
        M.DECRYPT_KEY, M.UnityPyBoost = previous, boost
    return out


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    inputs = json.loads(child.run(
        ["cargo", "run", "--quiet", "--locked", "-p", "unityrs-core", "--example", "unitycn_fixtures"],
        cwd=ROOT, capture_output=True, text=True, check=True,
    ).stdout)
    output = json.dumps(record(inputs), indent=2, sort_keys=True) + "\n"
    if args.check:
        if not DEST.is_file() or DEST.read_text() != output:
            raise SystemExit("UnityCN oracle JSON is missing or stale; run tools/fixtures/unitycn.py")
    else:
        DEST.parent.mkdir(parents=True, exist_ok=True)
        DEST.write_text(output)
        print(DEST.relative_to(ROOT))


if __name__ == "__main__":
    main()
