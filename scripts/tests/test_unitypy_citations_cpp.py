"""scripts/unitypy_citations.py on UnityPyBoost's C++: the brace-matching scan, its pins and the definition rule.

Review round 14 found the C++ ranges held to the blank-line rule only, and five
citations of TypeTreeHelper.cpp 4-5 lines early, each starting in the function
above the one it cites. No test needs UnityPyBoost's sources (they are in
UnityPy's sdist, not in the repository); `--boost DIR` checks the pins against
a copy of them.
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

# A stand-in TypeTreeHelper.cpp of two functions, and its package.
TWO_FUNCTIONS = "template <bool swap>\nint a(int x)\n{\n    return x;\n}\n\nint b(int x)\n{\n    return x;\n}\n"
PACKAGE = T.package({"UnityPyBoost/TypeTreeHelper.cpp": TWO_FUNCTIONS}, {})

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
        shape = C.cpp_shape(SOURCE)
        self.assertEqual(shape.functions, DEFINITIONS)
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
            f"TypeTreeHelper.cpp:{3}-4",
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
            definitions = "1-3 f 5-5 g"
            for pins, error in (
                ((5, sha, "4", definitions), None),
                ((5, sha, "3", definitions), "line count or blank lines for UnityPyBoost/X.cpp"),
                ((6, sha, "4", definitions), "line count or blank lines for UnityPyBoost/X.cpp"),
                ((5, sha, "4", "1-3 f"), "definitions for UnityPyBoost/X.cpp are not those of its source, which are"),
                ((5, sha, "4", "2-3 f 5-5 g"), "definitions for UnityPyBoost/X.cpp"),
                ((5, sha, "4", "1-3 f 5-5 h"), "definitions for UnityPyBoost/X.cpp"),
            ):
                with self.subTest(pins), mock.patch.dict(C.BOOST, {"UnityPyBoost/X.cpp": pins}, clear=True):
                    if error is None:
                        got = load(boost)
                        self.assertEqual(got.shapes["UnityPyBoost/X.cpp"].functions, ((1, 3, "f"), (5, 5, "g")))
                    else:
                        with self.assertRaisesRegex(ValueError, error):
                            load(boost)

    def test_the_pins_are_well_formed(self):
        for name, (count, sha, blanks, definitions) in C.BOOST.items():
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
        self.assertNotEqual(hashlib.sha256(b"").hexdigest(), C.BOOST["UnityPyBoost/Mesh.hpp"][1])
        self.assertRegex(C.PACKAGE, r"^[0-9a-f]{64}$")
        for copy, blob in C.HEAD_FILES.values():
            self.assertEqual(C.blob_id((T.ROOT / C.HEAD_DIR / copy).read_bytes()), blob)


if __name__ == "__main__":
    unittest.main()
