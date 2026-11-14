"""Record the scene accessor portion of the P2a mesh-export fixtures.

Run in the pinned oracle container (UnityPy 1.25.2):
    python tools/fixtures/mesh_export.py
    python tools/fixtures/mesh_export.py --check

Only synthetic class instances and real PPtrs are used; no file, TPK tree,
or game data is read. Collision cases use hand-built trees read through
UnityPyBoost object mode, so raw names are cleaned by the real oracle.
Rust rebuilds the input dicts on hand-built trees.
Tier B answers come directly from the component dict, beside the exception
the unmodified oracle raises. Further exporters can extend this recorder.
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from types import SimpleNamespace

import UnityPy
from UnityPy.classes import GameObject, MeshFilter, MeshRenderer, PPtr, SkinnedMeshRenderer
from UnityPy.enums import ClassIDType
from UnityPy.export.MeshRendererExporter import get_mesh
from UnityPy.helpers import TypeTreeHelper
from UnityPy.streams import EndianBinaryReader, EndianBinaryWriter

from meshdecode_build import leaf, to_node, vec
from meshdecode_trees import node

OUT = Path(__file__).resolve().parents[2] / "crates/unityrs-scene/tests/fixtures/accessors.json"
ACCESSORS = {"m_Transform": 4, "m_Animator": 95, "m_Animation": 111,
             "m_MeshRenderer": 23, "m_SkinnedMeshRenderer": 137, "m_MeshFilter": 33}
INPUTS = [
    ("all_first_match", "modern", [[4, 40], [95, 95], [111, 111], [23, 23], [137, 137], [33, 33], [4, 41]]),
    ("rect_is_not_transform", "modern", [[224, 224], [33, 33]]),
    ("empty", "empty", []),
    ("none", "none", []),
    ("legacy_pairs", "legacy", [[4, 40], [33, 33], [33, 34]]),
]
MESH_INPUTS = [
    {"case": "skinned", "skin": True, "mesh": 9, "filters": []},
    {"case": "skinned_null", "skin": True, "mesh": 0, "filters": []},
    {"case": "no_filter", "skin": False, "mesh": 0, "filters": []},
    {"case": "filter", "skin": False, "mesh": 0, "filters": [9]},
    {"case": "null_then_mesh", "skin": False, "mesh": 0, "filters": [0, 9]},
    {"case": "null_filter", "skin": False, "mesh": 0, "filters": [0]},
    {"case": "legacy_filter", "skin": False, "mesh": 0, "filters": [9], "legacy": True},
]


def instance(cls, **fields):
    value = cls.__new__(cls)
    for name, item in fields.items():
        object.__setattr__(value, name, item)
    return value


def pointer(file, path_id):
    return PPtr(m_FileID=0, m_PathID=path_id, assetsfile=file)


def reader(path_id, class_id, value):
    return SimpleNamespace(path_id=path_id, type=ClassIDType(class_id), parse_as_object=lambda: value)


def outcome(call):
    try:
        return {"value": call()}
    except (IndexError, AttributeError, ValueError, KeyError, FileNotFoundError) as error:
        return {"error": type(error).__name__}


def accessor_case(name, form, components):
    file = SimpleNamespace(objects={pid: reader(pid, cls, None) for cls, pid in components})
    ptrs = [pointer(file, pid) for _, pid in components]
    items = list(zip([cls for cls, _ in components], ptrs)) if form == "legacy" else [
        SimpleNamespace(component=p) for p in ptrs]
    go = instance(GameObject, m_Component=None if form == "none" else items)
    oracle = {"components": outcome(lambda: [p.m_PathID for p in go.m_Components])}
    for accessor in ACCESSORS:
        def read(name=accessor):
            p = getattr(go, name)
            return p.m_PathID if p is not None else None
        oracle[accessor] = outcome(read)
    answer = {"components": [pid for _, pid in components]}
    answer.update({name: next((pid for cls, pid in components if cls == cid), None)
                   for name, cid in ACCESSORS.items()})
    return {"case": name, "form": form, "input": components, "oracle": oracle, "answer": answer}


def mesh_case(case):
    file = SimpleNamespace(objects={})
    mesh = SimpleNamespace(object_reader=SimpleNamespace(path_id=9))
    file.objects[9] = reader(9, 43, mesh)
    components = [SimpleNamespace(component=pointer(file, 0))]
    for i, mesh_id in enumerate(case["filters"], 20):
        file.objects[i] = reader(i, 33, instance(MeshFilter, m_Mesh=pointer(file, mesh_id)))
        components.append((33, pointer(file, i)) if case.get("legacy") else
                          SimpleNamespace(component=pointer(file, i)))
    go = instance(GameObject, m_Component=components)
    file.objects[1] = reader(1, 1, go)
    renderer = instance(SkinnedMeshRenderer, m_Mesh=pointer(file, case["mesh"])) if case["skin"] else instance(
        MeshRenderer, m_GameObject=pointer(file, 1))

    def result():
        found = get_mesh(renderer)
        return found.object_reader.path_id if found is not None else None
    return {**case, "oracle": outcome(result)}


def collision_cases():
    """P2a PR 2: last cleaned attribute wins; graph facts keep raw names.

    Keep these separate from the existing replay cases until accessors use
    core objmode. The recorder must require Boost for this contract.
    """
    def ptr(name, target):
        return node(f"PPtr<{target}>", name, leaf("int", "m_FileID"), leaf("SInt64", "m_PathID"))

    def components(name):
        return vec(name, node("ComponentPair", "data", ptr("component", "Component")))

    def p(path_id):
        return {"m_FileID": 0, "m_PathID": path_id}

    def parse(rows, value, file):
        root = to_node(rows)
        writer = EndianBinaryWriter(endian="<")
        TypeTreeHelper.write_typetree(value, root, writer)
        data = writer.bytes
        obj = TypeTreeHelper.read_typetree(
            root, EndianBinaryReader(data, "<"), as_dict=False, byte_size=len(data), assetsfile=file)
        return obj, {"rows": rows, "data": data.hex()}

    cases = []
    for component_collision in (False, True):
        file = SimpleNamespace(objects={})
        for mesh_id in (9, 10):
            mesh = SimpleNamespace(object_reader=SimpleNamespace(path_id=mesh_id))
            file.objects[mesh_id] = reader(mesh_id, 43, mesh)
        if component_collision:
            for filter_id, mesh_id in ((20, 9), (21, 10)):
                file.objects[filter_id] = reader(
                    filter_id, 33, instance(MeshFilter, m_Mesh=pointer(file, mesh_id)))
            rows = node("GameObject", "Base", components("m_Component"), components("m.Component"),
                        leaf("bool", "m_IsActive"), leaf("int", "m_Layer"),
                        leaf("string", "m_Name"), leaf("int", "m_Tag"))
            go, source = parse(rows, {
                "m_Component": [{"component": p(20)}], "m.Component": [{"component": p(21)}],
                "m_IsActive": True, "m_Layer": 0, "m_Name": "collision", "m_Tag": 0,
            }, file)
            oracle = {"components": outcome(lambda: [v.m_PathID for v in go.m_Components]),
                      "m_MeshFilter": outcome(lambda: go.m_MeshFilter.m_PathID)}
            case = "component_list_clean_name_collision"
        else:
            rows = node("MeshFilter", "Base", ptr("m_GameObject", "GameObject"),
                        ptr("m_Mesh", "Mesh"), ptr("m.Mesh", "Mesh"))
            mesh_filter, source = parse(rows, {
                "m_GameObject": p(1), "m_Mesh": p(9), "m.Mesh": p(10),
            }, file)
            file.objects[20] = reader(20, 33, mesh_filter)
            go = instance(GameObject, m_Component=[SimpleNamespace(component=pointer(file, 20))])
            oracle = {"m_Mesh": outcome(lambda: mesh_filter.m_Mesh.m_PathID)}
            case = "mesh_filter_clean_name_collision"
        file.objects[1] = reader(1, 1, go)
        renderer = instance(MeshRenderer, m_GameObject=pointer(file, 1))
        oracle["get_mesh"] = outcome(lambda: get_mesh(renderer).object_reader.path_id)
        cases.append({"case": case, "input": source, "oracle": oracle})
    return cases


def fixture():
    return {"reference": "UnityPy 1.25.2", "cases": [accessor_case(*c) for c in INPUTS],
            "mesh_cases": [mesh_case(c) for c in MESH_INPUTS], "collision_cases": collision_cases()}


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    if UnityPy.__version__ != "1.25.2" or TypeTreeHelper.read_typetree_boost is None:
        print("requires UnityPy 1.25.2 with UnityPyBoost", file=sys.stderr)
        return 2
    body = json.dumps(fixture(), indent=2, ensure_ascii=False) + "\n"
    if args.check:
        if not OUT.exists() or OUT.read_text() != body:
            print("stale fixture: accessors.json", file=sys.stderr)
            return 1
    else:
        OUT.parent.mkdir(parents=True, exist_ok=True)
        OUT.write_text(body)
    return 0


if __name__ == "__main__":
    sys.exit(main())
