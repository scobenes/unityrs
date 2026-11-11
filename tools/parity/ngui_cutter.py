"""The NGUI atlas cutter's reference: docs/BATCH_SPEC.md section 5 (docs/plans/extract.md E21 to E26) on the oracle.

Written from section 5, 4.3 to 4.5 and E21 to E26, not ported from any Rust (docs/plans/extract.md section 3, item
3). Every byte it writes comes from the oracle stack: objects are read through UnityPy 1.25.2, a sheet is the image
UnityPy's `parse_image_data` gives (`Texture2D.image`'s, classes/legacy_patch/Texture2D.py:8-11), a crop is Pillow
9.5.0's `Image.crop` (whose zero padding is E23's definition), each image is written by Pillow's
`save(fp, "PNG")`, and the atlas JSON is CPython 3.11's `json.dumps(value, indent=2)` (E24). Its own logic is the
spec's, each rule cited where it is applied:

- the reads (`ngui_read.py`): the view's Environment, sealed; recognition from the typetree; the sheet's name
  (step 1, whose `PPtr.deref` may cross into the view's other bundles) and its texture among the input's own, by
  the name `peek_name` reads (steps 1 and 2); a streamed sheet's resource by 4.3's own-chain procedure, over each
  container's directory rows; and each failure classified as `batch.md` 2.7 classifies it, a `check_read`
  refusal `asset_bundle`;
- the plan of 4.4 (names, the skips that need no pixels, keys with 2.1's templates and 2.3's rules but
  `prefix_conflict` and `archive_conflict`, E30's later sprite), and its export order;
- the units of 4.5 (E26, E32); their winners across inputs (4.5's steps 1 to 3, with E31 and E61) are
  `ngui_winners.py`'s.

**Its input** is one input's view (4.2): `{bundle id: bytes}`, the input among them. The input's own SerializedFiles
are the SerializedFile values of its bundle's `files` (section 3).

**Its output** (`Result`) is one entry per file the cutter produces: each `Output` (kind `texture`, when the options
name a texture key, for a sheet's texture, in its unit, and for every other texture, a plain candidate;
`atlas_json`; `atlas_sprite`) with its key, bytes and unit key; each `Skip` with its object, the output it skips
(`texture`, `atlas` for an atlas's JSON and crops together, `atlas_json`, `atlas_sprite` with the sprite's index),
its reason and detail (7.3's `skips` row); the input's `unresolved_scripts`; and each atlas whose sheet was found,
with the step that found it (`sheets`, `material_name` or `atlas_name`: no record names it, a check's). Sprites
(`outputs.sprites`), budgets and 4.6's prefix conflicts are not modelled.

    python tools/parity/ngui_cutter.py --input ID --bundle ID=PATH [--bundle ID=PATH ...] [--out DIR] \\
        [--texture-key T] [--sprite-key T] [--json-key T] [--script-class C ...] [--skip-name-suffix S ...]

prints the result as JSON (each output's bytes as its sha256 and md5) and, with `--out`, writes each output under
DIR at its key. The bundles given are the input's view.
"""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import re
import sys
from dataclasses import dataclass, field
from pathlib import Path

from UnityPy.enums import ClassIDType
from UnityPy.files import SerializedFile

import ngui_read as rd

VARIABLES = {
    "texture": {"input", "safe_input", "type", "name", "safe_name", "path_id"},
    "atlas_json": {"input", "safe_input", "path_id", "atlas", "safe_atlas"},
    "atlas_sprite": {"input", "safe_input", "path_id", "atlas", "safe_atlas", "sprite", "safe_sprite"},
}
bytewise = rd.bytewise


@dataclass(frozen=True)
class Options:
    """The cutter's options (BATCH_SPEC 1.5's `atlases`; `texture_key` is `outputs.textures`' key, or None)."""

    sprite_key: str = "{atlas}/{sprite}.png"
    json_key: str = "{atlas}.json"
    texture_key: str | None = None
    script_classes: tuple[str, ...] = ("NGUIAtlas",)
    skip_name_suffixes: tuple[str, ...] = ()
    sheet_scope: str = "own_files_by_name"
    prefix: str = ""


@dataclass(frozen=True)
class Output:
    kind: str
    key: str
    data: bytes
    serialized_file: str
    path_id: int
    sprite: int | None = None
    unit: str | None = None
    mode: str | None = None
    size: tuple[int, int] | None = None


@dataclass(frozen=True)
class Skip:
    serialized_file: str
    path_id: int
    cls: str
    output: str
    reason: str
    sprite: int | None = None
    detail: str | None = None


@dataclass
class Result:
    input: str
    outputs: list[Output] = field(default_factory=list)
    skips: list[Skip] = field(default_factory=list)
    unresolved_scripts: int = 0
    sheets: list[tuple[str, int, str, str, int]] = field(default_factory=list)


# ------------------------------------------------------------------ keys (BATCH_SPEC 2.1 to 2.3)


def raw(name: str) -> bytes:
    """A name's stored bytes: UnityPy decodes a string with `surrogateescape`."""
    return name.encode("utf-8", "surrogateescape")


def safe(value: bytes) -> bytes:
    """2.2: per code point (an invalid byte one of its own), outside `[A-Za-z0-9._-]` to `_`, strip `.`."""
    text = re.sub(r"[^A-Za-z0-9._-]", "_", value.decode("utf-8", "surrogateescape")).strip(".")
    return (text or "unnamed").encode("ascii")


def expand(template: str, kind: str, values: dict[str, bytes]) -> bytes:
    """2.1: `{{` and `}}` one brace, `{name}` a variable's bytes, its `safe_` form through `safe()`."""
    out, i = bytearray(), 0
    while i < len(template):
        if template.startswith(("{{", "}}"), i):
            out += template[i].encode()
            i += 2
        elif template[i] == "{":
            end = template.index("}", i)
            name = template[i + 1 : end]
            if name not in VARIABLES[kind]:
                raise ValueError(f"{template!r}: {{{name}}} is not a variable of {kind}")
            out += safe(values[name[5:]]) if name.startswith("safe_") else values[name]
            i = end + 1
        elif template[i] == "}":
            raise ValueError(f"{template!r}: a lone }}")
        else:
            out += template[i].encode()
            i += 1
    return bytes(out)


def _rule(key: bytes) -> str | None:
    if not key:
        return "empty"
    try:
        text = key.decode("utf-8")
    except UnicodeDecodeError:
        return "not_utf8"
    if any(ord(c) < 0x20 or ord(c) == 0x7F for c in text):
        return "control_char"
    if key.startswith(b"/"):
        return "leading_slash"
    if key.endswith(b"/"):
        return "trailing_slash"
    segments = key.split(b"/")
    checks = (
        ("empty_segment", b"//" in key),
        ("dot_segment", any(s in (b".", b"..") for s in segments)),
        ("segment_too_long", any(len(s) > 255 for s in segments)),
        ("too_long", len(key) > 1024),
        ("reserved_prefix", key == b"_unityrs" or key.startswith(b"_unityrs/")),
        ("reserved_name", any(s.startswith(b".unityrs-tmp-") for s in segments)),
    )
    return next((name for name, broken in checks if broken), None)


def key_refusal(key: bytes, prefix: bytes = b"") -> str | None:
    """2.3's first broken rule, but `prefix_conflict` and `archive_conflict`: as given, then after the prefix."""
    return _rule(key) or (_rule(prefix + key) if prefix else None)


def view_of(deps: dict[str, list[str]], x: str) -> list[str]:
    """4.2: the smallest set holding `x` and the `deps` of every bundle it holds, in bytewise order of id."""
    seen, todo = {x}, [x]
    while todo:
        for d in deps.get(todo.pop(), []):
            if d not in seen:
                seen.add(d)
                todo.append(d)
    return sorted(seen, key=bytewise)


def png(image) -> bytes:
    buf = io.BytesIO()
    image.save(buf, "PNG")
    return buf.getvalue()


# ------------------------------------------------------------------ the plan (BATCH_SPEC 4.4) and the export


@dataclass(eq=False)
class Tex:
    """An own Texture2D: its name as `peek_name` reads it, its object read, or the read's failure."""

    sf: SerializedFile
    obj: object
    name: str | None
    texture: object
    failure: rd.Failure | None


@dataclass(eq=False)
class Item:
    """One planned output, in plan order."""

    kind: str
    pos: tuple
    sf: SerializedFile
    obj: object
    key: bytes
    atlas: Atlas | None = None
    sprite: int | None = None
    dropped: bool = False
    tex: Tex | None = None


@dataclass(eq=False)
class Atlas:
    sf: SerializedFile
    obj: object
    name: str
    tree: dict
    sheet: Tex
    items: list[Item] = field(default_factory=list)


def own_textures(objects) -> list[Tex]:
    """Every own Texture2D, in object order, read once without its pixels: its name peeked (4.4 step 1; a failed
    peek gives no name, and its failure is the texture's), then its object read."""
    out = []
    for sf, obj in objects:
        if obj.type != ClassIDType.Texture2D:
            continue
        try:
            name = rd.shaped(rd.read(obj.peek_name), str)
        except rd.Failure as f:
            out.append(Tex(sf, obj, None, None, f))
            continue
        try:
            out.append(Tex(sf, obj, name, rd.texture_of(obj), None))
        except rd.Failure as f:
            out.append(Tex(sf, obj, name, None, f))
    return out


def cut(input_id: str, view: dict[str, bytes], opts: Options | None = None) -> Result:
    """Section 5 on one input over its view."""
    opts = opts or Options()
    if opts.sheet_scope != "own_files_by_name":
        raise ValueError(f"sheet scope {opts.sheet_scope!r}: version 1 has only own_files_by_name")
    bundle = rd.load(view, input_id)
    result = Result(input_id)
    objects = [(sf, o) for sf in bundle.files.values() if isinstance(sf, SerializedFile) for o in sf.objects.values()]
    common = {"input": raw(input_id)}
    plan: list[tuple[tuple, Skip | Item]] = []  # (plan position, a step 1 or 2 skip, or a planned output)

    def skip(sf, obj, cls, output, reason, sprite=None, detail=None):
        return Skip(sf.name, obj.path_id, cls, output, reason, sprite, detail)

    textures = own_textures(objects)

    def sheet_of(atlas_name: str, tree: dict, obj) -> tuple[Tex | None, str | None]:
        """Steps 1 and 2: the first own texture of the material's name, else of the atlas's, and which."""
        for step, name in (("material_name", rd.maintex_name(tree, obj)), ("atlas_name", atlas_name)):
            for t in textures:
                if name is not None and t.name is not None and raw(t.name) == raw(name):
                    return t, step
        return None, None

    # Kind `textures`, when exported: every own Texture2D (plan position 0, by object order).
    sheet_items: dict[int, Item] = {}
    if opts.texture_key is not None:
        names = set()
        for n, t in enumerate(textures):
            pos = (0, n)
            if t.name is not None and raw(t.name) in names:  # E29: the first of one m_Name takes it
                plan.append((pos, skip(t.sf, t.obj, "Texture2D", "texture", "duplicate_name")))
                continue
            if t.name is not None:
                names.add(raw(t.name))
            reason, detail = (t.failure.reason, t.failure.detail) if t.failure else (None, None)
            reason = reason or rd.selection(t.texture, t.obj.platform)
            if reason:
                plan.append((pos, skip(t.sf, t.obj, "Texture2D", "texture", reason, detail=detail)))
                continue
            values = {**common, "type": b"Texture2D", "name": raw(t.name or ""), "path_id": str(t.obj.path_id).encode()}
            item = Item("texture", pos, t.sf, t.obj, expand(opts.texture_key, "texture", values), tex=t)
            sheet_items[id(t.obj)] = item
            plan.append((pos, item))

    # Kind `atlases`: every MonoBehaviour, in object order; each atlas's JSON, then its sprites.
    atlases: list[Atlas] = []
    names = set()
    for n, (sf, obj) in enumerate(objects):
        if obj.type != ClassIDType.MonoBehaviour:
            continue
        pos = (2, n, -1)
        kind, name = rd.recognise(obj, opts.script_classes)
        if kind == "unresolved":
            result.unresolved_scripts += 1
            continue
        if kind == "none":
            continue
        if raw(name) in names:  # E29: the first of one class and one m_Name takes it
            plan.append((pos, skip(sf, obj, "MonoBehaviour", "atlas", "duplicate_name")))
            continue
        names.add(raw(name))
        # 4.4 step 2's order for an atlas.
        reason = {"no_typetree": "no_typetree", "layout": "ngui_layout_unsupported"}.get(kind)
        detail = None
        if reason is None and name == "":
            reason = "unnamed_atlas"
        if reason is None and any(raw(name).endswith(raw(s)) for s in opts.skip_name_suffixes):
            reason = "reference_atlas"
        if reason is None:
            try:
                tree = rd.atlas_tree(obj)
            except rd.Failure as f:  # the atlas's own read, before its sheet (section 5)
                reason, detail = f.reason, f.detail
        if reason is None:
            sheet, step = sheet_of(name, tree, obj)
            if sheet is None:
                reason = "sheet_not_found"
            else:
                result.sheets.append((sf.name, obj.path_id, step, sheet.sf.name, sheet.obj.path_id))
                if sheet.failure:  # the matched texture's read: the sheet's answer
                    reason, detail = sheet.failure.reason, sheet.failure.detail
                else:
                    reason = rd.selection(sheet.texture, sheet.obj.platform)
        if reason is not None:
            plan.append((pos, skip(sf, obj, "MonoBehaviour", "atlas", reason, detail=detail)))
            continue
        atlas = Atlas(sf, obj, name, tree, sheet)
        atlases.append(atlas)
        values = {**common, "path_id": str(obj.path_id).encode(), "atlas": raw(name)}
        item = Item("atlas_json", pos, sf, obj, expand(opts.json_key, "atlas_json", values), atlas)
        atlas.items.append(item)
        plan.append((pos, item))
        for i, element in enumerate(tree["mSprites"]):
            x, y, width, height = (element[k] for k in rd.RECT)  # integers, by recognition 2
            spos = (2, n, i)
            refusal = "empty_rect" if width <= 0 or height <= 0 else None
            crop = None if refusal else rd.crop_refusal(x, y, width, height)
            if refusal or crop:
                reason, detail = ("decode_error", crop) if crop else (refusal, None)
                plan.append((spos, skip(sf, obj, "MonoBehaviour", "atlas_sprite", reason, i, detail)))
                continue
            key = expand(opts.sprite_key, "atlas_sprite", {**values, "sprite": raw(element["name"])})
            sitem = Item("atlas_sprite", spos, sf, obj, key, atlas, i)
            atlas.items.append(sitem)
            plan.append((spos, sitem))

    # Step 3, keys: the first in plan order takes a key, but of two sprites of one atlas the later (E30).
    plan.sort(key=lambda entry: entry[0])
    prefix = opts.prefix.encode()
    owner: dict[bytes, Item] = {}
    skips: list[tuple[tuple, Skip]] = []
    for pos, entry in plan:
        if isinstance(entry, Skip):
            skips.append((pos, entry))
            continue
        cls = "Texture2D" if entry.kind == "texture" else "MonoBehaviour"
        rule = key_refusal(entry.key, prefix)
        held = owner.get(entry.key)
        if rule:
            entry.dropped = True
            skips.append((pos, skip(entry.sf, entry.obj, cls, entry.kind, f"unwritable_key({rule})", entry.sprite)))
        elif held is None:
            owner[entry.key] = entry
        elif entry.kind == held.kind == "atlas_sprite" and entry.atlas is held.atlas:
            held.dropped = True
            owner[entry.key] = entry
            skips.append((held.pos, skip(held.sf, held.obj, cls, held.kind, "duplicate_key", held.sprite)))
        else:
            entry.dropped = True
            skips.append((pos, skip(entry.sf, entry.obj, cls, entry.kind, "duplicate_key", entry.sprite)))
    skips.sort(key=lambda s: s[0])
    result.skips = [s for _, s in skips]
    export(result, atlases, sheet_items, skip)
    return result


def export(result: Result, atlases: list[Atlas], sheet_items: dict[int, Item], skip) -> None:
    """4.4's export order: each sheet with the atlases that cut it, in the order of each group's first atlas,
    then every other planned texture in plan order (a plain candidate)."""
    groups: dict[int, list[Atlas]] = {}
    for atlas in atlases:
        groups.setdefault(id(atlas.sheet.obj), []).append(atlas)
    for group in groups.values():
        t = group[0].sheet
        try:
            image, failure = rd.decode(t.texture, t.obj), None
        except rd.Failure as f:
            image, failure = None, f
        unit = None
        titem = sheet_items.get(id(t.obj))
        if titem is not None and not titem.dropped:
            if failure:
                result.skips.append(skip(t.sf, t.obj, "Texture2D", "texture", failure.reason, detail=failure.detail))
            else:
                unit = titem.key.decode()
                result.outputs.append(
                    Output("texture", unit, png(image), t.sf.name, t.obj.path_id, unit=unit, mode=image.mode,
                           size=image.size)
                )  # fmt: skip
        for atlas in group:
            live = [i for i in atlas.items if not i.dropped]
            if not live:
                continue
            if failure:
                result.skips.append(
                    skip(atlas.sf, atlas.obj, "MonoBehaviour", "atlas", failure.reason, detail=failure.detail)
                )
                continue
            made = [o for o in (made_output(atlas, item, image, result, skip) for item in live) if o]
            # 4.5: the sheet's texture output when made; else the atlas's JSON when made; else no unit.
            own = unit or next((o.key for o in made if o.kind == "atlas_json"), None)
            result.outputs += [Output(**{**o.__dict__, "unit": own}) for o in made]
    cut_sheets = {id(a.sheet.obj) for a in atlases}
    for item in sheet_items.values():
        if id(item.obj) in cut_sheets or item.dropped:
            continue
        try:
            image = rd.decode(item.tex.texture, item.obj)
        except rd.Failure as f:
            result.skips.append(skip(item.sf, item.obj, "Texture2D", "texture", f.reason, detail=f.detail))
            continue
        result.outputs.append(
            Output("texture", item.key.decode(), png(image), item.sf.name, item.obj.path_id, mode=image.mode,
                   size=image.size)
        )  # fmt: skip


def made_output(atlas: Atlas, item: Item, image, result: Result, skip) -> Output | None:
    """A live item's output: the JSON (E24; `json_unwritable`, E25, recorded) or a crop (E23)."""
    if item.kind == "atlas_json":
        sprites = atlas.tree["mSprites"]
        value = {
            "name": atlas.name,
            "texture_width": image.width,
            "texture_height": image.height,
            "pixel_size": atlas.tree.get("mPixelSize"),
            "settings": atlas.tree.get("mSettings"),
            "sprite_count": len(sprites),
            "sprites": sprites,
        }
        try:
            text = json.dumps(value, indent=2)
        except TypeError:  # E25: a value json.dumps cannot write (bytes)
            result.skips.append(skip(atlas.sf, atlas.obj, "MonoBehaviour", "atlas_json", "json_unwritable"))
            return None
        return Output("atlas_json", item.key.decode(), text.encode("ascii"), atlas.sf.name, atlas.obj.path_id)
    element = atlas.tree["mSprites"][item.sprite]
    x, y, width, height = (element[k] for k in rd.RECT)
    crop = image.crop((x, y, x + width, y + height))
    return Output("atlas_sprite", item.key.decode(), png(crop), atlas.sf.name, atlas.obj.path_id,
                  sprite=item.sprite, mode=crop.mode, size=crop.size)  # fmt: skip


# ------------------------------------------------------------------ reports and the CLI


def report(result: Result) -> dict:
    """The result as JSON: each output's bytes as its sha256 and md5, each image's mode and size."""
    outputs = []
    for o in result.outputs:
        row = {"kind": o.kind, "key": o.key, "unit": o.unit, "serialized_file": o.serialized_file,
               "path_id": o.path_id, "sprite": o.sprite, "sha256": hashlib.sha256(o.data).hexdigest(),
               "md5": hashlib.md5(o.data).hexdigest()}  # fmt: skip
        if o.mode is not None:
            row["mode"], row["size"] = o.mode, list(o.size)
        outputs.append(row)
    return {
        "input": result.input,
        "outputs": outputs,
        "skips": [s.__dict__ for s in result.skips],
        "unresolved_scripts": result.unresolved_scripts,
        "sheets": [list(t) for t in result.sheets],
    }


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--input", required=True, help="the input's bundle id")
    ap.add_argument("--bundle", action="append", required=True, metavar="ID=PATH", help="a bundle of the view")
    ap.add_argument("--out", type=Path, help="write each output under this folder at its key")
    ap.add_argument("--texture-key", help="outputs.textures' key: the sheets' textures join their atlases' units")
    ap.add_argument("--sprite-key", default=Options.sprite_key)
    ap.add_argument("--json-key", default=Options.json_key)
    ap.add_argument("--script-class", action="append", help="default: NGUIAtlas")
    ap.add_argument("--skip-name-suffix", action="append", default=[])
    args = ap.parse_args(argv)
    view = {}
    for b in args.bundle:
        bid, sep, path = b.partition("=")
        if not sep:
            ap.error(f"--bundle {b!r} is not ID=PATH")
        view[bid] = Path(path).read_bytes()
    if args.input not in view:
        ap.error(f"--input {args.input!r} is no --bundle")
    opts = Options(
        sprite_key=args.sprite_key,
        json_key=args.json_key,
        texture_key=args.texture_key,
        script_classes=tuple(args.script_class or Options.script_classes),
        skip_name_suffixes=tuple(args.skip_name_suffix),
    )
    result = cut(args.input, view, opts)
    if args.out is not None:
        for o in result.outputs:
            path = args.out / o.key
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(o.data)
    json.dump(report(result), sys.stdout, indent=1)
    print()
    return 0


if __name__ == "__main__":
    sys.exit(main())
