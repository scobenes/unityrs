"""Survey what the oracle's ShaderConverter does with every Shader in a corpus.

    scripts/dev python tools/parity/shader_survey.py --index /corpus/index.json --build 100056
    scripts/dev python tools/parity/shader_survey.py path/to/bundle-or-folder

Produces the numbers of `PARITY.md` section 19.1, "ShaderConverter fails on 6000.3". For
every Shader object it records the engine version of its file, the blob version of every
sub-program in its compressed blobs (read from the blob's offset table, as `ShaderProgram`
does, but without parsing the rest), whether a property has type Int (5), and what
`Shader.export()` does: return text, or raise (counted by exception type and by the
ShaderConverter line that raised). Prints one JSON object. Files are loaded one at a time.
"""

from __future__ import annotations

import argparse
import collections
import gc
import json
import os
import struct
import sys
import traceback

import UnityPy
from UnityPy.helpers import CompressionHelper

INT_PROPERTY = 5  # SerializedPropertyType.Int, Unity 2021.1 and later


def inputs(args) -> list[str]:
    if args.index:
        with open(args.input, "rb") as fh:
            index = json.loads(fh.read().decode("utf-8"))
        root = os.path.dirname(os.path.realpath(args.input))  # entries are relative to the index
        builds = set(args.build.split(",")) if args.build else None
        return sorted(
            os.path.join(root, e["path"])
            for e in index["files"]
            if e.get("kind") in args.kind.split(",") and (builds is None or str(e.get("build")) in builds)
        )
    if os.path.isdir(args.input):
        found = []
        for dirpath, dirnames, filenames in os.walk(args.input):
            dirnames.sort()
            found += [os.path.join(dirpath, n) for n in sorted(filenames)]
        return found
    return [args.input]


def entry(array, i):
    item = array[i]
    return item[0] if isinstance(item, list) else item


def blob_versions(shader, version) -> list[int]:
    """The `m_Version` of every sub-program, from each platform's blob (`ShaderProgram.__init__`)."""
    found = []
    blob = bytes(shader.compressedBlob or b"")
    for i in range(min(len(shader.platforms), len(shader.compressedLengths), len(shader.decompressedLengths))):
        offset, size = entry(shader.offsets, i), entry(shader.compressedLengths, i)
        data = CompressionHelper.decompress_lz4(blob[offset : offset + size], entry(shader.decompressedLengths, i))
        (capacity,) = struct.unpack_from("<i", data, 0)
        width = 12 if version >= (2019, 3) else 8
        for k in range(capacity):
            (start,) = struct.unpack_from("<i", data, 4 + k * width)
            found.append(struct.unpack_from("<i", data, start)[0])
    return found


def raise_site(e: BaseException) -> str:
    frames = [f for f in traceback.extract_tb(e.__traceback__) if f.filename.endswith("ShaderConverter.py")]
    where = f"ShaderConverter.py:{frames[-1].lineno}" if frames else "elsewhere"
    kind = type(e).__qualname__ if type(e).__module__ == "builtins" else f"{type(e).__module__}.{type(e).__qualname__}"
    return f"{kind} at {where}"


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("input", help="file, folder, or (with --index) a corpus index.json")
    ap.add_argument("--index", action="store_true", help="INPUT is a corpus index.json")
    ap.add_argument("--kind", default="bundle", help="index entry kinds (comma list)")
    ap.add_argument("--build", help="index builds (comma list)")
    args = ap.parse_args(argv)

    engines = collections.Counter()
    versions = collections.Counter()
    results = collections.Counter()
    shaders = with_int = files = 0
    for path in inputs(args):
        files += 1
        env = UnityPy.load(path)
        for obj in env.objects:
            if obj.type.name != "Shader":
                continue
            shaders += 1
            engines[".".join(map(str, obj.version[:2]))] += 1
            shader = obj.read()
            versions.update(blob_versions(shader, obj.version))
            props = getattr(getattr(shader, "m_ParsedForm", None), "m_PropInfo", None)
            if props is not None and any(p.m_Type == INT_PROPERTY for p in props.m_Props):
                with_int += 1
            try:
                shader.export()
                results["text"] += 1
            except Exception as e:  # noqa: BLE001 - the survey counts every failure
                results[raise_site(e)] += 1
        del env
        gc.collect()
    json.dump(
        {
            "files": files,
            "shaders": shaders,
            "engine": dict(sorted(engines.items())),
            "sub_programs": sum(versions.values()),
            "sub_program_versions": {str(k): v for k, v in sorted(versions.items())},
            "shaders_with_an_int_property": with_int,
            "export": dict(sorted(results.items())),
        },
        sys.stdout,
        indent=1,
    )
    print()
    return 0


if __name__ == "__main__":
    sys.exit(main())
