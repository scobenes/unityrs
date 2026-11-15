# Ported from UnityPy git 417d595 (MIT): UnityPy/tools/TpkClassGenerator.py. See LICENSE-UNITYPY and THIRD_PARTY_LICENSES.
"""Generates the classes for the UnityPy objects from the TypeTree of the TPK files."""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Dict, List, Optional, Set, Tuple

from .._class_tpk import load, source
from ._names import clean_name
from ._generation_limits import Budget, refuse

TPK = load(source())
NODES = TPK.NodeBuffer
STRINGS = TPK.StringBuffer

BASE_TYPE_MAP = {
    "char": "int",  # used for byte data
    "short": "int",
    "int": "int",
    "long long": "int",
    "unsigned short": "int",
    "unsigned int": "int",
    "unsigned long long": "int",
    "UInt8": "int",
    "UInt16": "int",
    "UInt32": "int",
    "UInt64": "int",
    "SInt8": "int",
    "SInt16": "int",
    "SInt32": "int",
    "SInt64": "int",
    "Type*": "int",
    "FileSize": "int",
    "float": "float",
    "double": "float",
    "bool": "bool",
    "string": "str",
    "TypelessData": "bytes",
}

GENERATED_HEADER = """
# type: ignore
from __future__ import annotations

from abc import ABC
from typing import List, Optional, Tuple, TypeVar, Union

from attrs import define as attrs_define

from .math import (
    ColorRGBA,
    Matrix3x4f,
    Matrix4x4f,
    Quaternionf,
    Vector2f,
    Vector3f,
    Vector4f,
    float3,
    float4,
)
from .Object import Object
from .PPtr import PPtr

T = TypeVar("T")


def unitypy_define(cls: T) -> T:
    \"\"\"
    A hacky solution to bypass multiple problems related to attrs and inheritance.

    The class inheritance is very lax and based on the typetrees.
    Some of the child classes might not have the same attributes as the parent class,
    which would make type-hinting more tricky, and breaks attrs.define.

    Therefore this function bypasses the issue
    by redefining the bases for problematic classes for the attrs.define call.
    \"\"\"
    bases = cls.__bases__
    if bases[0] in (object, Object, ABC):
        cls = attrs_define(cls, slots=True, unsafe_hash=True)
    else:
        cls.__bases__ = (Object,)
        cls = attrs_define(cls, slots=False, unsafe_hash=True)
        cls.__bases__ = bases
    return cls
"""[1:]

MATH_CLASSES = {
    "ColorRGBA",
    "Matrix3x4f",
    "Matrix4x4f",
    "Quaternionf",
    "Vector2f",
    "Vector3f",
    "Vector4f",
    "float3",
    "float4",
}

FORBIDDEN_CLASSES = {"bool", "float", "int", "void"} | MATH_CLASSES

CLASS_CACHE_ID: Dict[Tuple[int, str], NodeClass] = {}
CLASS_CACHE_NAME: Dict[str, NodeClass] = {}
TYPE_CACHE: Dict[int, str] = {}
BUDGET = Budget()


@dataclass
class NodeClassField:
    ids: Set[int]
    name: str
    types: Set[str] = field(default_factory=set)
    optional: bool = True

    def source_size(self) -> int:
        # Include union/optional wrappers and the largest normalization growth.
        return len(self.name) + sum(len(t) + 2 for t in self.types) + 60

    def generate_str(self, charged: bool = False) -> str:
        if not charged:
            BUDGET.source_charge(self.source_size())
        if len(self.types) == 1:
            typ = next(iter(self.types))
        else:
            BUDGET.annotation_size(sum(len(t) + 2 for t in self.types) + 7)
            typ = f"Union[{', '.join(sorted(self.types))}]"

        if self.optional:
            BUDGET.annotation_size(len(typ) + 10)
            return f"    {self.clean_name}: Optional[{typ}] = None"
        else:
            return f"    {self.clean_name}: {typ}"

    @property
    def clean_name(self) -> str:
        BUDGET.name(self.name, 'field')
        return clean_name(self.name)


@dataclass
class NodeClass:
    ids: Set[int]
    name: str
    aliases: Set[str] = field(default_factory=set)
    fields: Dict[str, NodeClassField] = field(default_factory=dict)
    field_ids: Set[int] = field(default_factory=set)
    key_fields: Set[str] = field(default_factory=set)
    abstract: bool = False
    base: Optional[str] = None

    @staticmethod
    def sort_fields(field: NodeClassField) -> Tuple[bool, str]:
        return (field.optional, field.clean_name)

    def generate_str(self) -> str:
        # order fields by 1. non-optional>optional, 2. name
        BUDGET.source_charge(len(self.name) + len(self.base or '') + 40 +
                             sum(f.source_size() for f in self.fields.values()))
        parents: List[str] = []

        if self.base:
            parents.append(self.base)

        if self.abstract:
            parents.append("ABC")

        parentsString = f"({', '.join(parents)})" if parents else ""

        if len(self.fields) == 0:
            field_strings = ["    pass"]
        else:
            field_strings = map(
                lambda f: f.generate_str(charged=True),
                sorted(self.fields.values(), key=NodeClass.sort_fields),
            )
        return "\n".join(
            [
                "@unitypy_define",
                f"class {self.name}{parentsString}:",
                *field_strings,
            ]
        )


def implement_node_class(
    node_id: int,
    node: Optional[object] = None,
    override_name: Optional[str] = None,
) -> NodeClass:
    BUDGET.visit()
    if node is None:
        node = NODES[node_id]
    raw_name = BUDGET.name(STRINGS[node.TypeName], 'type')
    cls_name = BUDGET.name(override_name, 'class') if override_name else raw_name

    cls = CLASS_CACHE_ID.get((node_id, cls_name))
    if cls is not None:
        return cls

    cls = CLASS_CACHE_NAME.get(cls_name)
    first_impl = False
    if cls is None:
        BUDGET.class_record()
        first_impl = True
        cls = NodeClass(ids={node_id}, name=cls_name)
        CLASS_CACHE_NAME[cls_name] = cls
    else:
        cls.ids.add(node_id)

    BUDGET.cache()
    CLASS_CACHE_ID[(node_id, cls_name)] = cls
    if override_name and override_name != raw_name:
        cls.aliases.add(raw_name)

    BUDGET.visit(len(node.SubNodes))
    field_names: Set[str] = set()
    for subnode_id in node.SubNodes:
        subnode = NODES[subnode_id]
        subname = BUDGET.name(STRINGS[subnode.Name], 'field')

        field_names.add(subname)

        if subnode_id in cls.field_ids:
            continue

        field = cls.fields.get(subname)
        if field is None:
            BUDGET.field()
            field = NodeClassField({subnode_id}, subname)
            cls.fields[subname] = field
        else:
            field.ids.add(subnode_id)

        field_type = generate_field_type(subnode_id, subnode)
        BUDGET.string_charge(len(field_type))
        field.types.add(field_type)

    cls.field_ids |= set(node.SubNodes)

    if first_impl:
        cls.key_fields = set(cls.fields.keys())
        for field in cls.fields.values():
            field.optional = False
    else:
        deprecated_field_names = cls.key_fields - field_names
        cls.key_fields -= deprecated_field_names
        for deprecated_name in deprecated_field_names:
            cls.fields[deprecated_name].optional = True

    return cls


def generate_field_type(node_id: int, node: Optional[object] = None) -> str:
    if BUDGET.depth >= 64:
        refuse('annotation depth')
    BUDGET.depth += 1
    try:
        return _field_type(node_id, node)
    finally:
        BUDGET.depth -= 1


def _field_type(node_id: int, node: Optional[object] = None) -> str:
    BUDGET.visit()
    res = TYPE_CACHE.get(node_id)
    if res is not None:
        return res

    if node is None:
        node = NODES[node_id]

    typename = BUDGET.name(STRINGS[node.TypeName], 'type')

    py_typ = BASE_TYPE_MAP.get(typename)
    if py_typ:
        res = py_typ

    elif typename == "pair":
        # Children:
        #   Typ1 first
        #   Typ2 second
        typ1 = generate_field_type(node.SubNodes[0])
        typ2 = generate_field_type(node.SubNodes[1])
        res = BUDGET.annotation('Tuple[', typ1, ', ', typ2, ']')

    elif typename.startswith("PPtr<"):
        BUDGET.annotation(typename)
        res = typename.replace("<", "[").replace(">", "]")

    else:
        # map & vector
        subnode0 = NODES[node.SubNodes[0]] if len(node.SubNodes) > 0 else None
        if subnode0 and BUDGET.name(STRINGS[subnode0.TypeName], 'type') == "Array":
            # Children:
            #   Array Array
            #       SInt32 size
            #       Typ data
            subtype_node = NODES[subnode0.SubNodes[1]]
            subtype_name = BUDGET.name(STRINGS[subtype_node.TypeName], 'type')
            if subtype_name in BASE_TYPE_MAP:
                res = BUDGET.annotation('List[', BASE_TYPE_MAP[subtype_name], ']')
            else:
                subtype = generate_field_type(subnode0.SubNodes[1], subtype_node)
                res = BUDGET.annotation('List[', subtype, ']')
        else:
            # custom class
            implement_node_class(node_id, node)
            res = BUDGET.annotation(typename)

    BUDGET.cache()
    TYPE_CACHE[node_id] = res
    return res


def generate_classes():
    BUDGET.source_charge(len(GENERATED_HEADER))
    main_classes: Set[str] = set()
    deps: Dict[str, List[str]] = {}

    for _class_id, class_info in TPK.ClassInformation.items():
        BUDGET.visit(1 + len(class_info))
        abstract = True
        base = None
        cls_name: Optional[str] = None

        for _version, unity_class in class_info:
            if unity_class is None:
                continue
            cls_name = BUDGET.name(STRINGS[unity_class.Name], 'class')
            base = BUDGET.name(STRINGS[unity_class.Base], 'base')

            if unity_class.ReleaseRootNode is not None:
                abstract = False
                cls = implement_node_class(unity_class.ReleaseRootNode, override_name=cls_name)
                cls.base = base

        if isinstance(cls_name, str):
            if abstract:
                BUDGET.class_record()
                CLASS_CACHE_NAME[cls_name] = NodeClass({0}, name=cls_name, base=base, abstract=True)

            main_classes.add(cls_name)
            if base:
                if base in deps:
                    deps[base].append(cls_name)
                else:
                    deps[base] = [cls_name]

    CLASS_CACHE_NAME.pop("Object")
    sorted_classes: List[str] = []

    stack = sorted(deps.pop("Object"), reverse=True)
    while stack:
        BUDGET.visit()
        cls_name = stack.pop()
        sorted_classes.append(cls_name)
        if cls_name in deps:
            stack.extend(sorted(deps.pop(cls_name), reverse=True))

    sorted_classes += sorted(set(CLASS_CACHE_NAME.keys()) - set(sorted_classes) - FORBIDDEN_CLASSES)
    names = set()
    unique_classes = []
    for name in sorted_classes:
        if name not in names:
            names.add(name)
            unique_classes.append(name)

    return GENERATED_HEADER + "\n\n" + "\n\n\n".join(
        CLASS_CACHE_NAME[name].generate_str() for name in unique_classes
    ) + "\n"
