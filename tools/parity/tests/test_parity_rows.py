"""PARITY.md's test column, checked against the row tests, and the rows on the package itself.

`TestColumn` (standard library) checks that every test a row's test column names exists in the module named with
it, and that the row modules, this one and `test_rows_*.py`, hold only tests a row names; the guards on the docs'
wording that go with them sit beside it. `Rows` runs sections 2, 2.1 and 23 against the oracle: the package's
aliases and re-exports, and the variables its module-level loops leave bound. The other rows a review disputed are
in `test_rows_*.py`, a module a topic. Each test names the PARITY.md rows it keeps honest; a row's test column names
the test back. `Rows` needs UnityPy 1.25.2 (the oracle image); skipped elsewhere.
"""

import io
import re
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE / "tests"))

from rows_support import ORACLE  # noqa: E402

import inventory  # noqa: E402

if ORACLE:
    import UnityPy
    from UnityPy.enums import TextureFormat
    from UnityPy.export import Texture2DConverter
    from UnityPy.streams import EndianBinaryReader


# DESIGN.md section 3's rule for Tier U, the lead of its bullet; PARITY.md section 0's U row repeats it.
UNSUPPORTED_RULE = "Unsupported until a sample and a reference exist, or until UnityPy's behaviour is characterised"


class TestColumn(unittest.TestCase):
    """PARITY.md's test column and the tests it names agree (standard library)."""

    CITED = re.compile(r"`(test_[a-z_]+\.py)` ([a-z0-9_]+)")

    def test_every_cited_test_exists_and_every_row_test_is_cited(self):
        text = (HERE.parent.parent / "PARITY.md").read_text(encoding="utf-8")
        cited = set(self.CITED.findall(text))
        for module, name in sorted(cited):
            source = (HERE / "tests" / module).read_text(encoding="utf-8")
            self.assertIn(f"def test_{name}(", source, f"PARITY.md cites {module} {name}, which does not exist")
        # The row modules hold only tests a row names.
        modules = [Path(__file__), *sorted((HERE / "tests").glob("test_rows_*.py"))]
        self.assertGreater(len(modules), 1)
        for path in modules:
            defined = set(re.findall(r"def test_([a-z0-9_]+)\(", path.read_text(encoding="utf-8")))
            defined -= {
                "every_cited_test_exists_and_every_row_test_is_cited",
                "every_status_agrees_with_its_test_column",
                "rows_are_cited_by_item_not_by_line",
                "head_dicts_are_a_target_while_their_fixtures_are_planned",
                "every_resolution_exception_is_listed",
                "an_empty_path_raises_only_where_no_cab_answers_it",
                "the_gpu_program_index_rule_is_int_in_base_10",
                "a_failure_to_write_is_tier_c_writing",
                "every_tier_has_a_design_rule",
                "tol_metrics_are_stated_or_said_to_be_planned",
                "module_level_data_is_tier_a_for_loop_variables",
                "a_non_local_miss_keeps_the_split_exception",
                "non_local_rows_state_only_the_tested_cases",
                "a_local_split0_sends_the_read_to_its_parts",
                "zero_size_writes_are_stated_off_switch",
            }
            named = {name for module, name in cited if module == path.name}
            self.assertEqual(sorted(defined - named), [], f"tests no PARITY.md row names, in {path.name}")

    @classmethod
    def probed(cls, test_cell):
        """Whether the test column names a test, or the corpus dump, outside a `planned` clause."""
        for clause in test_cell.split(";"):
            if "planned" in clause:
                continue
            if cls.CITED.search(clause) or re.search(r"`test_[a-z_]+\.py` \(all\)", clause) or "corpus dump" in clause:
                return True
        return False

    def test_every_status_agrees_with_its_test_column(self):
        """Section 0's status rules: a row that names a test is pinned or verified, one that names none is
        unverified (source reading), n/a (tier —) or a pointer to another row; `planned` is section 17.1's. A test
        or the corpus dump named inside a `planned` clause probes nothing, every row states its own status, and
        every test a test column names carries its module."""
        text = (HERE.parent.parent / "PARITY.md").read_text(encoding="utf-8")
        seen = 0
        for header, rows in inventory.tables(text):
            if "status" not in header or "test" not in header:
                continue
            test, status = header.index("test"), header.index("status")
            for cells in rows:
                seen += 1
                probed = self.probed(cells[test])
                state = cells[status]
                with self.subTest(row=cells[0][:60], status=state):
                    # Every test is named with its module, so a renamed test fails the check above.
                    self.assertIsNone(re.search(r"`test_[a-z_]+\.py` [a-z0-9_]+, [a-z]", cells[test]), "a bare name")
                    if state.startswith(("pinned P0", "verified P0")):
                        self.assertTrue(probed, "a probed row names what probes it")
                    elif state.startswith("unverified (source reading); impl P"):
                        self.assertFalse(probed, "a row a test runs is not unverified")
                    elif state.startswith("planned ("):
                        self.assertIn("reference", header, "only section 17.1's references are planned")
                    elif state == "n/a":
                        self.assertEqual(cells[header.index("tier")], "—")
                    else:
                        self.assertEqual(state, "", "an unknown status")
                        self.assertEqual(set(cells[2:]), {""}, "a row with no status points at another section")
        self.assertGreater(seen, 500)

    def test_rows_are_cited_by_item_not_by_line(self):
        """A row points at another by its item and section ("the `.splitN` merging row, section 3"), never by a
        line number of PARITY.md, which every edit above it moves; nor do the docs and the parity tools and tests.
        So none says "row N" in any case, "line N of this file", or `PARITY.md:N`, and a "line N" stands only right
        after the source file it is a line of (`fmod.py` lines 129-137)."""
        root = HERE.parent.parent
        paths = [root / "PARITY.md", *sorted((root / "docs").glob("*.md")), *sorted(HERE.glob("*.md"))]
        paths += [*sorted(HERE.glob("*.py")), *sorted((HERE / "tests").glob("*.py"))]
        for path in paths:
            text = path.read_text(encoding="utf-8")
            with self.subTest(path.name):
                self.assertEqual(re.findall(r"(?i)\brows? \d+(?![\d%])", text), [])
                self.assertEqual(re.findall(r"(?i)\b(?:PARITY|DESIGN|README|CANONICAL_DUMP)\.md:\d+", text), [])
                self.assertEqual(
                    re.findall(r"(?i)\blines? \d+\S* of (?:this|that|the) (?:file|table|section)", text), []
                )
                bare = [
                    m.group(0)
                    for m in re.finditer(r"(?i)\blines? \d+", text)
                    if not re.search(r"`[^`\n]+` $", text[: m.start()])
                ]
                self.assertEqual(bare, [], "a line number with no source file before it")

    # DESIGN.md section 3's resolution clause names the Tier B, C and U exceptions to "`obj.container` and dependency
    # resolution match". Each bullet of those lists that touches resolution (it names one of RESOLUTION) maps here
    # to the phrase the clause names it by.
    RESOLUTION = (
        "find_file",
        "PPtr.deref",
        "load_file",
        "load_dependencies",
        "load_assets",
        "path_dict",
        "registry",
        "`.splitN`",
    )
    NAMED_BY = {
        "a stream whose `name` is not a `str`": ("B", "`PPtr.deref` reaching an `int` or `None` `files` key"),
        "a record that `PPtr.deref` resolves": ("B", "a record whose file's basename holds an upper-case letter"),
        "a name read through a non-local fsspec filesystem, which": (
            "B",
            "a name read through a non-local fsspec filesystem where UnityPy raises",
        ),
        "a `.splitN` path whose part is a folder": ("B", "a path through a `.splitN` folder"),
        "a ZIP that `load_file` opens from a path": ("B", "a ZIP on disk that `find_file` loads"),
        "a local path with a URL-style prefix (": ("B", "a local path with a URL-style prefix where UnityPy raises"),
        "a local path with a URL-style prefix where": (
            "C",
            "a local path with a URL-style prefix where `load_file` returns `None`",
        ),
        "two bundles holding a CAB of one name": ("C", "two bundles' same-named CABs"),
        "a path holding `.splitN` before its end": ("C", "a path holding `.splitN` before its end"),
        "`find_file`'s suffix match": ("C", "`find_file`'s suffix match with no separator boundary"),
        "split members of a zip": ("C", "split members of a zip"),
        "a name read through a non-local fsspec filesystem where": (
            "C",
            "a name read through a non-local fsspec filesystem where `load_file` returns `None`",
        ),
        "`load_dependencies` loads dependencies": ("C", "`load_dependencies`"),
        "`ContainerHelper.path_dict` ignores": ("C", "`ContainerHelper.path_dict`'s file id"),
        "a name read through a non-local fsspec filesystem in a case": (
            "U",
            "a name read through a non-local fsspec filesystem outside the cases `PARITY.md` section 3 states",
        ),
    }

    # Tier B, C or U bullets that name a RESOLUTION word without being an exception to resolution opt out here, by
    # their lead; each lead must still name a bullet.
    NOT_RESOLUTION = ()

    def resolution_bullets(self, reading, opt_out):
        """(tier, bullet) for each Tier B, C or U bullet of `reading` that names a RESOLUTION word, and the opt-out
        leads that named a bullet."""
        leads = {"- **Tier B": "B", "- **Tier C": "C", f"- **{UNSUPPORTED_RULE}": "U"}
        tier, found, used = None, [], set()
        for line in reading.splitlines():
            if line.startswith("- **"):
                tier = next((name for lead, name in leads.items() if line.startswith(lead)), None)
            elif tier and line.startswith("  - ") and any(word in line for word in self.RESOLUTION):
                skipped = [lead for lead in opt_out if line[4:].startswith(lead)]
                used.update(skipped)
                if not skipped:
                    found.append((tier, line[4:]))
        return found, used

    def test_every_resolution_exception_is_listed(self):
        """Every Tier B, C or U bullet of DESIGN.md section 3 that touches dependency resolution is named, under its
        tier, in the clause that lists the exceptions to "dependency resolution match"; a bullet that names a
        RESOLUTION word and is no such exception opts out in NOT_RESOLUTION."""
        sample = "- **Tier B: x.**\n  - a lead `load_file` names;\n  - another lead `find_file` names;\n"
        self.assertEqual(
            self.resolution_bullets(sample, ("a lead",)), ([("B", "another lead `find_file` names;")], {"a lead"})
        )
        design = (HERE.parent.parent / "docs" / "DESIGN.md").read_text(encoding="utf-8")
        reading = design.split("**Reading**", 1)[1].split("**Writing**", 1)[0]
        clause = next(line for line in reading.splitlines() if "dependency resolution match" in line)
        listed = clause.split("lists below say:", 1)[1]
        named = {"B": listed.split("(B)", 1)[0], "C": listed.split("(B)", 1)[1].split("(C)", 1)[0]}
        named["U"] = listed.split("(C)", 1)[1].split("(U)", 1)[0]
        found, used = self.resolution_bullets(reading, self.NOT_RESOLUTION)
        self.assertEqual(used, set(self.NOT_RESOLUTION), "an opt-out lead that names no bullet")
        for tier, bullet in found:
            with self.subTest(tier=tier, bullet=bullet[:70]):
                lead = [key for key in self.NAMED_BY if bullet.startswith(key)]
                self.assertEqual(len(lead), 1, "a resolution exception no clause names: list it, and map it here")
                in_tier, phrase = self.NAMED_BY[lead[0]]
                self.assertEqual(in_tier, tier)
                self.assertIn(phrase, named[tier], "missing from the resolution clause")
        self.assertEqual(len(found), len(self.NAMED_BY))

    def test_an_empty_path_raises_only_where_no_cab_answers_it(self):
        """An empty resource path is joined to `Environment.path` and raises `IsADirectoryError` only where no CAB
        `""` is registered: a zip's directory entry registers one, which answers the path with `b""` first (section
        3). So every PARITY.md row and DESIGN.md line that has an empty path, or a texture with no data, raise there
        (or raise while its data is fetched) names CAB `""` too."""
        root = HERE.parent.parent
        lines = [("PARITY.md", line) for line in (root / "PARITY.md").read_text("utf-8").splitlines()]
        lines += [("DESIGN.md", line) for line in (root / "docs" / "DESIGN.md").read_text("utf-8").splitlines()]
        seen = 0
        for name, line in lines:
            raises = re.search(
                r"IsADirectoryError|raises as `Texture2D.get_image_data` does|raises while its data is fetched", line
            )
            empty = re.search(r"empty (?:stream |resource )?path|empty `m_Source`|empty name|with no data", line)
            if raises and empty:
                seen += 1
                with self.subTest(name, line=line[:80]):
                    self.assertIn('CAB `""`', line)
        self.assertGreaterEqual(seen, 6)

    def test_the_gpu_program_index_rule_is_int_in_base_10(self):
        """`GpuProgramIndex N` fills where `int()` in base 10 takes the rest of the line, which is not Python's
        integer-literal grammar (`+0` and `٠` fill, `0x0` raises; `test_rows_exports.py` shader_text): no line of
        PARITY.md or DESIGN.md that speaks of such a line uses the word literal or token, whatever qualifies it,
        unless its clause is a contrast ("need not be" one, or a clause opening with "not" after a comma or an
        opening parenthesis), and each that states the refusal of a line the table cannot fill names `int()` in base
        10."""
        # Any "literal" or "token", unless the clause it sits in (from the last comma, parenthesis, semicolon or
        # colon) opens with "not" or "which is not", or says "need not be".
        word = re.compile(r"(?i)\b(?:literal|token)s?\b")
        contrast = re.compile(r"(?i)^\s*(?:which is\s+)?not\b|\bneed not be\b")

        def affirmative(line):
            found = []
            for m in word.finditer(line):
                clause = re.split(r"[,(;:]", line[: m.start()])[-1]
                if not contrast.search(clause):
                    found.append(clause + m.group(0))
            return found

        for wording, calls in (
            ("the rest of the line is no Python `int` literal", True),
            ("N parsed as an integer literal", True),
            ("refused where the rest is not a Python int literal", True),
            ("filled where the rest is a valid Python integer literal", True),
            ("filled where the rest is a decimal integer literal", True),
            ("read with the int literal grammar", True),
            ("read as Python's integer-literal grammar", True),
            ("filled where the rest is a decimal literal", True),
            ("filled where the rest is a Python numeric literal", True),
            ("N parses as an integer token", True),
            ("filled where the rest is a base-10 integer literal", True),
            ("`int()` in base 10, which is not a valid Python integer literal", False),
            ("`int()` in base 10, which is not Python's integer-literal grammar", False),
            ("`int()` in base 10, not an integer literal (`0x0` raises)", False),
            ("`int()` in base 10 (not an integer literal)", False),
            ("N need not be a Python int literal", False),
        ):
            self.assertEqual(bool(affirmative(wording)), calls, wording)
        root = HERE.parent.parent
        refusals = 0
        for name, path in (("PARITY.md", root / "PARITY.md"), ("DESIGN.md", root / "docs" / "DESIGN.md")):
            for line in path.read_text(encoding="utf-8").splitlines():
                if "GpuProgramIndex" not in line:
                    continue
                with self.subTest(name, line=line[:80]):
                    self.assertEqual(affirmative(line), [])
                    if "cannot fill, where" in line:  # the refusal, not a tier cell that names it
                        refusals += 1
                        self.assertIn("`int()` in base 10", line)
        self.assertGreaterEqual(refusals, 2)

    def test_a_non_local_miss_keeps_the_split_exception(self):
        """Through a non-local fs a `.splitN` name loads, its parts read through `fs` (section 3, `Environment.fs`;
        `a_non_local_fs_loads_only_through_load_and_streams`). So a sentence of a PARITY.md cell or a DESIGN.md line
        that says something read through a non-local fs is not found is followed, in that sentence or a later one of
        the same cell, by one saying a `.splitN` name loads."""
        sentence_end = re.compile(r"(?<=\.)\s+(?=[A-Z`])")
        miss = re.compile(r"non-local.*\bnot found\b")
        split_loads = re.compile(r"`\.splitN`.*\bloads\b")
        for text, flagged in (
            ("with a non-local `fs`, a file argument other than a zip/apk, or a folder, is not found. A path.", True),
            ("with a non-local `fs` a name is not found. A `.splitN` name is the exception: so it loads", False),
            ("with a non-local `fs`, a `.splitN` file argument loads, while another is not found", False),
        ):
            sentences = sentence_end.split(text)
            unexcused = [
                i
                for i, s in enumerate(sentences)
                if miss.search(s) and not any(split_loads.search(t) for t in sentences[i:])
            ]
            self.assertEqual(bool(unexcused), flagged, text)
        root = HERE.parent.parent
        seen = 0
        for name, path in (("PARITY.md", root / "PARITY.md"), ("DESIGN.md", root / "docs" / "DESIGN.md")):
            for line in path.read_text(encoding="utf-8").splitlines():
                for cell in line.split(" | "):
                    sentences = sentence_end.split(cell)
                    for i, sentence in enumerate(sentences):
                        if miss.search(sentence):
                            seen += 1
                            with self.subTest(name, sentence=sentence[:80]):
                                self.assertTrue(any(split_loads.search(t) for t in sentences[i:]))
        # Only `Environment.fs` says it now: the constructor row states its tested cases, none of them "not found".
        self.assertGreaterEqual(seen, 1)

    def test_non_local_rows_state_only_the_tested_cases(self):
        """Through a non-local fs, section 3 states the constructor's outcomes and `load_folder`'s builtin `open` only
        for the cases `a_non_local_fs_loads_only_through_load_and_streams` runs (a `MemoryFileSystem` holding each
        argument under a top-level folder the local disk lacks, unless said otherwise), and every other case is not
        characterised (Tier U): a path the local disk also holds loads where an unscoped sentence said it raised or
        loaded nothing. So each sentence of PARITY.md or DESIGN.md that names a non-local fs with the constructor or
        an argument, and each sentence of the two rows about a non-local fs that names `load_file`, `load_folder` or
        the builtin `open`, and states an outcome, names that filesystem or the cases tested or stated, or says the
        rest is not characterised; a sentence that opens with "That", "This", "It" or "There" is read with the one
        before it, which it continues. The scope sentences themselves are held verbatim, since a rewording that
        drops "non-local" (or the words a row is found by) would leave the sentences after them unchecked; the U
        clauses and DESIGN.md's Tier U bullet are there."""
        sentence_end = re.compile(r"(?<=\.)\s+(?=[A-Z`])")
        continues = re.compile(r"(?:That|This|It|There)\b")
        outcome = re.compile(r"\b(?:loads?|raises?|not found|nothing)\b")
        scoped = re.compile(
            r"`MemoryFileSystem`|\bnot characterised\b|\bcases? (?:tested|stated|section 3 tests)\b"
            r"|\bcases? (?:its|the constructor) row states\b|\bwhere its row says so\b"
        )
        rows = ("| `Environment.fs` |", "| a name read through a non-local `fs` |")

        def sentences(cell):
            joined = []
            for sentence in sentence_end.split(cell):
                if joined and continues.match(sentence):
                    joined[-1] += " " + sentence
                else:
                    joined.append(sentence)
            return joined

        def unscoped(line):
            found = []
            for cell in line.split(" | "):
                for sentence in sentences(cell):
                    about_constructor = re.search(r"non-local", sentence) and re.search(
                        r"\b(?:constructor|argument)\b", sentence
                    )
                    about_rows = line.startswith(rows) and re.search(
                        r"`load_file`|`load_folder`|builtin `open`", sentence
                    )
                    if (about_constructor or about_rows) and outcome.search(sentence):
                        found.append((sentence, bool(scoped.search(sentence))))
            return found

        for line, flagged in (  # the round-14, -15 and -16 wordings, and today's
            ("| x | with a non-local `fs`, a file argument other than a zip/apk, or a folder, is not found |", True),
            ("| x | with a non-local `fs`, any other file argument raises naming its folder, or loads nothing |", True),
            ("| `Environment.fs` | `load_folder` opens it with the builtin `open`, which raises. |", True),
            # Round 17's put-back of round 16's wording: the scope moved out of the sentence that states the raise.
            (
                "| `Environment.fs` | `load_folder` opens what it finds with the builtin `open`. That raises "
                "`FileNotFoundError` naming the file. A path. |",
                True,
            ),
            # Round 16's `load_file` wording, which a local `{name}.split0` or a case variant the walk resolves makes
            # false: it loads, or raises naming the variant.
            (
                "| a name read through a non-local `fs` | `load_file` tests a name and walks `path` on the local disk, "
                "so a name the local disk lacks is not found even when `fs` holds it: `load_file` returns `None`. |",
                True,
            ),
            ("| `Environment.fs` | In the cases tested, `load_file` raises `FileNotFoundError`. It loads. |", False),
            ("| x | through a non-local `fs`, a `MemoryFileSystem` holds each argument: `x.assets` raises |", False),
            ("| x | Every other constructor argument through a non-local `fs` is not characterised (Tier U). |", False),
        ):
            self.assertEqual(any(not ok for _, ok in unscoped(line)), flagged, line)
        root = HERE.parent.parent
        parity = (root / "PARITY.md").read_text(encoding="utf-8")
        design = (root / "docs" / "DESIGN.md").read_text(encoding="utf-8")
        seen = 0
        for name, text in (("PARITY.md", parity), ("DESIGN.md", design)):
            for line in text.splitlines():
                for sentence, ok in unscoped(line):
                    seen += 1
                    with self.subTest(name, sentence=sentence[:80]):
                        self.assertTrue(ok)
        self.assertGreaterEqual(seen, 5)
        constructor = next(line for line in parity.splitlines() if line.startswith("| `Environment` constructor |"))
        self.assertIn("U for every other constructor argument through a non-local `fs` |", constructor)
        self.assertIn(
            "Every other constructor argument through a non-local `fs` is not characterised (Tier U).", constructor
        )
        self.assertIn(
            "Through a non-local `fs` the row states only the cases its test runs: a `MemoryFileSystem` holds each "
            "argument, at an absolute path under a top-level folder the local disk lacks unless said otherwise "
            "(`Environment.fs`). There ",
            constructor,
        )
        fs_row = next(line for line in parity.splitlines() if line.startswith(rows[0]))
        for sentence in (
            "In the cases tested, with a non-local `fs` a name is not found even when `fs` holds it: ",
            "In two more cases tested a test on the local disk sends the read to `fs`: ",
            "Any other name `load_file` reads through a non-local `fs` is not characterised (Tier U). `load_folder` "
            "walks `fs` but opens what it finds with the builtin `open` (`load_files`, `:75-81`). In the case tested "
            "(a file under a top-level folder the local disk lacks) that raises `FileNotFoundError` naming the file; "
            "any other file it opens with `open` is not characterised (Tier U).",
        ):
            self.assertIn(sentence, fs_row)
        non_local = next(line for line in parity.splitlines() if line.startswith(rows[1]))
        self.assertIn("In the cases section 3 tests, a name is not found even when `fs` holds it: ", non_local)
        self.assertIn(
            "section 3 leaves the other cases of `load_file`, `load_folder` and the constructor not", non_local
        )
        unsupported = design.split(f"- **{UNSUPPORTED_RULE}.**", 1)[1].split("\n- **")[0]
        self.assertRegex(
            unsupported, r"\n  - a name read through a non-local fsspec filesystem in a case [^\n]*not characterised"
        )

    def test_a_local_split0_sends_the_read_to_its_parts(self):
        """Through a non-local fs, where the local disk lacks the name and holds `{name}.split0`, `load_file` reads
        the parts through `fs`, whatever `fs` holds under the name, which it never reads
        (`non_local_load_file_follows_local_split_parts_and_case_variants`). So both rows about a non-local fs state
        that order, the 19.2 row in its UnityPy, tier and unityrs cells, none reads the name first, and that case is
        not among those where a name is not found; DESIGN.md's Tier C gloss names only the cases section 3 gives
        `None` for, a listable `path` that holds the part or a case variant not among them."""
        root = HERE.parent.parent
        lines = (root / "PARITY.md").read_text(encoding="utf-8").splitlines()
        fs_row = next(line for line in lines if line.startswith("| `Environment.fs` |"))
        non_local = next(line for line in lines if line.startswith("| a name read through a non-local `fs` |"))
        unitypy, tier, unityrs = non_local.split(" | ")[1:4]
        held = "whatever `fs` holds under the name"
        self.assertIn(f"{held}, which it never reads (`fs` holding the name too, in a case tested)", fs_row)
        self.assertIn(f"{held}, which is never read;", unitypy)
        self.assertTrue(
            tier.startswith(f"A where a local `{{name}}.split0` sends the read to its parts through `fs`, {held};")
        )
        self.assertTrue(
            unityrs.startswith(
                "where the local disk lacks the name, as given and joined to `path`, and holds `{name}.split0`, reads "
                f"the parts through `fs`, {held}, as UnityPy does;"
            ),
            unityrs[:80],
        )
        for cell in (fs_row, unitypy, unityrs):
            self.assertNotIn("where it is missing there", cell)
        miss = unitypy.split("a name is not found even when `fs` holds it: ", 1)[1].split(". ", 1)[0]
        self.assertNotIn("split0", miss)
        design = (root / "docs" / "DESIGN.md").read_text(encoding="utf-8")
        lead = "  - a name read through a non-local fsspec filesystem where `load_file` returns `None`"
        gloss = next(line for line in design.splitlines() if line.startswith(lead))
        none = "an empty local folder, or `/` given a name under a top-level folder the local disk lacks"
        self.assertIn(f"returns `None` where `path` is {none}.", fs_row)
        self.assertIn(f"in the cases stated in `PARITY.md` section 3: `Environment.path` {none}, ", gloss)
        self.assertNotIn("can list `Environment.path`", gloss)

    def test_zero_size_writes_are_stated_off_switch(self):
        """Section 19.1 states a zero-size `set_image` only for a texture that is not Switch-swizzled, the cases
        `zero_size_images_crash_etcpak` runs; on a Switch-swizzled texture the padding to whole GOB blocks raises
        where the row once said 0 bytes were written, and the case is not characterised (Tier U). So every cell of
        PARITY.md or DESIGN.md that says what a zero-size write does (writes 0 bytes or an empty `image_data`, or
        sends a zero-size image to Tier C (writing)) names that scope, and the Switch-swizzled case is Tier U in the
        19.1 row, in `Texture2D.set_image`'s tier and in DESIGN.md's Tier U list."""
        zero_write = re.compile(
            r"\bwrites? 0 bytes\b|\bwrites `image_data` empty\b|C \(writing\) for a zero-size image"
            r"|zero-size image[^;|]*\b(?:fails to write|C \(writing\))"
        )
        root = HERE.parent.parent
        parity = (root / "PARITY.md").read_text(encoding="utf-8")
        design = (root / "docs" / "DESIGN.md").read_text(encoding="utf-8")
        seen = 0
        for name, text in (("PARITY.md", parity), ("DESIGN.md", design)):
            for line in text.splitlines():
                for cell in line.split(" | "):
                    if zero_write.search(cell):
                        seen += 1
                        with self.subTest(name, cell=cell[:80]):
                            self.assertIn("not Switch-swizzled", cell)
        self.assertGreaterEqual(seen, 4)
        row = next(line for line in parity.splitlines() if line.startswith("| `set_image` of a zero-size image ("))
        unitypy, unityrs = row.split(" | ")[1:3]
        self.assertTrue(unitypy.startswith("Stated for a texture that is not Switch-swizzled"), unitypy[:80])
        self.assertIn(
            "On a Switch-swizzled texture, what `set_image` of a zero-size image does is not characterised", unitypy
        )
        self.assertTrue(unityrs.startswith("On a texture that is not Switch-swizzled,"), unityrs[:80])
        self.assertIn("On a Switch-swizzled texture, Tier U", unityrs)
        set_image = next(
            line for line in parity.splitlines() if line.startswith("| `Texture2D.set_image` | the target")
        )
        self.assertIn("U for a zero-size image on a Switch-swizzled texture", set_image)
        unsupported = design.split(f"- **{UNSUPPORTED_RULE}.**", 1)[1].split("\n- **")[0]
        self.assertRegex(
            unsupported, r"\n  - `set_image` of a zero-size image on a Switch-swizzled texture: [^\n]*not characterised"
        )

    WRITER = re.compile(r"set_image|image_to_texture2d|\bsave\b|save_fs|save_typetree|get_writeable_cab|set_raw_data")

    def test_a_failure_to_write_is_tier_c_writing(self):
        """DESIGN.md section 3, Writing: where UnityPy raises on a valid write, the case is Tier C (writing), never
        Tier B. So no Tier B bullet of DESIGN.md names a writer unless it sends that writer to Tier C (writing), no
        PARITY.md tier clause that starts with B names one or sits in a writer's row, and the two cases that once
        stood in Tier B have their Tier C (writing) rows."""
        root = HERE.parent.parent
        design = (root / "docs" / "DESIGN.md").read_text(encoding="utf-8")
        reading = design.split("**Reading**", 1)[1].split("**Writing**", 1)[0]
        tier = None
        for line in reading.splitlines():
            if line.startswith("- **"):
                tier = line[4:10]
            elif tier == "Tier B" and line.startswith("  - ") and self.WRITER.search(line):
                with self.subTest("DESIGN.md Tier B", bullet=line[4:70]):
                    self.assertIn("is Tier C (writing)", line)
        writing = design.split("| Tier C (writing) | acceptance |", 1)[1].split("\n\n", 1)[0]
        for lead in (
            "| `set_image` from an image whose mode Pillow cannot pack",
            "| `Environment.save` of a file keyed by a stream `name` that is not a `str`",
            "| `BundleFile.save` of a UnityWeb or UnityRaw bundle of a format above 3 but 6",
        ):
            self.assertIn("\n" + lead, writing)
        # A bundle save is byte-equal to UnityPy's only in the UnityPy-layout mode (the File / bundle save rule), so
        # a Tier C (writing) bundle row that asks for byte-equality names that mode.
        for row in writing.splitlines():
            if row.startswith("| `BundleFile.save`") and "byte-equal" in row:
                with self.subTest("DESIGN.md Tier C (writing)", row=row[:70]):
                    self.assertIn("UnityPy-layout mode", row)
        rows = 0
        for line in (root / "PARITY.md").read_text(encoding="utf-8").splitlines():
            cells = [cell.strip() for cell in line.strip("|").split(" | ")]
            if not line.startswith("| ") or line.startswith("|---") or len(cells) < 6:
                continue
            item, clauses = cells[0], [clause.strip() for clause in cells[2].split(";")]
            for clause in clauses:
                if re.match(r"B\b", clause):
                    rows += 1
                    with self.subTest("PARITY.md", item=item[:60], clause=clause[:60]):
                        self.assertNotRegex(clause, self.WRITER)
                        self.assertNotIn("cannot pack", clause)
                        self.assertNotRegex(item, self.WRITER)
        self.assertGreater(rows, 20)

    def test_every_tier_has_a_design_rule(self):
        """Every tier PARITY.md section 0 defines, but `—`, which marks no behaviour of its own, is decided by a
        bullet of DESIGN.md section 3's Reading list."""
        root = HERE.parent.parent
        parity = (root / "PARITY.md").read_text(encoding="utf-8")
        table = parity.split("| tier | meaning |", 1)[1].split("\n\n", 1)[0]
        tiers = [row.split(" | ", 1)[0][2:] for row in table.splitlines()[2:]]
        self.assertEqual(tiers, ["A", "B", "C", "Tol", "HEAD", "U", "gap", "A, C (or A; C for …)", "—"])
        design = (root / "docs" / "DESIGN.md").read_text(encoding="utf-8")
        reading = design.split("**Reading**", 1)[1].split("**Writing**", 1)[0]
        bullets = [lead.rstrip(".:") for lead in re.findall(r"^- \*\*(.+?)\*\*", reading, flags=re.M)]
        rule = {
            "A": "Tier A: bit-exact",
            "B": "Tier B: UnityPy raises on a valid request",
            "C": "Tier C: UnityPy is wrong",
            "Tol": "Tolerance",
            "HEAD": "HEAD tier",
            "U": UNSUPPORTED_RULE,
            "gap": "Gap",
        }
        for tier in tiers[:7]:
            with self.subTest(tier=tier):
                self.assertIn(rule[tier], bullets)

    def test_tol_metrics_are_stated_or_said_to_be_planned(self):
        """Both Tol definitions say each row states a metric and threshold. Until every Tol row does (a row names a
        `threshold`), both must say the rows are to state them in the phase each row's unityrs side lands in (its
        status's `impl Pn`: P1 for the Sprite affine path, P3 for audio) and that none does yet, not that each has
        one."""
        root = HERE.parent.parent
        parity = (root / "PARITY.md").read_text(encoding="utf-8")
        design = (root / "docs" / "DESIGN.md").read_text(encoding="utf-8")
        tol = []
        for header, rows in inventory.tables(parity):
            if "tier" in header and header[0] != "tier":
                tol += [cells for cells in rows if re.search(r"\bTol\b", cells[header.index("tier")])]
        self.assertGreaterEqual(len(tol), 8)
        stated = [cells for cells in tol if "threshold" in " ".join(cells)]
        definitions = [
            next(line for line in parity.splitlines() if line.startswith("| Tol | ")),
            next(line for line in design.splitlines() if line.startswith("- **Tolerance:**")),
        ]
        # The phase each Tol row's unityrs side lands in, by path: the Sprite affine path's rows and the audio rows.
        phases = {}
        for cells in tol:
            path = "the Sprite affine path" if "Sprite" in cells[0] else "audio"
            phases.setdefault(path, set()).update(re.findall(r"\bimpl (P\d)\b", cells[-1]))
        self.assertEqual(phases, {"the Sprite affine path": {"P1"}, "audio": {"P3"}})
        when = "(P1 for the Sprite affine path, P3 for audio)"
        if len(stated) < len(tol):
            for text in definitions:
                with self.subTest(text[:40]):
                    self.assertIn(f"when unityrs's side lands {when}", text)
                    self.assertRegex(text, r"\b(?:no row states them|none is stated) yet\b")
                    self.assertNotRegex(text, r"(?i)\b(?:has|within) a stated metric\b")

    def test_module_level_data_is_tier_a_for_loop_variables(self):
        """DESIGN.md section 3's Tier A rule for module-level data covers the variables a module-level loop leaves
        bound, which section 23 files as A; the other module-level values take their own rows' tiers (a constant
        nothing reads as behaviour is `—`). So DESIGN's sentence that cites section 23 is about a loop's variables,
        each section 23 row of a loop's variables is A and cites the test that pins them, and no `—` row is one."""
        root = HERE.parent.parent
        design = (root / "docs" / "DESIGN.md").read_text(encoding="utf-8")
        tier_a = design.split("- **Tier A: bit-exact**", 1)[1].split("\n- **", 1)[0]
        cites = [s for s in re.split(r"(?<=\.) ", tier_a) if "`PARITY.md` section 23" in s]
        self.assertEqual(len(cites), 1, cites)
        self.assertRegex(cites[0], r"^So is a variable a module-level loop leaves bound\b")
        parity = (root / "PARITY.md").read_text(encoding="utf-8")
        section = parity.split("\n## 23. Module-level data\n", 1)[1].split("\n## ", 1)[0]
        loops = 0
        for header, rows in inventory.tables(section):
            for cells in rows:
                about, tier = cells[header.index("UnityPy 1.25.2")], cells[header.index("tier")]
                with self.subTest(item=cells[0][:60]):
                    if about.startswith("the variables of the module-level loop"):
                        loops += 1
                        self.assertEqual(tier, "A")
                        test = cells[header.index("test")]
                        self.assertIn("`test_parity_rows.py` module_loop_variables_keep_the_last_pass", test)
                    elif tier == "—":
                        self.assertNotIn("module-level loop", about)
        self.assertEqual(loops, 2)

    def test_head_dicts_are_a_target_while_their_fixtures_are_planned(self):
        """Section 20's reading rows name the Unity 6000.4/6000.5 fixtures as planned, so no comparison with HEAD's
        dicts exists: every sentence or table cell of PARITY.md and DESIGN.md that speaks of dicts and HEAD (or those
        fixtures) calls that equality a target, or says no comparison has been run, and none states it as done."""
        root = HERE.parent.parent
        parity = (root / "PARITY.md").read_text(encoding="utf-8")
        section = parity.split("\n## 20. HEAD tier\n", 1)[1].split("\n## ", 1)[0]
        planned = [
            cells[0]
            for header, rows in inventory.tables(section)
            for cells in rows
            if "planned 6000.4/6000.5 fixtures" in cells[header.index("test")]
        ]
        if not planned:
            self.skipTest("section 20 names no planned HEAD fixture: the reading rows are measured")
        speaking = 0
        for name, text in (("PARITY.md", parity), ("DESIGN.md", (root / "docs" / "DESIGN.md").read_text("utf-8"))):
            for sentence in re.split(r"(?<=\.)\s+|\s*\|\s*|\n", text):
                if "dicts" in sentence and ("HEAD" in sentence or "6000.4/6000.5" in sentence):
                    speaking += 1
                    with self.subTest(name, sentence=sentence[:80]):
                        self.assertTrue(
                            "target" in sentence or "No comparison" in sentence, "HEAD's dicts stated as measured"
                        )
        self.assertGreaterEqual(speaking, 3)


@unittest.skipUnless(ORACLE, "needs UnityPy 1.25.2 (the oracle image)")
class Rows(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="unityrs-rows-"))
        self.addCleanup(lambda: __import__("shutil").rmtree(self.tmp))

    # Section 23: a module-level loop leaves its variables bound at the top level, holding the last pass's values.
    def test_module_loop_variables_keep_the_last_pass(self):
        reader_module = sys.modules["UnityPy.streams.EndianBinaryReader"]
        self.assertIs(Texture2DConverter.tf, TextureFormat.RGBA64_SIGNED)
        self.assertEqual((Texture2DConverter.split, Texture2DConverter.block_size), (["12", "12"], None))
        self.assertEqual(
            (
                reader_module.endian_s,
                reader_module.reader_type_name,
                reader_module.struct_type_char,
                reader_module.func_name,
                reader_module.struct.format,
            ),
            (">", "double", "d", "read_double", ">d"),
        )
        # The two reader functions each pass defines: the last pass's, the `>` read_double ones themselves.
        self.assertIs(reader_module.memory_read_func, reader_module.MEMORY_FUNCTIONS[">"]["read_double"])
        self.assertIs(reader_module.stream_read_func, reader_module.STREAM_FUNCTIONS[">"]["read_double"])
        one = bytes.fromhex("3ff0000000000000")
        self.assertEqual(reader_module.memory_read_func(EndianBinaryReader(one, endian="<")), 1.0)
        self.assertEqual(reader_module.stream_read_func(EndianBinaryReader(io.BytesIO(one), endian="<")), 1.0)
        # Every public name a module-level loop of UnityPy leaves bound (an AST scan: targets, assignments and
        # definitions in a top-level `for` or `while`, outside a function or class body) sits on a section 23 row
        # of a loop's variables, and only those.
        import ast

        def bound(node):
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
                return {node.name}
            if isinstance(node, ast.Name) and isinstance(node.ctx, ast.Store):
                return {node.id}
            return set().union(*(bound(child) for child in ast.iter_child_nodes(node)))

        def loops(body):
            for node in body:
                if isinstance(node, (ast.For, ast.AsyncFor, ast.While)):
                    yield node
                elif not isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
                    for field in ("body", "orelse", "finalbody", "handlers", "cases"):
                        yield from loops(getattr(node, field, []))

        package = Path(UnityPy.__file__).resolve().parent
        scanned = set()
        for path in sorted(package.rglob("*.py")):
            for loop in loops(ast.parse(path.read_text(encoding="utf-8")).body):
                scanned |= {f"{path.stem}.{name}" for name in bound(loop) if not name.startswith("_")}
        parity = inventory.DEFAULT_PARITY.read_text(encoding="utf-8")
        section = parity.split("\n## 23. Module-level data\n", 1)[1].split("\n## ", 1)[0]
        listed = set()
        for header, rows in inventory.tables(section):
            for cells in rows:
                if cells[header.index("UnityPy 1.25.2")].startswith("the variables of the module-level loop"):
                    listed |= set(re.findall(r"`([\w.]+)`", cells[0]))
        self.assertEqual(listed, scanned)
        self.assertEqual(len(scanned), 10)
        # No row elsewhere files one of them as `—`.
        for header, rows in inventory.tables(parity.replace(section, "")):
            for cells in rows if "tier" in header else ():
                if cells[header.index("tier")] == "—":
                    self.assertFalse(scanned & set(re.findall(r"`([\w.]+)`", cells[0])), cells[0][:60])

    # Sections 2, 2.1: the package's aliases and every re-export section 2.1 lists.
    def test_package_aliases_and_reexports(self):
        from UnityPy.helpers import ArchiveStorageManager

        self.assertIs(UnityPy.load, UnityPy.Environment)
        self.assertIs(UnityPy.AssetsManager, UnityPy.Environment)
        self.assertIs(UnityPy.set_assetbundle_decrypt_key, ArchiveStorageManager.set_assetbundle_decrypt_key)
        text = (HERE.parent.parent / "PARITY.md").read_text(encoding="utf-8")
        rows = next(body for header, body in inventory.tables(text) if header == ["package", "source", "members"])
        self.assertEqual(len(rows), 31)
        for package, _, members in rows:
            module = __import__(package.strip("`"), fromlist=["_"])
            for member in re.findall(r"`([A-Za-z_0-9]+)`", members):
                with self.subTest(package, member=member):
                    value = getattr(module, member)
                    self.assertFalse(isinstance(value, type(module)), "a re-export, not a submodule")
        self.assertIsInstance(UnityPy.files.BundleFile, type)
        self.assertTrue(callable(UnityPy.cli.update_tpk))
        # helpers and export re-export only their own submodules; export leaves ShaderConverter out.
        self.assertNotIn("ShaderConverter", UnityPy.export.__all__)
        self.assertEqual(len(UnityPy.helpers.__all__), 6)


if __name__ == "__main__":
    unittest.main()
