"""The value decoder compare.py reads canonical values with (docs/CANONICAL_DUMP.md section 3).

canonical.py names everything public here too; the tools and tests reach it there, as `C.decode()`.
"""

from __future__ import annotations

import hashlib
import struct

# ------------------------------------------------------------- value decoder
#
# The comparer works on a decoded tree in which every canonical-form choice is
# undone: packed float lists become lists of F32/F64, and inline and summarised
# raw bytes both become Blob(len, sha256). Two encodings that decode equal hold
# the same value; a byte difference between them is a format violation only.


class F32(int):
    """A float32 by bit pattern."""

    __slots__ = ()

    def __repr__(self):
        return f"f32:{self:08x}"


class F64(int):
    """A float64 by bit pattern."""

    __slots__ = ()

    def __repr__(self):
        return f"f64:{self:016x}"


class Blob(tuple):
    """Raw bytes: kind 'bytes', 'u8s' (a UInt8 vector) or 'str' (not UTF-8): (kind, len, sha256, hex|None)."""

    __slots__ = ()

    def __new__(cls, kind, length, sha, hexdata=None):
        return tuple.__new__(cls, (kind, length, sha, hexdata))

    @property
    def kind(self):
        return self[0]

    def __eq__(self, other):
        return isinstance(other, Blob) and self[:3] == other[:3]

    def __ne__(self, other):
        return not self == other

    def __hash__(self):
        return hash(self[:3])

    def __repr__(self):
        if self[3] is not None:
            return f"{self[0]}:{self[3]}"
        return f"{self[0]}:len={self[1]},sha256={self[2][:16]}..."


class Tuple(list):
    """A Python tuple (a typetree pair)."""

    __slots__ = ()


class Dict(list):
    """An ordered dict as a list of (key, value) pairs."""

    __slots__ = ()


class FormatError(ValueError):
    """A dump line that does not follow the spec."""


def _blob(o, kind):
    if "hex" in o:
        raw = bytes.fromhex(o["hex"])
        return Blob(kind, len(raw), hashlib.sha256(raw).hexdigest(), o["hex"])
    if type(o["len"]) is not int or type(o["sha256"]) is not str:
        # A Blob is hashed as a dict key: its members must be what a summary holds.
        raise TypeError(f"a summary's len and sha256 are {type(o['len']).__name__}, {type(o['sha256']).__name__}")
    return Blob(kind, o["len"], o["sha256"])


def _unpack(o, fmt, size, cls):
    raw = bytes.fromhex(o["v"])
    if not raw or len(raw) % size:
        raise FormatError(f"bad packed {o['t']} length {len(raw)}")
    return [cls(x) for x in struct.unpack(f">{len(raw) // size}{fmt}", raw)]


def _leaf(tv):
    """A tagged value that holds no other value, decoded; None for a dict or tuple, which hold others."""
    tag = tv.get("t")
    try:
        if tag == "f32":
            return F32(int(tv["v"], 16))
        if tag == "f64":
            return F64(int(tv["v"], 16))
        if tag == "f32s":
            return _unpack(tv, "I", 4, F32)
        if tag == "f64s":
            return _unpack(tv, "Q", 8, F64)
        if tag in ("bytes", "str", "u8s"):
            return _blob(tv, tag)
    except (KeyError, TypeError, ValueError) as e:
        raise FormatError(f"malformed {tag!r} value: {e}") from e
    if tag in ("dict", "tuple"):
        return None
    raise FormatError(f"unknown value tag {tag!r}")


def decode(tv):
    """Decode one tagged value into the comparison model, at any depth (a stack, not recursion: a value nests
    as deep as UnityPyBoost reads)."""
    root = [None]
    todo = [(tv, root, 0)]
    while todo:
        tv, slot, at = todo.pop()
        if type(tv) is _Freeze:
            tv.done()
            continue
        t = type(tv)
        if t is list:
            out = [None] * len(tv)
            todo.extend((v, out, i) for i, v in enumerate(tv))
        elif t is not dict:
            if t is float:
                raise FormatError("untagged JSON float in a value")
            out = tv
        else:
            out = _leaf(tv)
            if out is None:
                tag = tv["t"]
                try:
                    items = list(tv["v"])
                    if tag == "dict":
                        entries = [(decode_key(k), v) for k, v in items]
                except (KeyError, TypeError, ValueError) as e:
                    raise FormatError(f"malformed {tag!r} value: {e}") from e
                if tag == "tuple":
                    out = Tuple([None] * len(items))
                    todo.extend((v, out, i) for i, v in enumerate(items))
                else:
                    # Each entry is a (key, value) pair in the comparison model, made once its value is in:
                    # the freeze sits below the values on the stack, so it runs after all of them.
                    out = Dict([None] * len(entries))
                    todo.append((_Freeze(out), None, 0))
                    for i, (k, v) in enumerate(entries):
                        pair = [k, None]
                        out[i] = pair
                        todo.append((v, pair, 1))
        slot[at] = out
    return root[0]


class _Freeze:
    """A decoded Dict whose [key, value] lists become (key, value) tuples once every value is in."""

    __slots__ = ("d",)

    def __init__(self, d):
        self.d = d

    def done(self):
        self.d[:] = [tuple(p) for p in self.d]


def decode_key(k):
    """A dict key: a str, or a name's hex form, which decodes to a Blob (anything else is no key: section 3)."""
    if type(k) is str:
        return k
    if type(k) is dict and k.get("t") == "str":
        return _leaf(k)
    raise FormatError(f"a dict key that is no name: {type(k).__name__}")
