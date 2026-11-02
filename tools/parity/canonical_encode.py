"""The canonical value encoder (docs/CANONICAL_DUMP.md section 3): UnityPy's `parse_as_dict()` result as a tagged value.

canonical.py names everything public here too; the tools reach it there, as `C.ValueEncoder` and `C.enc_name`.
Typetree nodes are duck-typed (``m_Type``, ``m_Name``, ``m_Children``, ``m_MetaFlag``), so the encoder works on
UnityPyBoost nodes and on plain test doubles alike.
"""

from __future__ import annotations

import hashlib
import struct
from typing import NamedTuple

from canonical_json import dumps

# Raw-byte payloads longer than this are summarised as length + sha256. Inline
# hex keeps small diffs readable; the summary keeps texture and vertex blobs
# from doubling the dump size, and still separates any two different payloads.
INLINE_MAX = 64

FLOAT_WIDTH = {"float": "f32", "double": "f64"}

# Element types whose vectors pack as raw bytes ("u8s"): Font data, shader
# blobs and index buffers are megabytes of them, and as JSON integers they
# would dominate the dump.
U8_TYPES = frozenset(("UInt8", "char"))

ALIGN = 0x4000  # the align flag of a node's m_MetaFlag (helpers/TypeTreeHelper.py kAlignBytes)
REGISTRY = "ManagedReferencesRegistry"
REF_OBJECT = "ReferencedObject"
REF_DATA = "ReferencedObjectData"


class EncodeError(ValueError):
    """A value the canonical form cannot represent faithfully.

    Raised for a value whose Python type does not fit its typetree node, which
    means the dumper and UnityPy disagree about the value model; it is a bug in
    one of them, never an encoding to paper over.
    """


# ------------------------------------------------------------ scalar encoders


def enc_raw(kind: str, raw: bytes):
    """Bytes, or a str that is not valid UTF-8, as its raw bytes."""
    if len(raw) <= INLINE_MAX:
        return {"t": kind, "hex": raw.hex()}
    return {"t": kind, "len": len(raw), "sha256": hashlib.sha256(raw).hexdigest()}


def _raw_str(s: str) -> bytes | None:
    """The raw bytes behind a surrogate-escaped str, or None if it is valid UTF-8."""
    if s.isascii():
        return None
    try:
        s.encode("utf-8")
        return None
    except UnicodeEncodeError:
        pass
    try:
        return s.encode("utf-8", "surrogateescape")
    except UnicodeEncodeError as e:
        # A lone surrogate outside U+DC80..U+DCFF cannot come from decoding bytes.
        raise EncodeError(f"str holds a surrogate that no byte decodes to: {s!r}") from e


def enc_str(s: str):
    """A typetree string value: plain JSON when valid UTF-8, else its raw bytes."""
    raw = _raw_str(s)
    return s if raw is None else enc_raw("str", raw)


def enc_name(s):
    """A dict key or metadata string: like enc_str, but never summarised."""
    if s is None:
        return None
    raw = _raw_str(s)
    return s if raw is None else {"t": "str", "hex": raw.hex()}


def enc_hex(b) -> str | None:
    """Fixed metadata bytes (hashes, GUIDs) as lowercase hex, or None."""
    return None if b is None else bytes(b).hex()


def _check_floats(values, kind: str):
    if set(map(type, values)) - {float}:
        raise EncodeError(f"{kind} field holds a non-float: {values[:4]!r}")


def f32_hex(values) -> str:
    """Concatenated big-endian float32 bit patterns of already-widened floats.

    struct's 'f' narrows exactly for any value UnityPy widened from float32,
    NaN payloads included (the widening left the low 29 mantissa bits zero).
    The round trip below proves exactness, comparing bit patterns so NaNs and
    -0.0 count.
    """
    n = len(values)
    _check_floats(values, "f32")
    try:
        packed = struct.pack(f">{n}f", *values)
    except (OverflowError, struct.error) as e:
        raise EncodeError(f"f32 field holds a value outside float32: {e}") from e
    if struct.pack(f">{n}d", *values) != struct.pack(f">{n}d", *struct.unpack(f">{n}f", packed)):
        raise EncodeError("f32 field holds a value float32 cannot represent exactly")
    return packed.hex()


def u8_bytes(values) -> bytes:
    """A list of ints in 0..255 as bytes (bools are rejected, not coerced)."""
    if set(map(type, values)) - {int}:
        raise EncodeError(f"u8 vector holds a non-int: {values[:4]!r}")
    try:
        return bytes(values)
    except ValueError as e:
        raise EncodeError(f"u8 vector holds a value outside 0..255: {e}") from e


def f64_hex(values) -> str:
    _check_floats(values, "f64")
    return struct.pack(f">{len(values)}d", *values).hex()


# ------------------------------------------------------------- value encoder


def _element_node(node):
    """The element node of a vector node, or None: `children[0].children[1]`, children[0] being the `Array`.

    Both readers index it so (UnityPy's pure-Python reader reads an `Array` of three or more children through
    its second child; UnityPyBoost refuses any but two, so no value of its has one).
    """
    children = node.m_Children
    if children and children[0].m_Type == "Array" and len(children[0].m_Children) >= 2:
        return children[0].m_Children[1]
    return None


def _aligned(node) -> bool:
    return bool((getattr(node, "m_MetaFlag", 0) or 0) & ALIGN)


def _zero_length(v) -> bool:
    """Whether UnityPyBoost's `PyUnicode_GET_LENGTH` reads 0 from `v`, a ReferencedObject's `class` (spec 7).

    On Python 3.11 it reads the field after the object's header, whatever the object is: a str's length, an
    int's (a bool's) digit count, a float's bits, a bytes, list, tuple or dict's size.
    """
    t = type(v)
    if t is float:
        return struct.pack("<d", v) == bytes(8)
    if t is int or t is bool:
        return v == 0
    return len(v) == 0


class Ctx(NamedTuple):
    """How UnityPy read a value, which decides which child of its node typed each of its members.

    `boost`: UnityPyBoost read it (an object whose byte_size is above 0), else the pure-Python reader;
    `registry`: a ManagedReferencesRegistry was being read around it (section 3's scope), so the registries
    of a class it holds are skipped; `bare`: the pure-Python reader's array reader read it as an element of
    an aligned array (`read_value_array`), which checks no registry among a class's own children and reads
    a ReferencedObject's data however its type resolves.
    """

    boost: bool
    registry: bool
    bare: bool


_SKIP = object()  # a ReferencedObject's data the reader left out


class ValueEncoder:
    """Encodes UnityPy's ``parse_as_dict()`` result, guided by its typetree.

    The encoding follows the Python value; the node is consulted only where the
    value alone is ambiguous: the width of a float, and which child node types
    a dict entry, list element or tuple member. `ref_types` is the SerializedFile's
    `ref_types` (each with `m_ClassName`, `m_NameSpace`, `m_AssemblyName` and `node`),
    or None for a file that has none, from which a ReferencedObject's data node is
    resolved as each reader resolves it (`get_ref_type_node`).

    The value is walked with a stack, not by recursion, so it may nest as deep as UnityPyBoost reads.
    One encoder serves one SerializedFile: it caches what it works out per node id, and
    holding the node keeps that id from being reused.
    """

    def __init__(self, ref_types=None):
        self._ref_types = ref_types
        self._classes: dict[tuple, tuple[object, dict]] = {}
        self._memo: dict | None = None

    def encode(self, value, node, boost=True):
        """The tagged value of `value`, read with `node` as its root: by UnityPyBoost when `boost`, else by the
        pure-Python reader (an object whose byte_size is 0)."""
        self._memo = {}
        try:
            return self._run(value, node, Ctx(bool(boost), False, False))
        except RecursionError as e:
            # Only trying ref types nests (_candidates); a chain of such tries this deep is the encoder's limit.
            raise EncodeError("ReferencedObject data typed by trying ref types nests too deep") from e
        finally:
            self._memo = None

    def _run(self, value, node, ctx):
        root = [None]
        todo = [(value, node, ctx, root, 0)]
        while todo:
            v, n, c, slot, at = todo.pop()
            t = type(v)
            if t is int or t is bool or v is None:
                out = v
            elif t is str:
                out = enc_str(v)
            elif t is float:
                out = self._float(v, n)
            elif t is dict:
                out = {"t": "dict", "v": []}
                for key, item, typers in self._members(v, n, c):
                    entry = [enc_name(key), None]
                    out["v"].append(entry)
                    if len(typers) == 1:
                        todo.append((item, *typers[0], entry, 1))
                    else:
                        entry[1] = self._candidates(item, typers, n, key)
            elif t is list:
                elem = _element_node(n)
                if elem is None:
                    raise EncodeError(f"list under non-vector node {n.m_Type!r} {n.m_Name!r}")
                width = FLOAT_WIDTH.get(elem.m_Type) if v else None
                if width:
                    out = {"t": width + "s", "v": f32_hex(v) if width == "f32" else f64_hex(v)}
                elif v and elem.m_Type in U8_TYPES:
                    out = enc_raw("u8s", u8_bytes(v))
                else:
                    out = [None] * len(v)
                    ec = Ctx(c.boost, c.registry, not c.boost and _aligned(elem))
                    todo.extend((x, elem, ec, out, i) for i, x in enumerate(v))
            elif t is tuple:
                items = [None] * len(v)
                out = {"t": "tuple", "v": items}
                if n.m_Type == "pair" and len(n.m_Children) >= 2:
                    # Both readers read a pair's first two children (the pure-Python one ignores any more).
                    if len(v) != 2:
                        raise EncodeError(f"a {len(v)}-tuple under 'pair' {n.m_Name!r}")
                    pc = Ctx(c.boost, c.registry, False)
                    todo.append((v[0], n.m_Children[0], pc, items, 0))
                    todo.append((v[1], n.m_Children[1], pc, items, 1))
                else:
                    # UnityPy's pure-Python reader returns a tuple for an aligned primitive array.
                    elem = _element_node(n)
                    if elem is None:
                        raise EncodeError(f"tuple under {n.m_Type!r} {n.m_Name!r}")
                    todo.extend((x, elem, c, items, i) for i, x in enumerate(v))
            elif t is bytes or t is bytearray or t is memoryview:
                out = enc_raw("bytes", bytes(v))
            else:
                raise EncodeError(f"unexpected {t.__name__} under {n.m_Type!r} {n.m_Name!r}")
            slot[at] = out
        return root[0]

    def _float(self, value, node):
        width = FLOAT_WIDTH.get(node.m_Type)
        if width == "f32":
            return {"t": "f32", "v": f32_hex([value])}
        if width == "f64":
            return {"t": "f64", "v": f64_hex([value])}
        raise EncodeError(f"float under non-float node {node.m_Type!r} {node.m_Name!r}")

    # -------------------------------------------------- which child typed a member

    def _members(self, value, node, ctx):
        """(key, item, [(node, ctx)]) for each member of a dict value, in its order: the nodes the member may
        have been read with, one but where a ReferencedObject's lost `type` leaves several (_ref_typers)."""
        if node.m_Type == REF_OBJECT:
            for key, item in value.items():
                yield key, item, self._ref_typers(value, node, ctx, key)
            return
        typers = self._class_typers(node, ctx)
        for key, item in value.items():
            typer = typers.get(key)
            if typer is None:
                raise EncodeError(f"key {key!r} is not a child of {node.m_Type!r} {node.m_Name!r}")
            yield key, item, [typer]

    def _class_typers(self, node, ctx):
        """{name: (child, its ctx)} for a class: the last child of each name the reader read.

        A repeated name keeps the dict key's first position but the last value (PyDict_SetItem overwrites).
        Section 3's registry scope: the first ManagedReferencesRegistry child of a class read outside any
        registry is read, and every registry after it in the class, or anywhere below a registry being read,
        is skipped; the pure-Python array reader checks no registry among an element's own children.
        """
        checks = ctx.boost or not ctx.bare
        key = (id(node), ctx.boost, ctx.registry, checks)
        hit = self._classes.get(key)
        if hit is not None:
            return hit[1]
        registry = ctx.registry
        typers = {}
        for child in node.m_Children:
            if checks and child.m_Type == REGISTRY:
                if registry:
                    continue
                registry = True
            typers[child.m_Name] = (child, Ctx(ctx.boost, registry, False))
        self._classes[key] = (node, typers)
        return typers

    def _ref_typers(self, value, node, ctx, key):
        """The nodes that may have typed member `key` of a ReferencedObject's dict, each with its ctx.

        The reader reads every child in order, and a ReferencedObjectData child through the node its ref type
        resolves to from the dict's `type` at that moment, leaving the data out when that resolves to nothing.
        When no child named `type` comes at or after the data child, the dict's `type` is the one it resolved
        from, and the member's node is known. Otherwise (a hostile tree: a data child named `type`, or a later
        `type` child) the reader overwrote it, and every ref type's node is a candidate (_candidates).
        """
        children = node.m_Children
        child_ctx = Ctx(ctx.boost, ctx.registry, False)
        last_type = max((i for i, ch in enumerate(children) if ch.m_Name == "type"), default=-1)
        typers = []
        for i in range(len(children) - 1, -1, -1):
            child = children[i]
            if child.m_Name != key:
                continue
            if child.m_Type != REF_DATA:
                typers.append((child, child_ctx))
                break
            if last_type < i:
                found = self._resolve(value, ctx)
                if found is _SKIP:
                    continue
                typers.append((found, child_ctx))
                break
            typers.extend((n, child_ctx) for n in self._ref_nodes())
            if ctx.bare and not ctx.boost:
                break  # the pure-Python array reader never leaves data out
        if not typers:
            raise EncodeError(f"key {key!r} is not a child of {node.m_Type!r} {node.m_Name!r}")
        return list({(id(n), c): (n, c) for n, c in typers}.values())

    def _ref_nodes(self):
        if self._ref_types is None:
            raise EncodeError("ReferencedObject data in a file without ref_types")
        return [rt.node for rt in self._ref_types if rt.node is not None]

    def _resolve(self, value, ctx):
        """The node a ReferencedObject's data was read with, from its dict's `type`, or _SKIP (spec 3, 7).

        UnityPyBoost (TypeTreeHelper.cpp get_ref_type_node) leaves the data out when `class` has length 0 as
        `PyUnicode_GET_LENGTH` reads it (_zero_length), the pure-Python reader (helpers/TypeTreeHelper.py)
        when it equals "" (in an aligned array's element it reads on, and fails); both when the matching ref
        type has no tree. Anything a read that succeeded cannot have met is an EncodeError.
        """
        t = value.get("type")
        if type(t) is not dict or not {"class", "ns", "asm"} <= t.keys():
            raise EncodeError(f"cannot resolve ref type {t!r}")
        cls, ns, asm = t["class"], t["ns"], t["asm"]
        if self._ref_types is None or (not ctx.boost and not self._ref_types):
            raise EncodeError("ReferencedObject data in a file without ref_types")
        pure_array = ctx.bare and not ctx.boost
        if (_zero_length(cls) if ctx.boost else cls == "") and not pure_array:
            return _SKIP
        for rt in self._ref_types:
            names = (rt.m_ClassName, rt.m_NameSpace, rt.m_AssemblyName)
            if ctx.boost and not all(type(x) is str for x in (cls, ns, asm, *names)):
                continue  # PyUnicode_Compare raises on anything but str: no match
            if (cls, ns, asm) == names:
                if rt.node is None and not pure_array:
                    return _SKIP
                if rt.node is None:
                    break
                return rt.node
        raise EncodeError(f"no ref type for {t!r}")

    def _candidates(self, item, typers, node, key):
        """The one encoding every node that may have typed `item` gives, or an EncodeError when they differ."""
        texts, first, error = set(), None, None
        for n, c in typers:
            memo = (id(item), id(n), c)
            if memo not in self._memo:
                try:
                    self._memo[memo] = (self._run(item, n, c), None)
                except EncodeError as e:
                    self._memo[memo] = (None, e)
            out, e = self._memo[memo]
            if e is not None:
                error = error or e
                continue
            texts.add(dumps(out))
            first = out if first is None else first
        if len(texts) == 1:
            return first
        if not texts:
            raise error
        raise EncodeError(
            f"member {key!r} of {node.m_Type!r} {node.m_Name!r}: the `type` its data was resolved from is "
            f"overwritten, and {len(texts)} ref types encode it differently"
        )
