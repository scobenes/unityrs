"""What tools/fixtures/sprite.py builds its cases from, not a fixture script: the Sprite's and the SpriteAtlas's trees,
as node rows (meshdecode_build.py), and a case's inputs, its Sprite's and atlas's values and its textures.

Nothing here comes from a TPK or a dump (docs/plans/core.md section 9, Q2). A tree holds, of the fields UnityPy 1.25.2's
MIT class definitions (`classes/generated.py`) declare, those without a default, which object mode needs to build the
class, and those `get_image_from_sprite` reads; each node's type is the one a source names, as meshdecode_trees.py's
are, from `classes/generated.py` and AssetStudio's MIT readers of Perfare/AssetStudio's release v0.16.47 (format facts
only: no AssetStudio code is ported or kept here):
- the Sprite: meshdecode_trees.py's (its sources there), with `m_RD.alphaTexture` (`PPtr<Texture2D>`),
  `m_SpriteAtlas` (`PPtr<SpriteAtlas>`), `m_RenderDataKey` (a `pair` of a `GUID` and an `SInt64`, read as
  `ReadBytes(16)` and `ReadInt64`) and `m_AtlasTags` (a vector of `string`, `ReadStringArray`), as
  `AssetStudio/Classes/Sprite.cs` reads them;
- `GUID`: four `unsigned int`s, `data[0]` to `data[3]`, the 16 bytes AssetStudio reads, which `classes/generated.py`'s
  `GUID` names `data_0_` to `data_3_` (UnityPy's rename of `data[0]`); its equality compares them in that order;
- `pair` (`first`, `second`) and `map` (a vector of `pair`s): Unity's type names for `std::pair` and `std::map`, which
  `helpers/TypeTreeHelper.py` reads as a tuple and a list of tuples (`classes/generated.py`'s `Tuple` and `List`);
- `SpriteAtlasData` (`atlas_data`) and `SpriteAtlas` (`sprite_atlas`): `AssetStudio/Classes/SpriteAtlas.cs`'s
  readers: `texture` and `alphaTexture` (`PPtr<Texture2D>`), `textureRect` (a `Rectf`: `x`, `y`, `width` and `height`,
  `float`s, as `classes/generated.py`'s `Rectf`), `textureRectOffset` (`ReadVector2`, a `Vector2f`), `uvTransform`
  (`ReadVector4`, a `Vector4f`), `downscaleMultiplier` (`ReadSingle`, a `float`) and `settingsRaw` (`SpriteSettings`'s
  `ReadUInt32`, an `unsigned int`); `m_Name` and `m_Tag` (`string`), `m_PackedSprites` (a vector of `PPtr<Sprite>`),
  `m_PackedSpriteNamesToIndex` (`ReadStringArray`), `m_RenderDataMap` (a `map` of a `pair<GUID, SInt64>` to a
  `SpriteAtlasData`) and `m_IsVariant` (`ReadBoolean`, a `bool`).
`TREES` holds them by the name a case gives (`atlas_tree`), with an atlas whose `GUID` holds its words in the reverse
order, which object mode reads by name.

A vertex channel's format is UnityPy's `enums/VertexFormat.py` code for 2019.4: `float` (0), `UInt8` (6) or `SInt32`
(11), packed as `struct` packs `f`, `B` and `i`.

Run on its own it only imports (tools/parity/tests/test_fixture_modules.py).
"""

from __future__ import annotations

import struct
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from meshdecode_build import add, leaf, splitmix, vec, write  # noqa: E402
from meshdecode_cases import Obj, vtuple  # noqa: E402
from meshdecode_trees import node, xyz  # noqa: E402
from meshdecode_trees import tree as hand_tree  # noqa: E402

VERSION = "2019.4.0f1"
PIXEL = {"RGBA32": 4, "RGB24": 3}
TIGHT, RECT = 0, 2
KEY = ((1, 2, 3, 4), 7)
# A vertex channel's `struct` code: its VertexFormat (2019.4) and size.
VERTEX_FORMAT = {"f": (0, 4), "B": (6, 1), "i": (11, 4)}


# --- trees


def pptr(name: str, cls: str) -> list[list]:
    return node(f"PPtr<{cls}>", name, leaf("int", "m_FileID"), leaf("SInt64", "m_PathID"))


def pair(name: str, first: list[list], second: list[list]) -> list[list]:
    first = [[0, first[0][1], "first", first[0][3]], *first[1:]]
    second = [[0, second[0][1], "second", second[0][3]], *second[1:]]
    return [[0, "pair", name, 0], *[[r[0] + 1, *r[1:]] for r in first + second]]


def key_tree(name: str, reverse: bool = False) -> list[list]:
    words = [leaf("unsigned int", f"data[{i}]") for i in range(4)]
    guid = node("GUID", "first", *(words[::-1] if reverse else words))
    return pair(name, guid, leaf("SInt64", "second"))


def sprite_tree() -> list[list]:
    """meshdecode_trees.py's Sprite with what `get_image_from_sprite` reads beside the mesh."""
    rows = hand_tree(213, tuple(vtuple(VERSION)))
    rows = add(rows, ("m_RD",), pptr("alphaTexture", "Texture2D"))
    rows = add(rows, (), pptr("m_SpriteAtlas", "SpriteAtlas"))
    rows = add(rows, (), key_tree("m_RenderDataKey"))
    return add(rows, (), vec("m_AtlasTags", leaf("string", "data")))


def atlas_data(name: str) -> list[list]:
    """`SpriteAtlasData`, its fields without a default."""
    rect = node("Rectf", "textureRect", *[leaf("float", n) for n in ("x", "y", "width", "height")])
    return node(
        "SpriteAtlasData", name, pptr("texture", "Texture2D"), pptr("alphaTexture", "Texture2D"), rect,
        xyz("Vector2f", "textureRectOffset", "xy"), xyz("Vector4f", "uvTransform", "xyzw"),
        leaf("float", "downscaleMultiplier"), leaf("unsigned int", "settingsRaw"),
    )  # fmt: skip


def sprite_atlas(reverse: bool = False) -> list[list]:
    """`SpriteAtlas`, its fields without a default; `m_RenderDataMap` a map of pairs, its keys' GUID words reversed
    where `reverse`."""
    entry = pair("data", key_tree("first", reverse), atlas_data("second"))
    table = vec("m_RenderDataMap", entry)
    table[0][1] = "map"
    return node(
        "SpriteAtlas", "Base", leaf("string", "m_Name"), vec("m_PackedSprites", pptr("data", "Sprite")),
        vec("m_PackedSpriteNamesToIndex", leaf("string", "data")), table, leaf("string", "m_Tag"),
        leaf("bool", "m_IsVariant"),
    )  # fmt: skip


SPRITE_ROWS = sprite_tree()
TREES = {"sprite": SPRITE_ROWS, "atlas": sprite_atlas(), "atlas_guid_reversed": sprite_atlas(reverse=True)}


# --- inputs


def sheet(fmt: str, width: int, height: int, seed: int) -> dict:
    return {"format": fmt, "width": width, "height": height, "seed": seed}


def sheet_bytes(t: dict) -> bytes:
    return splitmix(t["seed"], t["width"] * t["height"] * PIXEL[t["format"]])


def mesh_data(o: Obj, verts: list | None, uv: list | None, dim=3, uv_dim=2, vfmt="f", uvfmt="f") -> Obj:
    """m_VertexData of `verts` (each `dim` components of format `vfmt`) and `uv` (each `uv_dim` of `uvfmt`), each where
    given, interleaved in one stream."""
    chans = [(i, v, d, f) for i, v, d, f in ((0, verts, dim, vfmt), (4, uv, uv_dim, uvfmt)) if v is not None]
    count = len(chans[0][1])
    channels, offset = {}, 0
    for index, _, d, fmt in chans:
        code, size = VERTEX_FORMAT[fmt]
        channels[index] = (0, offset, code, d)
        offset += size * d
    data = b"".join(struct.pack(f"<{d}{f}", *values[t]) for t in range(count) for _, values, d, f in chans)
    data += bytes(-len(data) % 16)
    return o.vertex_data(count, channels, data)


def make(name: str, *, rect=(0.0, 0.0, 4.0, 4.0), settings=RECT, ppu=1.0, verts=None, uv=None, subs=None,
         idx=None, tex=None, alpha=None, dim=3, uv_dim=2, vfmt="f", uvfmt="f", atlas=None, atlas_ptr=0, tags=None,
         atlas_tree="atlas", path=None) -> dict:  # fmt: skip
    """A case: the Sprite's fields, its textures by path id (1 the texture, 2 the alpha texture, 3 and 4 the atlas's),
    and the atlas, if any (path id 5), laid out from TREES[atlas_tree]. `path`, where given, is the one Pillow
    transform path (`scale`, `fixed` or `float`) every affine copy of the case must take, which the recorder asserts."""
    o = Obj("sprite", VERSION, SPRITE_ROWS)
    if verts is not None or uv is not None:
        mesh_data(o, verts, uv, dim, uv_dim, vfmt, uvfmt)
    if subs is not None:
        o.submeshes(*subs).indices(idx or [])
    x, y, w, h = rect
    o.set(textureRect={"x": x, "y": y, "width": w, "height": h}, settingsRaw=settings)
    o.set(texture={"m_FileID": 0, "m_PathID": 1}, alphaTexture={"m_FileID": 0, "m_PathID": 2 if alpha else 0})
    o.value["m_PixelsToUnits"] = ppu
    o.value["m_SpriteAtlas"] = {"m_FileID": 0, "m_PathID": atlas_ptr}
    o.value["m_RenderDataKey"] = ({f"data[{i}]": v for i, v in enumerate(KEY[0])}, KEY[1])
    o.value["m_AtlasTags"] = tags or []
    textures = {"1": tex or sheet("RGBA32", 8, 8, 1)}
    if alpha:
        textures["2"] = alpha
    case = {"name": name, "version": o.version, "sprite": write(SPRITE_ROWS, o.value).hex(), "textures": textures}
    if atlas is not None:
        case["atlas"] = write(TREES[atlas_tree], atlas["value"]).hex()
        if atlas_tree != "atlas":
            case["atlas_tree"] = atlas_tree
        textures.update(atlas["textures"])
    if path is not None:
        case["path"] = path
    return case


def atlas(name: str, entries: list) -> dict:
    """An atlas of `entries`: (key, rect, settings, alpha) each, its texture path id 3 and alpha texture 4."""
    table = []
    for key, (x, y, w, h), settings, alpha in entries:
        k = ({f"data[{i}]": v for i, v in enumerate(key[0])}, key[1])
        data = {
            "texture": {"m_FileID": 0, "m_PathID": 3}, "alphaTexture": {"m_FileID": 0, "m_PathID": 4 if alpha else 0},
            "textureRect": {"x": x, "y": y, "width": w, "height": h}, "textureRectOffset": {"x": 0.0, "y": 0.0},
            "uvTransform": {"x": 0.0, "y": 0.0, "z": 0.0, "w": 0.0}, "downscaleMultiplier": 1.0,
            "settingsRaw": settings,
        }  # fmt: skip
        table.append((k, data))
    value = {"m_Name": name, "m_PackedSprites": [], "m_PackedSpriteNamesToIndex": [], "m_RenderDataMap": table,
             "m_Tag": name, "m_IsVariant": False}  # fmt: skip
    return {"value": value, "textures": {"3": sheet("RGBA32", 8, 8, 3), "4": sheet("RGBA32", 8, 8, 4)}}


def square(n: float, z: float = 0.0) -> list:
    return [(0.0, 0.0, z), (n, 0.0, z), (0.0, n, z), (n, n, z)]


QUAD = [0, 1, 2, 2, 1, 3]


def uv_of(verts, ox: float, oy: float, scale: float, side: int, turn: bool = False) -> list:
    out = []
    for x, y, _z in verts:
        u, v = (y, -x) if turn else (x, y)
        out.append(((u * scale + ox) / side, (v * scale + oy) / side))
    return out


def coords(seed: int, n: int, top: float, step: float) -> list[float]:
    """`n` coordinates from 0 to `top` in steps of `step` (half-steps among them), from SplitMix64."""
    raw = splitmix(seed, 2 * n)
    return [round(int.from_bytes(raw[2 * i : 2 * i + 2], "little") % int(top / step + 1)) * step for i in range(n)]
