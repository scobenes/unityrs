"""Independent Y3 enum/table and UnityVersion observations for either package.

The recorder supplies UnityPy; replay supplies unityrs. No oracle data, native
extension, or third-party dependency is imported by this observation module.
"""

import importlib
import operator
from enum import Enum, EnumMeta, IntFlag


ENUM_MODULES = (
    "Audio", "BuildTarget", "BundleFile", "ClassIDType", "CommonString",
    "ExtendableEnum", "FileType", "GfxPrimitiveType", "GraphicsFormat",
    "MeshTopology", "PassType", "SerializedPropertyType", "ShaderCompilerPlatform",
    "ShaderGpuProgramType", "SpriteMeshType", "SpritePackingMode",
    "SpritePackingRotation", "TextureDimension", "TextureFormat", "VertexFormat",
)


def _typed(value):
    if isinstance(value, Enum):
        return [type(value).__name__, value.name, value.value]
    if isinstance(value, (list, tuple)):
        return [type(value).__name__, [_typed(item) for item in value]]
    if isinstance(value, dict):
        return ["dict", [[_typed(key), _typed(item)] for key, item in value.items()]]
    if isinstance(value, bytes):
        return ["bytes", value.hex()]
    return [type(value).__name__, value]


def _capture(call):
    try:
        return {"value": _typed(call())}
    except Exception as error:
        return {"error": type(error).__name__, "message": str(error)}


def _enum_record(cls):
    """Record every name, including aliases, and Python constructor behavior."""
    # IntFlag accepts a float equal to a previously cached integer pseudo-member,
    # but rejects that float before the integer has been materialized. Loading
    # files may have populated only the oracle's flag cache. Give both packages
    # the same prerequisites before the 0.0/1.0 constructor observations.
    if issubclass(cls, IntFlag):
        cls(0)
        cls(1)
    result = {
        "bases": [base.__name__ for base in cls.__bases__],
        "members": [],
    }
    canonical = {}
    iteration = []
    for name, member in cls.__members__.items():
        value = member.value
        first = canonical.setdefault(value, name)
        assert member.name == first
        assert repr(member) == f"<{cls.__name__}.{first}: {value}>"
        assert str(member) == str(value)
        assert isinstance(member, int) and int(member) == value and member == value
        assert hash(member) == hash(value) and bool(member) == bool(value)
        assert cls[name] is member and cls(value) is member
        if first == name and (not issubclass(cls, IntFlag) or value > 0 and value & (value - 1) == 0):
            iteration.append(name)
        result["members"].append([name, value])
    assert [member.name for member in cls] == iteration
    assert len(cls) == len(iteration)
    before = list(cls.__members__)
    result["constructors"] = [
        [_typed(value), _capture(lambda value=value: cls(value))]
        for value in (-123456789, 123456789, 1.0, 0.0, True, False, "0", None, [], b"0")
    ]
    try:
        unknown = cls(123456789)
    except (ValueError, TypeError):
        pass  # Constructor errors remain in the oracle observations above.
    else:
        assert cls(123456789) is unknown
        assert hash(unknown) == hash(123456789)
        assert int(unknown) == 123456789 and bool(unknown)
    result["unknown_repr"] = _capture(lambda: repr(cls(123456789)))
    result["unknown_str"] = _capture(lambda: str(cls(123456789)))
    result["unknown_name_lookup"] = _capture(lambda: cls["Unknown (123456789)"])
    assert list(cls.__members__) == before
    assert all(member.value != 123456789 for member in cls)
    result["missing_name"] = _capture(lambda: cls["does_not_exist"])
    if cls.__members__:
        first = next(iter(cls))
        last = next(reversed(cls.__members__.values()))
        result["operators"] = [
            _capture(lambda: first | last), _capture(lambda: first & last),
            _capture(lambda: first ^ last), _capture(lambda: ~first),
            _capture(lambda: first + 1), _capture(lambda: first < last),
        ]
    return result


def observe_enums(module):
    """All 20 handwritten submodules, package reexports, members and maps."""
    package = importlib.import_module(module.__name__ + ".enums")
    result = {"all": list(package.__all__), "modules": {}}
    for name in ENUM_MODULES:
        submodule = importlib.import_module(package.__name__ + "." + name)
        row = {"classes": {}, "tables": {}, "public": []}
        for key, value in vars(submodule).items():
            if key.startswith("_"):
                continue
            if isinstance(value, EnumMeta):
                row["public"].append([key, "enum", value.__name__])
                if value.__module__ == submodule.__name__:
                    row["classes"][key] = _enum_record(value)
            elif isinstance(value, dict):
                row["public"].append([key, "table"])
                row["tables"][key] = _typed(value)
        result["modules"][name] = row
    result["exports"] = []
    for name in package.__all__:
        value = getattr(package, name)
        if isinstance(value, EnumMeta):
            defining = importlib.import_module(value.__module__)
            result["exports"].append([name, "enum", getattr(defining, name) is value])
        else:
            result["exports"].append([name, _typed(value)])

    base = importlib.import_module(package.__name__ + ".ExtendableEnum").ExtendableEnum

    class Example(base):
        Zero = 0
        One = 1
        Alias = 1

    result["extendable_subclass"] = _enum_record(Example)
    return result


VERSION_STRINGS = (
    "0.0.0", "1.2.3", "5.6.7f1", "5.6.7q9post", "6.0.1", "2018.4.36",
    "2019.4.15f1", "2019.4.15F1", "2019.4.15q1", "2019.4.15Q1-tail",
    "2019.4.15alpha12", "2019.4.15f", "2019.4.15u1", "2019.4.15U1",
    "2019.4.15f1\npostfix", "2019.4.15\nf2", "2019.4.15.1",
    "2019.4.15a0", "2019.4.15b1", "2019.4.15c2", "2019.4.15p3", "2019.4.15x4",
    "65536.65537.65538f256", "2020.3.34f1024", "2020.3.34f65536",
    "２０２０.٣.३४f١", "0005.0006.0007F01", "2019.4.15f999999999999999999999",
    "", "2017.4", "abc", " 2019.4.15f1", "2019.4", "-1.2.3", "1.2.-3",
    "2019.4.15f-1", "2019.4.15f1-extra", "2019.4.15f1\x00post",
)


def _version_record(value):
    names = ("major", "minor", "build", "type", "type_str", "type_number", "postfix")
    assert isinstance(value, int) and hash(value) == hash(int(value))
    result = {"int": int(value)}
    result["fields"] = {name: _capture(lambda name=name: getattr(value, name)) for name in names}
    result["str"] = _capture(lambda: str(value))
    result["repr"] = _capture(lambda: repr(value))
    result["as_tuple"] = _capture(value.as_tuple)
    result["index"] = [
        [_typed(index), _capture(lambda index=index: value[index])]
        for index in (0, 1, 2, 3, 4, -1, -5, 5, -6, True, "0", None)
    ]
    result["slices"] = [
        [repr(index), _capture(lambda index=index: value[index])]
        for index in (slice(None), slice(1, 4), slice(None, None, -1),
                      slice(-4, -1), slice(None, None, 2), slice(20), slice(None, None, 0))
    ]
    return result


def observe_versions(module):
    """Version parsing, bit spill, properties, indexing and rich comparisons."""
    helper = importlib.import_module(module.__name__ + ".helpers.UnityVersion")
    cls = helper.UnityVersion
    result = {
        "all": list(helper.__all__),
        "type": _enum_record(helper.UnityVersionType),
        "pattern": [helper.VersionPattern.pattern, helper.VersionPattern.flags],
        "strings": [], "lists": [], "raw": [],
    }
    for text in VERSION_STRINGS:
        try:
            value = cls.from_str(text)
        except Exception as error:
            result["strings"].append([text, {"error": type(error).__name__, "message": str(error)}])
        else:
            result["strings"].append([text, _version_record(value)])
    for args in ((), (1,), (5, 6, 7), (2019, 4, 15, 3, 1), (65536, 65537, 65538, 3, 256),
                 (-1, 2, 3, 3, 1), (2019, -1, 3), (2019, 4, -1), (2019, 4, 15, 6, 1),
                 (2019, 4, 15, 255, 255), (2019, 4, 15, 3, 1024), (True, False, 1),
                 (1.0,), ("1",), (None,), (1, 2, 3, "f", 1), (1, 2, 3, 4, 5, 6)):
        try:
            value = cls.from_list(*args)
        except Exception as error:
            row = {"error": type(error).__name__, "message": str(error)}
        else:
            row = _version_record(value)
        result["lists"].append([_typed(args), row])
    for raw in (-1, 0, 1, 1536, 2**64 - 1, 2**80 + 1234):
        result["raw"].append([raw, _version_record(cls(raw))])
    result["invalid_input"] = [
        [_typed(raw), _capture(lambda raw=raw: cls.from_str(raw))]
        for raw in (None, 123, b"2019.4.15f1", [2019, 4, 15])
    ]
    value = cls.from_str("2019.4.15f1")
    operations = (operator.eq, operator.ne, operator.lt, operator.le, operator.gt, operator.ge)
    operands = (int(value) - 1, int(value), int(value) + 1, cls.from_str("2019.4.15f2"),
                (), (2019,), (2019, 4, 15), value.as_tuple(), (2019, 4, 15, 3, 2),
                (2019, 4, 15, 3, 1, 0), (2019, "bad"), None, 1.0, "2019", b"2019", [], True)
    result["comparisons"] = []
    for other in operands:
        row = [_typed(other), {}]
        for operation in operations:
            row[1][operation.__name__] = _capture(lambda operation=operation: operation(value, other))
            row[1]["reverse_" + operation.__name__] = _capture(lambda operation=operation: operation(other, value))
        result["comparisons"].append(row)
    result["container_operations"] = {
        "none_eq": _capture(lambda: None == value),
        "in_none_list": _capture(lambda: value in [None]),
        "int_dict_key": _capture(lambda: {value: "value"}[int(value)]),
        "int_set": _capture(lambda: len({value, int(value)})),
        "tuple_set": _capture(lambda: len({value, value.as_tuple()})),
    }
    value._type_str = "custom"
    value._postfix = "+suffix"
    result["mutable_metadata"] = _version_record(value)
    return result
