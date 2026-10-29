#!/usr/bin/env python3
"""rabex's code lines by module and by the fate the fork gives them (docs/spikes/rabex.md section 11).

    python rabex_loc.py RABEX_SRC_DIR

RABEX_SRC_DIR is rabex 0.2.1's `src/` (in the cargo registry, for example
`$CARGO_HOME/registry/src/*/rabex-0.2.1/src`). A code line is a non-blank
line that is not only a comment; lines of a test-only item (`#[cfg(test)]`
or `#[cfg(all(test, ...))]`) are counted apart as tests. Each module's fate is the table below.
"""

import json
import re
import sys
from pathlib import Path

# Module (path under src/, without .rs) -> (fate, why).
FATE = {
    "objects/class_id": (
        "regenerated",
        "generated from AssetRipper's TypeTreeDumps; regenerate from UnityPy's MIT ClassIDType",
    ),
    "files/serializedfile/builder": ("kept", "the builder API; its one panic! is an API-misuse assert"),
    "objects/pptr": ("kept", "PPtr file identifiers and resolve"),
    "archive_storage_manager": (
        "kept",
        "UnityCN block decryptor, a port of CNStudio's UnityCN.cs (MIT: Radu, Perfare); carry its notice",
    ),
    "read_ext": ("kept", "byte-reading helpers"),
    "files/bundlefile/builder": ("kept", "bundle builder"),
    "files/bundlefile/config": ("kept", "extraction config"),
    "typetree/provider": ("kept", "the TypeTreeProvider trait"),
    "files/unityfile": ("kept", "FileEntry"),
    "write_ext": ("kept", "byte-writing helpers"),
    "lib": ("kept", "crate root"),
    "objects/mod": ("kept", "module root"),
    "files": ("kept", "module root"),
    "files/serializedfile/mod": (
        "fixed",
        "reader and writer header gates, bigID, version strings, the writer's unwrap()s, todo!()s and data order",
    ),
    "files/bundlefile/mod": ("fixed", "header byte, format-6 alignment, panics, writer flags"),
    "files/bundlefile/reader": ("fixed", "UnityWeb unimplemented!(), the discard_front panic!"),
    "tpk": ("fixed", "unknown-class index panic; its node walk is adapted from UnityPy's Tpk.py (MIT)"),
    "typetree/mod": ("fixed", "depth bound on old-format tree parsing"),
    "typetree/typetree_cache": ("fixed", "key by (class, version), not class alone"),
    "unity_version": ("fixed", "raw version string, c/x types, build numbers above 255"),
    "commonstring": ("regenerated", "derived from the TPK; regenerate from UnityPy's MIT CommonString"),
    "serde_typetree/de": ("rewritten", "UnityPyBoost's read_typetree rules"),
    "serde_typetree/ser": (
        "rewritten",
        "UnityPy's Python write_typetree rules (UnityPyBoost has no writer); 21 live todo!() and 27 unreachable!()",
    ),
    "serde_typetree/error": ("rewritten", "error kinds"),
    "serde_typetree/mod": ("rewritten", "module root"),
}
ORDER = ("kept", "fixed", "regenerated", "rewritten")
# A test-only item: `#[cfg(test)]`, or `#[cfg(all(test, ...))]` (rabex's tpk.rs
# guards its test module with `all(test, feature = "embed-tpk")`).
TEST_CFG = re.compile(r"#\[cfg\((all\()?test\b")


def count(text):
    """(code lines, test lines) of one Rust file."""
    code = tests = 0
    in_block = False
    test_depth = None  # brace depth of the #[cfg(test)] item while inside it
    depth = 0
    pending_test = False
    for raw in text.splitlines():
        line = raw.strip()
        if in_block:
            if "*/" in line:
                in_block = False
            continue
        if line.startswith("/*"):
            in_block = "*/" not in line
            continue
        if not line or line.startswith("//"):
            continue
        if TEST_CFG.match(line):
            pending_test = True
        inside = pending_test or test_depth is not None
        if inside:
            tests += 1
        else:
            code += 1
        opens, closes = line.count("{"), line.count("}")
        if pending_test and opens:
            test_depth = depth
            pending_test = False
        depth += opens - closes
        if test_depth is not None and depth <= test_depth:
            test_depth = None
    return code, tests


def main():
    src = Path(sys.argv[1])
    modules = {}
    for f in sorted(src.rglob("*.rs")):
        name = f.relative_to(src).with_suffix("").as_posix()
        if name not in FATE:
            sys.exit(f"rabex_loc.py: no fate for {name}")
        modules[name] = count(f.read_text())
    missing = set(FATE) - set(modules)
    if missing:
        sys.exit(f"rabex_loc.py: fate for modules not in the tree: {sorted(missing)}")
    library = sum(c for c, _ in modules.values())
    out = {
        "code_lines": library + sum(t for _, t in modules.values()),
        "test_lines": sum(t for _, t in modules.values()),
        "library_lines": library,
        "fates": {},
    }
    for fate in ORDER:
        members = sorted(((m, modules[m][0]) for m in FATE if FATE[m][0] == fate), key=lambda x: -x[1])
        lines = sum(n for _, n in members)
        out["fates"][fate] = {
            "lines": lines,
            "percent": round(100 * lines / library, 1),
            "modules": {m: n for m, n in members},
        }
    changed = sum(out["fates"][f]["lines"] for f in ("regenerated", "rewritten"))
    out["rewritten_or_regenerated_percent"] = round(100 * changed / library, 1)
    changed_all = changed + out["fates"]["fixed"]["lines"]
    out["touched_percent"] = round(100 * changed_all / library, 1)
    print(json.dumps(out, indent=1))


if __name__ == "__main__":
    main()
