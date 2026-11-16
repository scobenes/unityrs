"""Object writer review regressions using synthetic assets only."""

import struct
import threading

from binding_cases import S
from binding_y4_save import _load


def observe_reentrant_writer(module):
    return _reentrant_writer(module, check_thread=True)


def _reentrant_writer(module, check_thread):
    tree = S.Node("Writer", "Base", [S.Node("int", "value")])
    file = _load(module, S.serialized_file([S.Type(49, tree)],
                                         [S.Obj(1, 0, struct.pack("<i", 7))]))
    obj = file.objects[1]
    caller = threading.get_ident()
    reads = []

    class Value:
        @property
        def value(self):
            # Refuse the old worker before reentry would block on its caller's
            # RLock. This makes the regression fail promptly on old wheels.
            if check_thread and threading.get_ident() != caller:
                raise AssertionError("writer getter ran outside the calling thread")
            reads.append(obj.parse_as_dict()["value"])
            return reads[-1] + 4

    saved = obj.save_typetree(Value())
    reloaded = _load(module, file.save()).objects[1].parse_as_dict()
    return {"reads": reads, "bytes": saved.hex(), "saved": reloaded}


class _Index:
    def __index__(self):
        return 3


class _Float:
    def __float__(self):
        return 1.25


class _Truth:
    def __bool__(self):
        return False


def observe_numeric_writer(module):
    kinds = [
        ("SInt8", "b"), ("UInt8", "B"), ("char", "B"),
        ("short", "h"), ("SInt16", "h"),
        ("unsigned short", "H"), ("UInt16", "H"),
        ("int", "i"), ("SInt32", "i"), ("unsigned int", "I"),
        ("UInt32", "I"), ("Type*", "I"),
        ("long long", "q"), ("SInt64", "q"),
        ("unsigned long long", "Q"), ("UInt64", "Q"), ("FileSize", "Q"),
        ("float", "f"), ("double", "d"), ("bool", "?"),
    ]
    common = [
        ("fraction", 1.2), ("integral_float", 1.0), ("huge_positive", 2**65),
        ("huge_negative", -(2**65)), ("enormous", 2**1100),
        ("negative", -1), ("zero", 0), ("true", True), ("false", False),
        ("none", None), ("string", "2"), ("empty_list", []),
        ("index_protocol", _Index()), ("float_protocol", _Float()),
        ("truth_protocol", _Truth()), ("float_overflow", 1e100),
        ("infinity", float("inf")), ("nan", float("nan")),
        ("negative_zero", -0.0),
    ]
    result = {}
    for kind, fmt in kinds:
        tree = S.Node("Numbers", "Base", [S.Node(kind, "value")])
        file = _load(module, S.serialized_file([S.Type(49, tree)],
                                             [S.Obj(1, 0, struct.pack("<" + fmt, 0))]))
        cases = list(common)
        if fmt in "bBhHiIqQ":
            bits = struct.calcsize(fmt) * 8
            low, high = (-(2**(bits - 1)), 2**(bits - 1) - 1) if fmt.islower() else (0, 2**bits - 1)
            cases += [("minimum", low), ("maximum", high),
                      ("underflow", low - 1), ("overflow", high + 1)]
        elif fmt in "fd":
            maximum = float.fromhex("0x1.fffffep127" if fmt == "f" else "0x1.fffffffffffffp1023")
            minimum = float.fromhex("0x1p-149" if fmt == "f" else "0x1p-1074")
            cases += [("maximum", maximum), ("minimum", -maximum),
                      ("subnormal", minimum), ("negative_subnormal", -minimum),
                      ("underflow", minimum / 2), ("negative_overflow", -1e100)]
        outcomes = {}
        for name, value in cases:
            try:
                outcomes[name] = {"bytes": file.objects[1].save_typetree({"value": value}).hex()}
            except Exception as error:
                outcomes[name] = {"error": type(error).__name__, "message": str(error)}
        result[kind] = outcomes
    return result
