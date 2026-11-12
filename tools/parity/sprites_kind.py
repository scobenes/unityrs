"""The `sprites` output kind's reference: docs/BATCH_SPEC.md 1.5's `outputs.sprites` (docs/plans/extract.md E17)
on the oracle, one PNG per Sprite of the input's own SerializedFiles.

Written from 4.4, 4.5, 7.3 and E17, not ported from any Rust (docs/plans/extract.md section 3, item 3). Every byte
it writes comes from the oracle stack: objects are read through UnityPy 1.25.2, a Sprite's image is UnityPy's
`SpriteHelper.get_image_from_sprite` (export/SpriteHelper.py:62-127, its sheet decoded by UnityPy and Pillow 9.5.0
inside it), and each image is written by Pillow's `save(fp, "PNG")`. Its own logic is the spec's:

- the plan (4.4), in object order: the Sprite's name (step 1, `ngui_read.name_of`, `duplicate_name` among Sprites,
  E29); the skips that need no pixels (step 2): the Sprite's checked object read as UnityPy constructs it (its
  root's type and cleaned field names: one that is no `Sprite`, an `UnknownObject` among them, is `read`), the shapes
  E17 reads (`fields`), its render data (`render_data`, export/SpriteHelper.py:63-81 as UnityPy runs it: a set
  `m_SpriteAtlas` dereferenced from the Sprite's file and read, whatever class it is constructed as; else, where
  `m_AtlasTags` is true, the first `SpriteAtlas` of the Sprite's file whose `peek_name` equals `m_AtlasTags[0]`;
  then the atlas's `m_RenderDataMap` walked as `next(...)` walks it, the matched value a `SpriteAtlasData` and of
  E17's shapes; any exception of the lookup `render`), then, in `get_image`'s order (export/SpriteHelper.py:44-59),
  its texture dereferenced from the file that holds that render data (`ngui_read.deref`) and its object read and,
  with no side of 0, section 3's answers 2 and 3 (`ngui_read.texture_of`, `ngui_read.selection`), then, where set,
  its alpha texture's the same; then its key from the `sprites` template (step 3, 2.1 and 2.3's rules but
  `prefix_conflict` and `archive_conflict`);
- the sprite groups (4.4): the planned Sprites by their texture and alpha texture, in the order of each group's first
  Sprite, exported first. A group's sheet takes section 3's answers 4 to 10 (`ngui_read.decode`, of the texture,
  then of the alpha texture; a texture with a side of 0 only its data's read, UnityPy's 0 x 0 image following), a
  failure every Sprite's of the group; then each Sprite is rendered by UnityPy, in plan order, its exception
  classified (7.3's closed codes: `DecompressionBombError` `decompression_bomb`, an `OverflowError` of a C `int` or a
  C `long` `crop_overflow`, a `MemoryError` Pillow raises `pillow_memory_error`, any other `render`), an image with a
  side of 0 `empty_image` (Pillow writes no PNG of one), and section 3's answer 11 asked before its PNG
  (`ngui_read.encoder_refusal`).

UnityPy's `get_image` keeps a sheet in its file's `_cache` by its texture's `path_id` alone
(export/SpriteHelper.py:44-59), so two sheets of one path id in two files would share it; the reference empties that
cache before each Sprite, and each Sprite's sheet is its own render data's.

Objects are constructed by UnityPy itself, so the reference follows its classes (B3's review round 3, S3a and S3b):
a pointer is a `PPtr` only where its node's type starts with `PPtr<` and its fields fit the class, any other value
is followed as UnityPy follows it (its truth Python's, its `.path_id` or `deref` an `AttributeError`, `render`); a
`Sprite` with an extra field named for an attribute its instances cannot set (`SETTERLESS`) is an `UnknownObject`,
`read`; an object read whose value has no `set_object_reader`, or a field that shadows it, fails, `read`; and an
`UnknownObject`'s render data with no `alphaTexture` is `render` where `get_image_from_sprite` reads it
(export/SpriteHelper.py:84). Each Sprite is rendered from the render data its plan resolved (`Resolved`, S6), so the
divergences below hold at its render too.

**Its stated divergences** (PARITY.md, "a Sprite's fields of another shape"): E17's shapes (`fields`,
`render_data_shape`) are `read` where UnityPy would go on or raise later; the Sprite's `m_RD` is read as E17's
render data whatever class it is constructed as; a key compares equal only where both are a tuple of a constructed
`GUID` of number words and a number id (`comparable`), where UnityPy's `==` compares any two values; and a tag
equals only a name that is a string.

**Its input** is one input's view (4.2), as `ngui_cutter.cut`'s, and the `sprites` key. **Its output** is an
`ngui_cutter.Result`: each `Output` (kind `sprite`, a plain candidate of 4.5, no unit) and each `Skip` (class
`Sprite`, output `sprite`), the plan's skips in object order, then the export's in export order. Budgets are not
modelled.

    python tools/parity/sprites_kind.py --input ID --bundle ID=PATH [--bundle ID=PATH ...] --key T

prints the result as JSON (`ngui_cutter.report`).
"""

from __future__ import annotations

import argparse
import json
import sys
from dataclasses import dataclass
from pathlib import Path
from types import SimpleNamespace

import PIL
from PIL import Image
from UnityPy.classes import GUID, PPtr, Sprite, SpriteAtlasData
from UnityPy.enums import ClassIDType
from UnityPy.export import SpriteHelper
from UnityPy.files import SerializedFile, WebFile
from UnityPy.helpers.TypeTreeNode import TypeTreeNode

import ngui_cutter as nc
import ngui_read as rd

# The messages of the `OverflowError`s Pillow's crop raises for a box edge outside a C `int` or a C `long`.
C_OVERFLOW = ("signed integer is ", "Python int too large to convert to C long")
# Where Pillow's own Python lives: a `MemoryError` raised there (`Image.new` and the crop of an image wider than it
# makes) is `pillow_memory_error`; any other propagates.
PILLOW = PIL.__path__[0]


# The names a `Sprite`'s extra field cannot be set under, so that UnityPyBoost's `parse_class` builds an
# `UnknownObject` (UnityPyBoost/TypeTreeHelper.cpp:538-574): `image` (classes/legacy_patch/Sprite.py:10),
# `assets_file` (classes/Object.py:23-27), and the three every instance with a `__dict__` refuses. A `Texture2D`
# refuses the same names, but its `image` takes a value that is a path to an image on the host, which the engine and
# the references refuse too, whatever it names (`ngui_read.sealed_images`, a stated divergence). The engine's
# `SETTERLESS` (`ngui/read.rs`) is this list; `test_sprites_kind.py` checks both against UnityPy's classes.
SETTERLESS = ("image", "assets_file", "__class__", "__dict__", "__weakref__")


@dataclass(eq=False)
class Planned:
    """A planned Sprite: its file, object, read Sprite, key, the render data its plan resolved and the sheet it
    renders from."""

    sf: SerializedFile
    obj: object
    sprite: Sprite
    key: str
    data: object
    texture: tuple  # (object, its read Texture2D)
    alpha: tuple | None


def group_of(p: Planned) -> tuple:
    """A sprite group's identity (4.4): the texture's and the alpha texture's (file, path id)."""
    t = (id(p.texture[0].assets_file), p.texture[0].path_id)
    a = None if p.alpha is None else (id(p.alpha[0].assets_file), p.alpha[0].path_id)
    return t, a


def zero(texture) -> bool:
    """A side of 0: UnityPy's `parse_image_data` returns its 0 x 0 `RGBA` image before it reads the format
    (export/Texture2DConverter.py:265-266)."""
    return not texture.m_Width or not texture.m_Height


def answered(obj) -> tuple:
    """A texture's object read and, with no side of 0, section 3's answers 2 and 3: `(obj, texture)`, or the skip as
    a `Failure`."""
    texture = rd.texture_of(obj)
    reason = None if zero(texture) else rd.selection(texture, obj.platform)
    if reason:
        raise rd.Failure(reason)
    return obj, texture


def number(value) -> bool:
    """A number as E17 reads one: a `float` (a `float` or `double` node's), not an integer."""
    return isinstance(value, float)


def pointer(value) -> bool:
    """A `PPtr` of two integer ids."""
    return isinstance(value, PPtr) and all(
        isinstance(getattr(value, n, None), int) and not isinstance(getattr(value, n), bool)
        for n in ("m_FileID", "m_PathID")
    )


def ids(value) -> bool:
    """A pointer field E17 takes: a `PPtr` of integer ids, or any value that is no `PPtr` (followed as UnityPy
    follows it); a `PPtr` of other ids is `read` (a stated rule)."""
    return not isinstance(value, PPtr) or pointer(value)


MISSING = object()


def render_data_shape(data) -> None:
    """A `SpriteRenderData` or `SpriteAtlasData` of E17's shapes (unityrs-texture's `SpriteFields`): a texture,
    an alpha texture or none (`ids`), a rectangle of four numbers, an integer `settingsRaw`; else `read`."""
    texture = getattr(data, "texture", MISSING)
    alpha = getattr(data, "alphaTexture", None)
    rect = getattr(data, "textureRect", None)
    settings = getattr(data, "settingsRaw", None)
    ok = (
        texture is not MISSING
        and ids(texture)
        and ids(alpha)
        and rect is not None
        and all(number(getattr(rect, n, None)) for n in rd.RECT)
        and isinstance(settings, int)
        and not isinstance(settings, bool)
        and settings >= 0
    )
    if not ok:
        raise rd.Failure(*rd.READ)


def fields(sprite) -> None:
    """4.4 step 2's shapes of the fields E17 reads: `m_PixelsToUnits` a number, `m_RD` render data, `m_SpriteAtlas`
    as `ids` takes it; else `read`. `m_AtlasTags` and `m_RenderDataKey` are read as the lookup reads them."""
    if not number(sprite.m_PixelsToUnits):
        raise rd.Failure(*rd.READ)
    render_data_shape(sprite.m_RD)
    if not ids(sprite.m_SpriteAtlas):
        raise rd.Failure(*rd.READ)


def numeric(value) -> bool:
    """A number `==` compares by value: an `int` (a `bool` among them) or a `float`."""
    return isinstance(value, (int, float))


def comparable(key) -> bool:
    """A key the engine compares (a stated divergence): a tuple of a constructed `GUID` whose four words are numbers
    and a number id. UnityPy's `==` compares any two values; a `GUID` that is an `UnknownObject` equals no other."""
    return (
        isinstance(key, tuple)
        and len(key) == 2
        and isinstance(key[0], GUID)
        and all(numeric(getattr(key[0], f"data_{i}_", None)) for i in range(4))
        and numeric(key[1])
    )


def atlas_name(obj):
    """`peek_name` of a `SpriteAtlas` (files/ObjectReader.py:189-197) as `ngui_read.name_of` reads a name, from the
    object's own nodes: its value, whatever its type; None for a root with no `m_Name` or `name` child."""
    node = rd.typed(obj).serialized_type.node
    at = next((i for i, child in enumerate(node.m_Children) if child.m_Name in ("m_Name", "name")), None)
    if at is None:
        return None
    peek = TypeTreeNode(
        node.m_Level, node.m_Type, node.m_Name, node.m_ByteSize, node.m_Version, node.m_Children[: at + 1]
    )
    return rd.read(lambda: obj.parse_as_dict(peek, check_read=False))[node.m_Children[at].m_Name]


RENDER = ("decode_error", "render")


def render_data(sprite, obj):
    """E17's render data (export/SpriteHelper.py:63-81, as UnityPy runs it, but the stated divergences): `(data,
    holder)`, the file that holds it."""
    atlas_obj = None
    if sprite.m_SpriteAtlas:  # `bool(PPtr)`, `m_PathID != 0`, or any other value's truth
        if not isinstance(sprite.m_SpriteAtlas, PPtr):  # its `deref_parse_as_object` raises
            raise rd.Failure(*RENDER)
        atlas_obj = rd.deref(sprite.m_SpriteAtlas.m_FileID, sprite.m_SpriteAtlas.m_PathID, obj.assets_file)
    elif sprite.m_AtlasTags:
        tags = sprite.m_AtlasTags
        for o in obj.assets_file.objects.values():
            if o.type != ClassIDType.SpriteAtlas:
                continue
            name = atlas_name(o)
            try:
                tag = tags[0]  # a true value that is not indexable raises here, at the first atlas
            except Exception:
                raise rd.Failure(*RENDER) from None
            if isinstance(tag, str) and name == tag:
                atlas_obj = o
                break
    if atlas_obj is None:
        return sprite.m_RD, obj.assets_file
    atlas = rd.checked(rd.typed(atlas_obj).read)  # whatever class: an `UnknownObject` is true
    key = sprite.m_RenderDataKey
    try:
        items = iter(atlas.m_RenderDataMap)
    except (AttributeError, TypeError):
        raise rd.Failure(*RENDER) from None
    for item in items:  # `next(...)`'s walk
        try:
            k, value = item
        except (TypeError, ValueError):
            raise rd.Failure(*RENDER) from None
        if comparable(k) and comparable(key) and k == key:
            break
    else:
        raise rd.Failure(*RENDER)  # `StopIteration`
    if not isinstance(value, SpriteAtlasData):  # `:79`'s `assert`
        raise rd.Failure(*RENDER)
    render_data_shape(value)
    return value, atlas_obj.assets_file


def resolve(obj) -> tuple[Sprite, object, tuple, tuple | None]:
    """4.4 step 2 past the name: the Sprite's checked read and its fields' shapes (`fields`), its render data
    (`render_data`) and its `alphaTexture` (an `UnknownObject`'s none raises, export/SpriteHelper.py:84), then, in
    `get_image`'s order (export/SpriteHelper.py:44-59), the `.path_id` of each pointer it takes (one that is no `PPtr`
    raises, `render`), its texture's dereference from the file that holds it and its answers (`answered`), then,
    where set, its alpha texture's."""
    sprite = rd.shaped(rd.checked(rd.typed(obj).read), Sprite)
    fields(sprite)
    data, holder = render_data(sprite, obj)
    if not hasattr(data, "alphaTexture"):
        raise rd.Failure(*RENDER)
    texture, alpha_ptr = data.texture, data.alphaTexture
    taken = (texture, alpha_ptr) if alpha_ptr else (texture,)  # export/SpriteHelper.py:47, then each `.path_id`
    if not all(isinstance(p, PPtr) for p in taken):
        raise rd.Failure(*RENDER)
    t = answered(rd.deref(texture.m_FileID, texture.m_PathID, holder))
    a = None
    if alpha_ptr:  # `bool(PPtr)`: `m_PathID != 0`
        a = answered(rd.deref(alpha_ptr.m_FileID, alpha_ptr.m_PathID, holder))
    return sprite, data, t, a


def raised_in_pillow(e: BaseException) -> bool:
    """Whether `e`'s innermost Python frame is Pillow's own."""
    tb = e.__traceback__
    while tb is not None and tb.tb_next is not None:
        tb = tb.tb_next
    return tb is not None and tb.tb_frame.f_code.co_filename.startswith(PILLOW)


class Resolved:
    """A Sprite as `get_image_from_sprite` reads it (export/SpriteHelper.py:62-127), its lookup's answer the render
    data its plan resolved (B3's review round 3, S6): where that is the Sprite's own `m_RD`, an `m_SpriteAtlas` and
    `m_AtlasTags` that are false; else a true `m_SpriteAtlas` whose atlas holds that data alone, under a key equal to
    the Sprite's `m_RenderDataKey` alone. Every other attribute is the Sprite's, so the render is UnityPy's own."""

    def __init__(self, sprite, data):
        self._sprite = sprite
        self.m_AtlasTags = None
        self.m_RenderDataKey = key = object()
        self.m_SpriteAtlas = None
        if data is not sprite.m_RD:
            atlas = SimpleNamespace(m_RenderDataMap=[(key, data)])
            self.m_SpriteAtlas = SimpleNamespace(deref_parse_as_object=lambda: atlas)

    def __getattr__(self, name):
        return getattr(self._sprite, name)


def rendered(p: Planned):
    """The Sprite's image, UnityPy's, from the render data its plan resolved (`Resolved`), its file's sheet cache
    emptied first; or its skip as a `Failure`."""
    p.sprite.assets_file._cache.clear()
    view = Resolved(p.sprite, p.data)
    try:
        return SpriteHelper.get_image_from_sprite(view)
    except Image.DecompressionBombError:
        raise rd.Failure("decode_error", "decompression_bomb") from None
    except OverflowError as e:
        code = "crop_overflow" if str(e).startswith(C_OVERFLOW) else "render"
        raise rd.Failure("decode_error", code) from None
    except MemoryError as e:
        if not raised_in_pillow(e):
            raise
        raise rd.Failure("decode_error", "pillow_memory_error") from None
    except Exception:  # every other exception the render raises
        raise rd.Failure("decode_error", "render") from None


@rd.sealed_images()
def cut(input_id: str, view: dict[str, bytes], key: str, prefix: str = "") -> nc.Result:
    """The `sprites` kind on one input over its view."""
    bundle = rd.load(view, input_id)
    web = next((path for path, f in rd.rows(bundle) if isinstance(f, WebFile)), None)
    if web is not None:
        raise rd.Divergence(f"{input_id}: a top-level WebFile row {web!r}, which fails the input (entry_error)")
    result = nc.Result(input_id)
    objects = [(sf, o) for sf in bundle.files.values() if isinstance(sf, SerializedFile) for o in sf.objects.values()]

    def skip(sf, obj, reason, detail=None):
        result.skips.append(nc.Skip(sf.name, obj.path_id, "Sprite", "sprite", reason, None, detail))

    names, owner, planned = set(), set(), []
    for sf, obj in objects:
        if obj.type != ClassIDType.Sprite:
            continue
        try:
            name = rd.name_of(obj)
        except rd.Failure as f:
            skip(sf, obj, f.reason, f.detail)
            continue
        if nc.raw(name) in names:  # E29: the first of one m_Name takes it
            skip(sf, obj, "duplicate_name")
            continue
        names.add(nc.raw(name))
        try:
            sprite, data, texture, alpha = resolve(obj)
        except rd.Failure as f:
            skip(sf, obj, f.reason, f.detail)
            continue
        values = {"input": nc.raw(input_id), "type": b"Sprite", "name": nc.raw(name)}
        values["path_id"] = str(obj.path_id).encode()
        k = nc.expand(key, "sprite", values)
        if rule := nc.key_refusal(k, prefix.encode()):
            skip(sf, obj, f"unwritable_key({rule})")
            continue
        if k in owner:
            skip(sf, obj, "duplicate_key")
            continue
        owner.add(k)
        planned.append(Planned(sf, obj, sprite, k.decode(), data, texture, alpha))
    groups: dict[tuple, list[Planned]] = {}
    for p in planned:
        groups.setdefault(group_of(p), []).append(p)
    for members in groups.values():
        first = members[0]
        try:
            for obj, texture in filter(None, (first.texture, first.alpha)):
                if zero(texture):
                    rd.data_of(texture, obj.assets_file)
                else:
                    rd.decode(texture, obj)
        except rd.Failure as f:
            for p in members:
                skip(p.sf, p.obj, f.reason, f.detail)
            continue
        for p in members:
            try:
                image = rendered(p)
            except rd.Failure as f:
                skip(p.sf, p.obj, f.reason, f.detail)
                continue
            if not image.width or not image.height:
                skip(p.sf, p.obj, "empty_image")
                continue
            if refusal := rd.encoder_refusal(image):
                skip(p.sf, p.obj, "decode_error", refusal)
                continue
            result.outputs.append(
                nc.Output("sprite", p.key, nc.png(image), p.sf.name, p.obj.path_id, mode=image.mode, size=image.size)
            )
    return result


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--input", required=True, help="the input's bundle id")
    ap.add_argument("--bundle", action="append", required=True, metavar="ID=PATH", help="a bundle of the view")
    ap.add_argument("--key", required=True, help="outputs.sprites' key")
    args = ap.parse_args(argv)
    view = {}
    for b in args.bundle:
        bid, sep, path = b.partition("=")
        if not sep:
            ap.error(f"--bundle {b!r} is not ID=PATH")
        view[bid] = Path(path).read_bytes()
    if args.input not in view:
        ap.error(f"--input {args.input!r} is no --bundle")
    json.dump(nc.report(cut(args.input, view, args.key)), sys.stdout, indent=1)
    print()
    return 0


if __name__ == "__main__":
    sys.exit(main())
