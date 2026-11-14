"""Record simple-export L7 payloads from synthetic testkit-built objects.

Run in the pinned UnityPy 1.25.2 oracle. --check regenerates every result
and compares the deterministic JSON spelling, including ensure_ascii=False.
VideoClip is Tier X and has a native resource-slice test, no UnityPy exporter.
Each case runs in a child process so a UnityPyBoost abort names its input.
"""
import argparse
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools/parity"))


def record_case(case):
    import UnityPy
    from UnityPy.tools import extractor
    import scratch

    exporters = {49: extractor.exportTextAsset, 128: extractor.exportFont,
                 114: extractor.exportMonoBehaviour}
    env = UnityPy.Environment(bytes.fromhex(case["input"]))
    reader = next(obj for obj in env.objects if obj.path_id == 1)
    assert reader.type.value == case["class_id"]
    with scratch.folder("simple-export-") as folder:
        # Reader input avoids the separate export_obj/MonoBehaviour
        # return-value crash; the actual exporter writes the oracle bytes.
        exporters[case["class_id"]](reader, str(folder / "payload"), case["extension"])
        files = {p.name: p.read_bytes().hex() for p in sorted(folder.iterdir())}
    return {**case, "files": files}


def record(inputs):
    import child

    results = []
    for case in inputs:
        try:
            proc = child.run([sys.executable, str(Path(__file__).resolve()), "--record-case"],
                             input=json.dumps(case), capture_output=True, text=True)
        except child.SystemKilled as exc:
            raise RuntimeError(f"simple export case {case['case']!r}: {exc}") from exc
        if proc.returncode:
            raise RuntimeError(f"simple export case {case['case']!r} failed "
                               f"(exit {proc.returncode}):\n{proc.stderr}")
        results.append(json.loads(proc.stdout))
    return {"reference": "UnityPy 1.25.2", "level": "L7", "cases": results}


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--check", action="store_true")
    parser.add_argument("--record-case", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    try:
        import UnityPy
    except ImportError:
        parser.exit(2, "requires the pinned UnityPy 1.25.2 oracle\n")
    if UnityPy.__version__ != "1.25.2":
        parser.exit(2, "requires UnityPy 1.25.2\n")
    if args.record_case:
        print(json.dumps(record_case(json.load(sys.stdin)), ensure_ascii=False))
        return 0
    import child
    proc = child.run(["cargo", "run", "--quiet", "--locked", "-p", "unityrs-export",
                      "--example", "simple_inputs"], cwd=ROOT, check=True,
                     capture_output=True, text=True)
    try:
        recorded = record(json.loads(proc.stdout))
    except RuntimeError as exc:
        parser.exit(1, f"{exc}\n")
    body = json.dumps(recorded, indent=2, ensure_ascii=False) + "\n"
    path = ROOT / "crates/unityrs-export/tests/fixtures/simple_export.json"
    if args.check:
        if not path.exists() or path.read_bytes() != body.encode("utf8"):
            parser.exit(1, "stale or missing simple_export.json; regenerate in the oracle\n")
    else:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(body.encode("utf8"))
    return 0


if __name__ == "__main__":
    sys.exit(main())
