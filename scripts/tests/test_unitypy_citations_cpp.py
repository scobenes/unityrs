"""scripts/unitypy_citations.py on UnityPyBoost's C++: the brace-matching scans, their pins, the definition rule and
the brace rule (scripts/unitypy_boost.py).

Review round 14 found the C++ ranges held to the blank-line rule only, and five
citations of TypeTreeHelper.cpp 4-5 lines early, each starting in the function
above the one it cites. Review round 15 found six more that each lie inside one
definition but start on a `}` or end inside a block, which the brace rule now
refuses. No test needs UnityPyBoost's sources (they are in UnityPy's sdist, not
in the repository); `--boost DIR` checks the pins against a copy of them.
"""

import hashlib
import sys
import unittest
from pathlib import Path
from unittest import mock

TESTS = Path(__file__).resolve().parent
sys.path[:0] = [str(TESTS), str(TESTS.parent), str(TESTS.parents[1] / "tools" / "parity")]

import test_unitypy_citations as T  # noqa: E402

import scratch  # noqa: E402
import unitypy_boost as B  # noqa: E402
import unitypy_citations as C  # noqa: E402

# Every construct UnityPyBoost's eight sources use, with a brace where the scan must not count one.
SOURCE = (
    "// a comment { with a brace\n"  # 1
    '#include "x.hpp"\n'
    "#define BRACE {\n"  # 3: a preprocessor line
    "#define JOINED(x) \\\n"  # 4: one joined to the next
    "    { x }\n"
    "\n"
    "typedef struct Reader\n"  # 7: a type, to the `;` after its name
    "{\n"
    "    int *ptr; /* { */\n"
    "}\n"
    "ReaderT;\n"  # 11
    "\n"
    "enum Kind\n"  # 13
    "{\n"
    "    a = 0,\n"
    "};\n"  # 16
    "\n"
    "template <bool swap, int N = 4>\n"  # 18: a function from its template line; `=` in its parameters
    "inline int read(ReaderT *r, int count = 1)\n"
    "{\n"
    "    if (r->ptr[0] == '{')\n"
    "    {\n"
    "        return 1;\n"
    "    }\n"
    "#if X\n"
    "    return 2;\n"
    "#else\n"
    "    return 3;\n"
    "#endif\n"
    "CLEANUP:\n"
    "    return 0;\n"
    "}\n"  # 32
    "\n"
    "template <bool swap>\n"  # 34: a declaration, not the next definition's first line
    "int later(ReaderT *r);\n"
    "\n"
    "static int *_cache = nullptr;\n"
    "\n"
    "static const std::map<const char *, Kind> names = {\n"  # 39: a table
    '    {"a", a},\n'
    "};\n"  # 41
    "\n"
    "/* a comment\n"
    "   of } two lines */\n"
    'static const char *text = "{ \\" }";\n'  # 45: a literal holding braces and a quote
    "int f(int a) {\n"  # 46: its brace on its own line
    "    return a;\n"
    "}\n"  # 48
    "\n"
    "static Type made = []() -> Type\n"  # 50: a lambda's value, to the `;` after its call
    "{\n"
    "    Type t = {\n"
    "        0\n"
    "    };\n"
    "    return t;\n"
    "}\n"
    "();\n"  # 57
)
DEFINITIONS = (
    (7, 11, "Reader"),
    (13, 16, "Kind"),
    (18, 32, "read"),
    (39, 41, "names"),
    (46, 48, "f"),
    (50, 57, "made"),
)
# SOURCE's brace depth at the end of each of its 57 lines, read off it by hand: no brace counted in a comment, a
# literal or a preprocessor line (1-5, 9, 21, 43-45).
DEPTHS = "000000011000011000011221111111100000001100000110001221100"

# A stand-in TypeTreeHelper.cpp of one function, for the brace rule; each line's depth at its end on the right.
BLOCKS = (
    "int f(int x)\n"  # 1: 0
    "{\n"  # 2: 1
    "    if (x)\n"  # 3: 1
    "    {\n"  # 4: 2
    "        x = 1;\n"  # 5: 2
    "    }\n"  # 6: 1
    "    while (x) {\n"  # 7: 2
    "        x--;\n"  # 8: 2
    "    }\n"  # 9: 1
    "    return x;\n"  # 10: 1
    "}\n"  # 11: 0
)

# A stand-in TypeTreeHelper.cpp of two functions, and its package.
TWO_FUNCTIONS = "template <bool swap>\nint a(int x)\n{\n    return x;\n}\n\nint b(int x)\n{\n    return x;\n}\n"
PACKAGE = T.package({"UnityPyBoost/TypeTreeHelper.cpp": TWO_FUNCTIONS}, {})

# Review round 15's citations of TypeTreeHelper.cpp that each lie within one definition and so passed the definition
# rule: where, the old range, what the brace rule says of it, and the corrected range. The first five are the
# reviewer's (walk/boost.rs:574 and walk/dict.rs:305 cite one range); the rest are what the rule found in the other
# 125 UnityPyBoost citations.
REVIEW_15 = (
    ("walk/boost.rs R11", (835, 849), "closes at line 836 a block begun above line 835", (846, 855)),
    ("walk/boost.rs, all three keys", (611, 633), "opens at line 628 a block it does not close", (611, 635)),
    ("walk/boost.rs, the empty class", (635, 642), "closes at line 635 a block begun above line 635", (637, 644)),
    ("walk/boost.rs, the first ref type", (644, 670), "closes at line 644 a block begun above line 644", (646, 673)),
    ("container.rs, slotted", (505, 513), "closes at line 505 a block begun above line 505", (508, 515)),
    ("PARITY.md, get_ref_type_node", (585, 643), "opens at line 586 a block it does not close", (587, 644)),
    ("container.rs, parse_class", (458, 582), "opens at line 459 a block it does not close", (458, 583)),
    ("core.md R9, the switch", (707, 874), "opens at line 708 a block it does not close", (707, 875)),
    ("core.md R14, ref_types", (592, 596), "opens at line 594 a block it does not close", (592, 598)),
    ("tree/mod.rs, Array's children", (820, 833), "opens at line 820 a block it does not close", (822, 826)),
    ("core.md, the arity first", (820, 833), "opens at line 820 a block it does not close", (822, 836)),
)

# walk/boost.rs's six citations before review round 14, as the corrected text, and the problem each old one has.
REVIEW_14 = (
    (f"`_read_length` (UnityPy TypeTreeHelper.cpp:{262}-281)", f":{266}-286", "starts inside `read_num_array`"),
    (f"R6 (UnityPy TypeTreeHelper.cpp:{283}-300)", f":{288}-305", "starts inside `_read_length` (lines 266-286)"),
    (f"R7 (UnityPy TypeTreeHelper.cpp:{303}-319)", f":{307}-323", "starts inside `read_str` (lines 288-305)"),
    (f"R8 (UnityPy TypeTreeHelper.cpp:{322}-347)", f":{325}-351", "starts inside `read_bytes` (lines 307-323)"),
    (f"`read_num_array` and friends (UnityPy TypeTreeHelper.cpp:{88}-260)", f":{86}-264", "starts inside `read_bool"),
    (f"`read_pair_array` (UnityPy TypeTreeHelper.cpp:{350}-391)", f":{353}-392", "starts inside `read_pair` (lines"),
)


class ScanTests(unittest.TestCase):
    def test_every_construct_of_the_pinned_sources(self):
        self.assertEqual(C.cpp_definitions(SOURCE), DEFINITIONS)
        self.assertEqual(B.cpp_depths(SOURCE), DEPTHS)
        shape = C.cpp_shape(SOURCE)
        self.assertEqual((shape.functions, shape.depths), (DEFINITIONS, DEPTHS))
        self.assertEqual([n for n in range(1, 58) if shape.blank[n]], [6, 12, 17, 33, 36, 38, 42, 49])
        # Its statements are not read.
        self.assertEqual((any(shape.inside), any(shape.open), shape.header), (False, False, {}))

    def test_what_the_scan_skips(self):
        for label, text in {
            "a line comment": "// }\nint f() {}\n",
            "a block comment": "/* } */ int f() {}\n",
            "a string": 'int f() { return "}"; }\n',
            "an escaped quote": 'int f() { return "\\"}"; }\n',
            "a character": "int f() { return '}'; }\n",
            "a preprocessor line": "#if X }\nint f() {}\n",
            "a joined preprocessor line": "#define X \\\n }\nint f() {}\n",
        }.items():
            with self.subTest(label):
                self.assertEqual(C.cpp_definitions(text)[-1][2], "f")
        # A comment's lines are counted.
        self.assertEqual(C.cpp_definitions("/*\n\n*/ int f()\n{\n}\n"), ((3, 5, "f"),))
        self.assertEqual(B.cpp_depths("/*\n{\n*/ int f()\n{\n}\n"), "00010")

    def test_depths_one_digit_a_line_cannot_pin(self):
        for label, (text, error) in {
            "a line that closes a block and opens another": ("int f() {\n} else {\n}\n", "line 2: closes a block"),
            "a brace that closes nothing": ("}\n", "line 1: a brace depth of -1"),
            "a depth past 9": ("int f()\n" + "{" * 10 + "\n", "line 2: a brace depth of 10"),
        }.items():
            with self.subTest(label), self.assertRaisesRegex(ValueError, error):
                B.cpp_depths(text)

    def test_a_block_the_scan_cannot_place(self):
        for label, text in {
            "a namespace": "namespace x {\n}\n",
            'an extern "C" block': 'extern "C" {\n}\n',
            "a block with nothing before it": "{\n}\n",
            "a brace that closes nothing": "int f() {}\n}\n",
            "a definition left open": "int f() {\n",
            "a type whose `;` never comes": "struct A {}\n",
            "a brace before a type's `;`": "struct A {}\nstruct B {};\n",
        }.items():
            with self.subTest(label), self.assertRaises(ValueError):
                C.cpp_definitions(text)


class DefinitionRuleTests(T.Checked):
    def report(self, files):
        with scratch.folder("unityrs-citations-") as tmp:
            T.write(Path(tmp), files)
            return C.check(Path(tmp), PACKAGE)

    def test_a_range_that_cuts_a_definition_fails(self):
        for cite, message in {
            f"TypeTreeHelper.cpp:{3}-8": "starts inside `a` (lines 1-5) and runs past its end",
            f"TypeTreeHelper.cpp:{5}-7": "starts inside `a` (lines 1-5) and runs past its end",
            f"TypeTreeHelper.cpp:{1}-8": "ends inside `b` (lines 7-10), which begins after the range does",
            f"TypeTreeHelper.cpp:{4}-7": "starts inside `a` (lines 1-5) and runs past its end",
            f"TypeTreeHelper.cpp:{6}": "starts on a blank line",
            f"TypeTreeHelper.cpp:{5}-6": "ends on a blank line",
        }.items():
            with self.subTest(cite):
                report = self.report({"a.md": f"`{cite}`\n"})
                self.assertEqual(len(report.problems), 1, report.problems)
                self.assert_problem(report, f"a.md:{1}: {cite} is not a whole span of UnityPyBoost/TypeTreeHelper.cpp")
                self.assert_problem(report, message)

    def test_a_whole_definition_one_within_it_or_none_passes(self):
        for cite in (
            f"TypeTreeHelper.cpp:{1}-5",
            f"TypeTreeHelper.cpp:{2}-5",  # without its template line: within it
            f"TypeTreeHelper.cpp:{1}-10",
            f"TypeTreeHelper.cpp:{4}",  # a statement within it (`{` with it is no whole block: BraceRuleTests)
            f"TypeTreeHelper.cpp:{2}",
            f"TypeTreeHelper.cpp:{7}-10",
        ):
            with self.subTest(cite):
                report = self.assert_passes({"a.md": f"`{cite}`\n"}, checked=1)
                self.assertEqual(report.boost, 1)

    def test_the_pins_refuse_review_14s_citations(self):
        """Each of walk/boost.rs's six ranges before review round 14 is refused under the pins; each correction
        passes."""
        with scratch.folder("unityrs-citations-") as tmp:
            repo = T.repository(Path(tmp), {})
            folder = T.unitypy_folder(Path(tmp), {"environment.py": "a\n"})
            package = C.load_package(folder, None, repo, C.package_digest(folder))
            for old, new, message in REVIEW_14:
                with self.subTest(old):
                    T.write(repo, {"a.rs": f"// {old}\n"})
                    report = C.check(repo, package)
                    self.assertEqual(len(report.problems), 1, report.problems)
                    self.assertIn(
                        "is not a whole span of UnityPyBoost/TypeTreeHelper.cpp: it " + message, report.problems[0]
                    )
                    T.write(repo, {"a.rs": f"// {old.split(':')[0]}{new})\n"})
                    report = C.check(repo, package)
                    self.assertEqual((report.problems, report.boost), ([], 1))


class BraceRuleTests(T.Checked):
    """A range closes no block begun above its first line and leaves open no block it opens; its statements are not
    read, so a header whose `{` is on the next line may end it."""

    def report(self, files):
        with scratch.folder("unityrs-citations-") as tmp:
            T.write(Path(tmp), files)
            return C.check(Path(tmp), T.package({"UnityPyBoost/TypeTreeHelper.cpp": BLOCKS}, {}))

    def test_a_range_that_is_no_whole_span_of_blocks_fails(self):
        for cite, message in {
            f"TypeTreeHelper.cpp:{6}-8": "closes at line 6 a block begun above line 6",
            f"TypeTreeHelper.cpp:{5}-7": "closes at line 6 a block begun above line 5",
            f"TypeTreeHelper.cpp:{8}-10": "closes at line 9 a block begun above line 8",
            f"TypeTreeHelper.cpp:{9}": "closes at line 9 a block begun above line 9",
            f"TypeTreeHelper.cpp:{3}-5": "opens at line 4 a block it does not close",
            f"TypeTreeHelper.cpp:{7}": "opens at line 7 a block it does not close",
            f"TypeTreeHelper.cpp:{6}-7": "closes at line 6 a block begun above line 6",
            f"TypeTreeHelper.cpp:{1}-10": "opens at line 2 a block it does not close",
            f"TypeTreeHelper.cpp:{3}-8": "opens at line 7 a block it does not close",
        }.items():
            with self.subTest(cite):
                report = self.report({"a.md": f"`{cite}`\n"})
                self.assertEqual(len(report.problems), 1, report.problems)
                whole = "is not a whole span of UnityPyBoost/TypeTreeHelper.cpp: it"
                self.assert_problem(report, f"{cite} {whole} {message}")

    def test_a_range_from_the_first_line_starts_outside_every_block(self):
        self.assertEqual(B.brace_problem("11", 1, 1), "opens at line 1 a block it does not close")

    def test_a_whole_span_of_blocks_passes(self):
        for cite in (
            f"TypeTreeHelper.cpp:{1}-11",  # the function
            f"TypeTreeHelper.cpp:{1}",  # its head
            f"TypeTreeHelper.cpp:{3}-6",  # an `if` and its block
            f"TypeTreeHelper.cpp:{3}",  # the header alone: its `{` is on the next line
            f"TypeTreeHelper.cpp:{4}-6",  # a block from its `{`
            f"TypeTreeHelper.cpp:{5}",
            f"TypeTreeHelper.cpp:{7}-9",
            f"TypeTreeHelper.cpp:{3}-10",  # the body's statements
            f"TypeTreeHelper.cpp:{10}",
        ):
            with self.subTest(cite):
                self.assert_passes({"a.md": f"`{cite}`\n"}, checked=1)

    def test_the_pins_refuse_review_15s_citations(self):
        """Each range review round 15 found within one definition but cutting a block is refused under the pins, and
        its correction passes. A whole span that is the wrong one still passes, wherever it is cited: R11's old
        range, all of `read_typetree_value_array`; the two comment lines above the `__slots__` read cited for the
        read (PARITY.md, section 9.2); and the `default:` branch's array half cited as the branch (tree/mod.rs)."""
        with scratch.folder("unityrs-citations-") as tmp:
            repo = T.repository(Path(tmp), {})
            folder = T.unitypy_folder(Path(tmp), {"environment.py": "a\n"})
            package = C.load_package(folder, None, repo, C.package_digest(folder))
            for where, (a, b), message, (c, d) in REVIEW_15:
                with self.subTest(where):
                    T.write(repo, {"a.rs": f"// (TypeTreeHelper.cpp:{a}-{b})\n"})
                    report = C.check(repo, package)
                    self.assertEqual(len(report.problems), 1, report.problems)
                    self.assertIn(f"TypeTreeHelper.cpp: it {message}", report.problems[0])
                    T.write(repo, {"a.rs": f"// (TypeTreeHelper.cpp:{c}-{d})\n"})
                    self.assertEqual(C.check(repo, package).problems, [])
            for a, b in ((885, 940), (508, 509), (812, 861)):
                with self.subTest(f"{a}-{b}"):
                    T.write(repo, {"a.rs": f"// (TypeTreeHelper.cpp:{a}-{b})\n"})
                    self.assertEqual(C.check(repo, package).problems, [])


def loader(tmp: str):
    """load_package on a stand-in UnityPy package and a repository in `tmp`, with UnityPyBoost's pins or a copy."""
    repo = T.repository(Path(tmp), {})
    folder = T.unitypy_folder(Path(tmp), {"environment.py": "a\nb\n"})
    return lambda boost=None: C.load_package(folder, boost, repo, C.package_digest(folder))


class PinTests(unittest.TestCase):
    def test_the_pinned_boost_sources(self):
        with scratch.folder("unityrs-citations-") as tmp:
            got = loader(tmp)()
        name = "UnityPyBoost/TypeTreeHelper.cpp"
        self.assertEqual(got.lines[name], C.BOOST[name][0])
        # Without --boost, its blank lines are the pinned ones (review round 12 found a citation of TypeTreeHelper.cpp
        # ending on its blank line 1033), and so are its definitions.
        blank = [n for n in range(1, got.lines[name] + 1) if got.shapes[name].blank[n]]
        self.assertEqual(blank, [int(n) for n in C.BOOST[name][2].split()])
        self.assertIn(1033, blank)
        self.assertEqual(got.shapes[name].functions, C.pinned_definitions(C.BOOST[name][3]))
        self.assertIn((266, 286, "_read_length"), got.shapes[name].functions)
        # And so are its brace depths: lines 637-644, `if (PyUnicode_GET_LENGTH(cls) == 0)` and its block, read off
        # the source by hand.
        self.assertEqual(got.shapes[name].depths, B.BOOST[name][4])
        self.assertEqual(got.shapes[name].depths[636:644], "12222221")
        self.assertEqual(got.shapes["UnityPyBoost/Mesh.hpp"].functions, ())

    def test_a_boost_copy_must_be_the_pinned_one(self):
        with scratch.folder("unityrs-citations-") as tmp:
            load, boost = loader(tmp), Path(tmp) / "boost"
            boost.mkdir()
            for name in C.BOOST:
                (boost / Path(name).name).write_text("not the source\n")
            with self.assertRaisesRegex(ValueError, "is not UnityPy 1.25.2's UnityPyBoost/"):
                load(boost)
            # A copy matching its sha256, whose blank lines or definitions are not the pinned ones.
            text = "int f()\n{\n}\n\nint g() {}\n"
            (boost / "X.cpp").write_text(text)
            sha = hashlib.sha256(text.encode()).hexdigest()
            definitions, depths = "1-3 f 5-5 g", "01000"
            for pins, error in (
                ((5, sha, "4", definitions, depths), None),
                ((5, sha, "3", definitions, depths), "line count or blank lines for UnityPyBoost/X.cpp"),
                ((6, sha, "4", definitions, depths), "line count or blank lines for UnityPyBoost/X.cpp"),
                ((5, sha, "4", "1-3 f", depths), "definitions for UnityPyBoost/X.cpp are not those of its source"),
                ((5, sha, "4", "2-3 f 5-5 g", depths), "definitions for UnityPyBoost/X.cpp"),
                ((5, sha, "4", "1-3 f 5-5 h", depths), "definitions for UnityPyBoost/X.cpp"),
                ((5, sha, "4", definitions, "01100"), "brace depths for UnityPyBoost/X.cpp are not those of its"),
                ((5, sha, "4", definitions, "0100"), "brace depths for UnityPyBoost/X.cpp"),
            ):
                with self.subTest(pins), mock.patch.dict(C.BOOST, {"UnityPyBoost/X.cpp": pins}, clear=True):
                    if error is None:
                        got = load(boost)
                        self.assertEqual(got.shapes["UnityPyBoost/X.cpp"].functions, ((1, 3, "f"), (5, 5, "g")))
                        self.assertEqual(got.shapes["UnityPyBoost/X.cpp"].depths, depths)
                    else:
                        with self.assertRaisesRegex(ValueError, error):
                            load(boost)

    def test_the_pins_are_well_formed(self):
        for name, (count, sha, blanks, definitions, depths) in C.BOOST.items():
            with self.subTest(name):
                self.assertTrue(name.startswith("UnityPyBoost/") and count > 0)
                self.assertRegex(sha, r"^[0-9a-f]{64}$")
                numbers = [int(n) for n in blanks.split()]
                self.assertEqual(numbers, sorted(set(numbers)))
                self.assertTrue(all(1 <= n <= count for n in numbers))
                # Every pin is read, in order, none overlapping another, each on lines that are not blank.
                pinned = C.pinned_definitions(definitions)
                self.assertEqual(" ".join(f"{a}-{b} {n}" for a, b, n in pinned), definitions)
                bounds = [n for a, b, _ in pinned for n in (a, b)]
                self.assertTrue(all(a <= b for a, b, _ in pinned))
                self.assertTrue(all(b < a for (_, b, _), (a, _, _) in zip(pinned, pinned[1:], strict=False)))
                self.assertTrue(all(1 <= n <= count and n not in numbers for n in bounds))
                # A digit a line, ending at 0; each definition starting and ending at 0; a blank line, holding no
                # brace, at the depth of the line before it.
                self.assertRegex(depths, rf"^[0-9]{{{count}}}$")
                at = [0] + [int(d) for d in depths]
                self.assertEqual(at[-1], 0)
                self.assertTrue(all(at[a - 1] == at[b] == 0 < max(at[a : b + 1]) for a, b, _ in pinned))
                self.assertTrue(all(at[n] == at[n - 1] for n in numbers))
        self.assertNotEqual(hashlib.sha256(b"").hexdigest(), C.BOOST["UnityPyBoost/Mesh.hpp"][1])
        self.assertRegex(C.PACKAGE, r"^[0-9a-f]{64}$")
        for copy, blob in C.HEAD_FILES.values():
            self.assertEqual(C.blob_id((T.ROOT / C.HEAD_DIR / copy).read_bytes()), blob)


if __name__ == "__main__":
    unittest.main()
