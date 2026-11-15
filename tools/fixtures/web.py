"""Record C2 L0/L1/L2, UnityPy 1.25.2 and brotli 1.2.0.

Run in the oracle container: python tools/fixtures/web.py [--check].
All inputs are testkit builders. Gzip is Tier B: record UnityPy's load
failure and compare with the same WebFile loaded uncompressed. Name lengths
below -1 are Tier U and deliberately never handed to UnityPy's unbounded loop.
"""
from __future__ import annotations
import argparse
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools/parity"))
import child  # noqa: E402
import scratch  # noqa: E402
import UnityPy  # noqa: E402
import brotli  # noqa: E402
from UnityPy.files import File, BundleFile, WebFile, SerializedFile  # noqa: E402
from UnityPy.helpers import ImportHelper  # noqa: E402
from UnityPy.streams import EndianBinaryReader  # noqa: E402

DEST = ROOT / "crates/unityrs-core/tests/fixtures/web/oracle.json"
ORIGINAL = File.read_files


def read_files(self, reader, nodes):
    self._c2_directory = [[n.offset, n.size, getattr(n, "flags", None), n.path] for n in nodes]
    ORIGINAL(self, reader, nodes)


File.read_files = read_files


def tree(f, ty):
    if isinstance(f, BundleFile):
        kind = "bundle"
        l1 = {"signature": f.signature, "format": f.version,
              "player": f.version_player, "engine": f.version_engine,
              "hash": getattr(f, "_hash", None), "crc": getattr(f, "crc", None),
              "directory": f._c2_directory}
        if l1["hash"] is not None:
            l1["hash"] = bytes(l1["hash"]).hex()
    elif isinstance(f, WebFile):
        kind = "web"
        l1 = {"signature": f.signature, "packer": f.packer, "directory": f._c2_directory}
    else:
        kind = "serialized" if isinstance(f, SerializedFile) else "raw"
        l1 = None
    children = []
    if kind in ("bundle", "web"):
        for name, member in f.files.items():
            # parse_file already detected the entry. A parsed container or
            # SerializedFile has one type; raw readers retain detectable bytes.
            child_ty = ("BundleFile" if isinstance(member, BundleFile) else
                        "WebFile" if isinstance(member, WebFile) else
                        "AssetsFile" if isinstance(member, SerializedFile) else
                        ImportHelper.check_file_type(EndianBinaryReader(member.bytes))[0].name)
            children.append([name, tree(member, child_ty)])
    return {"kind": kind, "type": ty, "l1": l1, "children": children,
            "bytes": bytes(f.bytes).hex() if kind == "raw" else None}


def summary(data, stream):
    try:
        env = UnityPy.Environment()
        ty = ImportHelper.check_file_type(EndianBinaryReader(data))[0].name
        if stream:
            with scratch.folder() as folder:
                path = Path(folder) / "synthetic"
                path.write_bytes(data)
                f = env.load_file(str(path))
        else:
            f = env.load_file(data, name="synthetic")
        return {"file": tree(f, ty), "cabs": list(env.cabs),
                "objects": [{"path_id": o.path_id, "tree": o.read_typetree()} for o in env.objects]}
    except Exception:  # UnityPy's input failure; memory errors must escape.
        if isinstance(sys.exception(), MemoryError):
            raise
        return {"error": True}


def record(inputs):
    assert UnityPy.__version__ == "1.25.2", UnityPy.__version__
    assert brotli.__version__ == "1.2.0", brotli.__version__
    answers = []
    for case in inputs:
        data = bytes.fromhex(case["hex"])
        if case["tier"] == "U":
            answers.append({"memory": {"error": True}, "stream": {"error": True}})
            continue
        reference = bytes.fromhex(case["reference"]) if case["reference"] else data
        answer = {"memory": summary(reference, False), "stream": summary(reference, True)}
        if case["tier"] == "B":
            answer["gzip_actual"] = summary(data, False)
            assert answer["gzip_actual"] == {"error": True}
            for mode in ("memory", "stream"):
                answer[mode]["file"]["l1"]["packer"] = "gzip"
        answers.append(answer)
    return {"generator": "tools/fixtures/web.py", "unitypy": UnityPy.__version__,
            "brotli": brotli.__version__, "inputs": inputs, "answers": answers}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    inputs = json.loads(child.run(["cargo", "run", "--quiet", "--locked", "-p", "unityrs-core",
                                  "--example", "web_fixtures"], cwd=ROOT, capture_output=True,
                                 text=True, check=True).stdout)
    output = json.dumps(record(inputs), ensure_ascii=False, indent=2, sort_keys=True) + "\n"
    if args.check:
        if not DEST.is_file() or DEST.read_text(encoding="utf-8") != output:
            raise SystemExit("C2 oracle is missing or stale; run tools/fixtures/web.py")
    else:
        DEST.parent.mkdir(parents=True, exist_ok=True)
        DEST.write_text(output, encoding="utf-8")
        print(DEST.relative_to(ROOT))


if __name__ == "__main__":
    main()
