#!/usr/bin/env python3
"""The checks scripts/third-party-licenses runs around cargo-about.

    python3 scripts/third_party_licenses_check.py config [ABOUT_TOML DENY_TOML]
    python3 scripts/third_party_licenses_check.py notices FILE

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
    else:
        print("usage: third_party_licenses_check.py config [ABOUT_TOML DENY_TOML] | notices FILE", file=sys.stderr)
        return 2
    for p in problems:
        print(f"third-party-licenses: {p}", file=sys.stderr)
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
