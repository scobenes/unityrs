"""UnityPyBoost's C++ sources as scripts/unitypy_citations.py reads them: their pins, and the scans that make them.

UnityPyBoost's sources are in UnityPy 1.25.2's sdist, not in the wheel the oracle installs, so the citation
checker reads them from the pins below (BOOST): each file's line count, sha256, blank lines, top-level definitions
(cpp_definitions) and the depth of its braces at the end of each line (cpp_depths). check_copy checks a copy of a
source against its pins with the scans that made them (`unitypy_citations.py --boost DIR`).
scripts/tests/unitypyboost_1_25_2/ keeps that copy, byte for byte, beside the release's LICENSE: CI's parity job
runs `--boost` on it, and scripts/tests checks every pin against it (review round 16: nothing ran `--boost`, so a
wrong digit in a pin passed). A citation of one is held to the blank-line rule, the definition rule and the brace
rule (brace_problem); its statements are not read. Standard library only.
"""

from __future__ import annotations

import hashlib
import re
from pathlib import Path

VERSION = "1.25.2"
# UnityPyBoost's sources in UnityPy 1.25.2's sdist: (lines, sha256, the numbers of its blank lines, its top-level
# definitions as cpp_definitions finds them, `<first>-<last> <name>`, and the depth of its braces at the end of each
# line as cpp_depths finds them, one digit a line). `--boost DIR` checks each against its source (check_copy).
BOOST = {
    "UnityPyBoost/ArchiveStorageDecryptor.cpp": (
        90,
        "f387d5093e7f504ec5735d11e67f447da3ec881b70206c54aba10954bb4899ae",
        "2 5 17 21 25 35 37 51 54 60 67 75 78 83 87 90",
        "6-16 decrypt_byte 18-53 decrypt 55-89 decrypt_block",
        "000000111111111000111111112223332111112222333443211100111111222221112222211111122111111100",
    ),
    "UnityPyBoost/ArchiveStorageDecryptor.hpp": (
        5,
        "4b669249b28603ec64130fce258a35ced7f7adaf16ae7509ee47fe9daa5f301e",
        "4",
        "",
        "00000",
    ),
    "UnityPyBoost/Mesh.cpp": (
        210,
        "ee9542139cb02c1ab8a417192227784c6ee5de3867b9d5761daebf427ad65c8d",
        "4 6 22 35 44 46 48 57 65 76 89 97 100 106",
        "7-21 VertexFormat 23-210 unpack_vertexdata",
        (
            "0000000111111111111100011111111111112233221111111112222111122211112223333211122333443223333443211111"
            "1111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111111"
            "1111111110"
        ),
    ),
    "UnityPyBoost/Mesh.hpp": (
        5,
        "7e081fef7e5e9b95881cc7bba8c284aa1d13f025167a3f3b6a4b30bdbe18ec12",
        "4",
        "",
        "00000",
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
        (
            "13-18 Reader 20-26 TypeTreeReaderConfig 28-65 clean_name 67-72 align4 74-84 read_bool "
            "86-101 read_bool_array 103-111 read_u8 113-126 read_u8_array 128-136 read_s8 138-153 read_s8_array "
            "155-204 read_num 206-264 read_num_array 266-286 _read_length 288-305 read_str 307-323 read_bytes "
            "325-351 read_pair 353-392 read_pair_array 394-445 read_class 449-456 get_annotations 458-583 parse_class "
            "585-681 get_ref_type_node 686-699 SUPPORTED_VALUE_ARRAY_READ_TYPES 701-883 read_typetree_value "
            "885-940 read_typetree_value_array 942-949 set_none_if_null_n_incref 951-960 is_null_none_or_type "
            "962-1056 read_typetree 1059-1073 TypeTreeNode_finalize 1075-1103 typeToNodeDataType "
            "1105-1128 get_node_data_type 1130-1222 TypeTreeNode_init 1224-1232 TypeTreeNode_repr "
            "1234-1248 TypeTreeNode_members 1250-1270 TypeTreeNodeType 1272-1286 add_typetreenode_to_module"
        ),
        (
            "0000000000000111100011111000111122111122111122111111122111122111000111100011222111100011222111222211"
            "0001122211000112221112211000112221100011222111122111000011112222111221112211223322332112233223321122"
            "2210000111122221111222332223322334433443223344334432233333221110000112221112211222111000011122112221"
            "1111000011122112221111000011222111122111222111111100001122211111112211222333222333322222111000011112"
            "2233443344432223332223322332233332221112211100000111111000111111111111122111222333221122233344433211"
            "1122111111122211111122211122233222211122111122233444321122222222233344322111111111000112221112222111"
            "1222211111122222221122221112222222111222222111112222223333332222222233321111111100000111111111111100"
            "0011111222222222222222222222222222222222222222222223333444555666655666555445544455544555544433444555"
            "5444444433222233222333444333443334433334445544555666554334432233344334443211122111000011111222222222"
            "2222222222222222222222222222222211221110001122110001122111100011111111111122222111111221111111223332"
            "1112223322332222332233222333332111112211221111112211111000011111111111110011111111111111111111111111"
            "1100011221111221122334432110001222222222222111111111111111111111111111111122111111111112233322112211"
            "11111111111111111111100011111110011111111111111000122222211111111111100011111111111110"
        ),
    ),
    "UnityPyBoost/TypeTreeHelper.hpp": (
        55,
        "7ca69cd47ce2a71870bc82c3aae7600ad573d76acbd862e3053d664f05ec7543",
        "6 30 52 54",
        "7-29 NodeDataType 31-51 TypeTreeNodeObject",
        "0000000111111111111111111111000111111111111111111100000",
    ),
    "UnityPyBoost/UnityPyBoost.cpp": (
        47,
        "80f005e91ccb786c6705a9e50c19c3bfaf19444f40a65d24b82094c0f0417253",
        "6 8 27 40",
        "9-26 method_table 29-39 UnityPyBoost_module 42-47 PyInit_UnityPyBoost",
        "00000000122212221222122210001111111111000011110",
    ),
    "UnityPyBoost/swap.hpp": (
        78,
        "48d0ef17909c5c6b07d6b3c50750b3828b9472c2d872f51c4ec36f5699bdf719",
        "52 62",
        "53-78 swap_any_inplace",
        "000000000000000000000000000000000000000000000000000000112211222112211221122210",
    ),
}
# A C++ source's tokens, as cpp_tokens reads them: a word, any other character; and what it skips.
CPP_TOKEN = re.compile(
    r"(?P<newline>\n)|(?P<space>[^\S\n]+)|(?P<comment>//[^\n]*|/\*.*?\*/)"
    r"|(?P<literal>\"(?:\\.|[^\"\\\n])*\"|'(?:\\.|[^'\\\n])*')|(?P<word>\w+)|(?P<other>.)",
    re.DOTALL,
)
IDENTIFIER = re.compile(r"[A-Za-z_]\w*")
TYPE_KEYS = {"struct", "enum", "class", "union"}


def cpp_tokens(text: str):
    """(line, token) for each token of a C++ source outside its comments, its string and character literals and
    its preprocessor lines (a `#` line with the lines its trailing backslashes join to it)."""
    line, fresh, directive, joined = 1, True, False, False
    for m in CPP_TOKEN.finditer(text):
        kind, tok = m.lastgroup, m[0]
        if kind == "newline":
            line, fresh, directive = line + 1, True, directive and joined
            continue
        joined = tok == "\\"
        if kind != "space":
            directive, fresh = directive or (fresh and tok == "#"), False
            if kind in ("word", "other") and not directive:
                yield line, tok
        line += tok.count("\n")


def cpp_declaration(words: list[str], line: int) -> tuple[str, bool]:
    """What the top-level declaration whose tokens before its `{` (on `line`) are `words` defines: its name, and
    whether it goes on past its `}` to a `;` (a variable or a type) or ends there (a function)."""
    if words[:1] == ["template"]:  # `template <...>`: its parameters are no part of what follows
        depth = 0
        for i, w in enumerate(words):
            depth += (w == "<") - (w == ">")
            if w == ">" and not depth:
                words = words[i + 1 :]
                break
    top, depth = [], 0  # the tokens outside parentheses and brackets: `f(int a = 3)` is a function
    for w in words:
        depth -= w in (")", "]")
        top.append(None if depth else w)
        depth += w in ("(", "[")
    if "=" in top:  # a variable: `T name[] = {`, `T name = []() -> T {`
        names, ends_on_semicolon = top[: top.index("=")], True
    elif "(" in top:  # a function: its name is the word before its parameters
        names, ends_on_semicolon = top[: top.index("(")], False
    else:  # a type: `typedef struct Reader {`, `enum NodeDataType {`
        names, ends_on_semicolon = [w for w, key in zip(top[1:], top[:-1], strict=True) if key in TYPE_KEYS], True
    names = [w for w in names if w and IDENTIFIER.fullmatch(w)]
    if not names:  # `namespace x {`, `extern "C" {`, a block with nothing before it
        raise ValueError(f"line {line}: a top-level block that is no function, type or table")
    return names[-1], ends_on_semicolon


def cpp_definitions(text: str) -> tuple[tuple[int, int, str], ...]:
    """Each top-level definition of a C++ source whose body is in braces: a function, from its first line (a
    `template` line's) to its `}`; a variable (a table, or a lambda's value) or a type (a struct, an enum), to the
    `;` its declaration ends on. Its first line, its last and its name. A brace-matching scan, made for
    UnityPyBoost's sources, which have no namespace or `extern "C"` block, no raw string and no brace in a
    preprocessor branch; a block it cannot place is an error, not a guess."""
    out, head, depth, opened, pending = [], [], 0, (0, "", False), None
    for line, tok in cpp_tokens(text):
        if depth:
            depth += (tok == "{") - (tok == "}")
            if not depth and opened[2]:
                pending = opened[:2]
            elif not depth:
                out.append((opened[0], line, opened[1]))
        elif pending is not None:  # `} ReaderT;`, `}();`: a declaration that goes on to its `;`
            if tok in ("{", "}"):
                raise ValueError(f"line {line}: a brace before the `;` that ends the declaration of {pending[1]}")
            if tok == ";":
                out.append((pending[0], line, pending[1]))
                pending = None
        elif tok == "{":
            name, ends_on_semicolon = cpp_declaration([w for _, w in head], line)
            opened, depth, head = (head[0][0], name, ends_on_semicolon), 1, []
        elif tok == "}":
            raise ValueError(f"line {line}: a `}}` that closes no block")
        elif tok == ";":
            head = []
        else:
            head.append((line, tok))
    if depth or pending is not None:
        raise ValueError(f"the definition of {opened[1]} runs past the source's end")
    return tuple(out)


def pinned_definitions(pins: str) -> tuple[tuple[int, int, str], ...]:
    """BOOST's `<first>-<last> <name>` pins of a source's top-level definitions."""
    return tuple((int(a), int(b), name) for a, b, name in re.findall(r"(\d+)-(\d+) (\w+)", pins))


def cpp_depths(text: str) -> str:
    """The depth of a C++ source's braces at the end of each line, one digit a line, its braces read as
    cpp_tokens reads them (comments, literals and preprocessor lines aside). A digit a line says where a range
    of lines starts and ends in the blocks only when no line closes a block and opens another (`} else {`),
    which UnityPyBoost's sources never do; such a line, a depth past 9, or a `}` that closes nothing is an
    error, not a guess."""
    moves: dict[int, list[int]] = {}
    for line, tok in cpp_tokens(text):
        if tok in ("{", "}"):
            moves.setdefault(line, []).append(1 if tok == "{" else -1)
    out, depth = [], 0
    for line in range(1, len(text.splitlines()) + 1):
        start = low = depth
        for step in moves.get(line, ()):
            depth += step
            low = min(low, depth)
        if low < 0 or depth > 9:
            raise ValueError(f"line {line}: a brace depth of {min(low, 0) or depth}, which one digit cannot pin")
        if low < min(start, depth):
            raise ValueError(f"line {line}: closes a block and opens another, which one digit a line cannot pin")
        out.append(str(depth))
    return "".join(out)


def brace_problem(depths: str, first: int, last: int) -> str | None:
    """Why lines first..last of a C++ source whose cpp_depths are `depths` are not a whole span of its blocks, or
    None when they are: the range closes no block begun above its first line and leaves open no block it opens.
    So it neither starts on, nor reaches, a `}` whose `{` is above it, nor ends inside a block it opened (a
    single line included: a lone `}`, or `while (x) {` without its body)."""

    def at(line: int) -> int:  # the depth at the end of `line`; 0 before the first
        return int(depths[line - 1]) if line > 0 else 0

    start = at(first - 1)
    for line in range(first, last + 1):
        if at(line) < start:
            return f"closes at line {line} a block begun above line {first}"
    if at(last) > start:
        opened = max(line for line in range(first, last + 1) if at(line - 1) == start < at(line))
        return f"opens at line {opened} a block it does not close"
    return None


def check_copy(name: str, data: bytes, where: Path) -> None:
    """Refuses `data`, the copy of UnityPyBoost's `name` at `where`, unless it is the source BOOST pins: its
    sha256, and its line count, blank lines, definitions and brace depths as the scans find them."""
    count, sha, blanks, definitions, depths = BOOST[name]
    if hashlib.sha256(data).hexdigest() != sha:
        raise ValueError(f"{where} is not UnityPy {VERSION}'s {name} (sha256 differs)")
    text = data.decode("utf-8")
    rows = text.splitlines()
    blank = [n for n, row in enumerate(rows, 1) if not row.strip()]
    if (len(rows), blank) != (count, [int(n) for n in blanks.split()]):
        raise ValueError(f"BOOST's line count or blank lines for {name} are not those of its source")
    if (found := cpp_definitions(text)) != pinned_definitions(definitions):
        scan = " ".join(f"{a}-{b} {n}" for a, b, n in found)
        raise ValueError(f"BOOST's definitions for {name} are not those of its source, which are {scan!r}")
    if (scanned := cpp_depths(text)) != depths:
        raise ValueError(f"BOOST's brace depths for {name} are not those of its source, which are {scanned!r}")
