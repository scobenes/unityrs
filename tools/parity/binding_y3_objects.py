"""Embedded-tree ObjectReader helper observations, including vendored HEAD reads.

All input trees are built by this repository's MIT synthetic fixture builders.
No provider trees, generated classes, or TPK values enter these observations.
"""

import contextlib
import importlib
import io
import struct
import sys
from pathlib import Path

from binding_cases import S, typed

_FIELDS = ("m_Level", "m_Type", "m_Name", "m_ByteSize", "m_Version", "m_TypeFlags",
           "m_VariableCount", "m_Index", "m_MetaFlag", "m_RefTypeHash")


def _call(function):
    try:
        with contextlib.redirect_stdout(io.StringIO()):
            return {"value": typed(function())}
    except Exception as error:
        return {"error": type(error).__name__, "message": str(error)}


def _rows(node):
    return [[getattr(child, field) for field in _FIELDS] + [len(child.m_Children)]
            for child in node.traverse()]


def _load(module, data):
    with contextlib.redirect_stdout(io.StringIO()):
        return module.Environment().load_file(data, name="embedded.assets")


def observe_objects(module):
    node_class = importlib.import_module(module.__name__ + ".helpers.TypeTreeNode").TypeTreeNode
    tree = S.Node("Y3Embedded", "Base", [S.Node("string", "m_Name"), S.Node("string", "body")])
    payload = S.s32("name") + S.s32("body")
    data = S.serialized_file([S.Type(49, tree)], [S.Obj(1, 0, payload)])
    obj = _load(module, data).objects[1]
    node = obj._get_typetree_node()
    rows = [{field: getattr(child, field) for field in _FIELDS}
            for child in node.traverse()]
    fresh = [node_class(child.m_Level, child.m_Type, child.m_Name, child.m_ByteSize, child.m_Version)
             for child in node.traverse()]
    result = {
        "tree": _rows(node), "stable_identity": obj._get_typetree_node() is node,
        "dump": _call(obj.dump_typetree_structure),
        "dump_indent": _call(lambda: obj.dump_typetree_structure(indent="> ")),
        "explicit_node_identity": obj._get_typetree_node(node) is node,
        "explicit_node": _call(lambda: obj.parse_as_dict(node)),
        "explicit_dict_rows": _call(lambda: obj.parse_as_dict(rows)),
        "explicit_node_rows": _call(lambda: obj.parse_as_dict(fresh)),
        "explicit_dump": _call(lambda: obj.dump_typetree_structure(rows)),
    }
    for label, bad in [("tuple", ()), ("dict", {}), ("zero", 0),
                       ("rows_tuple", tuple(rows)), ("empty", [])]:
        result["bad_" + label] = _call(lambda bad=bad: obj.parse_as_dict(bad))
    # from_list appends children in place, including already-linked children.
    own = _load(module, data).objects[1]._get_typetree_node()
    result["duplicated_children_read"] = _call(lambda: obj.parse_as_dict(list(own.traverse())))
    result["duplicated_children_count"] = len(own.m_Children)
    mutable = _load(module, data).objects[1]
    mutable._get_typetree_node().m_Children[0].m_Name = "renamed"
    result["mutated_embedded_node"] = _call(mutable.parse_as_dict)
    shared_data = S.serialized_file([S.Type(49, tree)],
                                    [S.Obj(i, 0, payload) for i in (1, 2, 3)])
    shared = _load(module, shared_data).objects
    shared_node = shared[1]._get_typetree_node()
    result["shared_type_identity"] = shared_node is shared[2]._get_typetree_node()
    shared_node.m_Children[0].m_Name = "shared_name"
    result["shared_type_reads"] = [_call(shared[i].parse_as_dict) for i in (2, 3)]
    result["versions"] = {}
    for version in (5, 6, 7, 8, 9, 12, 15, 19, 22):
        file = _load(module, S.serialized_file([S.Type(49, tree)], [S.Obj(1, 49 if version < 16 else 0, payload)], version=version))
        reader = file.objects[1]
        result["versions"][str(version)] = {
            "version": _call(lambda: None if reader.version is None else [type(reader.version).__name__, int(reader.version), str(reader.version)]),
            "version2": _call(lambda: reader.version2),
            "platform": _call(lambda: None if reader.platform is None else [type(reader.platform).__name__, int(reader.platform), reader.platform.name]),
            "nodes": _call(lambda: _rows(reader._get_typetree_node())),
            "read": _call(reader.parse_as_dict),
        }
    return result


def observe_head(module):
    """Compare H1's embedded local-string trees to HEAD 417d595, never TPK data."""
    sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "fixtures"))
    from head_read import sf_cases, readers

    result = {}
    # readers patches UnityPy.files.SerializedFile, which ImportHelper's
    # environment dispatch resolves dynamically. Both sides retain detection.
    with readers() if module.__name__ == "UnityPy" else contextlib.nullcontext():
        for name, data in sf_cases():
            for stream in (False, True):
                key = name + ("-stream" if stream else "-memory")
                try:
                    with contextlib.redirect_stdout(io.StringIO()):
                        source = io.BytesIO(data) if stream else data
                        file = _load(module, source)
                    if not isinstance(getattr(file, "objects", None), dict):
                        result[key] = {"raw": file.bytes.hex()}
                        continue
                    def call(function):
                        answer = _call(function)
                        # H1 preserves the exception category; its read errors
                        # can carry different reader/check_read diagnostics.
                        if "error" in answer:
                            answer.pop("message")
                        return answer
                    result[key] = [
                        {"path_id": obj.path_id, "nodes": call(lambda obj=obj: _rows(obj._get_typetree_node())),
                         "dump": call(obj.dump_typetree_structure), "read": call(obj.parse_as_dict),
                         "version": str(obj.version), "format": obj.version2,
                         "platform": int(obj.platform)}
                        for obj in file.objects.values()
                    ]
                    if name in ("shared-<", "shared->", "nested-<", "nested->"):
                        obj = next(iter(file.objects.values()))
                        left, right = obj._get_typetree_node().m_Children
                        identity = [left.m_Children is right.m_Children,
                                    left.m_Children[0] is right.m_Children[0]]
                        left.m_Children[0].m_Name = "renamed"
                        renamed = call(obj.parse_as_dict)
                        # A list mutation must also propagate, independently
                        # of sharing the original node objects.
                        left.m_Children.append(left.m_Children[0])
                        result[key].append({"identity": identity, "renamed": renamed,
                                            "child_counts": [len(left.m_Children), len(right.m_Children)]})
                        if name == "shared-<":
                            result[key][-1]["cross_type"] = _head_cross_type(module)
                except Exception as error:
                    # H1 malformed-envelope messages are reader-specific; the
                    # existing H1 rows promise the exception category.
                    result[key] = {"error": type(error).__name__}
    return result


def _head_cross_type(module):
    sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "fixtures"))
    from head_read import handler, node, serialized, readers
    key = b"A" * 16
    root = [node(0, "Synthetic", "Base"), node(1, "Part", "part", 32)]
    part = [node(0, "Part", "Ignored"), node(1, "int", "value")]
    info = handler(root, [key])
    data = serialized(26, [info, info], [handler(part, key=key)],
                      data=[struct.pack("<i", 7), struct.pack("<i", 8)])
    with readers() if module.__name__ == "UnityPy" else contextlib.nullcontext():
        objects = list(_load(module, data).objects.values())
    first = objects[0]._get_typetree_node().m_Children[0]
    child = first.m_Children[0]
    child.m_Name = "across"
    replacement = []
    child.m_Children = replacement
    # Parse the other type before asking for its mutable Python tree.
    value = _call(objects[1].parse_as_dict)
    second = objects[1]._get_typetree_node().m_Children[0]
    return [value, first.m_Children is second.m_Children,
            child is second.m_Children[0], child.m_Children is replacement]
