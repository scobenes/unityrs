#!/usr/bin/env python3
"""The checks scripts/third-party-licenses runs around cargo-about.

    python3 scripts/third_party_licenses_check.py config [ABOUT_TOML DENY_TOML]
    python3 scripts/third_party_licenses_check.py notices FILE
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
names no one.

`ports` holds the ported code to its notices. A ported file names each source
in a header line at its top (`// Ported from <source> (<SPDX id>): ...`, or
`#` in Python), and:
- about.hbs's "Ported code" part has a section whose title starts with that
  source, its `Licence:` line naming that id;
- a file under crates/<name>/ has that id in its crate's `license`;
- a file a section lists (`crates/...`) carries that section's header, unless
  it is generated;
- each licence of a workspace member beyond the repository's own (MIT,
  Apache-2.0) is a file of the crate's, `LICENSE-*`, whose text the section
  of that licence holds, so cargo-about lists the member with that text, not
  the SPDX template (which, for PSF-2.0, names no holder of ours to refuse).

Exit status: 0 when the check passes, 1 when it fails (each problem on its
own line on stderr), 2 for a usage error. Standard library only (tomllib,
Python 3.11 or later).
"""

from __future__ import annotations

import re
import sys
import tomllib
from pathlib import Path

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


def notice_problems(text: str) -> list[str]:
    return [
        f"line {n}: {line.strip()!r} is an SPDX template's placeholder, not a notice; give the crate its licence "
        "file (a workspace member: link LICENSE-MIT and LICENSE-APACHE) or a clarification in about.toml"
        for n, line in enumerate(text.splitlines(), 1)
        if PLACEHOLDER.search(line)
    ]


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


def ported_sections(hbs: str) -> list[tuple[str, str, str]]:
    """The (title, licence id, text) of each section of about.hbs's "Ported code" part with a `Licence:` line."""
    part = hbs.split("\nPorted code\n", 1)[-1].split("\nCrates\n", 1)[0]
    out = []
    for chunk in part.split(RULE + "\n")[1:]:
        text = chunk.split("\n" + "=" * 80, 1)[0]
        licence = re.search(r"^Licence: ([A-Za-z0-9.+-]+?)\.(?:\s|$)", text, re.M)
        if licence:
            out.append((text.split("\n", 1)[0], licence[1], text))
    return out


def _words(text: str) -> str:
    return " ".join(text.split())


def _crate_licence(root: Path, crate: str) -> str:
    with open(root / "crates" / crate / "Cargo.toml", "rb") as fh:
        licence = tomllib.load(fh)["package"].get("license", "")
    if isinstance(licence, dict):
        with open(root / "Cargo.toml", "rb") as fh:
            licence = tomllib.load(fh)["workspace"]["package"]["license"]
    return licence


def _terms(expression: str) -> set[str]:
    return set(re.findall(r"[A-Za-z0-9.+-]+", expression)) - {"AND", "OR", "WITH"}


def port_problems(root: Path) -> list[str]:
    sections = ported_sections((root / "about.hbs").read_text(encoding="utf-8"))
    problems = []
    files = []
    for top in ("crates", "tools", "scripts"):
        for path in sorted((root / top).rglob("*")):
            parts = path.relative_to(root).parts
            if path.suffix in (".rs", ".py") and not {"target", "__pycache__", ".wt"} & set(parts):
                files.append(path)
    for path in files:
        rel = path.relative_to(root).as_posix()
        for source, spdx in port_headers(path.read_text(encoding="utf-8", errors="replace")):
            named = [s for s in sections if s[0].startswith(source)]
            if not named:
                problems.append(f"{rel}: ported from {source}, which no section of about.hbs's ported code names")
            elif named[0][1] != spdx:
                problems.append(f"{rel}: ported from {source} under {spdx}, but about.hbs gives {named[0][1]}")
            if rel.startswith("crates/") and spdx not in _terms(_crate_licence(root, rel.split("/")[1])):
                problems.append(f"{rel}: ported under {spdx}, which its crate's license does not name")
    for title, _licence, text in sections:
        for rel in sorted(set(re.findall(r"crates/[\w./-]+\.rs", text))):
            path = root / rel
            if not path.is_file():
                problems.append(f"about.hbs: {title!r} lists {rel}, which does not exist")
                continue
            body = path.read_text(encoding="utf-8", errors="replace")
            if "@generated" not in body and not any(title.startswith(s) for s, _ in port_headers(body)):
                problems.append(f"{rel}: about.hbs's {title!r} lists it, but its header does not name that source")
    for cargo in sorted((root / "crates").glob("*/Cargo.toml")):
        crate = cargo.parent.name
        for term in sorted(_terms(_crate_licence(root, crate)) - OWN_LICENCES):
            held = [_words(s[2]) for s in sections if s[1] == term]
            texts = [_words(f.read_text(encoding="utf-8")) for f in sorted(cargo.parent.glob("LICENSE-*"))]
            if not any(t and t in h for t in texts for h in held):
                problems.append(
                    f"crates/{crate}: its license names {term}, but none of its LICENSE-* files holds a text that "
                    f"about.hbs's {term} section gives"
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
    elif args[:1] == ["notices"] and len(args) == 2:
        problems = notice_problems(Path(args[1]).read_text(encoding="utf-8"))
    elif args[:1] == ["ports"] and len(args) in (1, 2):
        problems = port_problems(Path(args[1]) if len(args) == 2 else ROOT)
    else:
        print(
            "usage: third_party_licenses_check.py config [ABOUT_TOML DENY_TOML] | notices FILE | ports [ROOT]",
            file=sys.stderr,
        )
        return 2
    for p in problems:
        print(f"third-party-licenses: {p}", file=sys.stderr)
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
