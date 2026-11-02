"""How compare.py diffs two records with one key: field by field, type-exact, with paths (docs/CANONICAL_DUMP.md
section 6). compare.py names everything public here too; the tools and tests reach it there.

A value nests as deep as UnityPyBoost reads (PARITY.md 19.3, "value depth"), so nothing here recurses: each
walk keeps its own stack, and a record of any depth the dumps hold is compared, never a RecursionError.
"""

from __future__ import annotations

import json

import canonical as C

MAX_DIFFS_PER_RECORD = 20
MAX_ELEMENT_DIFFS = 5


class Diff:
    __slots__ = ("level", "cls", "path", "kind", "a", "b", "where")

    def __init__(self, level, cls, path, kind, a=None, b=None, where=""):
        self.level, self.cls, self.path, self.kind = level, cls, path, kind
        self.a, self.b, self.where = a, b, where

    def as_dict(self):
        return {
            "level": self.level,
            "class": self.cls,
            "path": self.path,
            "kind": self.kind,
            "where": self.where,
            "a": short(self.a),
            "b": short(self.b),
        }


def short(v, limit=160):
    """A value's repr (a str's JSON), cut to `limit` characters; past Python's recursion limit, the start of it.
    compare.run quotes an integer of any width through it, as it runs with the digit limit lifted."""
    if v is None:
        return None
    if isinstance(v, str):
        text = json.dumps(v, ensure_ascii=False)
    else:
        try:
            text = repr(v)
        except RecursionError:
            text = _deep_repr(v, limit + 1)
    return text if len(text) <= limit else text[: limit - 3] + "..."


def _deep_repr(v, limit):
    """`repr(v)`'s first `limit` characters or so, written with a stack."""
    out, size = [], 0
    todo = [v]
    while todo and size < limit:
        o = todo.pop()
        if type(o) is _Raw:
            text = o.text
        elif type(o) is dict:
            text = "{"
            todo.append(_Raw("}"))
            items = list(o.items())
            for i in range(len(items) - 1, -1, -1):
                todo.append(items[i][1])
                todo.append(_Raw(repr(items[i][0]) + ": "))
                if i:
                    todo.append(_Raw(", "))
        elif isinstance(o, (list, tuple)):
            text = "(" if isinstance(o, tuple) else "["
            todo.append(_Raw(")" if isinstance(o, tuple) else "]"))
            for i in range(len(o) - 1, -1, -1):
                todo.append(o[i])
                if i:
                    todo.append(_Raw(", "))
        else:
            text = repr(o)
        out.append(text)
        size += len(text)
    return "".join(out)


class _Raw:
    __slots__ = ("text",)

    def __init__(self, text):
        self.text = text


def where(rec):
    parts = [p if isinstance(p, str) else json.dumps(p) for p in rec.get("file", [])]
    w = ":".join(parts)
    if rec["rec"] == "object":
        w += f":path_id={rec['path_id']}"
    return w or rec["rec"]


def record_class(rec):
    if rec["rec"] == "object":
        l2 = rec.get("l2") if isinstance(rec.get("l2"), dict) else {}
        rt = l2.get("root_type")
        if isinstance(rt, str):
            return rt
        l1 = rec.get("l1") if isinstance(rec.get("l1"), dict) else {}
        class_id = l1.get("class_id")
        return f"class_id={class_id if type(class_id) is int else short(class_id)}"
    if rec["rec"] == "file":
        l0 = rec.get("l0") if isinstance(rec.get("l0"), dict) else {}
        parsed = l0.get("parsed_as")
        return "file:" + (parsed if isinstance(parsed, str) else str(short(parsed)))
    return rec["rec"]


# ------------------------------------------------------------------- diffing


def strip_notes(v):
    """`v` without any member named "note", at any depth (a list subclass becomes a list)."""
    root = [None]
    todo = [(v, root, 0)]
    while todo:
        x, slot, at = todo.pop()
        if isinstance(x, dict):
            out = {}
            for k, y in x.items():
                if k != "note":
                    out[k] = None
                    todo.append((y, out, k))
        elif isinstance(x, list):
            out = [None] * len(x)
            todo.extend((y, out, i) for i, y in enumerate(x))
        else:
            out = x
        slot[at] = out
    return root[0]


def same(a, b):
    """Type-exact equality at any depth. Python's == is not: True == 1 == 1.0, and F32(0) == F64(0)."""
    todo = [(a, b)]
    while todo:
        a, b = todo.pop()
        t = type(a)
        if t is not type(b):
            return False
        if t is dict:
            if list(a) != list(b):
                return False
            todo.extend((a[k], b[k]) for k in a)
        elif t is list or t is tuple or t is C.Dict or t is C.Tuple:
            if len(a) != len(b):
                return False
            todo.extend(zip(a, b))
        elif a != b:
            return False
    return True


class _Emit:
    """A difference to append when the walk reaches it, so the order is the recursive walk's."""

    __slots__ = ("item",)

    def __init__(self, item):
        self.item = item


def _walk(a, b, path, out, step):
    """Run `step(a, b, path)`, which returns the tasks under a pair in order, over a stack, appending each
    _Emit to `out` when reached: the depth-first order a recursion would give."""
    todo = [(a, b, path)]
    while todo:
        task = todo.pop()
        if type(task) is _Emit:
            out.append(task.item)
            continue
        todo.extend(reversed(step(*task)))


def _elements(a, b, path, more):
    """The tasks of two lists' elements: the length, then each differing pair, at most MAX_ELEMENT_DIFFS."""
    tasks = []
    if len(a) != len(b):
        tasks.append(_Emit((path, "length", len(a), len(b))))
    shown = 0
    for i, (x, y) in enumerate(zip(a, b, strict=False)):
        if not same(x, y):
            if shown == MAX_ELEMENT_DIFFS:
                tasks.append(_Emit((f"{path}[...]", "more", None, None)))
                break
            shown += 1
            tasks.append(more(x, y, f"{path}[{i}]"))
    return tasks


def json_diff(a, b, path, out):
    """Plain JSON structures (metadata): exact, with paths."""

    def step(a, b, path):
        if type(a) is not type(b):
            return [_Emit((path, "type", a, b))]
        if isinstance(a, dict):
            tasks = []
            for k in [*a, *(k for k in b if k not in a)]:
                if k not in b:
                    tasks.append(_Emit((f"{path}.{k}", "missing", a[k], None)))
                elif k not in a:
                    tasks.append(_Emit((f"{path}.{k}", "extra", None, b[k])))
                else:
                    tasks.append((a[k], b[k], f"{path}.{k}"))
            if list(a) != list(b) and a.keys() == b.keys():
                tasks.append(_Emit((path, "key_order", list(a), list(b))))
            return tasks
        if isinstance(a, list):
            return _elements(a, b, path, lambda x, y, p: (x, y, p))
        return [_Emit((path, "value", a, b))] if a != b else []

    _walk(a, b, path, out, step)


def _key_text(k):
    return k if isinstance(k, str) else short(k)


def _key_id(k):
    """A dict key as a type-exact dict key: "a" and a hex-tagged key never collide, nor 1 and True."""
    return (type(k), k)


def value_diff(a, b, path, out):
    """Decoded typetree values (see canonical.decode): type-exact at any depth, with paths."""

    def step(a, b, path):
        ta, tb = type(a), type(b)
        if ta is not tb:
            return [_Emit((path, "type", a, b))]
        if ta is C.Dict:
            ka, kb = [_key_id(k) for k, _ in a], [_key_id(k) for k, _ in b]
            da, db = {_key_id(k): v for k, v in a}, {_key_id(k): v for k, v in b}
            tasks = []
            if ka != kb:
                if set(ka) == set(kb):
                    tasks.append(_Emit((path, "key_order", [k for _, k in ka], [k for _, k in kb])))
                for k in ka:
                    if k not in db:
                        tasks.append(_Emit((f"{path}.{_key_text(k[1])}", "missing_key", da[k], None)))
                for k in kb:
                    if k not in da:
                        tasks.append(_Emit((f"{path}.{_key_text(k[1])}", "extra_key", None, db[k])))
            for k in ka:
                if k in db:
                    tasks.append((da[k], db[k], f"{path}.{_key_text(k[1])}"))
            return tasks
        if ta is list or ta is C.Tuple:
            return _elements(a, b, path, lambda x, y, p: (x, y, p))
        if a == b:
            return []
        kind = "value"
        # CPython widens a float32 through the FPU, which quiets a signalling
        # NaN; UnityPy's dict cannot hold the file's bits (spec: "Floats").
        # The candidate must be that signalling NaN: a reference qNaN against
        # an infinity differs in the same bit, and is a real error.
        if (
            ta is C.F32
            and (a ^ b) == 0x00400000
            and (a & 0x7F800000) == 0x7F800000
            and a & 0x00400000
            and b & 0x007FFFFF
        ):
            kind = "f32_snan_quieted"
        return [_Emit((path, kind, a, b))]

    _walk(a, b, path, out, step)


def compare_section(level, cls, name, a, b, w, out, value=False):
    raw = []
    if value:
        if (a is None) != (b is None):
            raw.append((name, "type", a, b))
        elif a is not None:
            try:
                da, db = C.decode(a), C.decode(b)
            except C.FormatError as e:
                raw.append((name, "invalid", str(e), None))
            else:
                value_diff(da, db, name, raw)
    else:
        json_diff(strip_notes(a), strip_notes(b), name, raw)
    for path, kind, x, y in raw:
        out.append(Diff(level, cls, path, kind, x, y, w))


def _section(rec, name):
    v = rec.get(name)
    return v if isinstance(v, dict) else {}


def compare_records(ra, rb, levels, out):
    w, cls = where(ra), record_class(ra)
    diffs = []
    if ra["rec"] == "file":
        if "L0" in levels:
            compare_section("L0", cls, "l0", ra.get("l0"), rb.get("l0"), w, diffs)
        if "L1" in levels:
            compare_section("L1", cls, "l1", ra.get("l1"), rb.get("l1"), w, diffs)
        compare_section("L1", cls, "error", ra.get("error"), rb.get("error"), w, diffs)
    elif ra["rec"] == "object":
        if "L1" in levels:
            compare_section("L1", cls, "l1", ra.get("l1"), rb.get("l1"), w, diffs)
        if "L2" in levels:
            l2a, l2b = _section(ra, "l2"), _section(rb, "l2")
            meta_a = {k: v for k, v in l2a.items() if k != "value"}
            meta_b = {k: v for k, v in l2b.items() if k != "value"}
            compare_section("L2", cls, "l2", meta_a, meta_b, w, diffs)
            compare_section("L2", cls, "l2.value", l2a.get("value"), l2b.get("value"), w, diffs, value=True)
        if "L3" in levels:
            compare_section("L3", cls, "l3", ra.get("l3"), rb.get("l3"), w, diffs)
    elif ra["rec"] == "end":
        compare_section("format", cls, "end", ra, rb, w, diffs)
    if len(diffs) > MAX_DIFFS_PER_RECORD:
        extra = len(diffs) - MAX_DIFFS_PER_RECORD
        diffs = diffs[:MAX_DIFFS_PER_RECORD]
        diffs.append(Diff(diffs[0].level, cls, "(record)", "more", extra, None, w))
    out.extend(diffs)
    return bool(diffs)


_LEVEL_SECTION = {"L1": "l1", "L2": "l2", "L3": "l3"}


def compared_text(rec, levels):
    """The canonical text of what compare_records looks at, notes left out."""
    kind = rec["rec"]
    if kind == "file":
        parts = [rec.get("error")] + [rec.get(lv.lower()) for lv in ("L0", "L1") if lv in levels]
    elif kind == "object":
        parts = [rec.get(_LEVEL_SECTION[lv]) for lv in ("L1", "L2", "L3") if lv in levels]
    else:
        parts = [rec]
    return C.dumps(strip_notes(parts))
