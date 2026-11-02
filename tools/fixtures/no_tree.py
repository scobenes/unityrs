"""Oracle fixture: which exception UnityPy 1.25.2 raises for an object no tree answers (docs/plans/core.md 4.2).

Each case is a stripped SerializedFile of one object, built here with `tools/parity/tests/synth.py`, read with
one of `tools/fixtures/tpk.py`'s synthetic TPKs in place of the bundled one. UnityPy's `_get_typetree_node`
(`files/ObjectReader.py:285-307`) returns the TPK's tree, or raises its `KeyError` (no such class),
`ValueError` (no entry at or below the version, or the class removed there) or `AssertionError` (no release
root), or, for a file below format 7, whose `ObjectReader.version` is `None`, the comparison's
`NotImplementedError` (or the `KeyError` before it, or the `ValueError` of a class with no entries). Each case
records the file (hex), the TPK (hex) and the outcome: "tree", or the exception's name.
`cargo test -p unityrs-tpk --test no_tree` replays them through the core's object read and write, whose
`NoTypeTree` error must name the same exception (review round 11: nothing public told them apart).

    scripts/dev python tools/fixtures/no_tree.py           # write the fixture
    scripts/dev python tools/fixtures/no_tree.py --check   # regenerate and compare
"""

from __future__ import annotations

import argparse
import contextlib
import io
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "crates/unityrs-tpk/tests/fixtures/tpk/no_tree.json"
sys.path.insert(0, str(ROOT / "tools/parity"))
sys.path.insert(0, str(ROOT / "tools/parity/tests"))
sys.path.insert(0, str(ROOT / "tools/fixtures"))

import synth as S  # noqa: E402
import tpk  # noqa: E402
import UnityPy  # noqa: E402

from scaffold import dump  # noqa: E402

TREE = S.Node("TextAsset", "Base", [S.Node("string", "m_Name"), S.Node("string", "m_Script")])

# (TPK case, class id, file format, unity version): the format-22 files carry the version, the format-6 ones none.
CASES = [
    ("root_flags", 46, 22, "2018.4.2f1"),  # a class the TPK does not hold
    ("root_flags", 11, 22, "5.0.0f1"),  # below the class's first entry
    ("root_flags", 10, 22, "2018.4.2f1"),  # an entry with neither root
    ("root_flags", 20, 22, "2022.1.0f1"),  # an entry with an editor root alone
    ("root_flags", 11, 22, "2018.4.2f1"),  # an entry with both roots: a tree
    ("root_flags", 11, 6, None),  # no version: the comparison
    ("root_flags", 46, 6, None),  # no version, no class
    ("entries", 50, 22, "2018.0.0f1"),  # the class removed at that version
    ("entries", 52, 22, "2018.0.0f1"),  # a class with no entries
    ("entries", 52, 6, None),  # no version, no entries: nothing to compare
    ("entries", 54, 6, None),  # no version, an entry that marks the class removed
]


def tpk_bytes(name: str) -> bytes:
    return next(data for case, data, _depth in tpk.synthetic_cases() if case == name)


def stripped_file(class_id: int, version: int, unity_version: str | None) -> bytes:
    data = S.s32("a") + S.s32("b")
    if version >= 16:
        return S.serialized_file(
            [S.Type(class_id, TREE)],
            [S.Obj(1, 0, data, class_id=class_id)],
            version=version,
            unity_version=unity_version.encode(),
            enable_type_tree=False,
        )
    # Below format 16 an object names its class; the one type record is another class's, so none is the object's.
    return S.serialized_file([S.Type(1, TREE)], [S.Obj(1, class_id, data)], version=version)


def outcome(file: bytes, blob) -> str:
    with tpk.Installed(blob), contextlib.redirect_stdout(io.StringIO()):
        sf = UnityPy.Environment().load_file(file, name="CAB-N")
        obj = sf.objects[1]
        try:
            obj._get_typetree_node()
        except Exception as e:  # noqa: BLE001 - the raise is the recorded outcome
            return type(e).__name__
    return "tree"


def fixture() -> dict:
    cases = []
    for name, class_id, version, unity_version in CASES:
        data = tpk_bytes(name)
        file = stripped_file(class_id, version, unity_version)
        cases.append(
            {
                "tpk_case": name,
                "tpk": data.hex(),
                "class_id": class_id,
                "format": version,
                "unity_version": unity_version,
                "file": file.hex(),
                "outcome": outcome(file, tpk.load(data)),
            }
        )
    return {"generator": "tools/fixtures/no_tree.py", "unitypy": UnityPy.__version__, "cases": cases}


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--check", action="store_true", help="regenerate and compare, write nothing")
    args = ap.parse_args()
    if UnityPy.__version__ != "1.25.2":
        print(f"needs UnityPy 1.25.2, found {UnityPy.__version__}", file=sys.stderr)
        return 2
    text = dump(fixture())
    if args.check:
        if not OUT.exists() or OUT.read_text() != text:
            print(f"stale fixture: {OUT.name}", file=sys.stderr)
            return 1
        return 0
    OUT.write_text(text)
    return 0


if __name__ == "__main__":
    sys.exit(main())
