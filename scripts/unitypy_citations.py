#!/usr/bin/env python3
"""Every citation of UnityPy's source in the repository names lines that exist and are a whole span.

    python3 scripts/unitypy_citations.py [ROOT] [--unitypy DIR] [--boost DIR]

A citation is `<path>:<line>` or `<path>:<first>-<last>`, where the path ends
in .py, .pyi, .cpp or .hpp, in a text file of the repository (vendored
UnityPy files, build output and caches aside). It is UnityPy's when its path
names a file of UnityPy 1.25.2's package or of UnityPyBoost's C++ sources: a
path from the package's root (`environment.py`, `files/BundleFile.py`, with
or without a leading `UnityPy/`; `UnityPyBoost/TypeTreeHelper.cpp`), or the
end of exactly one file's path (`TypeTreeHelper.py`, `TypeTreeHelper.cpp`).
Any other path (CPython's `Lib/gzip.py`, a file of this repository) is not
checked, unless it is written as UnityPy's (below).

A bare line is `:<line>` or `:<first>-<last>` in backticks, opening a comment
(`// :53-54`), or after a space or an opening parenthesis and before a space,
a `,`, `.`, `;`, `:` or `)` or the line's end (`, :<line>`, `(:<line>)`). It
cites the file named last before it in its paragraph: a blank line ends a
paragraph, and so does a heading in Markdown, while a list item and a
Markdown table row are each one of their own. A UnityPy citation, or a
UnityPy file named with no line, names a file; so does a citation of any
other file (`Lib/gzip.py:120`, `stream.rs:31`), whose bare lines are not
checked. A bare line its paragraph names no file for cites its default: in
Markdown, the file a sentence "Lines are in `<file>`" names for its section
(up to the next heading of that section's level or above); in a source, the
one UnityPy file its `Ported from UnityPy 1.25.2` header names.

A path is written as UnityPy's when it starts with `UnityPy/` or
`UnityPyBoost/` or its first folder is one of UnityPy's package folders
(`helpers/`, `files/`, ...). A citation of such a path that names no file of
UnityPy 1.25.2, nor of this repository (`tools/parity/compare.py:12`), is
refused, not taken for another project's file; so is a "Lines are in" sentence
naming anything but one UnityPy file, a path a `UnityPy 1.25.2` header names
that is no UnityPy file, and a bare line after a path written as UnityPy's
that names no file. A line of a UnityPy file cited in words next to its name
(`<file>` line <N>, `<file>`'s lines <N>-<M>, line <N> of `<file>`) is refused:
write `<file>:<N>`. No other wording of a line is read, and a path with no
folder that ends no UnityPy file's path (`fmod.py:61`) is another project's.

A citation is of UnityPy git 417d595 (the HEAD oracle, PARITY.md section 20)
when `HEAD` or `417d595` comes right before its path, when its path runs
through a `head_417d595/` folder, or when it is in a Markdown table's column
whose header names HEAD or 417d595, and so are the bare lines after it (a
bare line in such a table that follows a citation of the other column's
revision is refused: write the path). One of a file
tools/parity/tests/head_417d595/ keeps is checked against that copy (its git
blob id is pinned below); one of any other file is counted.

It fails:
- on a citation whose path ends more than one UnityPy file's (`BundleFile.py`:
  files/ and enums/), which could be either: write the path from the root;
- on a citation of a line past its file's end, of line 0, or of a range that
  runs backwards;
- on a citation that is not a whole span of the Python file it cites: one
  that starts or ends on a blank line; a range that starts inside a
  statement begun above it and runs past that statement's end; one that
  starts where a statement starts and ends inside a statement that goes on
  below it (a range within one statement, such as some entries of a table,
  is a span); one that ends on a decorator, or on the header of a `def`,
  `class`, `else`, `try`, `except`, `finally` or `with` that begins after the
  range does, without what it heads (an `if`, `elif`, `for` or `while` header
  holds a condition, which a range may end on); one that holds part of a
  function (a `def` with its decorators) and lines outside it: a range holds
  a whole function, lies within one, or misses it. Reviews 11 and 12 found
  `Length` in EndianBinaryReader.py cited by a range that stopped on its
  `def`, and `read` by one that started on the blank line above it; review 13
  found `set_version` in SerializedFile.py cited by one whose first lines end
  `load_dependencies`;
- on a bare line with neither a file before it in its paragraph nor a
  default, and on one whose paragraph names a file other than its default,
  which a reader could take for a line of either: write the path;
- on a path that names no UnityPy file where it must name one (a "Lines are
  in" sentence, a `UnityPy 1.25.2` header) or is written as UnityPy's (a
  citation, a bare line's paragraph), and on a line cited in words beside a
  UnityPy file's name;
- on a file under a crate's src/ (test modules aside: `tests.rs`, `*_tests.rs`
  and `tests/` folders) that cites UnityPy and has no header line naming
  UnityPy 1.25.2 under MIT, `// Ported from UnityPy 1.25.2 (MIT): ...` or, for
  a file generated from that source, `// Generated by <generator> from UnityPy
  1.25.2 (MIT): ...`, which about.hbs says every such file has.

UnityPy's package is the installed one (which must be 1.25.2) or `--unitypy
DIR`, and either is refused unless its .py and .pyi files are UnityPy
1.25.2's byte for byte (PACKAGE pins their digest), so a mistyped or empty
folder is an error, not a pass. UnityPyBoost's sources are in UnityPy's
sdist, not in the wheel the oracle installs, so their line counts and blank
lines are pinned below (BOOST) with each file's sha256, and `--boost DIR`
checks a copy of the sources against the pins. A UnityPyBoost citation is
held to the blank-line rule only: the statement rules are Python's.

A whole span need not be the right one: a citation a few lines off that lands
on another whole statement passes, and so does one of a function of the same
name in another class (review 13: `EndianBinaryReader.read_aligned_string`
cited at the memoryview reader's override). Requiring a cited range to fall
within, or hold, the definition of the name a comment gives with it is not
a check: even in the narrowest form, `` `name` (`path:lines`) ``, most of
the citations whose range misses the name's definition are right, citing a
caller, an alias, an attribute or a module table.

Exit status: 0 when every citation passes (a summary on stdout), 1 when one
does not (each problem on its own line on stderr), 2 for a usage error, a
package that is not UnityPy 1.25.2's, or a HEAD copy that is not 417d595's.
Standard library only.
"""

from __future__ import annotations

import argparse
import ast
import hashlib
import importlib.metadata
import importlib.util
import io
import os
import re
import sys
import tokenize
from pathlib import Path
from typing import NamedTuple

sys.path.insert(0, str(Path(__file__).resolve().parent))

from third_party_licenses_check import source_headers  # noqa: E402

ROOT = Path(__file__).resolve().parents[1]
VERSION = "1.25.2"
SOURCE = "UnityPy 1.25.2"
# The sha256 of the lines "<path from the package root> <sha256>\n" of UnityPy 1.25.2's 87 .py and .pyi files,
# sorted by path (`package_digest`); each file's sha256 is the one the wheel's RECORD gives it.
PACKAGE = "b90eea2a46d82210de1e188c3351e9fa5e0aa5c7365c926aae07de5f6fcac397"
# UnityPyBoost's sources in UnityPy 1.25.2's sdist: (lines, sha256, the numbers of its blank lines).
BOOST = {
    "UnityPyBoost/ArchiveStorageDecryptor.cpp": (
        90,
        "f387d5093e7f504ec5735d11e67f447da3ec881b70206c54aba10954bb4899ae",
        "2 5 17 21 25 35 37 51 54 60 67 75 78 83 87 90",
    ),
    "UnityPyBoost/ArchiveStorageDecryptor.hpp": (
        5,
        "4b669249b28603ec64130fce258a35ced7f7adaf16ae7509ee47fe9daa5f301e",
        "4",
    ),
    "UnityPyBoost/Mesh.cpp": (
        210,
        "ee9542139cb02c1ab8a417192227784c6ee5de3867b9d5761daebf427ad65c8d",
        "4 6 22 35 44 46 48 57 65 76 89 97 100 106",
    ),
    "UnityPyBoost/Mesh.hpp": (
        5,
        "7e081fef7e5e9b95881cc7bba8c284aa1d13f025167a3f3b6a4b30bdbe18ec12",
        "4",
    ),
    "UnityPyBoost/TypeTreeHelper.cpp": (
        1286,
        "9749bd26f2983ef73a5138c04513317eaeb7b4f2a3631545d23ebc10039db17e",
        (
            "7 10 12 19 27 36 42 48 51 57 63 66 73 85 102 112 127 137 154 159 205 210 265 287 306 324 333 352 361 "
            "364 390 393 438 443 446 448 457 470 475 500 507 517 537 542 575 584 599 607 626 636 645 662 667 674 "
            "679 682 685 700 706 779 818 827 837 876 881 884 890 938 941 950 961 971 974 981 984 989 992 1002 "
            "1033 1035 1044 1046 1054 1057 1074 1104 1111 1129 1145 1160 1178 1185 1200 1207 1213 1217 1223 1233 "
            "1249 1271"
        ),
    ),
    "UnityPyBoost/TypeTreeHelper.hpp": (
        55,
        "7ca69cd47ce2a71870bc82c3aae7600ad573d76acbd862e3053d664f05ec7543",
        "6 30 52 54",
    ),
    "UnityPyBoost/UnityPyBoost.cpp": (
        47,
        "80f005e91ccb786c6705a9e50c19c3bfaf19444f40a65d24b82094c0f0417253",
        "6 8 27 40",
    ),
    "UnityPyBoost/swap.hpp": (
        78,
        "48d0ef17909c5c6b07d6b3c50750b3828b9472c2d872f51c4ec36f5699bdf719",
        "52 62",
    ),
}
# The other UnityPy revision the repository cites, and the files of it that HEAD_DIR keeps, by their path in that
# revision's package: (the copy's name, its git blob id at 417d595, as tools/parity/tests/head_oracle.py pins it).
HEAD = "417d595"
HEAD_DIR = "tools/parity/tests/head_417d595/"
HEAD_FILES = {
    "files/BundleFile.py": ("BundleFile.py", "e589be5f5cabedeb24ab20c15ea98ed8a1885320"),
    "helpers/CompressionHelper.py": ("CompressionHelper.py", "922df19a5cb83a1913725da376a942606ad3fc62"),
}
PATH = r"(?:[\w-]+/)*[\w-]+\.(?:py|pyi|cpp|hpp)"
CITATION = re.compile(rf"(?<![\w./-])(?P<path>{PATH}):(?P<first>\d+)(?:-(?P<last>\d+))?(?![\d-])")
# A citation of any file (`Lib/gzip.py:120`, `stream.rs:31`), and a UnityPy-like path with no line.
ANY_CITATION = re.compile(r"(?<![\w./-])(?:[\w.-]+/)*[\w-]+\.[A-Za-z][A-Za-z0-9]{0,4}:\d+(?:-\d+)?")
MENTION = re.compile(rf"(?<![\w./-])(?P<path>{PATH})(?![\w-]|:\d)")
BARE = re.compile(r"(?<=`):(?P<first>\d+)(?:-(?P<last>\d+))?(?=`)")
BARE_COMMENT = re.compile(r"^\s*(?://[/!]?|#)\s*:(?P<first>\d+)(?:-(?P<last>\d+))?(?=[\s:,.;)]|$)")
# A bare line in prose, after a space or an opening parenthesis: `, :295`, `(:223-225)`.
BARE_PROSE = re.compile(r"(?:(?<=\s)|(?<=\()):(?P<first>\d+)(?:-(?P<last>\d+))?(?=[\s:,.;)]|$)")
# A line cited in words next to a file's name: `<file>` line <N>, `<file>`'s lines <N>-<M>, line <N> of `<file>`.
WORDS_AFTER = re.compile(r"^`?(?:'s)?,? lines? \d")
WORDS_BEFORE = re.compile(r"\blines? \d+(?:-\d+)?(?:(?:,| and| or) \d+(?:-\d+)?)* (?:of|in) `?$")
REVISION_BEFORE = re.compile(rf"(?<!\w)(?:HEAD|{HEAD})\s+`?$")
REVISION_NAMED = re.compile(rf"(?<!\w)(?:HEAD|{HEAD})(?!\w)")
DEFAULT = re.compile(rf"\blines are in `(?P<path>{PATH})`(?: and `(?P<also>{PATH})`)?", re.IGNORECASE)
HEADING = re.compile(r"^(#+) ")
COMMENT_MARK = re.compile(r"^\s*(?://[!/]?|#!?|\*(?!\S))")
LIST_ITEM = re.compile(r"(?:[-*+]|\d+\.) ")
BODY_HEADERS = {"def", "async", "class", "else", "try", "except", "finally", "with"}
TABLE_RULE = re.compile(r"\s*\|[\s:|-]+")
TEXT_SUFFIXES = {".rs", ".py", ".pyi", ".md", ".hbs", ".toml", ".yml", ".yaml", ".sh", ".txt", ""}
SKIP_DIRS = {".git", "target", "__pycache__", ".wt", "node_modules", ".venv"}
# The copies of another revision's files, which cite nothing.
SKIP_PATHS = (HEAD_DIR,)


class Report(NamedTuple):
    problems: list[str]
    checked: int
    bare: int
    boost: int
    files: int
    head_checked: int
    head_unchecked: int
    ported: int


class Shape(NamedTuple):
    """Where each line of a source stands in its statements (1-based; index 0 unused)."""

    blank: list[bool]
    # The line continues a statement that began on an earlier line.
    inside: list[bool]
    # The statement the line is part of goes on past it.
    open: list[bool]
    # The last line of a decorator, or of a header that heads nothing it names (`def`, `class`, `else`, `try`,
    # `except`, `finally`, `with`) -> the line it begins on. An `if`, `elif`, `for` or `while` header is left out:
    # its condition is a rule a citation may end on.
    header: dict[int, int]
    # Each function (`def`, nested ones too): its first line (its first decorator's), its last, and its name.
    functions: tuple[tuple[int, int, str], ...] = ()


class Package(NamedTuple):
    """Each UnityPy 1.25.2 file's line count and shape, and the same for the HEAD files the repository keeps."""

    lines: dict[str, int]
    shapes: dict[str, Shape]
    head_lines: dict[str, int]
    head_shapes: dict[str, Shape]


class Source(NamedTuple):
    revision: str
    # The UnityPy files the path can name: one, or several when it is ambiguous; () for a file that is not UnityPy's.
    named: tuple[str, ...]
    # A path written as UnityPy's that names no file of it (a mistyped one), which no bare line may follow.
    unknown: str = ""


ANOTHER_FILE = Source("", ())


class Cite(NamedTuple):
    n: int
    text: str
    source: Source
    first: int
    last: int
    bare: bool


def shape(text: str) -> Shape:
    """`text`'s lines as Python's tokenizer reads them."""
    rows = text.splitlines()
    n = len(rows)
    blank = [False] + [not row.strip() for row in rows]
    inside, open_, header = [False] * (n + 1), [False] * (n + 1), {}
    start = first = last = None
    skip = (tokenize.NL, tokenize.COMMENT, tokenize.INDENT, tokenize.DEDENT, tokenize.ENCODING)
    for tok in tokenize.generate_tokens(io.StringIO(text).readline):
        if tok.type in skip:
            continue
        if tok.type in (tokenize.NEWLINE, tokenize.ENDMARKER):
            if start is not None:
                end = min(tok.start[0], n)
                for row in range(start + 1, end + 1):
                    inside[row] = True
                for row in range(start, end):
                    open_[row] = True
                if first.string == "@" or (last.string == ":" and first.string in BODY_HEADERS):
                    header[end] = start
            start = first = last = None
            continue
        if start is None:
            start, first = tok.start[0], tok
        last = tok
    functions = tuple(
        (min([node.lineno] + [d.lineno for d in node.decorator_list]), node.end_lineno, node.name)
        for node in ast.walk(ast.parse(text))
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef))
    )
    return Shape(blank, inside, open_, header, functions)


def blank_shape(rows: list[str]) -> Shape:
    """Only which lines are blank, for a source that is not Python."""
    n = len(rows)
    return Shape([False] + [not row.strip() for row in rows], [False] * (n + 1), [False] * (n + 1), {})


def span_problem(s: Shape, first: int, last: int) -> str | None:
    """Why lines first..last of the source `s` describes are not a whole span, or None when they are."""
    if s.blank[first] or s.blank[last]:
        return f"{'starts' if s.blank[first] else 'ends'} on a blank line"
    if last == first:
        return None
    if s.inside[first] and not all(s.open[first:last]):
        return f"starts inside a statement that begins above line {first} and runs past its end"
    if s.open[last] and not s.inside[first]:
        return f"ends inside a statement that goes on past line {last}"
    if s.header.get(last, first) > first:
        return f"ends on the decorator or header that begins at line {s.header[last]}, without what it heads"
    for start, end, name in s.functions:
        if start < first <= end < last:
            return f"starts inside `{name}` (lines {start}-{end}) and runs past its end"
        if first < start <= last < end:
            return f"ends inside `{name}` (lines {start}-{end}), which begins after the range does"
    return None


def package_digest(unitypy: Path) -> str:
    """The sha256 of one line "<path> <sha256>" for each .py and .pyi file under `unitypy`, sorted by path."""
    entries = []
    for path in sorted(unitypy.rglob("*")):
        if path.suffix in (".py", ".pyi") and "__pycache__" not in path.parts and path.is_file():
            entries.append(f"{path.relative_to(unitypy).as_posix()} {hashlib.sha256(path.read_bytes()).hexdigest()}\n")
    return hashlib.sha256("".join(entries).encode()).hexdigest()


def blob_id(data: bytes) -> str:
    """The git blob id of `data` (`git hash-object`)."""
    return hashlib.sha1(b"blob %d\0" % len(data) + data).hexdigest()


def load_package(unitypy: Path, boost: Path | None, root: Path, digest: str = PACKAGE) -> Package:
    """UnityPy's package at `unitypy`, refused unless its digest is `digest`; UnityPyBoost's pins or the copy at
    `boost`; and the HEAD files under `root`, each refused unless it is 417d595's."""
    if not unitypy.is_dir() or package_digest(unitypy) != digest:
        raise ValueError(
            f"{unitypy} is not UnityPy {VERSION}'s package: its .py and .pyi files are not the pinned ones"
        )
    lines, shapes = {}, {}
    for path in sorted(unitypy.rglob("*")):
        if path.suffix in (".py", ".pyi") and "__pycache__" not in path.parts and path.is_file():
            rel, text = path.relative_to(unitypy).as_posix(), path.read_text(encoding="utf-8")
            lines[rel], shapes[rel] = len(text.splitlines()), shape(text)
    for name, (count, sha, blanks) in BOOST.items():
        pinned = ["x"] * count  # a line of the source as the pins have it: blank, or not
        for n in blanks.split():
            pinned[int(n) - 1] = ""
        if boost is not None:
            data = (boost / Path(name).name).read_bytes()
            if hashlib.sha256(data).hexdigest() != sha:
                raise ValueError(f"{boost / Path(name).name} is not UnityPy {VERSION}'s {name} (sha256 differs)")
            if blank_shape(data.decode("utf-8").splitlines()) != blank_shape(pinned):
                raise ValueError(f"BOOST's line count or blank lines for {name} are not those of its source")
        lines[name], shapes[name] = count, blank_shape(pinned)
    head_lines, head_shapes = {}, {}
    for name, (copy, blob) in HEAD_FILES.items():
        data = (root / HEAD_DIR / copy).read_bytes()
        if blob_id(data) != blob:
            raise ValueError(f"{HEAD_DIR}{copy} is not {name} of UnityPy git {HEAD}: its blob id differs")
        text = data.decode("utf-8")
        head_lines[name], head_shapes[name] = len(text.splitlines()), shape(text)
    return Package(lines, shapes, head_lines, head_shapes)


def resolve(cited: str, lines: dict[str, int]) -> list[str]:
    """The UnityPy files a cited path can name: the one at that path from the root, or each whose path ends with it."""
    path = cited.removeprefix("UnityPy/") if not cited.startswith("UnityPyBoost/") else cited
    if path in lines:
        return [path]
    return [k for k in lines if k.endswith("/" + path)]


def is_test_module(rel: str) -> bool:
    parts = rel.split("/")
    return parts[-1] == "tests.rs" or parts[-1].endswith("_tests.rs") or "tests" in parts[3:-1]


def text_files(root: Path):
    for top, dirs, names in os.walk(root):
        dirs[:] = sorted(d for d in dirs if d not in SKIP_DIRS)
        for name in sorted(names):
            path = Path(top) / name
            rel = path.relative_to(root).as_posix()
            if path.suffix in TEXT_SUFFIXES and not rel.startswith(SKIP_PATHS) and path.is_file():
                try:
                    yield rel, path.read_text(encoding="utf-8")
                except UnicodeDecodeError:
                    continue


def cell_starts(row: str) -> list[int]:
    """Where each cell of a Markdown table row starts: after each `|` outside backticks."""
    out, code = [], False
    for i, c in enumerate(row):
        if c == "`":
            code = not code
        elif c == "|" and not code:
            out.append(i + 1)
    return out


def column(row: str, pos: int) -> int:
    return sum(start <= pos for start in cell_starts(row)) - 1


def source_of(path: str, before: str, lines: dict[str, int], revision: str) -> Source:
    """The UnityPy files `path` (written after `before`) can name, and of which revision."""
    if f"head_{HEAD}/" in path:
        revision, path = HEAD, path.split(f"head_{HEAD}/", 1)[1]
    elif REVISION_BEFORE.search(before):
        revision = HEAD
    return Source(revision, tuple(resolve(path, lines)))


def header_default(text: str, lines: dict[str, int]) -> list[Source] | None:
    """The file a source's bare lines name by default: the one UnityPy file its `Ported from` header names, if one."""
    named = {
        tuple(resolve(m["path"], lines))
        for h in source_headers(text)
        if (h.source, h.generator) == (SOURCE, None)
        for m in MENTION.finditer(h.what)
    } - {()}
    return [Source(VERSION, named.pop())] if len(named) == 1 else None


def written_as_unitypys(path: str, lines: dict[str, int]) -> bool:
    """Whether `path` is written as a path of UnityPy's package: from `UnityPy/` or `UnityPyBoost/`, or through one
    of the package's folders (`helpers/Tpk.py`)."""
    head, _, rest = path.partition("/")
    return bool(rest) and (head in ("UnityPy", "UnityPyBoost") or any(k.startswith(head + "/") for k in lines))


def one_file(path: str, lines: dict[str, int]) -> str | None:
    """Why `path` does not name exactly one UnityPy file, or None when it does."""
    found = resolve(path, lines)
    if not found:
        return f"names no file of UnityPy {VERSION}"
    if len(found) > 1:
        return f"could be any of {', '.join(sorted(found))}; write the path from the root"
    return None


def header_problems(rel: str, text: str, lines: dict[str, int]) -> list[str]:
    """Each path a `UnityPy 1.25.2` header of the file names that is not one UnityPy file."""
    return [
        f"{rel}: its `{SOURCE}` header names {m['path']}, which {why}"
        for h in source_headers(text)
        if h.source == SOURCE
        for m in MENTION.finditer(h.what)
        if (why := one_file(m["path"], lines))
    ]


def citations(
    rel: str, text: str, lines: dict[str, int], exists=lambda path: False
) -> tuple[list[Cite], list[str]]:
    """Each citation of UnityPy in a file, explicit or bare, with the file it names; and each problem a citation
    has before its lines are read: a bare line that names no file, or that a reader could take for a line of
    either of two; a path written as UnityPy's that names no file of it or of the repository (`exists`); a
    section default that is not one UnityPy file; a line cited in words."""
    markdown = rel.endswith(".md")
    cites, problems = [], []
    rows = text.splitlines()

    def unknown(path: str, source: Source) -> bool:
        return not source.named and source.revision == VERSION and written_as_unitypys(path, lines) and not exists(path)

    # The file named last in this paragraph (a Markdown table row is one); the file the section's "Lines are in"
    # (Markdown) or the file's header names, with the heading level that sentence is under.
    current: Source | None = None
    level, default = 0, (None if markdown else (99, header_default(text, lines)))
    revisions: dict[int, str] = {}  # the current table's columns of another revision
    for n, line in enumerate(rows, 1):
        if not (text_ := COMMENT_MARK.sub("", line, count=1).strip()) or LIST_ITEM.match(text_):
            current = None
        if markdown:
            row = line.lstrip().startswith("|")
            if heading := HEADING.match(line):
                level, current = len(heading[1]), None
                if default is not None and level <= default[0]:
                    default = None
            if row:
                current = None
                if n < len(rows) and TABLE_RULE.fullmatch(rows[n]):
                    starts = cell_starts(line)
                    revisions = {
                        i: HEAD for i, s in enumerate(starts) if REVISION_NAMED.search(line[s:].split("|", 1)[0])
                    }
            else:
                revisions = {}
            if m := DEFAULT.search(line):
                named = [m["path"]] + ([m["also"]] if m["also"] else [])
                default = (level, [Source(VERSION, tuple(resolve(p, lines))) for p in named])
                for p in named:
                    if why := one_file(p, lines):
                        problems.append(f'{rel}:{n}: "Lines are in `{p}`" {why}')
        events = [(m.start(), "cite", m) for m in ANY_CITATION.finditer(line)]
        spans = [(m.start(), m.end()) for _, _, m in events]
        events += [
            (m.start(), "mention", m) for m in MENTION.finditer(line) if not any(a <= m.start() < b for a, b in spans)
        ]
        bare = {m.start("first"): m for m in BARE_PROSE.finditer(line)}
        bare |= {m.start("first"): m for m in BARE.finditer(line)}
        if m := BARE_COMMENT.match(line):
            bare[m.start("first")] = m
        events += [(pos, "bare", m) for pos, m in bare.items()]
        for pos, kind, m in sorted(events, key=lambda e: e[0]):
            revision = revisions.get(column(line, pos), VERSION) if revisions else VERSION
            if kind == "cite":
                cite = CITATION.fullmatch(m[0])
                source = source_of(cite["path"], line[:pos], lines, revision) if cite else ANOTHER_FILE
                if cite and unknown(cite["path"], source):
                    problems.append(
                        f"{rel}:{n}: {cite[0]} names no file of UnityPy {VERSION} or of the repository, though its "
                        "path is written as UnityPy's: correct the path"
                    )
                    source = Source(VERSION, (), cite["path"])
                current = source if source.named or source.unknown else ANOTHER_FILE
                if source.named:
                    first, last = int(cite["first"]), int(cite["last"] or cite["first"])
                    cites.append(Cite(n, cite[0], source, first, last, False))
                continue
            if kind == "mention":
                source = source_of(m["path"], line[:pos], lines, revision)
                if unknown(m["path"], source):
                    source = Source(VERSION, (), m["path"])
                current = source if source.named or source.unknown else current
                if source.named and (WORDS_AFTER.match(line[m.end() :]) or WORDS_BEFORE.search(line[:pos])):
                    problems.append(f"{rel}:{n}: cites a line of {m['path']} in words: write `{m['path']}:<line>`")
                continue
            written = ":" + m["first"] + (f"-{m['last']}" if m["last"] else "")
            files = default[1] if default is not None and default[1] else None
            source = current
            if source is not None and source.unknown:
                problems.append(f"{rel}:{n}: {written} follows {source.unknown}, which names no file of {SOURCE}")
                continue
            if source is None and files is None:
                problems.append(f"{rel}:{n}: {written} names no file: no file is named before it in its paragraph")
                continue
            if source is None and len(files) > 1:
                problems.append(f"{rel}:{n}: {written} could be a line of any of the files its section's lines are in")
                continue
            if source is None:
                source = files[0]
            elif files is not None and source.revision == VERSION and source.named not in {f.named for f in files}:
                after = ", ".join(source.named) or "another file"
                where = "section's lines are" if markdown else "header says it ports"
                problems.append(f"{rel}:{n}: {written} follows {after}, but its {where} another: write the path")
                continue
            if revisions and source.named and source.revision != revision:
                problems.append(f"{rel}:{n}: {written} follows a citation of another revision than its column's")
                continue
            if source.named:
                cites.append(Cite(n, written, source, int(m["first"]), int(m["last"] or m["first"]), True))
    return cites, problems


def check(root: Path, package: Package) -> Report:
    problems, checked, bare, boost, files, head_checked, head_unchecked, ported = [], 0, 0, 0, 0, 0, 0, 0
    for rel, text in text_files(root):
        cites, unnamed = citations(rel, text, package.lines, lambda path: (root / path).is_file())
        unnamed += header_problems(rel, text, package.lines)
        problems += unnamed
        for cite in cites:
            where, named = f"{rel}:{cite.n}: {cite.text}", cite.source.named
            if len(named) > 1:
                problems.append(f"{where} could be any of {', '.join(sorted(named))}; write the path from the root")
                continue
            if cite.source.revision == HEAD:
                if named[0] not in package.head_lines:
                    head_unchecked += 1
                    continue
                head_checked += 1
                lines, shapes, of = package.head_lines, package.head_shapes, f"UnityPy git {HEAD}'s "
            else:
                checked += 1
                bare += cite.bare
                boost += named[0].startswith("UnityPyBoost/")
                lines, shapes, of = package.lines, package.shapes, ""
            if not 1 <= cite.first <= cite.last <= lines[named[0]]:
                problems.append(f"{where} is not a range of {of}{named[0]}'s {lines[named[0]]} lines")
            elif named[0] in shapes and (why := span_problem(shapes[named[0]], cite.first, cite.last)):
                problems.append(f"{where} is not a whole span of {of}{named[0]}: it {why}")
        files += bool(cites or unnamed)
        if (cites or unnamed) and re.fullmatch(r"crates/[^/]+/src/.+\.rs", rel) and not is_test_module(rel):
            if any((h.source, h.spdx) == (SOURCE, "MIT") for h in source_headers(text)):
                ported += 1
            else:
                problems.append(
                    f"{rel}: cites UnityPy, but has no `// Ported from {SOURCE} (MIT): ...` (or `// Generated by "
                    f"<generator> from {SOURCE} (MIT): ...`) header line at its top"
                )
    return Report(problems, checked, bare, boost, files, head_checked, head_unchecked, ported)


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    parser.add_argument("root", nargs="?", type=Path, default=ROOT)
    parser.add_argument("--unitypy", type=Path, help="UnityPy 1.25.2's package folder (default: the installed one)")
    parser.add_argument("--boost", type=Path, help="a folder of UnityPyBoost's sources from UnityPy 1.25.2's sdist")
    args = parser.parse_args(argv)
    unitypy = args.unitypy
    if unitypy is None:
        spec = importlib.util.find_spec("UnityPy")
        if spec is None or not spec.submodule_search_locations:
            print("unitypy-citations: UnityPy is not installed; pass --unitypy DIR", file=sys.stderr)
            return 2
        if (installed := importlib.metadata.version("UnityPy")) != VERSION:
            print(f"unitypy-citations: UnityPy {installed} is installed, not {VERSION}", file=sys.stderr)
            return 2
        unitypy = Path(spec.submodule_search_locations[0])
    try:
        package = load_package(unitypy, args.boost, args.root, PACKAGE)
    except (OSError, ValueError) as e:
        print(f"unitypy-citations: {e}", file=sys.stderr)
        return 2
    report = check(args.root, package)
    for p in report.problems:
        print(f"unitypy-citations: {p}", file=sys.stderr)
    if report.problems:
        return 1
    print(
        f"unitypy-citations: {report.checked} citations of UnityPy {VERSION} in {report.files} files "
        f"({report.bare} of them bare lines, {report.boost} UnityPyBoost's) name lines that exist and whole spans; "
        f"{report.head_checked + report.head_unchecked} of UnityPy git {HEAD}, {report.head_checked} of them checked "
        f"against its copies; {report.ported} files under crates' src/ cite it, each with its header"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
