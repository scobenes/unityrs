#!/usr/bin/env python3
"""List UnityPy's public surface, and check that PARITY.md names every part of it.

`DESIGN.md` section 3 asks for an inventory that maps every public UnityPy API and
behaviour to a tier. This script keeps that inventory complete: `list` prints what
the installed UnityPy defines, and `check` fails when PARITY.md leaves any of it out.

PARITY.md names each symbol inside a code span (backticks), by group:
- `modules`, every `.py` file under `UnityPy/`: its path, alone or with `:line`
  (`helpers/MeshHelper.py:504`). A path names its module and nothing else.
- `classes`, every public class defined in any module: its bare name (`Texture2D`),
  alone or inside a dotted name.
- `functions`, every public module-level function: `Module.function`
  (`ImportHelper.check_file_type`), where `Module` is the file's name without `.py`,
  or the package's name for an `__init__.py`.
- `data`, every public name a module assigns at its top level whose value is not a
  class, function, module or typing construct (tables, registries, settings,
  caches): `Module.NAME` (`CompressionHelper.DECOMPRESSION_MAP`), `Module` as above.
- `methods`, the public members of the API classes below and every member a
  `classes/legacy_patch` module adds: `Class.member` (`Texture2D.set_image`).
- `boost`, every public name UnityPyBoost exports: `UnityPyBoost.read_typetree`.
- `texture_formats`, every `TextureFormat` member: a table row with a cell
  `` `Name` `` and a cell holding its value.

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

# The classes whose members make up UnityPy's object API, as (module, class).
API_CLASSES = (
    ("environment", "Environment"),
    ("files.File", "File"),
    ("files.BundleFile", "BundleFile"),
    ("files.WebFile", "WebFile"),
    ("files.SerializedFile", "SerializedFile"),
    ("files.SerializedFile", "SerializedType"),
    ("files.ObjectReader", "ObjectReader"),
    ("classes.Object", "Object"),
    ("classes.PPtr", "PPtr"),
    ("classes.UnknownObject", "UnknownObject"),
    ("helpers.ContainerHelper", "ContainerHelper"),
    ("helpers.TypeTreeNode", "TypeTreeNode"),
    ("helpers.TypeTreeGenerator", "TypeTreeGenerator"),
    ("helpers.MeshHelper", "MeshHandler"),
    ("helpers.ArchiveStorageManager", "ArchiveStorageDecryptor"),
    ("helpers.UnityVersion", "UnityVersion"),
)

GROUPS = ("modules", "classes", "functions", "data", "methods", "boost", "texture_formats")

# The package whose modules add members to the generated classes.
LEGACY_PATCH = "UnityPy.classes.legacy_patch"

CODE_SPAN = re.compile(r"`([^`\n]+)`")
DOTTED = re.compile(r"[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*")
UNESCAPED_PIPE = re.compile(r"(?<!\\)\|")


def _module_label(rel: str) -> str:
    path = Path(rel)
    return path.parent.name if path.stem == "__init__" else path.stem


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
        or type(value).__module__ == "typing"
    )


def collect() -> dict[str, list]:
    """Every public symbol of the installed UnityPy, by group, in a stable order."""
    import UnityPy

    root = Path(UnityPy.__file__).resolve().parent
    groups: dict[str, list] = {group: [] for group in GROUPS}
    for path in sorted(root.rglob("*.py")):
        rel = path.relative_to(root).as_posix()
        if "__pycache__" in rel:
            continue
        groups["modules"].append(rel)
        dotted = "UnityPy." + rel[: -len(".py")].replace("/", ".")
        dotted = dotted.removesuffix(".__init__")
        module = importlib.import_module(dotted)
        label = _module_label(rel)
        for name, value in vars(module).items():
            if name.startswith("_") or getattr(value, "__module__", None) != module.__name__:
                continue
            if inspect.isclass(value):
                groups["classes"].append(name)
                for member, attr in vars(value).items():
                    if not member.startswith("_") and _is_member(attr) and _defined_in(attr, LEGACY_PATCH):
                        groups["methods"].append(f"{name}.{member}")
            elif inspect.isfunction(value):
                groups["functions"].append(f"{label}.{name}")
        for name in _assigned_names(path.read_text(encoding="utf-8")):
            if _is_data(getattr(module, name)):
                groups["data"].append(f"{label}.{name}")

    for module_name, class_name in API_CLASSES:
        cls = getattr(importlib.import_module(f"UnityPy.{module_name}"), class_name)
        for member, attr in vars(cls).items():
            if not member.startswith("_") and _is_member(attr):
                groups["methods"].append(f"{class_name}.{member}")

    boost = importlib.import_module("UnityPy.UnityPyBoost")
    groups["boost"] = [f"UnityPyBoost.{name}" for name in sorted(dir(boost)) if not name.startswith("_")]

    from UnityPy.enums import TextureFormat

    groups["texture_formats"] = [[member.name, member.value] for member in TextureFormat]
    for group in ("classes", "functions", "data", "methods"):
        groups[group] = sorted(set(groups[group]))
    return groups


def table_cells(line: str) -> list[str] | None:
    """The cells of a Markdown table line, or None for any other line."""
    line = line.strip()
    if not (line.startswith("|") and line.endswith("|") and len(line) > 1):
        return None
    return [cell.strip() for cell in UNESCAPED_PIPE.split(line[1:-1])]


class Mentions:
    """What a Markdown document names inside its code spans and table rows."""

    def __init__(self, text: str):
        self.paths: set[str] = set()
        self.names: set[str] = set()
        self.rows: list[list[str]] = []
        for span in CODE_SPAN.findall(text):
            path = span.strip().removeprefix("UnityPy/").split(":", 1)[0]
            if path.endswith(".py"):
                # A path names its module only: `files/BundleFile.py` does not name the class BundleFile.
                self.paths.add(path)
                continue
            for chain in DOTTED.findall(span):
                parts = chain.split(".")
                for start in range(len(parts)):
                    for end in range(start + 1, len(parts) + 1):
                        self.names.add(".".join(parts[start:end]))
        for line in text.splitlines():
            cells = table_cells(line)
            if cells is not None:
                self.rows.append(cells)

    def has_texture_row(self, name: str, value: int) -> bool:
        return any(f"`{name}`" in row and str(value) in row for row in self.rows)


def texture_rows(text: str) -> dict[str, dict[str, str]]:
    """The TextureFormat matrix (the table whose header starts `value | format`): cells by header, per format."""
    rows: dict[str, dict[str, str]] = {}
    header: list[str] | None = None
    for line in text.splitlines():
        cells = table_cells(line)
        if cells is None:
            header = None
        elif header is None:
            header = cells if cells[:2] == ["value", "format"] else []
        elif header and not set("".join(cells)) <= set("-: "):
            row = dict(zip(header, cells, strict=False))
            rows[row["format"].strip("`")] = row
    return rows


def missing(symbols: dict[str, list], text: str) -> dict[str, list[str]]:
    """The symbols, by group, that `text` does not name the way the module docstring asks."""
    seen = Mentions(text)
    result: dict[str, list[str]] = {}
    for group in GROUPS:
        if group == "modules":
            absent = [rel for rel in symbols.get(group, []) if rel not in seen.paths]
        elif group == "texture_formats":
            formats = symbols.get(group, [])
            absent = [f"{name} ({value})" for name, value in formats if not seen.has_texture_row(name, value)]
        else:
            absent = [name for name in symbols.get(group, []) if name not in seen.names]
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
            print(json.dumps(symbols, indent=1))
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
    total = sum(len(items) for items in symbols.values())
    for group, items in absent.items():
        print(f"{group}: {len(items)} missing", file=sys.stderr)
        for item in items:
            print(f"  {item}", file=sys.stderr)
    count = sum(len(items) for items in absent.values())
    print(f"inventory: {total - count} of {total} symbols named in {args.parity.name}", file=sys.stderr)
    return 1 if count else 0


if __name__ == "__main__":
    sys.exit(main())
