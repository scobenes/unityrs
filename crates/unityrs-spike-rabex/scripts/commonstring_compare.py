#!/usr/bin/env python3
"""rabex's generated common-string table against UnityPy's (docs/spikes/rabex.md, the fork plan).

    python commonstring_compare.py RABEX_SRC_DIR

rabex 0.2.1's `src/commonstring.rs` is generated from its embedded TPK.
UnityPy's `UnityPy.enums.CommonString` (MIT) is the table a fork can
regenerate it from. Prints the sizes and the entries only one side has.
"""

import json
import re
import sys
from pathlib import Path

from UnityPy.enums.CommonString import CommonString


def main():
    text = (Path(sys.argv[1]) / "commonstring.rs").read_text()
    rabex = {int(k): v for k, v in re.findall(r'\((\d+), "([^"]*)"\)', text)}
    unitypy = dict(CommonString)
    print(
        json.dumps(
            {
                "rabex_entries": len(rabex),
                "unitypy_entries": len(unitypy),
                "same_offset_same_string": sum(1 for k, v in rabex.items() if unitypy.get(k) == v),
                "only_rabex": {k: rabex[k] for k in sorted(set(rabex) - set(unitypy))},
                "only_unitypy": {k: unitypy[k] for k in sorted(set(unitypy) - set(rabex))},
                "same_offset_different_string": {
                    k: [rabex[k], unitypy[k]] for k in sorted(set(rabex) & set(unitypy)) if rabex[k] != unitypy[k]
                },
            },
            indent=1,
        )
    )


if __name__ == "__main__":
    main()
