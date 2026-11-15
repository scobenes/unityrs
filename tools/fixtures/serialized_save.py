"""Record B1 L4 saves with UnityPy 1.25.2, from unityrs-testkit inputs.

--check rebuilds every input and compares the recorded bytes. Actual oracle
failures and malformed saves are kept beside the corrected Tier C outputs:
old tree fields, mandatory pre-v13 trees, pre-v9 data offset and common offset
zero. Rust replays both the UnityPy layout and the semantic acceptance rule.
No asset data or TPK contents are stored.
"""
from __future__ import annotations

import argparse
import inspect
import json
import os
import sys
import textwrap
from contextlib import contextmanager
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools/parity"))


def patched_function(function, old, new, extra=()):
    source = textwrap.dedent(inspect.getsource(function))
    assert old in source
    namespace = dict(function.__globals__)
    source = source.replace(old, new)
    for old, new in extra:
        assert old in source
        source = source.replace(old, new)
    exec(compile(source, "<corrected-writing-oracle>", "exec"), namespace)
    return namespace[function.__name__]


@contextmanager
def corrected(common_zero):
    from UnityPy.files.SerializedFile import SerializedFile, SerializedType
    from UnityPy.helpers.TypeTreeNode import TypeTreeNode
    from UnityPy.streams import EndianBinaryWriter

    # files/SerializedFile.py:151-198, :389-493;
    # helpers/TypeTreeNode.py:213-278. Whole functions, changed only at the
    # documented Tier C writing bugs; no replacement file builder.
    writer = patched_function(SerializedType.write, "serialized_file.dump(writer, version)",
                              "self.node.dump(writer, version)")
    old_tree = patched_function(TypeTreeNode.dump, "self.m_", "node.m_")
    extra = []
    if common_zero:
        extra.append(("meta_writer.write_string_to_null(self.userInformation)",
                      'meta_writer.write_string_to_null(self.userInformation)\n'
                      '        if len(self.userInformation.encode("utf8", "surrogateescape")) == 32767:\n'
                      '            meta_writer.write_bytes(b"\\0")'))
    save = patched_function(SerializedFile.save, "writer.write_u_int(32)", "writer.write_u_int(16)", extra)
    blob = TypeTreeNode.dump_blob
    if common_zero:
        blob = patched_function(blob, "if common_offset:", "if common_offset is not None:")
    string = EndianBinaryWriter.write_string_to_null
    if common_zero:
        # streams/EndianBinaryWriter.py:84-86, corrected for sequential
        # metadata at the reader's full window (blob fixtures are shorter).
        string = patched_function(string, 'self.write(b"\\0")',
                                  'if len(value.encode("utf8", "surrogateescape")) != 32767:\n        self.write(b"\\0")')
    with (patch.object(EndianBinaryWriter, "write_string_to_null", string),
          patch.object(SerializedType, "write", writer), patch.object(TypeTreeNode, "dump", old_tree),
          patch.object(SerializedFile, "save", save), patch.object(TypeTreeNode, "dump_blob", blob)):
        yield


def load(case):
    from UnityPy import config
    from UnityPy.files.SerializedFile import SerializedFile
    from UnityPy.streams import EndianBinaryReader
    with patch.object(config, "SERIALIZED_FILE_PARSE_TYPETREE", case["parse_trees"]):
        return SerializedFile(EndianBinaryReader(bytes.fromhex(case["input"])))


def attempt_save(sf):
    try:
        return {"hex": sf.save(packer="none").hex()}
    except Exception as exc:
        return {"error": type(exc).__name__}


def record(case):
    from UnityPy.files.File import File
    sf = load(case)
    # Two parent File instances pin propagation; neither is an Environment.
    sf.parent = File(parent=File())
    obj = sf.objects[9]
    before = obj.get_raw_data()
    mutation = case["mutation"]
    replacement = None
    if mutation == "raw":
        replacement = b"replacement-\x00\xff\xce\xbb"
        obj.set_raw_data(replacement)
    elif mutation == "empty":
        replacement = b""
        obj.set_raw_data(replacement)
    elif mutation == "typetree":
        value = obj.parse_as_dict()
        value["value"] = -123456
        replacement = obj.save_typetree(value)
    assert obj.get_raw_data() == before
    actual = attempt_save(sf)
    with corrected(common_zero=False):
        sf._enable_type_tree = sf._enable_type_tree or sf.header.version < 13
        layout = sf.save()
    with corrected(common_zero=True):
        semantic = sf.save()
        # A reload of a save must save identically, also under a false config.
        back_case = {**case, "input": semantic.hex()}
        back = load(back_case)
        back._enable_type_tree = back._enable_type_tree or back.header.version < 13
        assert back.save() == semantic, case["name"]
        assert [tuple(getattr(e, key, None) for key in ("temp_empty", "guid", "type", "path")) for e in back.externals] == [
            tuple(getattr(e, key, None) for key in ("temp_empty", "guid", "type", "path")) for e in sf.externals]
        assert {k: o.get_raw_data() for k, o in back.objects.items()} == {
            k: replacement if k == 9 and replacement is not None else o.get_raw_data()
            for k, o in sf.objects.items()}
    return {**case, "replacement": None if replacement is None else replacement.hex(),
            "original": before.hex(), "changed": [sf.is_changed, sf.parent.is_changed, sf.parent.parent.is_changed],
            "actual": actual, "layout": layout.hex(), "semantic": semantic.hex()}


def blobs():
    from UnityPy.helpers.TypeTreeNode import TypeTreeNode
    from UnityPy.streams import EndianBinaryReader, EndianBinaryWriter
    sys.path.insert(0, str(ROOT / "tools/parity/tests"))
    import synth
    cases = []
    tree = synth.Node("AABB", "Base", [synth.Node("int", "AABB"),
                        synth.Node("AABB", "local-λ"), synth.Node("int", "local-λ")])
    for version in (10, 12, 18, 19, 22):
        for endian in ("<", ">"):
            source = synth.blob(tree, version, endian)
            node = TypeTreeNode.parse_blob(EndianBinaryReader(source, endian=endian), version)
            writer = EndianBinaryWriter(endian=endian)
            node.dump_blob(writer, version)
            with corrected(common_zero=True):
                semantic = EndianBinaryWriter(endian=endian)
                node.dump_blob(semantic, version)
            cases.append({"version": version, "big": endian == ">", "input": source.hex(),
                          "layout": writer.bytes.hex(), "semantic": semantic.bytes.hex()})
    return cases


def intern_bytes(records, trees):
    """One shared hex string per distinct byte sequence, in first-use order."""
    data = []
    indexes = {}

    def intern(value):
        if value is None:
            return None
        if value not in indexes:
            indexes[value] = len(data)
            data.append(value)
        return indexes[value]

    for case in records:
        for key in ("input", "replacement", "original", "layout", "semantic"):
            case[key] = intern(case[key])
        if "hex" in case["actual"]:
            case["actual"]["hex"] = intern(case["actual"]["hex"])
    for case in trees:
        for key in ("input", "layout", "semantic"):
            case[key] = intern(case[key])
    return data


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--check", action="store_true")
    parser.add_argument("--inputs", type=Path, help="prebuilt serialized_save_inputs JSON")
    args = parser.parse_args()
    import UnityPy
    import child
    assert UnityPy.__version__ == "1.25.2"
    if args.inputs:
        inputs = json.loads(args.inputs.read_text())
    else:
        env = dict(os.environ, CARGO_ENCODED_RUSTFLAGS="")
        proc = child.run(["cargo", "run", "--quiet", "--locked", "-p", "unityrs-core",
                          "--example", "serialized_save_inputs"], cwd=ROOT, env=env,
                         check=True, capture_output=True, text=True)
        inputs = json.loads(proc.stdout)
    records = [record(case) for case in inputs]
    trees = blobs()
    data = intern_bytes(records, trees)
    # One case per line: fixtures remain reviewable and below the size gate.
    body = '{"reference":"UnityPy 1.25.2","data":[\n'
    body += ",\n".join(json.dumps(value) for value in data) + '],"cases":[\n'
    body += ",\n".join(json.dumps(case, ensure_ascii=True, separators=(",", ":")) for case in records)
    body += '],"blobs":[\n' + ",\n".join(json.dumps(case, separators=(",", ":")) for case in trees) + "]}\n"
    path = ROOT / "crates/unityrs-core/tests/fixtures/serialized_save.json"
    if args.check:
        if not path.exists() or path.read_bytes() != body.encode():
            parser.exit(1, "stale serialized_save.json; regenerate in the pinned oracle\n")
    else:
        path.write_bytes(body.encode())
    print(f"serialized_save: {len(records)} files, 10 blob trees")
    return 0


if __name__ == "__main__":
    sys.exit(main())
