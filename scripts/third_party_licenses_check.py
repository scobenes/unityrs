#!/usr/bin/env python3
"""The checks scripts/third-party-licenses runs around cargo-about.

    python3 scripts/third_party_licenses_check.py config [ABOUT_TOML DENY_TOML]
    python3 scripts/third_party_licenses_check.py notices FILE [ROOT]
    python3 scripts/third_party_licenses_check.py ports [ROOT]

`config` holds about.toml (cargo-about, which writes THIRD_PARTY_LICENSES) and
deny.toml (cargo-deny, which gates the dependency graph) to the same licences,
so that nothing cargo-about would list is a licence cargo-deny never checks:
- about.toml's `accepted` is deny.toml's `[licenses] allow`, in the same order;
- each crate's `accepted` in about.toml is the `allow` of deny.toml's exception
  for that crate, and every exception there has its `accepted` here;
- each crate's `clarify` in about.toml has a clarification in deny.toml with
  the same expression and the same files, and the other way round. Each tool
  checks its own hashes of those files (sha256 and xxHash32).

`notices` fails when the generated file still holds an SPDX template's
placeholders (`<year>`, `<copyright holders>`, `<owner>`): cargo-about uses the
template when a crate ships no licence file, and a notice with a placeholder
names no one. It also fails when the text of a workspace member's `LICENSE-*`
file is not in its Crates part: cargo-about lists a licence file only when it recognises
the licence in it, and otherwise falls back to the template, or to nothing,
without a warning (a PSF agreement with a paragraph of ours before it is not
recognised).

`ports` holds the ported code to its notices. A ported file names each source
in a header line at its top (`// Ported from <source> (<SPDX id>): ...`, or
`#` in Python). about.hbs's "Ported code" part has a section for each source,
titled with exactly that source (an aside in parentheses may follow it), with
a `Licence:` line naming the id, and ending in the notices the source's
licence asks every copy to keep, each after a marker line
`{{!-- notice: LICENSE-<NAME> --}}` (or `NOTICE-<NAME>`, for a text such as
a summary of changes that is no licence; a handlebars comment, which renders
nothing) and running to the next marker or the section's end. And:
- a header's source is a section's, exactly (not a prefix of its title),
  under the same id;
- a file under crates/<name>/ has that id in its crate's `license` in every
  alternative the SPDX expression offers (a term of a top-level AND, or one
  on each side of every OR): an `OR` choice lets a user take the crate under
  a licence that does not keep the notice;
- a file a section lists (`crates/...`) carries that section's header, unless
  it is generated;
- a crate holds a section when one of its files names the section's source in
  its header, or the section lists one of its files. It then has each of
  the section's notices, the same words, as its own file of the name the
  marker gives (a symlink to another crate's is one), so `cargo package`,
  which packs every file of the crate's folder, ships them with the code; a
  crate whose
  manifest sets `include` or `exclude` is refused, since this check cannot
  then say what is packed;
- each licence of a workspace member beyond the repository's own (MIT,
  Apache-2.0) is a section's, whose notices the crate has as those files, so
  cargo-about lists the member with that text, not the SPDX template (which,
  for PSF-2.0, names no holder of ours to refuse; `notices` checks that it
  did).

Exit status: 0 when the check passes, 1 when it fails (each problem on its
own line on stderr), 2 for a usage error. Standard library only (tomllib,
Python 3.11 or later).
"""

from __future__ import annotations

import re
import sys
import tomllib
from pathlib import Path
from typing import NamedTuple

ROOT = Path(__file__).resolve().parents[1]
# about.toml's top-level keys that configure cargo-about itself rather than one crate.
ABOUT_GLOBAL = frozenset(
    (
        "accepted",
        "targets",
        "ignore-build-dependencies",
        "ignore-dev-dependencies",
        "ignore-transitive-dependencies",
        "filter-noassertion",
        "workarounds",
        "private",
        "no-clearly-defined",
    )
)
PORT_HEADER = re.compile(r"^(?://|#) Ported from (?P<source>.+?) \((?P<spdx>[A-Za-z0-9.+-]+)\): ")
# A section's title: its source, then an optional aside in parentheses.
TITLE = re.compile(r"(?P<source>[^()]*[^() ])(?: \([^()]*\))?")
# The line that starts a section's notice (a handlebars comment: it renders nothing).
NOTICE = re.compile(r"^\{\{!--\s*notice:\s*(?P<file>(?:LICENSE|NOTICE)-[A-Z0-9-]+)\s*--\}\}\n", re.M)
SPDX_TOKEN = re.compile(r"\s*(?:(?P<paren>[()])|(?P<word>[A-Za-z0-9.+:-]+)|(?P<other>\S))")
OPERATORS = ("AND", "OR", "WITH")
RULE = "-" * 80
OWN_LICENCES = frozenset(("MIT", "Apache-2.0"))
PLACEHOLDER = re.compile(r"<(?:year|copyright holders?|owner|name of author)>")


def _crate_name(entry: dict) -> str | None:
    """The crate a deny.toml clarification or exception names (`crate`, or the older `name`), without a version."""
    spec = entry.get("crate", entry.get("name"))
    return spec.split("@")[0].split(":")[0] if isinstance(spec, str) else None


def config_problems(about: dict, deny: dict) -> list[str]:
    problems = []
    licenses = deny.get("licenses", {})
    if about.get("accepted") != licenses.get("allow"):
        problems.append(
            f"about.toml accepts {about.get('accepted')}, but deny.toml allows {licenses.get('allow')}; "
            "keep them equal, in the same order"
        )

    crates = {k: v for k, v in about.items() if k not in ABOUT_GLOBAL and isinstance(v, dict)}

    exceptions = {}
    for e in licenses.get("exceptions", []):
        exceptions.setdefault(_crate_name(e), []).append(sorted(e.get("allow", [])))
    for name in sorted(set(exceptions) | {n for n, c in crates.items() if "accepted" in c}):
        here = sorted(crates.get(name, {}).get("accepted", []))
        there = exceptions.get(name, [])
        if there != [here] or not here:
            problems.append(
                f"{name}: about.toml accepts {here or 'nothing extra'} for it, but deny.toml's exceptions for it "
                f"allow {there or 'nothing'}; give it one exception with exactly those licences"
            )

    clarified = {}
    for c in licenses.get("clarify", []):
        files = sorted(f.get("path") for f in c.get("license-files", []))
        clarified.setdefault(_crate_name(c), []).append((c.get("expression"), files))
    for name in sorted(set(clarified) | {n for n, c in crates.items() if "clarify" in c}):
        clarify = crates.get(name, {}).get("clarify")
        here = [(clarify.get("license"), sorted(f.get("path") for f in clarify.get("files", [])))] if clarify else []
        there = clarified.get(name, [])
        if here != there:
            problems.append(
                f"{name}: about.toml clarifies it as {here or 'nothing'}, but deny.toml as {there or 'nothing'} "
                "(expression, files); clarify it the same way in both"
            )
    return problems


def notice_problems(text: str, root: Path | None = None) -> list[str]:
    """The generated file's placeholders, and (with `root`) each member licence file whose text it does not hold."""
    problems = [
        f"line {n}: {line.strip()!r} is an SPDX template's placeholder, not a notice; give the crate its licence "
        "file (a workspace member: link LICENSE-MIT and LICENSE-APACHE) or a clarification in about.toml"
        for n, line in enumerate(text.splitlines(), 1)
        if PLACEHOLDER.search(line)
    ]
    if root is not None:
        crates_part = "\n".join(("=" * 80, "Crates", "=" * 80, ""))
        if crates_part not in text:
            return [*problems, "no Crates part (a line 'Crates' between two rules of '=')"]
        listed = _words(text.split(crates_part, 1)[1])
        for path in sorted((root / "crates").glob("*/LICENSE-*")):
            if (path.parent / "Cargo.toml").is_file() and _words(path.read_text(encoding="utf-8")) not in listed:
                problems.append(
                    f"crates/{path.parent.name}/{path.name}: its text is not among the licences listed under "
                    "Crates; cargo-about did not recognise the licence in it, so it listed the SPDX template or "
                    "nothing (keep the file the licence's text alone, and put any text of ours in a NOTICE-* file)"
                )
    return problems


def port_headers(text: str) -> list[tuple[str, str]]:
    """The (source, SPDX id) of each `Ported from` line in the comments at the top of a file."""
    out = []
    for n, line in enumerate(text.splitlines()):
        if n == 0 and line.startswith("#!/"):
            continue
        if not line.startswith(("//", "#")) or line.startswith(("//!", "#!", "#[")):
            break
        if m := PORT_HEADER.match(line):
            out.append((m["source"], m["spdx"]))
    return out


class Section(NamedTuple):
    title: str
    # The title without its aside; None when the title is not `<source>` or `<source> (<aside>)`.
    source: str | None
    licence: str
    text: str
    # (file, text) for each notice marker: the file it names, and the text from it to the next marker or the end.
    notices: tuple[tuple[str, str], ...]


def ported_sections(hbs: str) -> list[Section]:
    """Each section of about.hbs's "Ported code" part that has a `Licence:` line."""
    part = hbs.split("\nPorted code\n", 1)[-1].split("\nCrates\n", 1)[0]
    out = []
    for chunk in part.split(RULE + "\n")[1:]:
        text = chunk.split("\n" + "=" * 80, 1)[0]
        licence = re.search(r"^Licence: ([A-Za-z0-9.+-]+?)\.(?:\s|$)", text, re.M)
        if not licence:
            continue
        title = text.split("\n", 1)[0]
        source = m["source"] if (m := TITLE.fullmatch(title)) else None
        markers = list(NOTICE.finditer(text))
        ends = [m.start() for m in markers[1:]] + [len(text)]
        notices = tuple((m["file"], text[m.end() : ends[i]]) for i, m in enumerate(markers))
        out.append(Section(title, source, licence[1], text, notices))
    return out


def _words(text: str) -> str:
    return " ".join(text.split())


def spdx_parse(expression: str) -> tuple:
    """The SPDX licence expression as a tree: ("ID", "X" or "X WITH Y"), ("AND", a, b) or ("OR", a, b).

    WITH binds tighter than AND, and AND than OR (SPDX 2.3, annex D). Raises
    ValueError on anything else, a lower-case operator and cargo's old `/`
    included, so no check reads an expression it does not understand.
    """
    tokens = []
    for m in SPDX_TOKEN.finditer(expression):
        if m["other"]:
            raise ValueError(f"{m['other']!r} is not part of an SPDX expression")
        tokens.append(m["paren"] or m["word"])
    pos = 0

    def peek():
        return tokens[pos] if pos < len(tokens) else None

    def take():
        nonlocal pos
        if pos == len(tokens):
            raise ValueError("it ends where a licence is expected")
        pos += 1
        return tokens[pos - 1]

    def either():
        node = both()
        while peek() == "OR":
            take()
            node = ("OR", node, both())
        return node

    def both():
        node = simple()
        while peek() == "AND":
            take()
            node = ("AND", node, simple())
        return node

    def simple():
        word = take()
        if word == "(":
            node = either()
            if take() != ")":
                raise ValueError("a parenthesis is not closed")
            return node
        if word in OPERATORS or word == ")":
            raise ValueError(f"{word!r} where a licence is expected")
        if peek() == "WITH":
            take()
            exception = take()
            if exception in (*OPERATORS, "(", ")"):
                raise ValueError(f"{exception!r} where a licence exception is expected")
            return ("ID", f"{word} WITH {exception}")
        return ("ID", word)

    tree = either()
    if pos != len(tokens):
        raise ValueError(f"{tokens[pos]!r} where an operator or the end is expected")
    return tree


def spdx_requires(tree: tuple, licence: str) -> bool:
    """Whether every alternative the expression `tree` offers includes `licence` itself (not with an exception)."""
    if tree[0] == "ID":
        return tree[1] == licence
    if tree[0] == "AND":
        return spdx_requires(tree[1], licence) or spdx_requires(tree[2], licence)
    return spdx_requires(tree[1], licence) and spdx_requires(tree[2], licence)


def spdx_ids(tree: tuple) -> set[str]:
    """Every licence the expression `tree` names, in any alternative."""
    return {tree[1]} if tree[0] == "ID" else spdx_ids(tree[1]) | spdx_ids(tree[2])


def _manifest(root: Path, crate: str) -> dict:
    with open(root / "crates" / crate / "Cargo.toml", "rb") as fh:
        return tomllib.load(fh)["package"]


def _crate_licence(root: Path, crate: str) -> str:
    licence = _manifest(root, crate).get("license", "")
    if isinstance(licence, dict):
        with open(root / "Cargo.toml", "rb") as fh:
            licence = tomllib.load(fh)["workspace"]["package"]["license"]
    return licence


def _source_files(root: Path) -> list[Path]:
    files = []
    for top in ("crates", "tools", "scripts"):
        for path in sorted((root / top).rglob("*")):
            parts = path.relative_to(root).parts
            if path.suffix in (".rs", ".py") and not {"target", "__pycache__", ".wt"} & set(parts):
                files.append(path)
    return files


def _notice_problem(root: Path, crate: str, section: Section) -> str | None:
    """Why the crate does not carry each of the section's notices as its own file, or None when it does."""
    for name, notice in section.notices:
        path = root / "crates" / crate / name
        if not path.is_file():
            return f"has no {name} with about.hbs's {section.title!r} notice"
        if _words(path.read_text(encoding="utf-8")) != _words(notice):
            return f"has a {name} that is not about.hbs's {section.title!r} notice"
    return None


def port_problems(root: Path) -> list[str]:
    sections = ported_sections((root / "about.hbs").read_text(encoding="utf-8"))
    problems = []
    by_source = {}
    for section in sections:
        if section.source is None:
            problems.append(f"about.hbs: {section.title!r} is not a title of the form '<source> (<aside>)'")
        elif section.source in by_source:
            problems.append(f"about.hbs: two sections are for {section.source!r}")
        else:
            by_source[section.source] = section
        if not section.notices or not all(text.strip() for _, text in section.notices):
            problems.append(
                f"about.hbs: {section.title!r} has no notice: end it with a line "
                "`{{!-- notice: LICENSE-<NAME> --}}` and then the text its licence asks every copy to keep"
            )

    crates = sorted(cargo.parent.name for cargo in (root / "crates").glob("*/Cargo.toml"))
    expressions = {}
    for crate in crates:
        try:
            expressions[crate] = spdx_parse(_crate_licence(root, crate))
        except ValueError as e:
            problems.append(f"crates/{crate}: its license {_crate_licence(root, crate)!r} does not parse: {e}")

    # The sections whose code each crate holds: a header names the source, or the section lists the file.
    held = {crate: set() for crate in crates}
    for path in _source_files(root):
        rel = path.relative_to(root).as_posix()
        crate = rel.split("/")[1] if rel.startswith("crates/") else None
        for source, spdx in port_headers(path.read_text(encoding="utf-8", errors="replace")):
            section = by_source.get(source)
            if section is None:
                problems.append(f"{rel}: ported from {source}, which no section of about.hbs's ported code names")
                continue
            if section.licence != spdx:
                problems.append(f"{rel}: ported from {source} under {spdx}, but about.hbs gives {section.licence}")
            if crate in held:
                held[crate].add(section)
            if crate in expressions and not spdx_requires(expressions[crate], spdx):
                problems.append(
                    f"{rel}: ported under {spdx}, which its crate's license "
                    f"({_crate_licence(root, crate)!r}) does not name in every alternative"
                )
    for section in by_source.values():
        for rel in sorted(set(re.findall(r"crates/[\w./-]+\.rs", section.text))):
            path = root / rel
            if not path.is_file():
                problems.append(f"about.hbs: {section.title!r} lists {rel}, which does not exist")
                continue
            if rel.split("/")[1] in held:
                held[rel.split("/")[1]].add(section)
            body = path.read_text(encoding="utf-8", errors="replace")
            if "@generated" not in body and section.source not in {s for s, _ in port_headers(body)}:
                problems.append(
                    f"{rel}: about.hbs's {section.title!r} lists it, but its header does not name that source"
                )

    for crate in crates:
        needed = {s for s in held[crate] if s.notices}
        if needed and {"include", "exclude"} & set(_manifest(root, crate)):
            problems.append(
                f"crates/{crate}: its manifest sets include or exclude, so this check cannot say its notices are packed"
            )
        for section in sorted(needed, key=lambda s: s.title):
            if why := _notice_problem(root, crate, section):
                problems.append(f"crates/{crate}: it holds {section.source}'s code, but {why}")
        for term in sorted(spdx_ids(expressions[crate]) - OWN_LICENCES if crate in expressions else ()):
            under = [s for s in by_source.values() if s.licence == term and s.notices]
            if not any(_notice_problem(root, crate, s) is None for s in under):
                problems.append(
                    f"crates/{crate}: its license names {term}, but it does not carry the notices "
                    f"of an about.hbs section under {term}"
                )
    return problems


def main(argv=None) -> int:
    args = sys.argv[1:] if argv is None else argv
    if args[:1] == ["config"] and len(args) in (1, 3):
        paths = args[1:] if len(args) == 3 else (ROOT / "about.toml", ROOT / "deny.toml")
        about_path, deny_path = (Path(p) for p in paths)
        with open(about_path, "rb") as fh:
            about = tomllib.load(fh)
        with open(deny_path, "rb") as fh:
            deny = tomllib.load(fh)
        problems = config_problems(about, deny)
    elif args[:1] == ["notices"] and len(args) in (2, 3):
        root = Path(args[2]) if len(args) == 3 else ROOT
        problems = notice_problems(Path(args[1]).read_text(encoding="utf-8"), root)
    elif args[:1] == ["ports"] and len(args) in (1, 2):
        problems = port_problems(Path(args[1]) if len(args) == 2 else ROOT)
    else:
        print(
            "usage: third_party_licenses_check.py config [ABOUT_TOML DENY_TOML] | notices FILE [ROOT] | ports [ROOT]",
            file=sys.stderr,
        )
        return 2
    for p in problems:
        print(f"third-party-licenses: {p}", file=sys.stderr)
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
