#!/usr/bin/env python3
"""Compare `rabex-dump save` output with the originals and UnityPy (docs/spikes/rabex.md).

    python save_compare.py SAVE_DIR CORPUS_ROOT

For each re-saved bundle:
- each SerializedFile entry rabex re-saved: equal to the original bytes? Is
  the original's object table in data order? If the re-save differs, does
  the first difference fall in the metadata (before the header's data
  offset) or in the data? UnityPy's `SerializedFile.save()` of the same
  entry: equal to the original?
- the bundle: byte-equal to UnityPy's `save(packer="none")` of the original?
  Its header flags beside the original's and UnityPy's.
- on reload by UnityPy, every SerializedFile field `sf_diffs` covers: header
  fields (the reserved bytes and the version-22 unknown field included),
  types and ref types (with their type dependencies, ref-type names, and trees,
  each node's m_Index and m_RefTypeHash included), script types, externals,
  the object table, each object's raw bytes, and the layout (metadata size,
  data offset, file size, and each object's offset in the data). A bundle is
  "semantically equal" when only the layout differs.
"""

import json
import sys
from collections import Counter
from pathlib import Path

import UnityPy
from UnityPy.files import SerializedFile

LAYOUT = ("layout", "object_offsets")


def tree_rows(node):
    if node is None:
        return None
    rows, stack = [], [node]
    while stack:
        n = stack.pop()
        rows.append(
            (
                n.m_Level,
                n.m_Type,
                n.m_Name,
                n.m_ByteSize,
                getattr(n, "m_Version", None),
                getattr(n, "m_TypeFlags", None),
                getattr(n, "m_MetaFlag", None),
                # Stored per node; rabex writes the pre-order position, not the m_Index it read.
                getattr(n, "m_Index", None),
                getattr(n, "m_RefTypeHash", None),  # blob format at version >= 19
                getattr(n, "m_VariableCount", None),  # old format, tree version 2
            )
        )
        stack.extend(reversed(getattr(n, "m_Children", None) or []))
    return rows


def type_rows(types):
    return [
        (
            t.class_id,
            getattr(t, "is_stripped_type", None),
            getattr(t, "script_type_index", None),
            getattr(t, "script_id", None),
            getattr(t, "old_type_hash", None),
            # Version >= 21: a ref type's class, namespace and assembly names; a type's dependency list.
            getattr(t, "m_ClassName", None),
            getattr(t, "m_NameSpace", None),
            getattr(t, "m_AssemblyName", None),
            getattr(t, "type_dependencies", None),
            tree_rows(getattr(t, "node", None)),
        )
        for t in types or []
    ]


def fs_flags(data):
    """A UnityFS bundle's header flags (after the signature, format, two version strings, size and two sizes)."""
    p = data.index(b"\0") + 1 + 4
    p = data.index(b"\0", p) + 1
    p = data.index(b"\0", p) + 1
    return int.from_bytes(data[p + 16 : p + 20], "big")


def sf_fields(sf):
    """The fields of a parsed SerializedFile that a re-save must keep, by group."""
    h = sf.header
    objs = list(sf.objects.values())
    return {
        "header": (
            h.version,
            h.endian,
            getattr(h, "reserved", None),  # 3 bytes at version >= 9; rabex writes zeros at 22+
            getattr(sf, "unknown", None),  # the i64 after data_offset at version >= 22
            sf.unity_version,
            sf.target_platform,
            sf._enable_type_tree,  # noqa: SLF001
            getattr(sf, "big_id_enabled", 0),
            getattr(sf, "userInformation", None),
        ),
        "types": type_rows(sf.types),
        "ref_types": type_rows(getattr(sf, "ref_types", None)),
        "script_types": [
            (s.local_serialized_file_index, s.local_identifier_in_file) for s in sf.script_types or []
        ],
        "externals": [(e.path, getattr(e, "guid", None), getattr(e, "type", None)) for e in sf.externals or []],
        "object_table": [
            (o.path_id, o.type_id, o.class_id, o.byte_size, o.is_destroyed, o.is_stripped) for o in objs
        ],
        "object_data": [(o.path_id, bytes(o.get_raw_data())) for o in objs],
        "layout": (h.metadata_size, h.data_offset, h.file_size),
        "object_offsets": [(o.path_id, o.byte_start - h.data_offset) for o in objs],
    }


def sf_diffs(a, b):
    """The field groups in which two parsed SerializedFiles differ, in a fixed order."""
    fa, fb = sf_fields(a), sf_fields(b)
    return [k for k in fa if fa[k] != fb[k]]


def moved_objects(a, b):
    """(objects whose offset in the data changed, objects) between two parsed files."""
    oa = dict(sf_fields(a)["object_offsets"])
    ob = dict(sf_fields(b)["object_offsets"])
    return sum(1 for pid, off in oa.items() if ob.get(pid) != off), len(oa)


def main():
    out, root = Path(sys.argv[1]), Path(sys.argv[2])
    report = json.loads((out / "report.json").read_text())
    # Always reported, so a 0 is printed rather than left out.
    tally = Counter(original_sf_with_type_dependencies=0, original_sf_with_ref_types=0)
    diff_fields = Counter()
    flags = Counter()
    moved = [0, 0]
    examples = []
    for r in report["bundles"]:
        if "error" in r:
            tally["rabex_bundle_error"] += 1
            examples.append(r)
            continue
        orig = UnityPy.Environment().load_file(str(root / r["input"]))
        saved_bytes = (out / r["saved"]).read_bytes()
        upy = bytes(orig.save(packer="none"))
        tally["bundle_equal_unitypy_none" if upy == saved_bytes else "bundle_differs_unitypy_none"] += 1
        original_bytes = (root / r["input"]).read_bytes()
        flags[f"original {fs_flags(original_bytes):#x}, rabex {fs_flags(saved_bytes):#x}, unitypy {fs_flags(upy):#x}"] += 1
        if upy != saved_bytes:
            first = next(
                (i for i, (x, y) in enumerate(zip(upy, saved_bytes, strict=False)) if x != y),
                min(len(upy), len(saved_bytes)),
            )
            if len(examples) < 8:
                examples.append(
                    {
                        "input": r["input"],
                        "bundle_first_diff": first,
                        "unitypy_size": len(upy),
                        "rabex_size": len(saved_bytes),
                    }
                )
        for e in r["serialized"]:
            sf = orig.files[e["entry"]]
            if "error" in e:
                tally["rabex_sf_error"] += 1
                if len(examples) < 8:
                    examples.append(e)
            else:
                outcome = "equal" if e["equal"] else "differs"
                tally[f"rabex_sf_{outcome}_original"] += 1
                # Is the original's object table in data order? rabex writes
                # object data in table order.
                starts = [o.byte_start for o in sf.objects.values()]
                order = "in" if starts == sorted(starts) else "out_of"
                tally[f"rabex_sf_{outcome}_table_{order}_offset_order"] += 1
                if not e["equal"]:
                    where = "metadata" if e["first_diff"] < sf.header.data_offset else "data"
                    tally["rabex_sf_first_diff_in_" + where] += 1
            original = bytes(sf.reader.bytes) if hasattr(sf.reader, "bytes") else None
            try:
                tally[
                    "unitypy_sf_equal_original" if bytes(sf.save()) == original else "unitypy_sf_differs_original"
                ] += 1
            except Exception:  # noqa: BLE001
                tally["unitypy_sf_error"] += 1
        # Reload rabex's bundle and compare entries, metadata and objects.
        try:
            again = UnityPy.Environment().load_file(saved_bytes)
        except Exception as ex:  # noqa: BLE001
            tally["semantic_reload_error"] += 1
            examples.append({"input": r["input"], "reload": str(ex)[:200]})
            continue
        a, b = orig.files, again.files
        fields = set()
        if list(a) != list(b):
            fields.add("entries")
        for name, x in a.items():
            y = b.get(name)
            if y is None or getattr(x, "flags", 0) != getattr(y, "flags", 0):
                fields.add("entries")
            elif isinstance(x, SerializedFile):
                if not isinstance(y, SerializedFile):
                    fields.add("entries")
                    continue
                fields.update(sf_diffs(x, y))
                # How much of the type and ref-type checks the originals exercise.
                if any(getattr(t, "type_dependencies", None) for t in x.types or []):
                    tally["original_sf_with_type_dependencies"] += 1
                if getattr(x, "ref_types", None):
                    tally["original_sf_with_ref_types"] += 1
                m, n = moved_objects(x, y)
                moved[0] += m
                moved[1] += n
            elif bytes(x.bytes) != bytes(y.bytes):
                fields.add("entry_bytes")
        diff_fields.update(fields)
        tally["semantic_equal" if fields <= set(LAYOUT) else "semantic_differs"] += 1
        tally["layout_equal" if not fields & set(LAYOUT) else "layout_differs"] += 1
    result = {
        "shim_fs6": report["shim_fs6"],
        "tally": tally,
        "bundles_differing_by_field": diff_fields,
        "bundle_header_flags": flags,
        "objects_moved": {"moved": moved[0], "of": moved[1]},
        "examples": examples,
    }
    json.dump(result, sys.stdout, indent=1)
    print()


if __name__ == "__main__":
    main()
