#!/usr/bin/env python3
"""Check `rabex-dump probe` results against UnityPy 1.25.2 (docs/spikes/rabex.md).

    python verify_probe.py PROBE.json CORPUS_ROOT

Cross-file PPtrs: UnityPy loads the bundles the probe read (its
`environment.bundles`, relative to CORPUS_ROOT) into one Environment, reads the source object in object mode, follows
the same field path and derefs the PPtr; the target's class and m_Name must
equal rabex's. Streamed textures: the sha256 of what UnityPy's
`get_resource_data` returns for m_StreamData (Texture2DConverter's path)
must equal the sha256 of the .resS range rabex read.
"""

import hashlib
import json
import sys
from pathlib import Path

import UnityPy
from UnityPy.helpers.ResourceReader import get_resource_data


def main():
    probe = json.loads(Path(sys.argv[1]).read_text())
    root = Path(sys.argv[2])
    rels = probe["environment"]["bundles"]
    env = UnityPy.load(*(str(root / r) for r in rels))  # the probe's bundles: the CAB registry
    by_name = {Path(r).name: r for r in rels}
    cabs = {}
    for f in env.files.values():
        for name, child in getattr(f, "files", {}).items():
            cabs[(by_name[Path(f.name).name], name)] = child

    def obj(case):
        return cabs[(case["bundle"], case["cab"])].objects[case["path_id"]]

    results = {"pptr": [0, 0], "stream": [0, 0], "mismatches": []}
    for case in probe["environment"]["pptr"]["cases"]:
        v = obj(case).read()
        for p in case["field"]:
            v = v[p] if isinstance(p, int) else getattr(v, p)
        target = v.deref()
        d = target.parse_as_dict()
        got = {"type": target.type.name, "name": d.get("m_Name")}
        ok = got == case["rabex"]
        results["pptr"][0 if ok else 1] += 1
        if not ok:
            results["mismatches"].append({"case": case, "unitypy": got})
    for case in probe["environment"]["stream"]["cases"]:
        o = obj(case)
        sd = o.parse_as_dict()["m_StreamData"]
        data = get_resource_data(sd["path"], o.assets_file, sd["offset"], sd["size"])
        got = hashlib.sha256(bytes(data)).hexdigest()
        ok = got == case["rabex"].get("sha256")
        results["stream"][0 if ok else 1] += 1
        if not ok:
            results["mismatches"].append({"case": case, "unitypy": got})
    json.dump(results, sys.stdout, indent=1)
    print()


if __name__ == "__main__":
    main()
