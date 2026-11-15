"""K1: synthetic provider/API records; --check also compares the image TPK in memory.

Only api.json is saved. --image runs the Rust stripped-file/L2b test in the
container; its temporary inputs and both TPK-derived answers stay outside git.
"""
from __future__ import annotations

import argparse
from contextlib import contextmanager
import importlib
import json
import os
from pathlib import Path
import struct
import sys
import warnings
from types import SimpleNamespace
from unittest.mock import patch

import UnityPy
from UnityPy import config
from UnityPy.helpers.TypeTreeNode import TypeTreeNode
from UnityPy.helpers import Tpk

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools/parity"))
sys.path.insert(0, str(ROOT / "tools/parity/tests"))
import child  # noqa: E402
import scratch  # noqa: E402
import synth as S  # noqa: E402

OUT = ROOT / "crates/unityrs-core/tests/fixtures/providers/api.json"
assert UnityPy.__version__ == "1.25.2", "the provider oracle must be UnityPy 1.25.2"
FIELDS = ("m_Level", "m_Type", "m_Name", "m_ByteSize", "m_Version",
          "m_TypeFlags", "m_VariableCount", "m_Index", "m_MetaFlag", "m_RefTypeHash")


def rows(node):
    return [[getattr(n, key) for key in FIELDS] for n in node.traverse()]


def outcome(fn):
    try:
        return {"value": fn()}
    except Exception as exc:
        return {"error": type(exc).__name__, "message": str(exc)}


@contextmanager
def reader_path(sf, raw, reader):
    """Use a real file stream: BytesIO accepts negative reads that files reject."""
    if reader != "python_stream":
        yield
        return
    from UnityPy.streams import EndianBinaryReader
    with scratch.folder(prefix="unityrs-provider-stream-") as tmp:
        path = Path(tmp) / "matrix.assets"
        path.write_bytes(raw)
        stream = EndianBinaryReader(str(path), endian="<")
        sf.reader = stream
        for obj in sf.objects.values():
            obj.reader = stream
        try:
            yield
        finally:
            stream.dispose()


def constructor_dict(value):
    """JSON child specs stand for actual constructor-supplied node instances."""
    value = dict(value)
    if "m_Children" in value:
        value["m_Children"] = [TypeTreeNode(**constructor_dict(c)) for c in value["m_Children"]]
    return value


def snapshot(node):
    flattened = list(node.traverse())
    # Number occurrences, not identities: repeated child edges stay repeated.
    children = [[] for _ in flattened]
    stack = [(node, None)]
    for index in range(len(flattened)):
        current, parent = stack.pop()
        if parent is not None:
            children[parent].append(index)
        stack.extend((child, index) for child in reversed(current.m_Children))
    return {"rows": rows(node), "children": children}


def catch_scope():
    """Handwritten trees only; exercise ObjectReader's actual ValueError catch."""
    results = []
    cases = ("python_pair", "boost_pair", "python_array", "boost_array",
             "python_eof", "boost_eof", "boost_bool", "boost_u8", "boost_s8",
             "boost_bytes", "boost_string", "boost_bool_array", "boost_u8_array",
             "boost_s8_array", "boost_int_array", "python_ref", "boost_ref",
             "python_s8_array",
             "missing_entry", "unknown_class", "no_release_root", "no_version",
             "check_read", "missing_attribute", "null_pointer", "missing_object",
             "invalid_file_id", "negative_file_id",
             "python_absent_refs", "boost_absent_refs",
             "python_non_pptr", "boost_non_pptr", "boost_extra_pptr",
             "numeric_namespace", "numeric_class", "bool_namespace",
             "float_namespace", "boost_bool_pointer", "boost_float_pointer",
             "python_bool_pointer", "python_float_pointer")
    for kind in cases:
        script = S.Node("pair" if kind.endswith("pair") else "MonoScript", "Base")
        if kind.endswith("eof"):
            script = S.Node("MonoScript", "Base", [S.Node("int", f"x{i}") for i in range(32)])
        payload = b"\0" if kind.startswith("boost_") or kind == "check_read" else b""
        if kind in ("python_array", "boost_array"):
            script = S.Node("Vector", "Base", [S.Node("Array", "Array")])
        if kind in ("boost_bool", "boost_u8", "boost_s8"):
            ty = {"boost_bool": "bool", "boost_u8": "UInt8", "boost_s8": "SInt8"}[kind]
            script = S.Node("MonoScript", "Base", [S.Node(ty, "x"), S.Node(ty, "y")])
        if kind in ("boost_bytes", "boost_string"):
            script = S.Node("TypelessData" if kind == "boost_bytes" else "string", "Base")
            payload = struct.pack("<i", 8) + b"x"
        if kind.endswith("_array") and kind not in ("python_array", "boost_array"):
            ty = {"boost_bool_array": "bool", "boost_u8_array": "UInt8",
                  "boost_s8_array": "SInt8", "boost_int_array": "int",
                  "python_s8_array": "SInt8"}[kind]
            script = S.Node("Vector", "Base", [S.Node("Array", "Array",
                [S.Node("int", "size"), S.Node(ty, "data", meta=0x4000)])])
            if kind.startswith("boost_"):
                payload = struct.pack("<i", 8) + b"x"
        if kind.endswith("_ref"):
            script = S.Node("ReferencedObject", "Base", [S.Node("ReferencedObjectData", "data")])
        if kind.endswith("_absent_refs"):
            script = S.Node("ReferencedObject", "Base", [S.Node("Type", "type", [
                S.Node("string", name) for name in ("class", "ns", "asm")]),
                S.Node("ReferencedObjectData", "data")])
            if kind.startswith("boost_"):
                payload = bytes(12)
        asks = []
        if kind in ("numeric_namespace", "numeric_class", "bool_namespace", "float_namespace"):
            namespace = {"numeric_namespace": ("int", struct.pack("<i", 7)),
                         "bool_namespace": ("bool", b"\1"),
                         "float_namespace": ("double", struct.pack("<d", 1.5))}.get(kind)
            script = S.Node("MonoScript", "Base", [S.Node("string", "m_AssemblyName"),
                S.Node(namespace[0] if namespace else "string", "m_Namespace"),
                S.Node("int" if kind == "numeric_class" else "string", "m_ClassName")])
            payload = S.s32("Assembly")
            payload += namespace[1] if namespace else S.s32("Synthetic")
            payload += struct.pack("<i", 7) if kind == "numeric_class" else struct.pack("<i", 7) + b"Example"
            payload += bytes(-len(payload) % 4)
        file_id = {"invalid_file_id": 1, "negative_file_id": -1}.get(kind, 0)
        path_id = {"null_pointer": 0, "missing_object": 3}.get(kind, 2)
        env = UnityPy.Environment()
        sf = env.load_file(S.serialized_file([S.Type(114), S.Type(115)],
            [S.Obj(1, 0, b"" if kind == "python_non_pptr" else struct.pack("<iq", file_id, path_id)),
             S.Obj(2, 1, payload)],
            enable_type_tree=False, version=17 if kind.endswith("_absent_refs") else 22), name="catch.assets")
        env.typetree_generator = SimpleNamespace(get_nodes_up=lambda a, f: asks.append([a, f]))
        head_rows = [dict(m_Level=i, m_Type=t, m_Name=n)
            for i, t, n in ((0, "MonoBehaviour", "Base"), (1, "PPtr<MonoScript>", "m_Script"),
                            (2, "int", "m_FileID"), (2, "SInt64", "m_PathID"))]
        if kind.endswith("non_pptr"):
            head_rows[1]["m_Type"] = "SyntheticPointer"
        if kind == "boost_extra_pptr":
            head_rows.append(dict(m_Level=2, m_Type="int", m_Name="extra"))
            # Keep the head's extra field within Boost's object slice.
            obj_data = struct.pack("<iqi", file_id, path_id, 0)
            sf = env.load_file(S.serialized_file([S.Type(114), S.Type(115)],
                [S.Obj(1, 0, obj_data), S.Obj(2, 1, payload)], enable_type_tree=False), name="extra.assets")
        if kind.endswith("_pointer") and kind != "null_pointer":
            # False and 0.0 compare equal to the integer null path id.
            ty = "bool" if "bool" in kind else "double"
            head_rows[3]["m_Type"] = ty
            data = bytes(5 if ty == "bool" else 12)
            objects = [S.Obj(1, 0, data if kind.startswith("boost") else b""),
                       S.Obj(2, 1, payload)]
            sf = env.load_file(S.serialized_file([S.Type(114), S.Type(115)], objects,
                               enable_type_tree=False), name="numeric-pointer.assets")
        mb_node = TypeTreeNode.from_list(head_rows)
        # Use the synthetic serializer to construct the script node, never a TPK.
        embedded = env.load_file(S.serialized_file([S.Type(115, script)], []),
                                 name="node.assets").types[0].node

        def lookup(class_id, version):
            if class_id == 114:
                return mb_node
            if kind == "missing_entry":
                return Tpk.get_item_for_version(version, [])
            if kind in ("unknown_class", "no_release_root", "no_version"):
                raise {"unknown_class": KeyError, "no_release_root": AssertionError,
                       "no_version": NotImplementedError}[kind]("synthetic")
            return embedded

        obj = sf.objects[1]
        with patch("UnityPy.files.ObjectReader.get_typetree_node", side_effect=lookup):
            results.append({"kind": kind, "head": rows(mb_node), "file": list(sf.reader.bytes),
                "script": rows(embedded), "payload": list(payload),
                "automatic": outcome(lambda: obj.parse_as_dict(check_read=False)),
                "explicit": outcome(lambda: rows(obj.generate_monobehaviour_node(mb_node))),
                "calls": asks})
    return results


def reader_exception_matrix():
    """Inject at each real reader boundary; the ObjectReader catches stay real.

    catch_scope supplies concrete malformed inputs as well. This cross product
    closes the catch contract even for exceptions requiring a custom stream or
    class binding, without manufacturing dangerous sizes or allocating to OOM.
    No tree here is read from a TPK or a dump.
    """
    from UnityPy.exceptions import TypeTreeError
    helper = importlib.import_module("UnityPy.helpers.TypeTreeHelper")

    class CustomValueError(ValueError):
        pass

    class CustomError(Exception):
        pass

    exceptions = [ValueError, UnicodeError, CustomValueError,
                  IndexError, EOFError, AttributeError, struct.error, OverflowError,
                  TypeError, KeyError, RuntimeError, AssertionError, NotImplementedError,
                  MemoryError, FileNotFoundError, OSError, CustomError]
    errors = [e("synthetic") for e in exceptions] + [TypeTreeError("synthetic", []),
        UnicodeDecodeError("utf-8", b"\xff", 0, 1, "synthetic"),
        UnicodeEncodeError("ascii", "例", 0, 1, "synthetic"),
        UnicodeTranslateError("例", 0, 1, "synthetic")]
    head = TypeTreeNode.from_list([dict(m_Level=i, m_Type=t, m_Name=n)
        for i, t, n in ((0, "MonoBehaviour", "Base"), (1, "PPtr<MonoScript>", "m_Script"),
                       (2, "int", "m_FileID"), (2, "SInt64", "m_PathID"))])
    script = TypeTreeNode.from_list([dict(m_Level=0, m_Type="MonoScript", m_Name="Base")])
    result = []
    for reader in ("python_memory", "python_stream", "boost"):
        for stage in ("head", "script"):
            for error in errors:
                env = UnityPy.Environment()
                raw = S.serialized_file([S.Type(114), S.Type(115)],
                    [S.Obj(1, 0, struct.pack("<iq", 0, 2)), S.Obj(2, 1, b"\0")],
                    enable_type_tree=False)
                sf = env.load_file(raw, name="matrix.assets")
                obj = sf.objects[1]
                env.typetree_generator = SimpleNamespace(get_nodes_up=lambda a, f: None)
                target = "MonoBehaviour" if stage == "head" else "MonoScript"
                boundary = "read_typetree_boost" if reader == "boost" else "read_value"
                original = getattr(helper, boundary)
                if reader != "boost":
                    sf.objects[1 if stage == "head" else 2].byte_size = 0

                def fail(*args):
                    node = args[1] if reader == "boost" else args[0]
                    if node.m_Type == target:
                        raise error
                    return original(*args)

                with reader_path(sf, raw, reader), patch("UnityPy.files.ObjectReader.get_typetree_node",
                           side_effect=lambda c, v: head if c == 114 else script), \
                     patch.object(helper, boundary, side_effect=fail):
                    automatic = outcome(lambda: rows(obj._get_typetree_node()))
                    explicit = outcome(lambda: rows(obj.generate_monobehaviour_node(head)))
                assert explicit["error"] == type(error).__name__
                caught = "error" not in automatic
                assert caught == isinstance(error, ValueError)
                result.append(dict(reader=reader, stage=stage, error=type(error).__name__,
                    value_error=isinstance(error, ValueError), caught=caught,
                    automatic=automatic, explicit=explicit))
    return result


def malformed_reader_matrix():
    """Real malformed inputs through each reader and both fallback reads."""

    def vector(element):
        return S.vec("probe", element)

    probes = [("pair", S.Node("pair", "probe"), b"", 22),
              ("array", S.Node("Vector", "probe", [S.Node("Array", "Array")]), bytes(4), 22),
              ("negative_array", vector(S.Node("int", "data")), struct.pack("<i", -1), 22),
              ("aligned_s8", vector(S.Node("SInt8", "data", meta=0x4000)), bytes(4), 22),
              ("aligned_bytes", vector(S.Node("TypelessData", "data", meta=0x4000)),
               struct.pack("<i", 1), 22),
              ("aligned_pair", vector(S.Node("pair", "data", meta=0x4000)), bytes(4), 22),
              ("aligned_vector", vector(S.Node("Vector", "data", [S.Node("Array", "Array")],
                                                meta=0x4000)), bytes(4), 22),
              ("missing_ref_type", S.Node("ReferencedObject", "probe", [
                  S.Node("ReferencedObjectData", "data")]), b"", 22),
              ("scalar_ref_type", S.Node("ReferencedObject", "probe", [
                  S.Node("int", "type"), S.Node("ReferencedObjectData", "data")]), bytes(4), 22),
              ("missing_ref_key", S.Node("ReferencedObject", "probe", [
                  S.Node("Type", "type"), S.Node("ReferencedObjectData", "data")]), b"", 22),
              ("pointer_missing_file", S.Node("PPtr<Synthetic>", "probe", [
                  S.Node("SInt64", "m_PathID")]), bytes(8), 22),
              ("pointer_missing_path", S.Node("PPtr<Synthetic>", "probe", [
                  S.Node("int", "m_FileID")]), bytes(4), 22),
              ("pointer_array_extra", vector(S.Node("PPtr<Synthetic>", "data", [
                  S.Node("int", "m_FileID"), S.Node("SInt64", "m_PathID"), S.Node("int", "extra")],
                  meta=0x4000)), struct.pack("<i", 1) + bytes(16), 22)]
    for fmt in (17, 22):
        probes.append((f"ref_types_{fmt}", S.Node("ReferencedObject", "probe", [
            S.Node("Type", "type", [S.Node("string", n) for n in ("class", "ns", "asm")]),
            S.Node("ReferencedObjectData", "data")]), bytes(12), fmt))
    for ty in ("bool", "UInt8", "SInt8", "int", "double"):
        probes.append((f"scalar_{ty}", S.Node("Synthetic", "probe", [
            S.Node(ty, f"x{i}") for i in range(96)]), b"\0", 22))
        probes.append((f"array_{ty}", vector(S.Node(ty, "data", meta=0x4000)),
                       struct.pack("<i", 256), 22))
    for ty in ("string", "TypelessData"):
        for size in (-2, 256):
            probes.append((f"{ty}_{size}", S.Node(ty, "probe"), struct.pack("<i", size), 22))

    result = []
    pointer = S.Node("PPtr<MonoScript>", "m_Script", [S.Node("int", "m_FileID"),
                                                     S.Node("SInt64", "m_PathID")])
    for reader in ("python_memory", "python_stream", "boost"):
        for stage in ("head", "script"):
            for name, probe, payload, fmt in probes:
                head = S.Node("MonoBehaviour", "Base", [probe, pointer] if stage == "head" else [pointer])
                script = S.Node("MonoScript", "Base", [probe] if stage == "script" else [])
                env = UnityPy.Environment()
                node_file = env.load_file(S.serialized_file([S.Type(114, head), S.Type(115, script)], []),
                                          name="nodes.assets")
                hnode, snode = (t.node for t in node_file.types)
                data = (payload or b"\0") if reader == "boost" else b""
                objects = [S.Obj(1, 0, data if stage == "head" else struct.pack("<iq", 0, 2))]
                if reader != "boost" and stage == "head":
                    objects.append(S.Obj(3, 0, payload + bytes(64)))
                objects.append(S.Obj(2, 1, data if stage == "script" else b"\0"))
                if reader != "boost" and stage == "script":
                    objects.append(S.Obj(3, 1, payload + bytes(64)))
                raw = S.serialized_file([S.Type(114), S.Type(115)], objects,
                                        enable_type_tree=False, version=fmt)
                sf = env.load_file(raw, name="matrix.assets")
                env.typetree_generator = SimpleNamespace(get_nodes_up=lambda a, f: None)
                obj = sf.objects[1]
                with reader_path(sf, raw, reader), patch("UnityPy.files.ObjectReader.get_typetree_node",
                           side_effect=lambda c, v: hnode if c == 114 else snode):
                    automatic = outcome(lambda: rows(obj._get_typetree_node()))
                    explicit = outcome(lambda: rows(obj.generate_monobehaviour_node(hnode)))
                result.append(dict(kind=f"{reader}_{stage}_{name}", reader=reader, stage=stage,
                    head=rows(hnode), script=rows(snode), file=list(raw),
                    automatic=automatic, explicit=explicit))
    return result


def api():
    base = [{"m_Level": 0, "m_Type": "Synthetic", "m_Name": "Base"},
            {"m_Level": 1, "m_Type": "int", "m_Name": "x"}]
    cases = []
    inputs = [[], base, [dict(n, m_ByteSize=-1, m_Version=2) for n in base],
              [base[0], dict(base[1], m_ByteSize=4)],
              [dict(base[0], m_Version=1), base[1]],
              [{"m_Type": "X", "m_Name": "Base"}],
              [base[0], {"m_Level": 1, "m_Name": "x"}],
              [base[0], dict(base[1], m_Level=3), dict(base[1], m_Level=1)],
              [base[0], base[1], dict(base[0], m_Name="Dropped"), base[1]],
              [base[0], dict(base[1], m_Level=-2)],
              [dict(base[0], m_Level=-1), base[1]]]
    for key in ("m_Level", "m_Type", "m_Name", "m_ByteSize", "m_Version"):
        complete = dict(base[0], m_ByteSize=4, m_Version=2)
        inputs.append([complete, {k: v for k, v in complete.items() if k != key}])
    inputs += [[base[0], dict(base[1], m_Version=2)],
               [dict(base[0], m_Children=[dict(base[1], m_Level=7, m_Name="kept",
                                             m_ByteSize=4, m_Version=1)]), base[1]],
               [dict(base[0], m_Children=[]), base[1]]]
    inputs.append([dict(base[0], m_Children=[dict(base[1], m_Level=-2,
                                                m_ByteSize=4, m_Version=1)])])
    for value in inputs:
        # Boost's failed constructors decref borrowed arguments. Isolate each
        # case and exit after recording, before interpreter teardown reuses them.
        result = child.run([sys.executable, __file__, "--constructor-case", json.dumps(value)],
                           capture_output=True, text=True, check=True)
        cases.append({"input": value, **json.loads(result.stdout)})
    node = TypeTreeNode.from_list(base)
    relinked = rows(TypeTreeNode.from_list(list(node.traverse())))
    nested = TypeTreeNode.from_list(base + [dict(base[1], m_Level=2, m_Name="y")])
    nested_relinks = []
    for _ in range(2):
        nested = TypeTreeNode.from_list(list(nested.traverse()))
        nested_relinks.append(rows(nested))
    jumped = TypeTreeNode.from_list([base[0], dict(base[1], m_Level=3),
                                    dict(base[1], m_Name="y")])
    jumped = TypeTreeNode.from_list(list(jumped.traverse()))
    jumped_relinked = snapshot(jumped)
    supplied_specs = [dict(m_Level=3, m_Type="int", m_Name="c", m_ByteSize=0, m_Version=0, m_Children=[]),
                      dict(m_Level=2, m_Type="Synthetic", m_Name="a", m_ByteSize=0, m_Version=0, m_Children=[0]),
                      dict(m_Level=1, m_Type="Synthetic", m_Name="b", m_ByteSize=0, m_Version=0, m_Children=[1])]
    supplied = []
    for spec in supplied_specs:
        supplied.append(TypeTreeNode(**dict(spec, m_Children=[supplied[i] for i in spec["m_Children"]])))
    shared_root = dict(base[0], m_Children=[2, 1])
    shared = TypeTreeNode.from_list([dict(shared_root, m_Children=[supplied[i] for i in shared_root["m_Children"]])])
    shared_children = {"nodes": supplied_specs, "root": shared_root, "initial": snapshot(shared)}
    shared_children["relinked"] = snapshot(TypeTreeNode.from_list(list(shared.traverse())))
    explicit_env = UnityPy.Environment()
    explicit = explicit_env.load_file(S.serialized_file([S.Type(987654)], [S.Obj(1,0,struct.pack("<i",42))],
        enable_type_tree=False), name="explicit.assets").objects[1]
    explicit_node = TypeTreeNode.from_list(base)
    node_reads = [{"case": name, **outcome(fn)} for name, fn in (
        ("dictionary_nodes", lambda: explicit.read_typetree(nodes=base)),
        ("node_instance", lambda: explicit.read_typetree(nodes=explicit_node)),
        ("relinked_node_instances", lambda: explicit.read_typetree(nodes=list(explicit_node.traverse()))),
        ("empty_list_constructor_rejection_deferred_to_binding", lambda: explicit.read_typetree(nodes=[])),
        ("empty_tuple_type_rejection_deferred_to_binding", lambda: explicit.read_typetree(nodes=())))]
    fallback = []
    old = config.FALLBACK_UNITY_VERSION
    try:
        with warnings.catch_warnings(record=True) as seen:
            warnings.simplefilter("once", UnityPy.exceptions.UnityVersionFallbackWarning)
            for value in (None, 7, "", "not-a-version", "2022.3.0f1", "2022.3.0f1", "", "2021.3.0f1", "版本\n"):
                config.FALLBACK_UNITY_VERSION = value
                start = len(seen)
                result = outcome(config.get_fallback_version)
                result.update(input=value, warnings=[{"category": w.category.__name__, "message": str(w.message)}
                                                    for w in seen[start:]])
                if result.get("error"):
                    try:
                        config.get_fallback_version()
                    except Exception as exc:
                        result["message"] = str(exc)
                fallback.append(result)
    finally:
        config.FALLBACK_UNITY_VERSION = old
    # No TPK here: supply our own head to ObjectReader and generator shim.
    head = S.Node("MonoBehaviour", "Base", [S.Node("PPtr<MonoScript>", "m_Script",
                   [S.Node("int", "m_FileID"), S.Node("SInt64", "m_PathID")])])
    string = lambda name: S.Node("string", name, [S.Node("Array", "Array",
                           [S.Node("int", "size"), S.Node("char", "data")], meta=0x4000)])
    script = S.Node("MonoScript", "Base", [string("m_AssemblyName"), string("m_Namespace"),
                                           string("m_ClassName")])
    data = S.serialized_file([S.Type(114, head), S.Type(115, script)],
                            [S.Obj(1, 0, struct.pack("<iqi", 0, 2, 42)),
                             S.Obj(2, 1, S.s32("Assembly") + S.s32("Synthetic") + S.s32("Example"))])
    env = UnityPy.Environment()
    sf = env.load_file(data, name="synthetic.assets")
    obj = sf.objects[1]
    mb_node = sf.types[0].node
    calls = []
    generated = TypeTreeNode.from_list(base)
    env.typetree_generator = SimpleNamespace(get_nodes_up=lambda a, f: calls.append([a, f]) or generated)
    mono = {"head": outcome(lambda: obj.parse_monobehaviour_head(mb_node).m_Script.m_PathID),
            "generated": outcome(lambda: rows(obj.generate_monobehaviour_node(mb_node))), "calls": calls}
    old_env = UnityPy.Environment()
    old_sf = old_env.load_file(S.serialized_file([S.Type(114,head),S.Type(115,script)],
        [S.Obj(1,114,struct.pack("<iqi",0,2,42)), S.Obj(2,115,S.s32("Assembly") + S.s32("Synthetic") + S.s32("Example"))],
        version=6),name="old.assets")
    old_env.typetree_generator = SimpleNamespace(get_nodes_up=lambda a,f: generated)
    mono["old_explicit"] = outcome(lambda: rows(old_sf.objects[1].generate_monobehaviour_node(mb_node)))
    env.typetree_generator = None
    mono["missing"] = outcome(lambda: obj.generate_monobehaviour_node(mb_node))
    mono["automatic"] = []
    # A synthetic fallback, supplied by a shim rather than a bundled TPK.
    obj.serialized_type.node = None
    class CustomError(Exception):
        pass

    class CustomValueError(ValueError):
        pass

    exceptions = {e.__name__: e for e in (ValueError, KeyError, RuntimeError, IndexError,
        EOFError, TypeError, AttributeError, AssertionError, NotImplementedError,
        OverflowError, MemoryError, FileNotFoundError, struct.error, CustomError, CustomValueError)}
    with patch("UnityPy.files.ObjectReader.get_typetree_node", return_value=mb_node):
        for kind in ("none", "miss", "UnicodeDecodeError", *exceptions, "hit"):
            def get_nodes(a, f):
                if kind == "miss":
                    return None
                if kind == "hit":
                    return generated
                if kind == "UnicodeDecodeError":
                    raise UnicodeDecodeError("utf-8", b"\xff", 0, 1, "synthetic")
                raise exceptions[kind]("synthetic")
            env.typetree_generator = None if kind == "none" else SimpleNamespace(get_nodes_up=get_nodes)
            mono["automatic"].append({"kind": kind, **outcome(lambda: obj.parse_as_dict(check_read=False)),
                "explicit": outcome(lambda: rows(obj.generate_monobehaviour_node(mb_node)))})
    mono["miss_messages"] = []
    for assembly, namespace, name in (("程序集'\\", "名字", '例"\\'), ("Assembly", "", '例"\\')):
        identity = SimpleNamespace(m_AssemblyName=assembly, m_Namespace=namespace, m_ClassName=name)
        ptr = SimpleNamespace(deref_parse_as_object=lambda: identity)
        env.typetree_generator = SimpleNamespace(get_nodes_up=lambda a, f: None)
        with patch.object(type(obj), "parse_monobehaviour_head", return_value=SimpleNamespace(m_Script=ptr)):
            mono["miss_messages"].append({"assembly": assembly, "namespace": namespace, "class": name,
                **outcome(lambda: obj.generate_monobehaviour_node(mb_node))})
    # Exercise the actual wrapper without installing or pretending to run a native generator.
    module = importlib.import_module("UnityPy.helpers.TypeTreeGenerator")
    wrapper = module.TypeTreeGenerator.__new__(module.TypeTreeGenerator)
    wrapper.cache = {}
    asks = []
    generator_source = [TypeTreeNode(n.m_Level,n.m_Type,n.m_Name,99,5,m_MetaFlag=n.m_MetaFlag,m_TypeFlags=7)
                        for n in generated.traverse()]
    wrapper.get_nodes = lambda a, f: asks.append([a, f]) or generator_source
    normalized = rows(wrapper.get_nodes_up("Assembly", "Synthetic.Example"))
    first = wrapper.get_nodes_up("Assembly", "Synthetic.Example")
    assert first is wrapper.get_nodes_up("Assembly", "Synthetic.Example")
    wrapper.get_nodes_up("Assembly.dll", "Synthetic.Example")
    return {"unitypy": "1.25.2", "from_list": cases, "relinked": relinked, "nested_relinks": nested_relinks,
            "jumped_relinked": jumped_relinked,
            "shared_children": shared_children,
            "node_reads": node_reads,
            "fallback": fallback, "mono": mono, "catch_scope": catch_scope(),
            "reader_exceptions": reader_exception_matrix(),
            "malformed_readers": malformed_reader_matrix(),
            "generator": {"rows": normalized, "calls": asks,
                "input": [[getattr(n,key) for key in FIELDS] for n in generator_source]}}


def image():
    with scratch.folder(prefix="unityrs-k1-") as tmp:
        path = Path(tmp)
        env = dict(os.environ, UNITYRS_K1_IMAGE="1", UNITYRS_K1_SCRATCH=str(path))
        child.run(["cargo", "test", "-p", "unityrs-tpk", "--test", "providers", "--", "--nocapture"],
                  cwd=ROOT, env=env, check=True)


def compare_image(path):
    """Both provider answers exist only in this process/Rust test's memory."""
    env = UnityPy.Environment()
    sf = env.load_file(path.read_bytes(), name="stripped.assets")
    assert not sf._enable_type_tree
    assert all(t.node is None for t in sf.types)
    result = {"rows": {str(c): rows(Tpk.get_typetree_node(c, sf.version)) for c in (49, 114, 115)},
              "values": {str(i): obj.parse_as_dict() for i, obj in sf.objects.items()}}
    print(json.dumps(result))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    parser.add_argument("--image", action="store_true")
    parser.add_argument("--compare-image", type=Path)
    parser.add_argument("--constructor-case")
    args = parser.parse_args()
    if args.constructor_case is not None:
        value = json.loads(args.constructor_case)
        result = outcome(lambda: snapshot(TypeTreeNode.from_list([constructor_dict(n) for n in value])))
        print(json.dumps(result), flush=True)
        os._exit(0)
    if args.compare_image:
        compare_image(args.compare_image)
        return
    text = json.dumps(api(), separators=(",", ":")) + "\n"
    if args.check:
        if OUT.read_text() != text:
            raise SystemExit("providers/api.json is stale")
        image()
    elif args.image:
        image()
    else:
        OUT.parent.mkdir(parents=True, exist_ok=True)
        OUT.write_text(text)


if __name__ == "__main__":
    main()
