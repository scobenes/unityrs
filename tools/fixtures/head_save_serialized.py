"""Record synthetic SF 23-26 saves against vendored HEAD 417d595 (MIT).

Run only in the oracle container: python tools/fixtures/head_save_serialized.py [--check].
HEAD files/SerializedFile.py:158-216 and HEAD helpers/TypeTreeNode.py:237-277,
HEAD helpers/TypeTreeNode.py:359-367 and HEAD helpers/TypeTreeNode.py:385-394.
Editor-built Unity 6000.4/6000.5 parity remains a separate target.
"""
from __future__ import annotations

import argparse
from io import BytesIO
import json
from pathlib import Path
import sys
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools/fixtures"))
sys.path.insert(0, str(ROOT / "tools/parity/tests"))
from head_read import sf_cases, serialized, handler  # noqa: E402
from head_oracle import HEAD_COMMIT, readers  # noqa: E402

DEST = ROOT / "crates/unityrs-core/tests/fixtures/head_save_serialized.json"


def record():
    import UnityPy
    from UnityPy.streams import EndianBinaryReader, EndianBinaryWriter
    assert UnityPy.__version__ == "1.25.2"
    result = {"head": HEAD_COMMIT, "generator": "tools/fixtures/head_save_serialized.py", "cases": []}
    # HEAD adds tell(), an alias of Position, to its otherwise unchanged writer.
    with mock.patch.object(EndianBinaryWriter, "tell", lambda self: self.Position, create=True), readers() as (sf, _, _):
        cases = list(sf_cases())
        cases += [("empty-" + str(v), serialized(v, [], trees=False)) for v in range(23, 27)]
        for name, data in cases:
            for parse_trees in (True, False):
                for mutation in (False, True):
                    with mock.patch.object(UnityPy.config, "SERIALIZED_FILE_PARSE_TYPETREE", parse_trees):
                        try:
                            file = sf.SerializedFile(EndianBinaryReader(data), name="CAB-synthetic")
                        except Exception:
                            continue
                        case = {"name": name, "input": data.hex(), "parse_trees": parse_trees, "mutation": mutation}
                        if mutation:
                            for obj in file.objects.values():
                                obj.set_raw_data(b"edited payload")
                        try:
                            saved = file.save()
                            case["output"] = saved.hex()
                            # The recorded save is reloaded with both oracle readers.
                            for stream in (False, True):
                                back = sf.SerializedFile(EndianBinaryReader(BytesIO(saved) if stream else saved), name="CAB-back")
                                assert [o.get_raw_data() for o in back.objects.values()] == [o.data if o.data is not None else o.get_raw_data() for o in file.objects.values()]
                                assert back.save() == saved
                        except AssertionError:
                            if "output" in case:
                                raise
                            case["error"] = "AssertionError"
                        result["cases"].append(case)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    result = record()
    output = json.dumps(result, sort_keys=True, separators=(",", ":")) + "\n"
    assert len(output) < 1_500_000
    if args.check:
        if not DEST.is_file() or DEST.read_text() != output:
            raise SystemExit("HEAD SerializedFile save fixture is stale")
    else:
        DEST.write_text(output)
    print(f"HEAD SerializedFile save oracle: {len(result['cases'])} cases checked")


if __name__ == "__main__":
    main()
