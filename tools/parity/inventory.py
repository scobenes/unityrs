#!/usr/bin/env python3
"""List UnityPy's public surface, and check that PARITY.md names every part of it.

`DESIGN.md` section 3 asks for an inventory that maps every public UnityPy API and
behaviour to a tier. This script keeps that inventory complete: `list` prints what
the installed UnityPy defines, and `check` fails when PARITY.md leaves any of it out.

The groups, and how PARITY.md names a symbol of each, inside a code span (backticks):
- `modules`, every `.py` file under `UnityPy/`: its path, alone or with `:line`
  (`helpers/MeshHelper.py:504`). A path names its module and nothing else.
- `classes`, every public class a module defines: its name (`Texture2D`).
- `functions`, every public module-level function: `Module.function`
  (`ImportHelper.check_file_type`), where `Module` is the file's name without `.py`,
  or the package's name for an `__init__.py`.
- `data`, every public name a module assigns at its top level whose value is not a
  class, function, module or typing construct (tables, registries, settings,
  caches): `Module.NAME` (`CompressionHelper.DECOMPRESSION_MAP`), `Module` as above.
- `methods`, every public callable or property a class defines in its own body
  (`Environment.load_file`), for every class UnityPy defines; for the classes of
  `classes/generated.py`, the members a `classes/legacy_patch` module adds
  (`Texture2D.set_image`). Inherited members are named under the class that defines them.
- `fields`, every public attrs field a class declares itself, for every attrs class
  UnityPy defines, the generated classes included: `Class.field` (`GameObject.m_Name`).
- `exports`, every public name a package's `__init__.py` holds that it imports from
  another UnityPy module or assigns as an alias: the dotted path from `UnityPy`
  (`UnityPy.files.File`, `UnityPy.load`). A package's own submodules are `modules`.
- `boost`, every public name UnityPyBoost exports: `UnityPyBoost.read_typetree`.
- `texture_formats`, every `TextureFormat` member: a table row with a cell
  `` `Name` `` and a cell holding its value.

Out of scope: enum members and other class-level constants (an enum is named by its
class; `TextureFormat` members have their own group), and private names.

A dotted name must be written whole, from the start of its dotted chain: `` `Wrong.ImportHelper.check_file_type` ``
does not name `ImportHelper.check_file_type`. The only prefixes allowed are the
symbol's own import path (`UnityPy.helpers.ImportHelper.check_file_type`), or a
package path that re-exports it (`UnityPy.classes.GameObject.m_Name`); the only
suffixes are what follows the chain (a call, a subscript, a further `.attribute`).

A long list may be written compactly instead: in a table with a column headed
`fields` or `members`, a row whose first cell's first code span is `Parent` names
`Parent.x` for every code span `` `x` `` in that column (Appendix A lists each
generated class's fields this way).

    scripts/dev python tools/parity/inventory.py list [--json]
    scripts/dev python tools/parity/inventory.py check [PARITY.md]

Both need UnityPy (the oracle image). `check` exits 0 when nothing is missing, 1 when
something is, and 2 when it cannot run.
"""

from __future__ import annotations

import argparse
import ast
import importlib
import inspect
import json
import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
DEFAULT_PARITY = REPO / "PARITY.md"
PINNED_VERSION = "1.25.2"

GROUPS = ("modules", "classes", "functions", "data", "methods", "fields", "exports", "boost", "texture_formats")
DOTTED_GROUPS = ("classes", "functions", "data", "methods", "fields", "exports", "boost")
COMPACT_COLUMNS = ("fields", "members")

# The package whose modules add members to the generated classes.
LEGACY_PATCH = "UnityPy.classes.legacy_patch"
GENERATED = "UnityPy.classes.generated"

CODE_SPAN = re.compile(r"`([^`\n]+)`")
DOTTED = re.compile(r"[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*")
UNESCAPED_PIPE = re.compile(r"(?<!\\)\|")


def _module_label(dotted: str) -> str:
    """`ImportHelper` for `UnityPy.helpers.ImportHelper`; a package's own name for its `__init__.py`."""
    return dotted.rsplit(".", 1)[-1]


def _is_member(value) -> bool:
    return callable(value) or isinstance(value, (property, classmethod, staticmethod))


def _defined_in(value, prefix: str) -> bool:
    func = value.fget if isinstance(value, property) else getattr(value, "__func__", value)
    return (getattr(func, "__module__", None) or "").startswith(prefix)


def _assigned_names(source: str) -> list[str]:
    """The public names a module's top level assigns (`X = ...`, `X: T = ...`, `X, Y = ...`)."""
    names = []
    for node in ast.parse(source).body:
        if isinstance(node, ast.Assign):
            targets = node.targets
        elif isinstance(node, ast.AnnAssign):
            targets = [node.target]
        else:
            continue
        for target in targets:
            for name in target.elts if isinstance(target, ast.Tuple) else [target]:
                if isinstance(name, ast.Name) and not name.id.startswith("_"):
                    names.append(name.id)
    return names


def _is_data(value) -> bool:
    """Data, as opposed to an alias of a class or function, a module, a TypeVar or a type alias."""
    return not (
        inspect.isclass(value)
        or inspect.isroutine(value)
        or inspect.ismodule(value)
        or type(value).__module__ in ("typing", "__future__")
    )


def _origin(value) -> str | None:
    """The module that defines a class or function, None for anything else."""
    if inspect.isclass(value) or inspect.isroutine(value):
        return getattr(value, "__module__", None)
    return None


def _modules(root: Path):
    """(relative path, dotted name, module, source) for every module under UnityPy/, sorted by path."""
    for path in sorted(root.rglob("*.py")):
        rel = path.relative_to(root).as_posix()
        if "__pycache__" in rel:
            continue
        dotted = ("UnityPy." + rel[: -len(".py")].replace("/", ".")).removesuffix(".__init__")
        yield rel, dotted, importlib.import_module(dotted), path.read_text(encoding="utf-8")


def collect() -> dict:
    """Every public symbol of the installed UnityPy, by group, in a stable order.

    The dotted groups hold each symbol's short form (the one PARITY.md must name), and
    `paths` maps each short form to the full import paths it may also be named by.
    """
    import attrs
    import UnityPy

    root = Path(UnityPy.__file__).resolve().parent
    groups: dict[str, list] = {group: [] for group in GROUPS}
    paths: dict[str, set[str]] = {}
    class_paths: dict[int, set[str]] = {}  # id(class) -> its import paths
    classes: list[tuple[type, str]] = []  # (class, its own name), each once
    function_names: dict[int, str] = {}  # id(function) -> its short form

    def add(group, short, *full):
        groups[group].append(short)
        paths.setdefault(short, set()).update(full)

    modules = list(_modules(root))
    for rel, dotted, module, source in modules:
        groups["modules"].append(rel)
        label = _module_label(dotted)
        for name, value in vars(module).items():
            if name.startswith("_") or _origin(value) != module.__name__:
                continue
            if inspect.isclass(value):
                add("classes", name, f"{dotted}.{name}")
                class_paths.setdefault(id(value), set()).add(f"{dotted}.{name}")
                if name == value.__name__:
                    classes.append((value, name))
            elif inspect.isfunction(value):
                add("functions", f"{label}.{name}", f"{dotted}.{name}")
                function_names[id(value)] = f"{label}.{name}"
        for name in _assigned_names(source):
            if _is_data(getattr(module, name)):
                add("data", f"{label}.{name}", f"{dotted}.{name}")

    # Package re-exports: what `UnityPy.files.File` or `UnityPy.load` reach.
    for rel, dotted, module, source in modules:
        if not rel.endswith("__init__.py"):
            continue
        assigned = set(_assigned_names(source))
        for name, value in vars(module).items():
            if name.startswith("_") or inspect.ismodule(value):
                continue
            origin = _origin(value)
            if origin is not None:
                if not origin.startswith("UnityPy") or (origin == dotted and name == value.__name__):
                    continue  # imported from outside UnityPy, or defined here (classes, functions)
            elif not _is_data(value) or name in assigned:
                continue  # a typing construct, or the package's own data
            export = f"{dotted}.{name}"
            add("exports", export, export)
            if inspect.isclass(value):
                class_paths.setdefault(id(value), set()).add(export)
                paths.setdefault(value.__name__, set()).add(export)
            elif id(value) in function_names:
                paths[function_names[id(value)]].add(export)

    for cls, name in classes:
        generated = cls.__module__ == GENERATED
        for member, attr in vars(cls).items():
            if member.startswith("_") or not _is_member(attr):
                continue
            if generated and not _defined_in(attr, LEGACY_PATCH):
                continue
            add("methods", f"{name}.{member}", *(f"{p}.{member}" for p in class_paths[id(cls)]))
        if attrs.has(cls):
            for field in attrs.fields(cls):
                if not field.name.startswith("_") and not field.inherited:
                    add("fields", f"{name}.{field.name}", *(f"{p}.{field.name}" for p in class_paths[id(cls)]))
    # legacy_patch adds members to generated classes it imports; they are found above.

    boost = importlib.import_module("UnityPy.UnityPyBoost")
    for name in sorted(dir(boost)):
        if not name.startswith("_"):
            add("boost", f"UnityPyBoost.{name}", f"UnityPy.UnityPyBoost.{name}")

    from UnityPy.enums import TextureFormat

    groups["texture_formats"] = [[member.name, member.value] for member in TextureFormat]
    for group in DOTTED_GROUPS:
        groups[group] = sorted(set(groups[group]))
    groups["paths"] = {short: sorted(full) for short, full in sorted(paths.items())}
    return groups


def table_cells(line: str) -> list[str] | None:
    """The cells of a Markdown table line, or None for any other line."""
    line = line.strip()
    if not (line.startswith("|") and line.endswith("|") and len(line) > 1):
        return None
    return [cell.strip() for cell in UNESCAPED_PIPE.split(line[1:-1])]


def _is_separator(cells: list[str]) -> bool:
    return bool(cells) and set("".join(cells)) <= set("-: ")


def tables(text: str):
    """Each Markdown table as (header cells, body rows)."""
    header: list[str] | None = None
    rows: list[list[str]] = []
    for line in text.splitlines() + [""]:
        cells = table_cells(line)
        if cells is None:
            if header is not None:
                yield header, rows
            header, rows = None, []
        elif header is None:
            header = cells
        elif not _is_separator(cells):
            rows.append(cells)


def _first_chain(cell: str) -> str | None:
    for span in CODE_SPAN.findall(cell):
        match = DOTTED.match(span.strip())
        if match:
            return match.group(0)
    return None


class Mentions:
    """What a Markdown document names inside its code spans and table rows."""

    def __init__(self, text: str):
        self.paths: set[str] = set()
        self.chains: set[str] = set()  # every chain written, and every leading part of one
        self.rows: list[list[str]] = []
        for span in CODE_SPAN.findall(text):
            path = span.strip().removeprefix("UnityPy/").split(":", 1)[0]
            if path.endswith(".py"):
                # A path names its module only: `files/BundleFile.py` does not name the class BundleFile.
                self.paths.add(path)
                continue
            for chain in DOTTED.findall(span):
                self._add(chain)
        for header, rows in tables(text):
            self.rows += [header, *rows]
            columns = [i for i, name in enumerate(header) if name in COMPACT_COLUMNS]
            for row in rows:
                parent = _first_chain(row[0]) if row else None
                if parent is None:
                    continue
                for i in columns:
                    if i < len(row):
                        for span in CODE_SPAN.findall(row[i]):
                            match = DOTTED.fullmatch(span.strip())
                            if match:
                                self._add(f"{parent}.{match.group(0)}")

    def _add(self, chain: str) -> None:
        parts = chain.split(".")
        for end in range(1, len(parts) + 1):
            self.chains.add(".".join(parts[:end]))

    def names(self, short: str, full: list[str]) -> bool:
        """Whether `short`, or a longer tail of one of its import paths, is written."""
        if short in self.chains:
            return True
        depth = short.count(".") + 1
        for path in full:
            parts = path.split(".")
            for start in range(len(parts) - depth):
                if ".".join(parts[start:]) in self.chains:
                    return True
        return False

    def has_texture_row(self, name: str, value: int) -> bool:
        return any(f"`{name}`" in row and str(value) in row for row in self.rows)


def texture_rows(text: str) -> dict[str, dict[str, str]]:
    """The TextureFormat matrix (the table whose header starts `value | format`): cells by header, per format."""
    rows: dict[str, dict[str, str]] = {}
    for header, body in tables(text):
        if header[:2] == ["value", "format"]:
            for cells in body:
                row = dict(zip(header, cells, strict=False))
                rows[row["format"].strip("`")] = row
    return rows


def missing(symbols: dict, text: str) -> dict[str, list[str]]:
    """The symbols, by group, that `text` does not name the way the module docstring asks."""
    seen = Mentions(text)
    paths = symbols.get("paths", {})
    result: dict[str, list[str]] = {}
    for group in GROUPS:
        if group == "modules":
            absent = [rel for rel in symbols.get(group, []) if rel not in seen.paths]
        elif group == "texture_formats":
            formats = symbols.get(group, [])
            absent = [f"{name} ({value})" for name, value in formats if not seen.has_texture_row(name, value)]
        else:
            absent = [name for name in symbols.get(group, []) if not seen.names(name, paths.get(name, []))]
        if absent:
            result[group] = absent
    return result


def _installed_unitypy() -> str | None:
    try:
        import UnityPy
    except ImportError:
        return None
    return UnityPy.__version__


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n", 1)[0])
    sub = parser.add_subparsers(dest="command", required=True)
    list_parser = sub.add_parser("list", help="print UnityPy's public symbols by group")
    list_parser.add_argument("--json", action="store_true", help="print one JSON object instead of lines")
    list_parser.add_argument("--counts", action="store_true", help="print the number of symbols in each group")
    check_parser = sub.add_parser("check", help="report the symbols PARITY.md does not name")
    check_parser.add_argument("parity", nargs="?", type=Path, default=DEFAULT_PARITY)
    args = parser.parse_args(argv)

    version = _installed_unitypy()
    if version != PINNED_VERSION:
        print(f"inventory: needs UnityPy {PINNED_VERSION} (the oracle image), found {version}", file=sys.stderr)
        return 2
    symbols = collect()

    if args.command == "list":
        if args.json:
            print(json.dumps({group: symbols[group] for group in GROUPS}, indent=1))
        elif args.counts:
            for group in GROUPS:
                print(f"{group}\t{len(symbols[group])}")
        else:
            for group in GROUPS:
                for item in symbols[group]:
                    print(f"{group}\t{item[0] if isinstance(item, list) else item}")
        return 0

    try:
        text = args.parity.read_text(encoding="utf-8")
    except OSError as error:
        print(f"inventory: cannot read {args.parity}: {error.strerror}", file=sys.stderr)
        return 2
    absent = missing(symbols, text)
    total = sum(len(symbols[group]) for group in GROUPS)
    for group, items in absent.items():
        print(f"{group}: {len(items)} missing", file=sys.stderr)
        for item in items:
            print(f"  {item}", file=sys.stderr)
    count = sum(len(items) for items in absent.values())
    print(f"inventory: {total - count} of {total} symbols named in {args.parity.name}", file=sys.stderr)
    return 1 if count else 0


if __name__ == "__main__":
    sys.exit(main())
