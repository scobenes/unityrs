"""The NGUI atlas cutter's reference: docs/BATCH_SPEC.md section 5 (docs/plans/extract.md E21 to E26) on the oracle.

Written from section 5 and E21 to E26, not ported from any Rust (docs/plans/extract.md section 3, item 3). Every
byte it writes comes from the oracle stack: objects are read through UnityPy 1.25.2, a sheet is `Texture2D.image`
(classes/legacy_patch/Texture2D.py:8-11), a crop is Pillow 9.5.0's `Image.crop` (whose zero padding is E23's
definition), each image is written by Pillow's `save(fp, "PNG")`, and the atlas JSON is CPython 3.11's
`json.dumps(value, indent=2)` (E24). Its own logic is selection only, each rule cited where it is applied:

- recognition and the skipped atlases (section 5's first two paragraphs);
- the sheet, scope `own_files_by_name` (section 5's "The sheet", E21): step 1 dereferences with UnityPy's
  `PPtr.deref` (classes/PPtr.py:43-90), so a name read may cross into the view's other bundles, and reads the
  texture's name with `ObjectReader.peek_name` (files/ObjectReader.py:189-197); step 2 matches it, or the atlas's
  name, among the input's own Texture2D objects only;
- the plan of docs/BATCH_SPEC.md 4.4 (names, the skips that need no pixels, keys with 2.1's templates and 2.3's
  rules but `prefix_conflict` and `archive_conflict`, E30's later sprite), and its export order;
- the units of 4.5 (E26, E32); their winners across inputs (4.5's steps 1 to 3, with E31 and E61) are
  `ngui_winners.py`'s.

**Its input** is one input's view (4.2): `{bundle id: bytes}`, the input among them. As the oracle side of the
gate builds it (docs/plans/extract.md section 5), one UnityPy Environment with an empty `path`, so `find_file`
never walks a directory, loads the view's other bundles with `is_dependency=True` in bytewise order of id, then
the input. The input's own SerializedFiles are the SerializedFile values of its bundle's `files` (section 3).

**Its output** (`Result`) is one entry per file the cutter produces: each `Output` (kind `texture` for a sheet's
texture when the options name a texture key, `atlas_json`, `atlas_sprite`) with its key, bytes and unit key;
each `Skip` with its object, the output it skips (`atlas`, for an atlas's JSON and crops together, `atlas_json`,
`atlas_sprite` with the sprite's index, `texture`), its reason and a detail; the input's `unresolved_scripts`;
the textures the plan gave a key that no atlas cuts, which the reference does not export (`uncut_textures`: the
texture exporter's, E15 and E66); and each atlas whose sheet was found, with the step that found it (`sheets`,
`material_name` or `atlas_name`: no record names it, a check's). Sprites (`outputs.sprites`) are not planned here.

**Errors.** `batch.md` 2.7's object-level classes are UnityPy's exceptions in `OBJECT_SKIP`; the reference has no
budgets, so nothing here is of the budget class. A case section 5 does not cover raises `SpecGap`; any other
exception is not classified and propagates, as an unknown error stops the engine (`internal`). A detail is the
exception's type name, or a texture decode's code (BATCH_SPEC 7.3); the engine's error codes are not UnityPy's.

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
import struct
import sys
from dataclasses import dataclass, field
from pathlib import Path

import UnityPy
from PIL import Image
from UnityPy.classes import PPtr
from UnityPy.enums import BuildTarget, ClassIDType, TextureFormat
from UnityPy.exceptions import TypeTreeError
from UnityPy.export import Texture2DConverter
from UnityPy.files import SerializedFile
from UnityPy.helpers import TextureSwizzler

# The errors `batch.md` 2.7 makes an object's skip, as UnityPy raises them: a null pointer (`ValueError`), an
# `m_FileID` past the externals or a CAB outside the view (`FileNotFoundError`), a missing object or field
# (`KeyError`), and the reader's and the tree's own (`struct.error`, `EOFError`, `TypeTreeError`, a value of the
# wrong shape).
OBJECT_SKIP = (ValueError, KeyError, IndexError, TypeError, EOFError, struct.error, FileNotFoundError, TypeTreeError)

FIVE = ("RGB24", "RGBA32", "DXT1", "DXT5", "BC7")
TEXTURE_PIXELS = 268_435_456
VARIABLES = {
    "texture": {"input", "safe_input", "type", "name", "safe_name", "path_id"},
    "atlas_json": {"input", "safe_input", "path_id", "atlas", "safe_atlas"},
    "atlas_sprite": {"input", "safe_input", "path_id", "atlas", "safe_atlas", "sprite", "safe_sprite"},
}


class SpecGap(Exception):
    """A case docs/BATCH_SPEC.md section 5 gives no answer for."""


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
    uncut_textures: list[tuple[str, int, str]] = field(default_factory=list)
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


# ------------------------------------------------------------------ reading


def bytewise(text: str) -> bytes:
    return text.encode("utf-8")


def view_of(deps: dict[str, list[str]], x: str) -> list[str]:
    """4.2: the smallest set holding `x` and the `deps` of every bundle it holds, in bytewise order of id."""
    seen, todo = {x}, [x]
    while todo:
        for d in deps.get(todo.pop(), []):
            if d not in seen:
                seen.add(d)
                todo.append(d)
    return sorted(seen, key=bytewise)


def load(view: dict[str, bytes], input_id: str):
    env = UnityPy.Environment(path="")
    for bid in sorted((b for b in view if b != input_id), key=bytewise):
        env.load_file(view[bid], name=bid, is_dependency=True)
    return env.load_file(view[input_id], name=input_id)


def deref(pointer: dict, assets_file):
    """A typetree pointer value dereferenced from the file that holds it (classes/PPtr.py:43-90)."""
    return PPtr(m_FileID=pointer["m_FileID"], m_PathID=pointer["m_PathID"], assetsfile=assets_file).deref()


def recognise(obj, opts: Options) -> tuple[str, str | None]:
    """Section 5's recognition: `none` (no atlas, not counted), `unresolved` (`m_Script`'s read failed, counted),
    `no_typetree`, `layout` (`ngui_layout_unsupported`) or `atlas`, with the MonoBehaviour's `m_Name`. `m_Script`
    is read with UnityPy's MonoBehaviour head (files/ObjectReader.py:325-330), which needs no typetree."""
    try:
        head = obj.parse_monobehaviour_head()
        script = head.m_Script.deref()
        if script.type != ClassIDType.MonoScript:
            return "none", None
        class_name = script.parse_as_dict()["m_ClassName"]
    except OBJECT_SKIP:
        return "unresolved", None
    if class_name not in opts.script_classes:
        return "none", None
    node = obj.serialized_type.node if obj.serialized_type is not None else None
    if node is None:
        return "no_typetree", head.m_Name
    # A field `mSprites` of the root that is an array: a node whose first child is `Array`, UnityPy's vector
    # (helpers/TypeTreeHelper.py:182-263).
    if not any(c.m_Name == "mSprites" and c.m_Children and c.m_Children[0].m_Type == "Array" for c in node.m_Children):
        return "layout", head.m_Name
    return "atlas", head.m_Name


def maintex_name(tree: dict, obj) -> str | None:
    """Step 1: the material's `_MainTex` texture's `m_Name`; None for no name, an object-level error among them."""
    try:
        material = deref(tree["material"], obj.assets_file)
        for first, second in material.parse_as_dict()["m_SavedProperties"]["m_TexEnvs"]:
            # The first a string, or a struct whose `name` is (classes/generated.py:11477).
            if first == "_MainTex" or (isinstance(first, dict) and first.get("name") == "_MainTex"):
                name = deref(second["m_Texture"], material.assets_file).peek_name()
                return name if isinstance(name, str) else None
    except OBJECT_SKIP:
        return None
    return None


def format_name(value: int) -> str:
    try:
        return TextureFormat(value).name
    except ValueError:
        return str(value)


def selection(texture, platform: int) -> str | None:
    """Section 3's answers 1 to 3, the skips that need no pixels, with UnityPy's predicates: a side of 0
    (export/Texture2DConverter.py:265-266), the format, then the platform (helpers/TextureSwizzler.py:121-127,
    export/Texture2DConverter.py:569)."""
    if not texture.m_Width or not texture.m_Height:
        return "empty_image"
    name = format_name(texture.m_TextureFormat)
    if name not in FIVE:
        return f"unsupported_format({name})"
    if TextureSwizzler.is_switch_swizzled(platform, texture.m_PlatformBlob):
        return "unsupported_platform(switch)"
    if platform == BuildTarget.XBOX360 and TextureFormat(texture.m_TextureFormat) in Texture2DConverter.XBOX_SWAP_FORMATS:
        return "unsupported_platform(xbox360)"
    return None


def decode(texture):
    """Section 3's answers 5 to 10 and the image: `Texture2D.image`, or a skip's (reason, detail)."""
    width, height = texture.m_Width, texture.m_Height
    try:
        data = texture.get_image_data()  # classes/legacy_patch/Texture2D.py:63-75
    except ValueError as e:
        if str(e) != "No image data found":
            raise
        data = b""
    if not data:
        return "decode_error", "no_image_data"
    if width < 0 or height < 0:
        return "decode_error", "negative_size"
    if width * height > TEXTURE_PIXELS:
        return f"limit(texture_pixels, {TEXTURE_PIXELS})", None
    try:
        return texture.image
    except MemoryError:
        return "decode_error", "pillow_memory_error"
    except Image.DecompressionBombError:
        return "decode_error", "decompression_bomb"
    except ValueError as e:
        if str(e) != "not enough image data":
            raise
        return "decode_error", "not_enough_data"


def png(image) -> bytes:
    buf = io.BytesIO()
    image.save(buf, "PNG")
    return buf.getvalue()


def rect(sprite) -> tuple[int, int, int, int]:
    """The element's `x`, `y`, `width` and `height` as integers."""
    if not isinstance(sprite, dict):
        raise SpecGap(f"an mSprites element that is no struct: {type(sprite).__name__}")
    out = []
    for k in ("x", "y", "width", "height"):
        v = sprite.get(k)
        if not isinstance(v, (int, float)):
            raise SpecGap(f"an mSprites element whose {k} is {type(v).__name__}, not a number")
        out.append(int(v))
    return out[0], out[1], out[2], out[3]


# ------------------------------------------------------------------ the plan (BATCH_SPEC 4.4) and the export


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


@dataclass(eq=False)
class Atlas:
    sf: SerializedFile
    obj: object
    name: str
    tree: dict
    sheet: object
    items: list[Item] = field(default_factory=list)


def cut(input_id: str, view: dict[str, bytes], opts: Options = Options()) -> Result:
    """Section 5 on one input over its view."""
    if opts.sheet_scope != "own_files_by_name":
        raise ValueError(f"sheet scope {opts.sheet_scope!r}: version 1 has only own_files_by_name")
    bundle = load(view, input_id)
    result = Result(input_id)
    objects = [(sf, o) for sf in bundle.files.values() if isinstance(sf, SerializedFile) for o in sf.objects.values()]
    common = {"input": raw(input_id)}
    plan: list[tuple[tuple, Skip | Item]] = []  # (plan position, a step 1 or 2 skip, or a planned output)

    def skip(sf, obj, cls, output, reason, sprite=None, detail=None):
        return Skip(sf.name, obj.path_id, cls, output, reason, sprite, detail)

    # Texture2D objects, each read once (no pixels): their names, for step 2 and for E29's names.
    textures = []
    for sf, obj in objects:
        if obj.type == ClassIDType.Texture2D:
            try:
                textures.append((sf, obj, obj.peek_name(), obj.read(), None))
            except OBJECT_SKIP as e:
                textures.append((sf, obj, None, None, type(e).__name__))

    def sheet_of(atlas_name: str, tree: dict, obj):
        """Steps 1 and 2: the first own texture of the material's name, else of the atlas's, and which."""
        for step, name in (("material_name", maintex_name(tree, obj)), ("atlas_name", atlas_name)):
            for t in textures:
                if name is not None and t[2] is not None and raw(t[2]) == raw(name):
                    return t, step
        return None, None

    # Kind `textures`, when exported: every own Texture2D (plan position 0, by object order).
    sheet_items = {}
    if opts.texture_key is not None:
        names = set()
        for n, (sf, obj, name, tex, failed) in enumerate(textures):
            pos = (0, n)
            if failed:
                plan.append((pos, skip(sf, obj, "Texture2D", "texture", "decode_error", detail=failed)))
                continue
            if name in names:
                plan.append((pos, skip(sf, obj, "Texture2D", "texture", "duplicate_name")))
                continue
            names.add(name)
            reason = selection(tex, obj.platform)
            if reason:
                plan.append((pos, skip(sf, obj, "Texture2D", "texture", reason)))
                continue
            values = {**common, "type": b"Texture2D", "name": raw(name), "path_id": str(obj.path_id).encode()}
            item = Item("texture", pos, sf, obj, expand(opts.texture_key, "texture", values))
            sheet_items[id(obj)] = item
            plan.append((pos, item))

    # Kind `atlases`: every MonoBehaviour, in object order; each atlas's JSON, then its sprites.
    atlases: list[Atlas] = []
    names = set()
    for n, (sf, obj) in enumerate(objects):
        if obj.type != ClassIDType.MonoBehaviour:
            continue
        pos = (2, n, -1)
        kind, name = recognise(obj, opts)
        if kind == "unresolved":
            result.unresolved_scripts += 1
            continue
        if kind == "none":
            continue
        if name in names:  # E29: the first of one class and one m_Name takes it
            plan.append((pos, skip(sf, obj, "MonoBehaviour", "atlas", "duplicate_name")))
            continue
        names.add(name)
        reason = {"no_typetree": "no_typetree", "layout": "ngui_layout_unsupported"}.get(kind)
        if reason is None and name == "":
            reason = "unnamed_atlas"
        if reason is None and any(raw(name).endswith(raw(s)) for s in opts.skip_name_suffixes):
            reason = "reference_atlas"
        detail = None
        if reason is None:
            try:
                tree = obj.parse_as_dict()
            except OBJECT_SKIP as e:
                reason, detail = "decode_error", type(e).__name__
        if reason is None:
            sheet, step = sheet_of(name, tree, obj)
            if sheet is None:
                reason = "sheet_not_found"
            else:
                result.sheets.append((sf.name, obj.path_id, step, sheet[0].name, sheet[1].path_id))
                reason = selection(sheet[3], sheet[1].platform)
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
            x, y, width, height = rect(element)
            spos = (2, n, i)
            if width <= 0 or height <= 0:
                plan.append((spos, skip(sf, obj, "MonoBehaviour", "atlas_sprite", "empty_rect", sprite=i)))
                continue
            if not isinstance(element.get("name"), str):
                raise SpecGap(f"an mSprites element whose name is {type(element.get('name')).__name__}")
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

    # The export: each sheet with the atlases that cut it, in the order of each group's first atlas.
    groups: dict[int, list[Atlas]] = {}
    for atlas in atlases:
        groups.setdefault(id(atlas.sheet[1]), []).append(atlas)
    for group in groups.values():
        sheet_sf, sheet_obj, _, texture, _ = group[0].sheet
        image = decode(texture)
        failure = image if isinstance(image, tuple) else None
        unit = None
        titem = sheet_items.get(id(sheet_obj))
        if titem is not None and not titem.dropped:
            if failure:
                result.skips.append(skip(sheet_sf, sheet_obj, "Texture2D", "texture", failure[0], detail=failure[1]))
            else:
                unit = titem.key.decode()
                result.outputs.append(
                    Output("texture", unit, png(image), sheet_sf.name, sheet_obj.path_id,
                           unit=unit, mode=image.mode, size=image.size)
                )  # fmt: skip
        for atlas in group:
            live = [i for i in atlas.items if not i.dropped]
            if not live:
                continue
            if failure:
                result.skips.append(skip(atlas.sf, atlas.obj, "MonoBehaviour", "atlas", failure[0], detail=failure[1]))
                continue
            made = []
            for item in live:
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
                        continue
                    made.append(Output("atlas_json", item.key.decode(), text.encode("ascii"), atlas.sf.name,
                                       atlas.obj.path_id))  # fmt: skip
                else:
                    x, y, width, height = rect(atlas.tree["mSprites"][item.sprite])
                    crop = image.crop((x, y, x + width, y + height))
                    made.append(Output("atlas_sprite", item.key.decode(), png(crop), atlas.sf.name, atlas.obj.path_id,
                                       sprite=item.sprite, mode=crop.mode, size=crop.size))  # fmt: skip
            # 4.5: the sheet's texture output when made; else the atlas's JSON when made; else no unit.
            own = unit or next((o.key for o in made if o.kind == "atlas_json"), None)
            result.outputs += [Output(**{**o.__dict__, "unit": own}) for o in made]
    cut_sheets = {id(a.sheet[1]) for a in atlases}
    result.uncut_textures = [
        (item.sf.name, item.obj.path_id, item.key.decode("utf-8", "surrogateescape"))
        for item in sheet_items.values()
        if id(item.obj) not in cut_sheets and not item.dropped
    ]
    return result


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
        "uncut_textures": [list(t) for t in result.uncut_textures],
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
