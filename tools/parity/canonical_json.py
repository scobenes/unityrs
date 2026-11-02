"""The canonical dump's JSON layer (docs/CANONICAL_DUMP.md section 2): a record to its line, and a line back.

canonical.py names everything public here too; the tools reach it there, as `C.dumps()` and `C.loads()`.

Two limits of Python's own `json` would otherwise stop a tool on one record's value:
- **Integer width.** Python 3.11 refuses to convert an integer of more than 4,300 decimal digits to or from
  text (`sys.get_int_max_str_digits()`), and a packed engine version can be wider (section 4.2). Both
  directions lift that limit while they run (`exact_ints`), and set it back after, so UnityPy, which the
  oracle runs between them, meets Python's default.
- **Depth.** `json`'s C scanner and encoder recurse once a container and stop at Python's recursion limit,
  a few hundred levels of a dict value (each level is an object, its `v` array and a pair). A value nests
  as deep as UnityPyBoost reads (PARITY.md 19.3, "value depth"): 21,823 levels of a ReferencedObject's data
  on the oracle's 8 MiB stack (`tools/fixtures/walk.py --boost-reach`, as unityrs_dump::LIFTED_VALUE_DEPTH
  records it). Where `json` raises `RecursionError`, the same text is written or read again by a loop with
  no recursion (`_dumps_deep`, `_loads_deep`), whose output is `json`'s
  (tools/parity/tests/test_canonical_json.py holds them equal).

A third limit is memory: a line longer than `LONG_LINE` may be read with its equal small objects shared
(`sharing_hook`), so a value of millions of equal elements is not millions of objects.
"""

from __future__ import annotations

import contextlib
import json
import re
import sys
from json.decoder import JSONDecodeError, scanstring
from json.encoder import encode_basestring

__all__ = ["LONG_LINE", "SHARED_NODES", "dumps", "exact_ints", "loads", "sharing_hook"]


@contextlib.contextmanager
def exact_ints():
    """Python's limit on an integer's decimal digits lifted, for as long as the block runs."""
    before = sys.get_int_max_str_digits()
    sys.set_int_max_str_digits(0)
    try:
        yield
    finally:
        sys.set_int_max_str_digits(before)


def dumps(obj) -> str:
    """Serialise one record exactly as the spec's JSON rules require.

    ``ensure_ascii=False`` with compact separators is byte-for-byte what serde_json's default writer emits
    for the same tree, and no float ever reaches JSON (floats are always hex-tagged), so number formatting
    cannot drift between producers.
    """
    with exact_ints():
        try:
            return json.dumps(obj, ensure_ascii=False, separators=(",", ":"), allow_nan=False, check_circular=False)
        except RecursionError:
            return _dumps_deep(obj)


def _float_text(f: float) -> str:
    if f != f or f in (float("inf"), float("-inf")):
        raise ValueError("Out of range float values are not JSON compliant: " + repr(f))
    return float.__repr__(f)


def _key_text(k) -> str:
    """A dict key as `json.dumps` writes it: a str as itself, a number, a bool or None as its JSON text."""
    if isinstance(k, str):
        return k
    if k is True:
        return "true"
    if k is False:
        return "false"
    if k is None:
        return "null"
    if isinstance(k, int):
        return int.__repr__(k)
    if isinstance(k, float):
        return _float_text(k)
    raise TypeError(f"keys must be str, int, float, bool or None, not {type(k).__name__}")


def _dumps_deep(obj) -> str:
    """`json.dumps(obj)` with `dumps`'s options, by a loop: the same text at any depth."""
    out = []
    # Each item is a value to write, or the text that closes a container or separates its members.
    todo = [obj]
    while todo:
        o = todo.pop()
        if type(o) is _Text:
            out.append(o.text)
        elif o is None:
            out.append("null")
        elif o is True:
            out.append("true")
        elif o is False:
            out.append("false")
        elif isinstance(o, str):
            out.append(encode_basestring(o))
        elif isinstance(o, int):
            out.append(int.__repr__(o))
        elif isinstance(o, float):
            out.append(_float_text(o))
        elif isinstance(o, (list, tuple)):
            out.append("[")
            todo.append(_CLOSE_LIST)
            for i in range(len(o) - 1, -1, -1):
                todo.append(o[i])
                if i:
                    todo.append(_COMMA)
        elif isinstance(o, dict):
            out.append("{")
            todo.append(_CLOSE_DICT)
            items = list(o.items())
            for i in range(len(items) - 1, -1, -1):
                k, v = items[i]
                todo.append(v)
                todo.append(_Text(encode_basestring(_key_text(k)) + ":"))
                if i:
                    todo.append(_COMMA)
        else:
            raise TypeError(f"Object of type {type(o).__name__} is not JSON serializable")
    return "".join(out)


class _Text:
    """Text `_dumps_deep` writes as it is."""

    __slots__ = ("text",)

    def __init__(self, text):
        self.text = text


_COMMA, _CLOSE_LIST, _CLOSE_DICT = _Text(","), _Text("]"), _Text("}")


def loads(text, parse_float=float, parse_constant=None, object_pairs_hook=None):
    """`json.loads(text, parse_float=..., parse_constant=..., object_pairs_hook=...)`, at any depth and integer
    width: `text` a str, or bytes in the encoding `json.loads` detects."""
    if isinstance(text, (bytes, bytearray)):
        text = text.decode(json.detect_encoding(text), "surrogatepass")
    kw = {"parse_float": parse_float}
    if parse_constant is not None:
        kw["parse_constant"] = parse_constant
    if object_pairs_hook is not None:
        kw["object_pairs_hook"] = object_pairs_hook
    with exact_ints():
        try:
            return json.JSONDecoder(**kw).decode(text)
        except RecursionError:
            return _loads_deep(text, parse_float, parse_constant or _CONSTANTS.__getitem__, object_pairs_hook)


# A line longer than this is parsed with its equal small JSON objects shared (`sharing_hook`). A value of
# millions of equal elements is a few bytes of input (an array of a class with no fields, read at any count
# the meter holds: docs/plans/core.md section 9, Q15 (c)), and parsed whole each element is objects of its
# own: `json.loads` of a list of 2^20 `{"t":"dict","v":[]}` grew the process's peak resident set by 310 bytes
# an element (Python 3.11), so one G3b mutant's two records, 33,554,432 such elements each, needed about 21 GB.
LONG_LINE = 64 << 20
# The most JSON values one shared object may hold, itself included: past it, it is parsed as it comes.
SHARED_NODES = 64


def _frozen(v, budget):
    """A key for the parsed JSON value `v`, equal only for values that serialise alike, or None past `budget`."""
    budget[0] -= 1
    if budget[0] < 0:
        return None
    t = type(v)
    if t is dict:
        # Already the shared object when it could be one (the hook runs on the innermost objects first),
        # and kept alive by the object that holds it.
        return (dict, id(v))
    if t is list:
        items = []
        for x in v:
            k = _frozen(x, budget)
            if k is None:
                return None
            items.append(k)
        return (list, tuple(items))
    if t is float:
        return (t, v.hex())  # 0.0 and -0.0 are equal in Python
    return (t, v)  # the type too: 1, 1.0 and true are equal in Python


def sharing_hook():
    """An `object_pairs_hook` for `loads` under which equal small objects (at most `SHARED_NODES` values each)
    are one Python object. No reader of what it parses may change an object."""
    shared = {}

    def hook(pairs):
        budget = [SHARED_NODES]
        key = []
        for k, v in pairs:
            f = _frozen(v, budget)
            if f is None:
                return dict(pairs)
            key.append((k, f))
        return shared.setdefault(tuple(key), dict(pairs))

    return hook


_CONSTANTS = {"NaN": float("nan"), "Infinity": float("inf"), "-Infinity": float("-inf")}
_WS = re.compile(r"[ \t\n\r]*")
# The C scanner's numbers (Modules/_json.c _match_number_unicode): ASCII digits only.
_NUMBER = re.compile(r"(-?(?:0|[1-9][0-9]*))(\.[0-9]+)?([eE][-+]?[0-9]+)?")


def _loads_deep(s: str, parse_float, parse_constant, object_pairs_hook=None):
    """`json.loads(s)` by a loop: the same value, or a `JSONDecodeError`, at any depth.

    It reads as the C scanner reads: whitespace is space, tab, LF and CR; strings through `json`'s own
    `scanstring` (strict: no control character unescaped); a repeated key keeps its first place and its last
    value; `NaN` and `Infinity` go to `parse_constant`, numbers with a fraction or an exponent to
    `parse_float`, and every other number is an `int`. Given an `object_pairs_hook`, each object, the empty
    one included, is what the hook makes of its list of (key, value) pairs, innermost first, as the C scanner
    calls it.
    """
    hook = object_pairs_hook
    ws = _WS.match

    def fail(msg, pos):
        raise JSONDecodeError(msg, s, pos)

    def key_at(i):
        """The key and the position after its `:` and the whitespace that follows."""
        if s[i : i + 1] != '"':
            fail("Expecting property name enclosed in double quotes", i)
        key, i = scanstring(s, i + 1, True)
        i = ws(s, i).end()
        if s[i : i + 1] != ":":
            fail("Expecting ':' delimiter", i)
        return key, ws(s, i + 1).end()

    i = ws(s, 0).end()
    # Open containers, innermost last: [a list, or a dict (an object's pairs when there is a hook), the key
    # its next value goes to, whether it is an object].
    stack = []
    while True:
        c = s[i : i + 1]
        if c == "{":
            i = ws(s, i + 1).end()
            if s[i : i + 1] != "}":
                key, i = key_at(i)
                stack.append([[] if hook else {}, key, True])
                continue
            value, i = (hook([]) if hook else {}), i + 1
        elif c == "[":
            i = ws(s, i + 1).end()
            if s[i : i + 1] != "]":
                stack.append([[], None, False])
                continue
            value, i = [], i + 1
        elif c == '"':
            value, i = scanstring(s, i + 1, True)
        else:
            m = _NUMBER.match(s, i)
            if m:
                integer, frac, exp = m.groups()
                value = parse_float(integer + (frac or "") + (exp or "")) if frac or exp else int(integer)
                i = m.end()
            else:
                for word, v in (("null", None), ("true", True), ("false", False)):
                    if s.startswith(word, i):
                        value, i = v, i + len(word)
                        break
                else:
                    for word in ("NaN", "Infinity", "-Infinity"):
                        if s.startswith(word, i):
                            value, i = parse_constant(word), i + len(word)
                            break
                    else:
                        fail("Expecting value", i)
        # The value goes to the innermost open container; each container it closes goes to the next.
        while True:
            if not stack:
                end = ws(s, i).end()
                if end != len(s):
                    fail("Extra data", end)
                return value
            top = stack[-1]
            container, is_object = top[0], top[2]
            if not is_object:
                container.append(value)
            elif hook:
                container.append((top[1], value))
            else:
                container[top[1]] = value
            i = ws(s, i).end()
            c = s[i : i + 1]
            if c == ",":
                i = ws(s, i + 1).end()
                if is_object:
                    top[1], i = key_at(i)
                break
            if c != ("}" if is_object else "]"):
                fail("Expecting ',' delimiter", i)
            stack.pop()
            value, i = (hook(container) if is_object and hook else container), i + 1
