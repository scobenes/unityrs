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
    return Blob(kind, o["len"], o["sha256"])


def _unpack(o, fmt, size, cls):
    raw = bytes.fromhex(o["v"])
    if not raw or len(raw) % size:
        raise FormatError(f"bad packed {o['t']} length {len(raw)}")
    return [cls(x) for x in struct.unpack(f">{len(raw) // size}{fmt}", raw)]


def decode(tv):
    """Decode one tagged value into the comparison model."""
    t = type(tv)
    if t is list:
        return [decode(v) for v in tv]
    if t is not dict:
        if t is float:
            raise FormatError("untagged JSON float in a value")
        return tv
    tag = tv.get("t")
    try:
        if tag == "dict":
            return Dict((decode_key(k), decode(v)) for k, v in tv["v"])
        if tag == "f32":
            return F32(int(tv["v"], 16))
        if tag == "f64":
            return F64(int(tv["v"], 16))
        if tag == "f32s":
            return _unpack(tv, "I", 4, F32)
        if tag == "f64s":
            return _unpack(tv, "Q", 8, F64)
        if tag == "tuple":
            return Tuple(decode(v) for v in tv["v"])
        if tag in ("bytes", "str", "u8s"):
            return _blob(tv, tag)
    except (KeyError, TypeError, ValueError) as e:
        raise FormatError(f"malformed {tag!r} value: {e}") from e
    raise FormatError(f"unknown value tag {tag!r}")


def decode_key(k):
    if type(k) is str:
        return k
    return decode(k)
