"""compare.py on records any one of which could stop it: a value deeper than Python's recursion limit, an
integer wider than Python 3.11 converts to text, and members of any JSON type where the format wants another.

Review round 10: every walk over a value recursed (strip_notes, same, value_diff, the decoder, the format
check), so two dumps of a value about 170 levels deep stopped the comparison with a RecursionError, and a
packed version wider than 4,300 digits did too. One record's value is that record's difference or format
finding, never the run's end (docs/CANONICAL_DUMP.md section 6).
"""

import random
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import canonical as C  # noqa: E402
import compare  # noqa: E402
import compare_diff  # noqa: E402
from compare_support import CompareCase, S, base_records, bundle_rec, end, f32, file_rec, obj_rec  # noqa: E402

DEPTH = 5_000


def clone(v):
    """A copy of a record at any depth (copy.deepcopy recurses)."""
    return C.loads(C.dumps(v))


def deep_dict(leaf, depth=DEPTH):
    v = leaf
    for _ in range(depth):
        v = S(["x", v])
    return v


def deep_list(leaf, depth=DEPTH):
    v = leaf
    for _ in range(depth):
        v = [v]
    return v


class DeepValues(CompareCase):
    def test_a_deep_value_that_differs_at_its_leaf_is_one_difference(self):
        a = [file_rec(), obj_rec(1, deep_dict(f32(0x3F800000))), end(1, 1)]
        b = [file_rec(), obj_rec(1, deep_dict(f32(0x3F800001))), end(1, 1)]
        _, d = self.run_compare(self.write("a.jsonl", a), self.write("b.jsonl", b))
        self.assertEqual([(r["level"], r["kind"]) for r in d["by_path"]], [("L2", "value")])
        self.assertEqual(d["by_path"][0]["path"], "l2.value" + ".x" * DEPTH)

    def test_deep_lists_that_differ_inside_are_one_difference(self):
        # `same` decides which elements differ; a list of lists that deep reached its recursion first.
        a = [file_rec(), obj_rec(1, S(["l", deep_list(1)])), end(1, 1)]
        b = [file_rec(), obj_rec(1, S(["l", deep_list(2)])), end(1, 1)]
        _, d = self.run_compare(self.write("a.jsonl", a), self.write("b.jsonl", b))
        # The report groups a path's indices as `[]`; the example keeps each.
        self.assertEqual([(r["kind"], r["path"]) for r in d["by_path"]], [("value", "l2.value.l" + "[]" * DEPTH)])

    def test_a_deep_value_in_lines_that_differ_in_a_note_alone_agrees(self):
        # compared_text strips the notes of the whole compared record, the value included.
        a = [file_rec(), obj_rec(1, deep_dict(7)), end(1, 1)]
        a[1]["l3"].update(resave="error", note="one wording")
        b = clone(a)
        b[1]["l3"]["note"] = "another"
        report, d = self.run_compare(self.write("a.jsonl", a), self.write("b.jsonl", b))
        self.assertEqual(d["by_path"], [])
        self.assertEqual(report.counts["matched"], 3)

    def test_a_deep_value_where_the_format_wants_none_is_a_format_finding(self):
        # A deep JSON value where a scalar belongs, in metadata and as a value's member.
        a = base_records()
        a[0]["l1"]["unity_version"] = deep_list(1)
        a[1]["l2"]["value"] = S(["x", deep_list({"t": "nope"})])
        path = self.write("a.jsonl", a)
        report, d = self.run_compare(path, path)
        found = {(r["level"], r["path"], r["kind"]) for r in d["by_path"]}
        self.assertEqual(
            found,
            {
                ("format", "l1.unity_version (reference)", "bad_name"),
                ("format", "l1.unity_version (candidate)", "bad_name"),
                ("format", "l2.value.x" + "[]" * DEPTH + " (reference)", "unknown_tag"),
                ("format", "l2.value.x" + "[]" * DEPTH + " (candidate)", "unknown_tag"),
            },
        )
        self.assertEqual(report.counts["identical"], 4)

    def test_a_deep_value_is_printed(self):
        a = [file_rec(), obj_rec(1, S(["x", 1])), end(1, 1)]
        b = [file_rec(), obj_rec(1, S(["x", deep_list(1)])), end(1, 1)]
        path_a, path_b = self.write("a.jsonl", a), self.write("b.jsonl", b)
        self.assertEqual(self.cli(path_a, path_b), 1)
        self.assertIn("l2.value.x", self.output)
        self.assertIn("[[[[", self.output)


class WideIntegers(CompareCase):
    def test_a_version_wider_than_4300_digits_is_read_and_printed(self):
        wide = 10**5000 + 1
        a, b = [bundle_rec(), end(1, 0)], [bundle_rec(), end(1, 0)]
        a[0]["l1"]["engine_version_packed"] = wide
        b[0]["l1"]["engine_version_packed"] = wide + 1
        path_a, path_b = self.write("a.jsonl", a), self.write("b.jsonl", b)
        _, d = self.run_compare(path_a, path_a)
        self.assertEqual(d["by_path"], [])
        _, d = self.run_compare(path_a, path_b)
        self.assertEqual([(r["path"], r["kind"]) for r in d["by_path"]], [("l1.engine_version_packed", "value")])
        self.assertEqual(d["by_path"][0]["examples"][0]["a"], "1" + "0" * 156 + "...")
        self.assertEqual(self.cli(path_a, path_b), 1)
        self.assertIn("l1.engine_version_packed", self.output)
        self.assertEqual(sys.get_int_max_str_digits(), 4300)


# compare.py's walks before review round 10, which recursed: the walks that replace them must give their
# results in their order, wherever these do not run out of stack.


def recursive_strip_notes(v):
    if isinstance(v, dict):
        return {k: recursive_strip_notes(x) for k, x in v.items() if k != "note"}
    if isinstance(v, list):
        return [recursive_strip_notes(x) for x in v]
    return v


def recursive_same(a, b):
    t = type(a)
    if t is not type(b):
        return False
    if t is dict:
        return list(a) == list(b) and all(recursive_same(a[k], b[k]) for k in a)
    if t is list or t is tuple or t is C.Dict or t is C.Tuple:
        return len(a) == len(b) and all(map(recursive_same, a, b))
    return a == b


def recursive_elements(a, b, path, out, more):
    if len(a) != len(b):
        out.append((path, "length", len(a), len(b)))
    shown = 0
    for i, (x, y) in enumerate(zip(a, b, strict=False)):
        if not recursive_same(x, y):
            if shown == compare.MAX_ELEMENT_DIFFS:
                out.append((f"{path}[...]", "more", None, None))
                break
            shown += 1
            more(x, y, f"{path}[{i}]", out)


def recursive_json_diff(a, b, path, out):
    if type(a) is not type(b):
        out.append((path, "type", a, b))
    elif isinstance(a, dict):
        for k in [*a, *(k for k in b if k not in a)]:
            if k not in b:
                out.append((f"{path}.{k}", "missing", a[k], None))
            elif k not in a:
                out.append((f"{path}.{k}", "extra", None, b[k]))
            else:
                recursive_json_diff(a[k], b[k], f"{path}.{k}", out)
        if list(a) != list(b) and a.keys() == b.keys():
            out.append((path, "key_order", list(a), list(b)))
    elif isinstance(a, list):
        recursive_elements(a, b, path, out, recursive_json_diff)
    elif a != b:
        out.append((path, "value", a, b))


def recursive_value_diff(a, b, path, out):
    key_id, key_text = compare_diff._key_id, compare_diff._key_text
    ta, tb = type(a), type(b)
    if ta is not tb:
        out.append((path, "type", a, b))
        return
    if ta is C.Dict:
        ka, kb = [key_id(k) for k, _ in a], [key_id(k) for k, _ in b]
        da, db = {key_id(k): v for k, v in a}, {key_id(k): v for k, v in b}
        if ka != kb:
            if set(ka) == set(kb):
                out.append((path, "key_order", [k for _, k in ka], [k for _, k in kb]))
            for k in ka:
                if k not in db:
                    out.append((f"{path}.{key_text(k[1])}", "missing_key", da[k], None))
            for k in kb:
                if k not in da:
                    out.append((f"{path}.{key_text(k[1])}", "extra_key", None, db[k]))
        for k in ka:
            if k in db:
                recursive_value_diff(da[k], db[k], f"{path}.{key_text(k[1])}", out)
    elif ta is list or ta is C.Tuple:
        recursive_elements(a, b, path, out, recursive_value_diff)
    elif a != b:
        snan = ta is C.F32 and (a ^ b) == 0x00400000 and (a & 0x7F800000) == 0x7F800000 and a & 0x00400000
        out.append((path, "f32_snan_quieted" if snan and b & 0x007FFFFF else "value", a, b))


def random_json(rng, depth=0):
    roll = rng.random()
    if depth > 5 or roll < 0.4:
        return rng.choice([None, True, False, 0, 1, -1, 1.5, "", "a", "note", [], {}])
    if roll < 0.7:
        return [random_json(rng, depth + 1) for _ in range(rng.randrange(8))]
    return {rng.choice(["a", "b", "note", "c"]): random_json(rng, depth + 1) for _ in range(rng.randrange(4))}


def random_value(rng, depth=0):
    """An encoded value (spec section 3) of every kind."""
    roll = rng.random()
    if depth > 5 or roll < 0.4:
        return rng.choice(
            [0, 1, -1, True, False, None, "s", "", {"t": "str", "hex": "ff"}, f32(0x7FA00000), f32(0x7FE00000),
             f32(0x3F800000), {"t": "f64", "v": "3ff0000000000000"}, {"t": "bytes", "hex": "00"},
             {"t": "u8s", "hex": "0001"}, {"t": "f32s", "v": "3f800000"}]
        )
    if roll < 0.6:
        return [random_value(rng, depth + 1) for _ in range(rng.randrange(8))]
    if roll < 0.7:
        return {"t": "tuple", "v": [random_value(rng, depth + 1) for _ in range(rng.randrange(3))]}
    keys = ["a", "b", "c", {"t": "str", "hex": "ff"}]
    return {"t": "dict", "v": [[rng.choice(keys), random_value(rng, depth + 1)] for _ in range(rng.randrange(4))]}


def perturb(rng, v):
    """`v` with a few of its members replaced or reordered."""
    v = clone(v)
    for _ in range(rng.randrange(4)):
        x = v
        while True:
            tagged = type(x) is dict and x.get("t") in ("dict", "tuple")
            kids = x["v"] if tagged else x if type(x) is list else None
            pairs = tagged and x["t"] == "dict"
            if not kids or rng.random() < 0.3:
                break
            i = rng.randrange(len(kids))
            if rng.random() < 0.4:
                if rng.random() < 0.5:
                    kids.reverse()
                else:
                    kids[i] = [kids[i][0], random_value(rng, 4)] if pairs else random_value(rng, 4)
                break
            x = kids[i][1] if pairs else kids[i]
    return v


class TheWalksAreTheRecursions(unittest.TestCase):
    def test_the_walks_give_the_recursions_results_in_their_order(self):
        rng = random.Random(10)
        for i in range(4000):
            a = random_value(rng)
            b = perturb(rng, a) if rng.random() < 0.8 else random_value(rng)
            da, db = C.decode(a), C.decode(b)
            got, want = [], []
            compare.value_diff(da, db, "v", got)
            recursive_value_diff(da, db, "v", want)
            with self.subTest(i=i, a=a, b=b):
                self.assertEqual(got, want)
                self.assertEqual(compare.same(da, db), recursive_same(da, db))
            ja, jb = random_json(rng), random_json(rng)
            got, want = [], []
            compare.json_diff(ja, jb, "j", got)
            recursive_json_diff(ja, jb, "j", want)
            with self.subTest(i=i, ja=ja, jb=jb):
                self.assertEqual(got, want)
                self.assertEqual(compare.strip_notes(ja), recursive_strip_notes(ja))
                self.assertEqual(compare.same(ja, jb), recursive_same(ja, jb))


class AnyMember(CompareCase):
    """Every member of every record replaced by JSON of another shape: compare.py reports, never raises."""

    SHAPES = [
        None, True, 0, -1, 2**64, 10**5000, "", "x", [], {}, [1, "a"], {"t": "str"}, {"t": "f32", "v": 1},
        {"t": "dict", "v": [[1, 2]]}, {"t": "bytes", "summary": {"len": "1"}}, {"t": "tuple"}, deep_list(0, 2_000),
        deep_dict(0, 2_000), {"t": "dict", "v": [[{"t": "str", "hex": 5}, 1]]}, {"t": "u8s", "hex": "zz"},
    ]

    def members(self, v, path=()):
        """Every (path, member) below a record, containers included."""
        out = []
        todo = [(v, path)]
        while todo:
            x, p = todo.pop()
            if p:
                out.append(p)
            if isinstance(x, dict):
                todo.extend((y, (*p, k)) for k, y in x.items())
            elif isinstance(x, list) and len(p) < 12:
                todo.extend((y, (*p, i)) for i, y in enumerate(x))
        return out

    def test_any_member_of_any_shape_is_reported_not_raised(self):
        rng = random.Random(10)
        base = base_records()
        sites = [(i, p) for i, rec in enumerate(base) for p in self.members(rec) if p != ("rec",)]
        clean = self.write("clean.jsonl", base)
        for n in range(150):
            recs = clone(base)
            for _ in range(rng.randrange(1, 4)):
                i, p = rng.choice(sites)
                target = recs[i]
                try:
                    for k in p[:-1]:
                        target = target[k]
                    target[p[-1]] = clone(rng.choice(self.SHAPES))
                except (KeyError, IndexError, TypeError):
                    continue  # an earlier replacement removed this site
            path = self.write(f"m{n}.jsonl", recs)
            with self.subTest(n=n, recs=C.dumps(recs)[:300]):
                for a, b in ((clean, path), (path, clean), (path, path)):
                    try:
                        report, _ = self.run_compare(a, b)
                    except C.FormatError:
                        continue  # a record without a key, or a second header: the dump is unreadable
                    report.as_dict()
                    self.assertTrue(report.has_diffs() or a == b)


if __name__ == "__main__":
    unittest.main()
