"""Print PARITY.md section 9.2.1: what object mode builds for every name `UnityPy.classes` holds, in the oracle image.

    scripts/dev python tools/parity/object_mode.py

Object mode looks a class node's `m_Type` up on the module `UnityPy.classes` with `getattr`
(`PyObject_GetAttr`, `UnityPyBoost/TypeTreeHelper.cpp:488`), so every name `getattr` finds there is a type name a
file can hold: what the module's `__dict__` holds, which `dir(module)` lists, and what the module's type holds
(`__class__`, `__repr__`, ...), which `dir` leaves out. For each class of the module, this parses a tree of that type holding
every attrs field the class has and one more (`m_Extra`) on both paths: UnityPyBoost's (`byte_size` given) and the
pure-Python one (no `byte_size`). It records what each path returns, beside the `__slots__` value UnityPyBoost's
`parse_class` reads (`UnityPyBoost/TypeTreeHelper.cpp:508-515`) and whether the class's instances have a
`__dict__`. For every other name, it parses a class node of that type, holding no field and
holding one, nested in a holder so that UnityPyBoost reads it; each runs in a process of its own, because
UnityPyBoost crashes on most of them.

`tests/test_object_mode.py` requires PARITY.md to hold exactly what this prints.
"""

import functools
import inspect
import json
import signal
import struct
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor

EXTRA = "m_Extra"
HOLDER = "ObjectModeHolder"  # a type UnityPy.classes lacks

# Run in a process of its own: a class node of type argv[1] holding argv[2] int fields, nested in HOLDER.
CHILD = f"""
import json, struct, sys
from UnityPy.helpers import TypeTreeHelper
from UnityPy.helpers.TypeTreeNode import TypeTreeNode
from UnityPy.streams import EndianBinaryReader
name, count = sys.argv[1], int(sys.argv[2])
nodes = [
    {{"m_Level": 0, "m_Type": "{HOLDER}", "m_Name": "Base", "m_ByteSize": -1, "m_Version": 1}},
    {{"m_Level": 1, "m_Type": "int", "m_Name": "pad", "m_ByteSize": 4, "m_Version": 1}},
    {{"m_Level": 1, "m_Type": name, "m_Name": "inner", "m_ByteSize": -1, "m_Version": 1}},
]
nodes += [{{"m_Level": 2, "m_Type": "int", "m_Name": f"f{{i}}", "m_ByteSize": 4, "m_Version": 1}} for i in range(count)]
data = struct.pack(f"<{{count + 1}}i", *range(count + 1))
try:
    holder = TypeTreeHelper.read_typetree(
        TypeTreeNode.from_list(nodes), EndianBinaryReader(data, "<"), False, len(data), check_read=True
    )
except Exception as error:
    print(json.dumps(["raises", type(error).__name__, str(error)]))
else:
    value = holder.inner
    node_type = value.get_type() if type(value).__name__ == "UnknownObject" else None
    print(json.dumps(["returns", type(value).__name__, node_type]))
"""


def tree(root_type, fields):
    from UnityPy.helpers.TypeTreeNode import TypeTreeNode

    nodes = [{"m_Level": 0, "m_Type": root_type, "m_Name": "Base", "m_ByteSize": -1, "m_Version": 1}]
    nodes += [{"m_Level": 1, "m_Type": "int", "m_Name": name, "m_ByteSize": 4, "m_Version": 1} for name in fields]
    return TypeTreeNode.from_list(nodes)


def parse(root_type, fields, boost):
    """`read_typetree` of a `root_type` node holding int `fields` 1, 2, ...; on UnityPyBoost or on the Python path."""
    from UnityPy.helpers import TypeTreeHelper
    from UnityPy.streams import EndianBinaryReader

    data = struct.pack(f"<{len(fields)}i", *range(1, len(fields) + 1))
    size = len(data) if boost else None  # no byte_size: the pure-Python path
    reader = EndianBinaryReader(data, "<")
    return TypeTreeHelper.read_typetree(tree(root_type, fields), reader, False, size, check_read=boost)


@functools.cache
def module_dir():
    """`dir(UnityPy.classes)` just after its import, read in a process of its own: looking `__annotations__` up on
    a module stores it in the module's `__dict__`, so in this process `dir` would depend on what ran before."""
    code = "import json, UnityPy.classes as m; print(json.dumps(dir(m)))"
    run = subprocess.run([sys.executable, "-B", "-c", code], capture_output=True, text=True, check=True, timeout=120)
    return tuple(json.loads(run.stdout))


def names():
    """Every name `getattr` finds on `UnityPy.classes`, split into the module's classes and the rest.

    `getattr` on a module finds what the module's `__dict__` holds, which `dir(module)` lists, and what its type's
    MRO holds, which `dir(type(module))` lists; `UnityPy.classes` defines no module `__getattr__`. A class found
    on the module's type (`__class__`, the module type itself) is not a class of the module: it goes with the rest.
    """
    import UnityPy.classes as module

    assert "__getattr__" not in vars(module)
    own = set(module_dir())
    every = sorted(name for name in own | set(dir(type(module))) if hasattr(module, name))
    classes = [name for name in every if name in own and inspect.isclass(getattr(module, name))]
    return classes, [name for name in every if name not in classes]


def type_names():
    """The names of `names()`'s rest that `dir(UnityPy.classes)` leaves out: attributes of the module's type."""
    return [name for name in names()[1] if name not in module_dir()]


def fields_of(clz):
    import attrs

    fields = [field.name for field in attrs.fields(clz)] if attrs.has(clz) else []
    assert EXTRA not in fields, clz
    return [*fields, EXTRA]


def outcome(name, fields, boost):
    """What one path returns for a `name` node holding every field of the class and `m_Extra`."""
    from UnityPy import classes

    clz = getattr(classes, name)
    try:
        value = parse(name, fields, boost)
    except Exception as error:  # noqa: BLE001 - the exception is the result
        return f"raises `{type(error).__name__}`"
    if type(value) is classes.UnknownObject and value.get_type() == name and clz is not classes.UnknownObject:
        assert value.__node__ is not None and getattr(value, EXTRA) == len(fields), name
        return "`UnknownObject`"
    assert type(value) is clz and getattr(value, EXTRA) == len(fields), (name, value)
    return "the class, `m_Extra` an attribute"


def slots_of(clz):
    """`__slots__` as `parse_class` reads it: `getattr` through the MRO; and whether instances have a `__dict__`."""
    slots = getattr(clz, "__slots__", None)
    assert isinstance(slots, tuple), clz  # every class of the module has one, through ABC at least
    return "a non-empty tuple" if slots else "`()`", "yes" if clz.__dictoffset__ else "no"


def class_rows():
    from UnityPy import classes

    groups = {}
    for name in names()[0]:
        clz = getattr(classes, name)
        fields = fields_of(clz)
        key = (*slots_of(clz), outcome(name, fields, True), outcome(name, fields, False))
        groups.setdefault(key, []).append(name)
    return groups


def child_outcome(name, count):
    run = subprocess.run(
        [sys.executable, "-B", "-c", CHILD, name, str(count)], capture_output=True, text=True, timeout=120
    )
    if run.returncode < 0:
        return f"the process dies of `{signal.Signals(-run.returncode).name}`"
    assert run.returncode == 0, (name, count, run.stderr[-400:])
    kind, type_name, detail = json.loads(run.stdout)
    if kind == "raises":
        return f'raises `{type_name}` "{detail}"'
    if type_name == "UnknownObject":
        assert detail == name, (name, detail)
        return "`UnknownObject` with the node"
    return f"returns {'an' if type_name[0] in 'AEIOUaeiou' else 'a'} `{type_name}`"


def other_rows():
    others = names()[1]
    jobs = [(name, count) for name in others for count in (0, 1)]
    with ThreadPoolExecutor(max_workers=4) as pool:
        results = dict(zip(jobs, pool.map(lambda job: child_outcome(*job), jobs), strict=True))
    groups = {}
    for name in others:
        groups.setdefault((results[name, 0], results[name, 1]), []).append(name)
    return groups


def spans(items):
    return ", ".join(f"`{item}`" for item in items)


def table():
    classes, others = names()
    lines = [
        "| `__slots__`, through the MRO | instances have a `__dict__` | classes | UnityPyBoost | pure-Python path |",
        "|---|---|---|---|---|",
    ]
    groups = class_rows()
    for key in sorted(groups, key=lambda k: (-len(groups[k]), k)):
        slots, has_dict, boost, python = key
        members = groups[key]
        lines.append(f"| {slots} | {has_dict} | {len(members)}: {spans(members)} | {boost} | {python} |")
    lines += [
        "",
        "| other names `getattr` finds on `UnityPy.classes` | no field | one field |",
        "|---|---|---|",
    ]
    groups = other_rows()
    for key in sorted(groups, key=lambda k: (-len(groups[k]), k)):
        lines.append(f"| {len(groups[key])}: {spans(groups[key])} | {key[0]} | {key[1]} |")
    import UnityPy.classes as module

    hidden = type_names()
    found = [name for name in hidden if inspect.isclass(getattr(module, name))]
    assert found == ["__class__"], found  # the module type
    lines += [
        "",
        f"{len(classes)} classes and {len(others)} other names: {len(others) - len(hidden)} attributes "
        f"`dir(UnityPy.classes)` lists, and {len(hidden)} attributes of the module's type that it leaves out, "
        f"of which {spans(found)}, the module type, is a class.",
    ]
    return "\n".join(lines)


def main():
    print(table())


if __name__ == "__main__":
    main()
