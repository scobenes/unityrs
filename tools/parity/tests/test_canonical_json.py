"""canonical_json.py: a record to its line and back at any depth and integer width the dumps hold.

Review round 10: `json`'s C scanner and encoder stop at Python's recursion limit (a value a few hundred
dicts deep), and Python 3.11 refuses to convert an integer of more than 4,300 digits, so one deep value, or one
packed version wider than that, stopped the oracle's whole run (DumperError) and compare.py's (a traceback,
or exit 2). The loops that take over where `json` raises must give `json`'s own text and values.
"""

import json
import random
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import canonical as C  # noqa: E402
import canonical_json as J  # noqa: E402

OPTS = {"ensure_ascii": False, "separators": (",", ":"), "allow_nan": False, "check_circular": False}
# Strings that exercise every escape json writes: quotes, backslashes, control characters, U+2028, astral.
STRINGS = ["", "a", 'q"b\\c', "\x00\x1f\x7f", "  ", "é😀", "\ud800x"]


def random_value(rng, depth=0):
    roll = rng.random()
    if depth > 6 or roll < 0.35:
        return rng.choice(
            [None, True, False, 0, -1, 2**70, -(2**64), 1.5, -0.0, 1e300, *STRINGS, rng.randrange(-1000, 1000)]
        )
    if roll < 0.65:
        return [random_value(rng, depth + 1) for _ in range(rng.randrange(4))]
    if roll < 0.7:
        return tuple(random_value(rng, depth + 1) for _ in range(rng.randrange(3)))
    keys = [*STRINGS, "k", "t", "v", 7, 2.5, True, None]
    return {rng.choice(keys): random_value(rng, depth + 1) for _ in range(rng.randrange(4))}


def nested(depth, leaf=1):
    """A canonical dict value `depth` dicts deep: {"t":"dict","v":[["x", ...]]}."""
    v = leaf
    for _ in range(depth):
        v = {"t": "dict", "v": [["x", v]]}
    return v


class Dumps(unittest.TestCase):
    def test_the_loop_writes_jsons_text(self):
        rng = random.Random(10)
        for i in range(3000):
            v = random_value(rng)
            with self.subTest(i=i):
                self.assertEqual(J._dumps_deep(v), json.dumps(v, **OPTS))

    def test_the_loop_refuses_what_json_refuses(self):
        for bad in (float("nan"), [float("inf")], {float("nan"): 1}, {(1,): 2}, [object()]):
            with self.subTest(bad=bad), self.assertRaises((ValueError, TypeError)) as caught:
                J._dumps_deep(bad)
            with self.assertRaises(type(caught.exception)):
                json.dumps(bad, **OPTS)

    def test_a_value_past_the_recursion_limit_is_written(self):
        v = nested(50_000)
        with self.assertRaises(RecursionError):
            json.dumps(v, **OPTS)
        line = C.dumps({"rec": "x", "value": v})
        self.assertEqual(line, '{"rec":"x","value":' + '{"t":"dict","v":[["x",' * 50_000 + "1" + "]]}" * 50_000 + "}")

    def test_an_integer_of_any_width_is_written_and_the_limit_restored(self):
        self.assertEqual(sys.get_int_max_str_digits(), 4300)
        big = 10**5000 + 7
        with self.assertRaises(ValueError):
            json.dumps(big)
        text = C.dumps([big, -big])
        self.assertEqual(text, "[1" + "0" * 4999 + "7,-1" + "0" * 4999 + "7]")
        self.assertEqual(sys.get_int_max_str_digits(), 4300)


class Loads(unittest.TestCase):
    def test_the_loop_reads_what_json_reads(self):
        rng = random.Random(11)
        for i in range(3000):
            text = json.dumps(random_value(rng), **OPTS)
            if rng.random() < 0.5:
                text = " \n" + text.replace(",", ", \t").replace(":", " :\r") + "\n "
            with self.subTest(i=i, text=text):
                got = J._loads_deep(text, float, json.decoder._CONSTANTS.__getitem__)
                self.assertEqual(json.dumps(got, **OPTS), json.dumps(json.loads(text), **OPTS))

    def test_the_loop_refuses_what_json_refuses(self):
        bad = [
            "",
            "[",
            "[1,]",
            "[1 2]",
            '{"a" 1}',
            '{"a":1,}',
            "{1:2}",
            "01",
            "1.",
            ".5",
            "-",
            "[1]x",
            '"\x01"',
            "nul",
            "tru",
            "[,1]",
            '{"a":}',
            '"\\x"',
            "[1e]",
            "[NaN",
            "+1",
            "١",
        ]
        for text in bad:
            with self.subTest(text=text):
                with self.assertRaises(ValueError):
                    json.loads(text)
                with self.assertRaises(json.JSONDecodeError):
                    J._loads_deep(text, float, json.decoder._CONSTANTS.__getitem__)

    def test_hooks_and_repeated_keys_are_jsons(self):
        floats = []
        text = '{"a":1,"b":[1.5,2e3,NaN,-Infinity],"a":2}'
        got = J._loads_deep(text, lambda s: floats.append(s) or s, lambda c: f"<{c}>")
        self.assertEqual(got, {"a": 2, "b": ["1.5", "2e3", "<NaN>", "<-Infinity>"]})
        self.assertEqual(list(got), ["a", "b"])
        self.assertEqual(floats, ["1.5", "2e3"])

    def test_the_loop_calls_a_pairs_hook_as_json_does(self):
        """Integration 10: the loop takes `object_pairs_hook` (compare.parse shares a long line's equal objects
        through one), and calls it as the C scanner does: once an object, the empty one too, innermost first,
        with every pair, a repeated key's included."""
        rng = random.Random(12)
        for i in range(2000):
            text = json.dumps(random_value(rng), **OPTS)
            if i % 7 == 0:
                text = '{"a":1,"b":{},"a":[{"c":2}]}'
            calls = {"json": [], "loop": []}

            def hook(side, calls=calls):
                def record(pairs):
                    calls[side].append(json.dumps(pairs, **OPTS))
                    return ("obj", len(calls[side]))

                return record

            with self.subTest(i=i, text=text):
                want = json.loads(text, object_pairs_hook=hook("json"))
                got = J._loads_deep(text, float, json.decoder._CONSTANTS.__getitem__, hook("loop"))
                self.assertEqual(calls["loop"], calls["json"])
                self.assertEqual(json.dumps(got, **OPTS), json.dumps(want, **OPTS))

    def test_a_deep_lines_equal_objects_are_shared(self):
        """A line past the recursion limit is read by the loop, which shares its equal objects as `json`
        would (C.sharing_hook)."""
        deep = '{"t":"dict","v":[["x",' * 50_000 + "1" + "]]}" * 50_000
        text = '{"rec":"x","value":[{"t":"dict","v":[]},{"t":"dict","v":[]},' + deep + "]}"
        got = C.loads(text, object_pairs_hook=C.sharing_hook())
        self.assertIs(got["value"][0], got["value"][1])
        self.assertEqual(C.dumps(got), text)

    def test_a_line_past_the_recursion_limit_is_read(self):
        text = '{"rec":"x","value":' + '{"t":"dict","v":[["x",' * 50_000 + "1" + "]]}" * 50_000 + "}"
        with self.assertRaises(RecursionError):
            json.loads(text)
        got = C.loads(text)
        self.assertEqual(C.dumps(got), text)

    def test_bytes_are_read_as_json_reads_them(self):
        for text in (r'{"a":[1,"\u00e9","é"]}', r'["\ud800"]'):
            for codec in ("utf-8", "utf-16", "utf-16-be", "utf-32-le"):
                raw = text.encode(codec)
                with self.subTest(text=text, codec=codec):
                    self.assertEqual(C.loads(raw), json.loads(raw))
        deep = ("[" * 5000 + "]" * 5000).encode()
        self.assertEqual(C.dumps(C.loads(deep)), deep.decode())

    def test_an_integer_of_any_width_is_read_and_the_limit_restored(self):
        big = "9" * 6000
        with self.assertRaises(ValueError):
            json.loads(big)
        self.assertEqual(C.loads(f"[{big}]"), [10**6000 - 1])
        self.assertEqual(sys.get_int_max_str_digits(), 4300)


if __name__ == "__main__":
    unittest.main()
