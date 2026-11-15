"""Record Python API observations; Rust tests replay them without UnityPy.

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
import scratch

HELPERS = [
    ("binding_y3_values", "observe_enums"),
    ("binding_y3_values", "observe_versions"),
    ("binding_y3_tree", "observe"),
    ("binding_y3_tree", "observe_mutation"),
    ("binding_y3_config", "observe_config"),
    ("binding_y3_config", "observe_container"),
    ("binding_y3_config", "observe_config_loads"),
    ("binding_y3_config", "observe_peek"),
    ("binding_y3_objects", "observe_objects"),
    ("binding_y3_objects", "observe_head"),
]


def helper_records():
    import importlib
    return [{"name": f"{observer}.{function}", "observer": observer, "function": function,
             "expected": getattr(importlib.import_module(observer), function)(UnityPy)}
            for observer, function in HELPERS]


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
    records.extend(helper_records())
    encoded = "[\n" + ",\n".join(json.dumps(r, separators=(",", ":")) for r in records) + "\n]\n"
    out = ROOT / "crates/unityrs-py/tests/fixtures/api.json"
    if args.check:
        if out.read_text() != encoded:
            raise SystemExit("Python API fixtures differ")
    else:
        out.write_text(encoded)
    print(f"Python API: {len(records)} synthetic API fixtures")


if __name__ == "__main__":
    main()
