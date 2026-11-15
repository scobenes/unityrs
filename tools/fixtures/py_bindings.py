"""Record Y1 API observations; Rust tests replay them without UnityPy.

    python tools/fixtures/py_bindings.py --check
"""

import argparse
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools/parity"))
import binding_cases as B
import UnityPy
import object_cases as O
import scratch


def main():
    parser = argparse.ArgumentParser(__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    if UnityPy.__version__ != "1.25.2":
        raise RuntimeError("requires UnityPy 1.25.2")
    records = [{"name": name, "hex": data.hex(), "expected": B.observe(UnityPy, data)} for name, data in B.cases()]
    with scratch.folder("unityrs-py-api-") as root:
        data = next(data for name, data in B.cases() if name == "text")
        records.append({"name": "paths", "hex": data.hex(), "expected": B.observe_paths(UnityPy, data, root)})
    encoded = "[\n" + ",\n".join(json.dumps(r, separators=(",", ":")) for r in records) + "\n]\n"
    out = ROOT / "crates/unityrs-py/tests/fixtures/api.json"
    if args.check:
        if out.read_text() != encoded:
            raise SystemExit("Y1 API fixtures differ")
    else:
        out.write_text(encoded)
    print(f"Y1: {len(records)} synthetic API fixtures")
    objects = [{"name": name, "hex": data.hex(), "expected": O.observe(UnityPy, data)} for name, data in O.cases()]
    encoded = "[\n" + ",\n".join(json.dumps(r, separators=(",", ":")) for r in objects) + "\n]\n"
    out = ROOT / "crates/unityrs-py/tests/fixtures/object_api.json"
    if args.check:
        if out.read_text() != encoded:
            raise SystemExit("Y2 object API fixtures differ")
    else:
        out.write_text(encoded)
    print(f"Y2: {len(objects)} synthetic object API fixtures")


if __name__ == "__main__":
    main()
