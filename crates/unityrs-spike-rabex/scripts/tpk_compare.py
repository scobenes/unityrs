#!/usr/bin/env python3
"""rabex's embedded TPK against UnityPy 1.25.2's lzma.tpk (docs/spikes/rabex.md).

    python tpk_compare.py PROBE.json

PROBE.json is `rabex-dump probe` output; its `tpk.nodes` hold rabex's node
rows `[level, type, name, byte_size, version, type_flags, meta_flag]` for a
few classes at a few versions. UnityPy builds the same classes with
`Tpk.get_typetree_node`; the rows are compared column by column.
"""

import json
import sys
from collections import Counter
from pathlib import Path

from UnityPy.helpers import Tpk
from UnityPy.helpers.UnityVersion import UnityVersion

COLUMNS = ["level", "type", "name", "byte_size", "version", "type_flags", "meta_flag"]


def rows(node):
    out = []
    stack = [(node, 0)]
    while stack:
        n, level = stack.pop()
        out.append([level, n.m_Type, n.m_Name, n.m_ByteSize, n.m_Version, n.m_TypeFlags, n.m_MetaFlag])
        stack.extend((c, level + 1) for c in reversed(n.m_Children))
    return out


def main():
    probe = json.loads(Path(sys.argv[1]).read_text())["tpk"]
    if Tpk.TPKTYPETREE is None:
        Tpk.init()
    blob = Tpk.TPKTYPETREE
    summary = {
        "unitypy": {
            "classes": len(blob.ClassInformation),
            "versions": len(blob.Versions),
            "first": str(blob.Versions[0]),
            "last": str(blob.Versions[-1]),
        },
        "rabex": {k: probe[k] for k in ("classes", "first", "last", "versions", "unknown_class_panics")},
    }
    cases = Counter()
    columns = Counter()
    details = []
    for case in probe["nodes"]:
        try:
            node = Tpk.get_typetree_node(case["class_id"], UnityVersion.from_str(case["version"]))
            u = rows(node)
        except Exception as e:  # noqa: BLE001
            u = f"error: {type(e).__name__}"
        r = case["nodes"]
        if isinstance(u, str) or not isinstance(r, list):
            cases["missing_on_one_side" if u != r else "both_missing"] += 1
            details.append(
                {
                    "version": case["version"],
                    "class_id": case["class_id"],
                    "unitypy": u if isinstance(u, str) else len(u),
                    "rabex": r if not isinstance(r, list) else len(r),
                }
            )
            continue
        if len(u) != len(r) or [x[:3] for x in u] != [x[:3] for x in r]:
            cases["different_tree"] += 1
            details.append(
                {
                    "version": case["version"],
                    "class_id": case["class_id"],
                    "unitypy_nodes": len(u),
                    "rabex_nodes": len(r),
                }
            )
            continue
        diff = {COLUMNS[i] for a, b in zip(u, r, strict=True) for i in range(len(COLUMNS)) if a[i] != b[i]}
        cases["same_tree" + ("" if not diff else ", columns differ")] += 1
        columns.update(diff)
    json.dump(
        {"summary": summary, "cases": cases, "differing_columns": columns, "details": details[:20]},
        sys.stdout,
        indent=1,
    )
    print()


if __name__ == "__main__":
    main()
