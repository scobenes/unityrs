#!/usr/bin/env python3
"""rabex's generated class-ID table against UnityPy's (docs/spikes/rabex.md, the fork plan).

    python classid_compare.py RABEX_SRC_DIR

rabex 0.2.1's `src/objects/class_id.rs` is generated from AssetRipper's
TypeTreeDumps (upstream's `utils/class_generator/generate_class_id.py`),
which has no licence file. UnityPy's `UnityPy.enums.ClassIDType` (MIT) is
the table a fork can regenerate it from. Prints the sizes, how many IDs carry
the same name on both sides, and the IDs only one side has or names
differently.
"""

import json
import re
import sys
from pathlib import Path

from UnityPy.enums.ClassIDType import ClassIDType

CONST = re.compile(r"pub const (\w+): ClassId = ClassId\((-?\d+)\);")


def rabex_ids(text):
    """id -> name from class_id.rs's `pub const NAME: ClassId = ClassId(ID);` lines."""
    out = {}
    for name, cid in CONST.findall(text):
        out.setdefault(int(cid), name)
    return out


def main():
    rabex = rabex_ids((Path(sys.argv[1]) / "objects" / "class_id.rs").read_text())
    unitypy = {m.value: m.name for m in ClassIDType}
    both = set(rabex) & set(unitypy)
    only_rabex = sorted(set(rabex) - set(unitypy))
    print(
        json.dumps(
            {
                "rabex_entries": len(rabex),
                "unitypy_entries": len(unitypy),
                "same_id_same_name": sum(1 for k in both if rabex[k] == unitypy[k]),
                "same_id_different_name": {k: [rabex[k], unitypy[k]] for k in sorted(both) if rabex[k] != unitypy[k]},
                "only_rabex_count": len(only_rabex),
                "only_rabex": {k: rabex[k] for k in only_rabex},
                "only_unitypy": {k: unitypy[k] for k in sorted(set(unitypy) - set(rabex))},
            },
            indent=1,
        )
    )


if __name__ == "__main__":
    main()
