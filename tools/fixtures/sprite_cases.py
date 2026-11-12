"""The cases tools/fixtures/sprite.py records, not a fixture script: each a Sprite (and an atlas) built by
sprite_build.py's `make`, named for what it covers. `cases()` gives them in the fixture's order.

They cover: the crop of `textureRect` (inside, past the sheet, fractional edges rounded half to even, zero sides and a
side rounded to 0, a negative side, NaN, infinity, the decompression-bomb check and its exact count, an edge past a C
`int` and past a C `long`, Pillow's width cap); every packing rotation, packed and not, and the bits past 6, a quarter
turn of an empty crop and of one past the width cap; the alpha texture's merge (RGB24 and RGBA32 either way, an unset
one, a size mismatch, zero sides); tight sprites masked by their mesh (RGBA32 and RGB24 crops, fractional and negative
positions, NaN, horizontal and degenerate triangles, fans with shared corners, several submeshes, a turned crop, a zero
`m_UV0` of floats and of ints, `SInt32` positions, and each error: no vertices, uv with no positions, 2-d vertices, an
index past the vertices, a submesh cut short); tight sprites rendered from their uv (copies of matching triangles, a
flat one among them, each of Pillow's affine paths pinned by its pixels, collinear destinations that do and do not
meet an exactly zero pivot, 3-d and axis-dropping vertices with x or y the constant axis, several submeshes, a negative
size, `SInt32` positions and `UInt8` uv, corners past `int64` and `uint64`, a canvas past the width cap, the crop's
errors before the mesh, and each error: a uv of three components, an index past the uv, no triangles, no positions);
and atlases (by pointer, by tag, a key it lacks, keys whose GUID words the two trees order differently, and tight
packing from an atlas, masked and from its uv).

Run on its own it only imports (tools/parity/tests/test_fixture_modules.py).
"""

from __future__ import annotations

import math
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from sprite_build import KEY, QUAD, RECT, TIGHT, atlas, coords, make, sheet, square, uv_of  # noqa: E402

QUARTER = 1 | RECT | 4 << 2


def crop_cases() -> list[dict]:
    rects = {
        "inside": (1.0, 1.0, 4.0, 3.0), "past_sheet": (6.0, 4.0, 4.0, 4.0), "negative_x": (-2.0, 0.0, 4.0, 2.0),
        "half_even": (0.5, 1.5, 2.0, 2.5), "half_even_odd": (2.5, 3.5, 1.0, 1.0), "zero_width": (1.0, 1.0, 0.0, 3.0),
        "zero_both": (1.0, 1.0, 0.0, 0.0), "point_four": (1.0, 1.0, 0.4, 3.0),
        "negative_width": (2.0, 1.0, -1.0, 3.0), "negative_height": (2.0, 1.0, 1.0, -1.0),
        "nan": (math.nan, 0.0, 1.0, 1.0), "nan_width": (0.0, 0.0, math.nan, 1.0), "infinity": (0.0, 0.0, math.inf, 1.0),
        "bomb": (0.0, 0.0, 20000.0, 20000.0), "bomb_2_121": (0.0, 0.0, 2.0**121, 1.0),
        "c_int": (3.0e9, 0.0, 1.0, 1.0), "c_int_negative": (-3.0e9, 0.0, 1.0, 1.0),
        "c_long": (1.0e19, 0.0, 0.0, 1.0), "c_long_negative": (-1.0e19, 0.0, 0.0, 1.0),
        # A float32 side holds 536,870,880 (2^29 - 32) below the cap and 2^29 past it, none between.
        "width_cap": (0.0, 0.0, 2.0**29, 0.0), "width_below_cap": (0.0, 0.0, 536870880.0, 0.0),
    }  # fmt: skip
    out = [make(f"crop_{k}", rect=r) for k, r in rects.items()]
    out.append(make("crop_rgb", rect=(1.0, 2.0, 5.0, 3.0), tex=sheet("RGB24", 7, 6, 11)))
    out.append(make("crop_one_pixel_sheet", rect=(0.0, 0.0, 1.0, 1.0), tex=sheet("RGB24", 1, 1, 12)))
    return out


def rotation_cases() -> list[dict]:
    out = []
    for rot in range(16):
        raw = 1 | RECT | rot << 2
        out.append(
            make(f"packed_rotation_{rot}", rect=(1.0, 0.0, 5.0, 3.0), settings=raw, tex=sheet("RGBA32", 8, 6, 20))
        )
    out.append(make("unpacked_rotation_3", rect=(1.0, 0.0, 5.0, 3.0), settings=RECT | 3 << 2))
    out.append(make("unpacked_rotation_9", rect=(1.0, 0.0, 5.0, 3.0), settings=RECT | 9 << 2))
    out.append(make("high_bits", rect=(1.0, 0.0, 5.0, 3.0), settings=1 | RECT | 4 << 2 | 1 << 6 | 0xFFFF << 7))
    out.append(make("rotate_rgb", rect=(0.0, 1.0, 3.0, 4.0), settings=QUARTER, tex=sheet("RGB24", 5, 6, 21)))
    out.append(make("rotate_zero_width", rect=(1.0, 1.0, 0.0, 3.0), settings=QUARTER))
    # A crop 0 wide and 6e8 high, which Pillow makes, turned 6e8 wide, which it refuses.
    out.append(make("rotate_width_cap", rect=(0.0, 0.0, 0.0, 6.0e8), settings=QUARTER))
    return out


def alpha_cases() -> list[dict]:
    return [
        make("alpha_rgb_rgba", tex=sheet("RGB24", 8, 8, 30), alpha=sheet("RGBA32", 8, 8, 31)),
        make("alpha_rgba_rgb", tex=sheet("RGBA32", 8, 8, 32), alpha=sheet("RGB24", 8, 8, 33)),
        make("alpha_size_mismatch", tex=sheet("RGB24", 8, 8, 34), alpha=sheet("RGBA32", 8, 4, 35)),
        make("alpha_tight", settings=TIGHT, ppu=1.0, verts=square(4.0), subs=[(0, 6, 0)], idx=QUAD,
             tex=sheet("RGB24", 8, 8, 36), alpha=sheet("RGB24", 8, 8, 37)),
        make("texture_zero", tex=sheet("RGBA32", 0, 0, 38)),
        make("alpha_zero_both", tex=sheet("RGB24", 0, 4, 39), alpha=sheet("RGBA32", 4, 0, 40)),
        make("alpha_zero_beside", tex=sheet("RGB24", 8, 8, 41), alpha=sheet("RGBA32", 0, 8, 42)),
        make("alpha_zero_main", tex=sheet("RGB24", 8, 0, 43), alpha=sheet("RGBA32", 8, 8, 44)),
    ]  # fmt: skip


def mask_cases() -> list[dict]:
    tri = [(0.0, 0.0, 0.0), (7.0, 1.0, 0.0), (2.0, 6.0, 0.0)]
    frac = [(0.25, 0.75, 1.0), (6.625, 0.5, 1.0), (3.375, 7.5, 1.0)]
    fan = [(4.0, 4.0, 0.0), (0.0, 0.0, 0.0), (8.0, 0.0, 0.0), (8.0, 8.0, 0.0), (0.0, 8.0, 0.0), (4.0, 0.0, 0.0)]
    fan_idx = [0, 1, 5, 0, 5, 2, 0, 2, 3, 0, 3, 4, 0, 4, 1]
    flat = [(0.0, 3.0, 0.0), (6.0, 3.0, 0.0), (3.0, 3.0, 0.0), (1.0, 0.0, 0.0), (5.0, 0.0, 0.0), (3.0, 6.0, 0.0)]
    big = sheet("RGBA32", 12, 12, 40)
    rect = (1.0, 2.0, 8.0, 8.0)
    m = dict(settings=TIGHT, rect=rect, tex=big)
    t = dict(subs=[(0, 3, 0)], idx=[0, 1, 2])
    return [
        make("mask_quad", verts=square(0.25), ppu=16.0, subs=[(0, 6, 0)], idx=QUAD, **m),
        make("mask_quad_rgb", verts=square(0.25), ppu=16.0, subs=[(0, 6, 0)], idx=QUAD, settings=TIGHT, rect=rect,
             tex=sheet("RGB24", 12, 12, 41)),
        make("mask_triangle", verts=tri, **t, **m),
        make("mask_fraction", verts=frac, **t, **m),
        make("mask_ppu", verts=[(-0.1, -0.1, 0.0), (0.45, -0.05, 0.0), (0.2, 0.6, 0.0)], ppu=10.0, **t, **m),
        make("mask_negative_ppu", verts=tri, ppu=-1.0, **t, **m),
        make("mask_nan", verts=[(math.nan, 0.0, 0.0), (7.0, 1.0, 0.0), (2.0, 6.0, 0.0)], **t, **m),
        make("mask_fan", verts=fan, subs=[(0, 15, 0)], idx=fan_idx, **m),
        make("mask_flat", verts=flat, subs=[(0, 6, 0)], idx=[0, 1, 2, 3, 4, 5], **m),
        make("mask_submeshes", verts=fan, subs=[(0, 6, 0), (12, 9, 0)], idx=fan_idx, **m),
        make("mask_turned", verts=tri, **t, settings=1 | 4 << 2, rect=(0.0, 0.0, 8.0, 5.0), tex=big),
        make("mask_zero_uv", verts=tri, uv=[(0.0, 0.0)] * 3, **t, **m),
        make("mask_zero_uv_int", verts=tri, uv=[(0, 0)] * 3, uvfmt="B", **t, **m),
        make("mask_sint32", verts=[(0, 0, 0), (7, 1, 0), (2, 6, 0)], vfmt="i", **t, **m),
        make("mask_sint32_ppu", verts=[(-1, 0, 0), (3, 1, 0), (1, 3, 0)], vfmt="i", ppu=1.5, **t, **m),
        make("mask_no_vertices", verts=[], subs=[(0, 0, 0)], **m),
        make("mask_uv_only", uv=[(0.0, 0.0)] * 3, **t, **m),
        make("mask_2d", verts=[(0.0, 0.0), (4.0, 0.0), (0.0, 4.0)], dim=2, **t, **m),
        make("mask_index_past", verts=tri, subs=[(0, 3, 0)], idx=[0, 1, 3], **m),
        make("mask_cut", verts=tri, subs=[(0, 4, 0)], idx=[0, 1, 2, 1], **m),
        make("mask_cut_late", verts=fan, subs=[(0, 6, 0), (12, 4, 0)], idx=fan_idx[:8], **m),
    ]  # fmt: skip


def mesh_cases() -> list[dict]:
    tri = [(0.0, 0.0, 0.0), (5.0, 1.0, 0.0), (1.0, 4.0, 0.0)]
    quad = square(4.0)
    big = sheet("RGBA32", 16, 16, 50)
    m = dict(settings=TIGHT, rect=(0.0, 0.0, 16.0, 16.0), tex=big)
    t = dict(subs=[(0, 3, 0)], idx=[0, 1, 2])
    q = dict(subs=[(0, 6, 0)], idx=QUAD)
    collinear = [(0.0, 0.0, 0.0), (2.0, 2.0, 0.0), (4.0, 4.0, 0.0), (0.0, 4.0, 0.0)]
    flat = [*quad, (2.0, 0.0, 0.0)]
    return [
        make("mesh_copy", verts=quad, uv=uv_of(quad, 3, 5, 1, 16), **q, **m),
        make("mesh_copy_rgb", verts=quad, uv=uv_of(quad, 3, 5, 1, 16), **q, settings=TIGHT, rect=(0.0, 0.0, 1.0, 1.0),
             tex=sheet("RGB24", 16, 16, 51)),
        make("mesh_copy_past_sheet", verts=quad, uv=uv_of(quad, 14, -2, 1, 16), **q, **m),
        make("mesh_copy_flat", verts=flat, uv=uv_of(flat, 3, 5, 1, 16), subs=[(0, 9, 0)], idx=[*QUAD, 0, 4, 1], **m),
        make("mesh_scale", verts=quad, uv=uv_of(quad, 2, 1, 2, 16), **q, **m),
        make("mesh_scale_down", verts=square(8.0), uv=uv_of(square(8.0), 1, 1, 0.5, 16), **q, **m),
        make("mesh_turn", verts=quad, uv=uv_of(quad, 4, 12, 2, 16, turn=True), **q, **m),
        make("mesh_shear", verts=tri, uv=[(0.1, 0.2), (0.45, 0.3), (0.2, 0.7)], **t, **m),
        make("mesh_fraction", verts=[(0.0, 0.0, 0.0), (5.5, 1.25, 0.0), (1.5, 4.75, 0.0)],
             uv=[(0.11, 0.23), (0.47, 0.31), (0.19, 0.73)], **t, **m),
        make("mesh_float_path", verts=quad, uv=uv_of(quad, 40000, 3, 3, 16, turn=True), **q, **m, path="float"),
        # A turn whose corners all lie within 32,768 of the origin: fixed point, its first pixel inside the sheet.
        make("mesh_fixed_path_far", verts=quad, uv=uv_of(quad, -1490, 1505, 3000, 16, turn=True), **q, **m,
             path="fixed"),
        make("mesh_float_path_scale", verts=quad, uv=uv_of(quad, -1490, -1495, 3000, 16), **q, **m, path="scale"),
        make("mesh_singular", verts=collinear, uv=[(0.1, 0.1), (0.3, 0.2), (0.5, 0.6), (0.2, 0.9)], subs=[(0, 6, 0)],
             idx=[0, 1, 2, 0, 2, 3], **m),
        make("mesh_3d", verts=[(0.0, 0.0, 0.0), (4.0, 1.0, 1.0), (1.0, 4.0, 2.0)], uv=[(0.1, 0.1)] * 3, **t, **m),
        make("mesh_x_constant", verts=[(2.0, 0.0, 0.0), (2.0, 4.0, 1.0), (2.0, 1.0, 4.0)],
             uv=[(0.1, 0.1), (0.4, 0.15), (0.2, 0.45)], **t, **m),
        make("mesh_y_constant", verts=[(0.0, 2.0, 0.0), (4.0, 2.0, 1.0), (1.0, 2.0, 4.0)],
             uv=[(0.1, 0.1), (0.4, 0.15), (0.2, 0.45)], **t, **m),
        make("mesh_one_vertex", verts=[(1.0, 1.0, 1.0)], uv=[(0.5, 0.5)], **t, **m),
        make("mesh_submeshes", verts=quad, uv=uv_of(quad, 3, 5, 2, 16), subs=[(0, 3, 0), (6, 3, 0)], idx=QUAD, **m),
        make("mesh_ppu", verts=[(-0.1, -0.1, 0.0), (0.45, -0.05, 0.0), (0.2, 0.6, 0.0)], ppu=10.0,
             uv=[(0.1, 0.1), (0.45, 0.15), (0.2, 0.6)], **t, **m),
        make("mesh_negative_ppu", verts=quad, uv=uv_of(quad, 3, 5, 1, 16), ppu=-1.0, **q, **m),
        make("mesh_sint32", verts=[(0, 0, 0), (4, 0, 0), (0, 4, 0), (4, 4, 0)], vfmt="i", uv=uv_of(quad, 3, 5, 1, 16),
             **q, **m),
        make("mesh_sint32_ppu", verts=[(0, 0, 0), (5, 1, 0), (1, 4, 0)], vfmt="i", ppu=1.5,
             uv=[(0.1, 0.2), (0.45, 0.3), (0.2, 0.7)], **t, **m),
        make("mesh_uint8_uv", verts=tri, uv=[(0, 0), (1, 0), (0, 1)], uvfmt="B", **t, **m),
        make("mesh_uv_3", verts=quad, uv=[(0.1, 0.2, 0.3)] * 4, uv_dim=3, **q, **m),
        make("mesh_uv_1", verts=quad, uv=[(0.1,)] * 4, uv_dim=1, **q, **m),
        make("mesh_index_past", verts=tri, uv=[(0.1, 0.1)] * 3, subs=[(0, 3, 0)], idx=[0, 1, 4], **m),
        make("mesh_cut", verts=tri, uv=[(0.1, 0.2), (0.4, 0.2), (0.2, 0.5)], subs=[(0, 5, 0)], idx=[0, 1, 2, 2, 1],
             **m),
        make("mesh_no_submeshes", verts=tri, uv=[(0.1, 0.1)] * 3, subs=[], **m),
        make("mesh_no_submeshes_indices", verts=tri, uv=[(0.1, 0.1)] * 3, subs=[], idx=[0, 1, 2], **m),
        make("mesh_uv_only", uv=[(0.1, 0.2), (0.4, 0.2), (0.2, 0.5)], **t, **m),
        make("mesh_2d", verts=[(0.0, 0.0), (4.0, 0.0), (0.0, 4.0)], dim=2, uv=[(0.1, 0.1)] * 3, **t, **m),
        # The crop is made, and checked, before the mesh is read (`export/SpriteHelper.py:90-97`).
        make("mesh_crop_nan", verts=quad, uv=uv_of(quad, 3, 5, 1, 16), **q, settings=TIGHT,
             rect=(math.nan, 0.0, 1.0, 1.0), tex=big),
        make("mesh_crop_bomb", verts=quad, uv=uv_of(quad, 3, 5, 1, 16), **q, settings=TIGHT,
             rect=(0.0, 0.0, 20000.0, 20000.0), tex=big),
        make("mesh_crop_c_int", verts=quad, uv=uv_of(quad, 3, 5, 1, 16), **q, settings=TIGHT,
             rect=(3.0e9, 0.0, 1.0, 1.0), tex=big),
        make("mesh_crop_width_cap", verts=quad, uv=uv_of(quad, 3, 5, 1, 16), **q, settings=TIGHT,
             rect=(0.0, 0.0, 2.0**29, 0.0), tex=big),
        # A canvas 6e8 wide and 0 high: Image.new refuses it. Then one at the cap, which it makes, and one past
        # it (the canvas's width is a float64 difference, so it can be either), each with one triangle of one corner.
        make("mesh_canvas_width_cap", verts=[(0.0, 0.0, 0.0), (6.0e8, 0.0, 0.0), (3.0e8, 0.0, 0.0)],
             uv=[(0.1, 0.1), (0.2, 0.1), (0.1, 0.2)], **t, **m),
        make("mesh_canvas_at_cap", verts=[(2.0, 0.0, 0.0), (2.0**29, 0.0, 0.0), (3.0e8, 0.0, 0.0)],
             uv=[(0.1, 0.1), (0.2, 0.1), (0.1, 0.2)], subs=[(0, 3, 0)], idx=[0, 0, 0], **m),
        make("mesh_canvas_past_cap", verts=[(1.0, 0.0, 0.0), (2.0**29, 0.0, 0.0), (3.0e8, 0.0, 0.0)],
             uv=[(0.1, 0.1), (0.2, 0.1), (0.1, 0.2)], subs=[(0, 3, 0)], idx=[0, 0, 0], **m),
    ]  # fmt: skip


def corner(u: float, v: float) -> list:
    """A triangle (0, 0), (4, 0), (0, 4) whose first uv corner is (u, v) on the 16-wide sheet, the others small."""
    return [(u / 16, v / 16), (4 / 16, 0.0), (0.0, 4 / 16)]


def affine_cases() -> list[dict]:
    """Pillow's three affine paths, each case's path asserted as it records (`path`), and numpy's conversion of a
    source corner (docs/plans/extract.md section 3, item 6)."""
    big = sheet("RGBA32", 16, 16, 52)
    m = dict(settings=TIGHT, rect=(0.0, 0.0, 16.0, 16.0), tex=big)
    t = dict(subs=[(0, 3, 0)], idx=[0, 1, 2])

    def tri(n: float) -> list:
        return [(0.0, 0.0, 0.0), (n, 0.0, 0.0), (0.0, n, 0.0)]

    def src(*pts) -> list:
        return [(x / 16, y / 16) for x, y in pts]

    flat_x = [(0.0, 0.0, 0.0), (0.0, 4.0, 0.0), (0.0, 2.0, 0.0)]

    # A shear (1/3 and 1/9) whose canvas a far vertex, unindexed, widens until a corner of the transform nears 32,768.
    def edge(w: int) -> dict:
        return dict(verts=[*tri(9.0), (float(w), 0.0, 0.0)], uv=src((0, 0), (3, 1), (1, 3), (0, 0)), **t, **m)

    return [
        # A sample in (-1, 0), which COORD makes -1 (outside), not 0: on the scale path and the float path.
        make("path_scale_negative_sample", verts=tri(4.0), uv=src((-1, 0), (1, 0), (-1, 2)), **t, **m, path="scale"),
        make("path_float_visible", verts=tri(2.0), uv=src((-15001, 2), (44999, 4), (-14999, 4)), **t, **m,
             path="float"),
        # A pure scale on which the fixed-point path's 16.16 rounding would sample other texels.
        make("path_scale_rounding", verts=tri(3.0), uv=src((-1, 0), (3, 0), (-1, 4)), **t, **m, path="scale"),
        # check_fixed's bound: a corner just inside 32,768, and just past it.
        make("path_fixed_bound_inside", **edge(98298), path="fixed"),
        make("path_fixed_bound_outside", **edge(98301), path="float"),
        # Collinear destinations whose LU meets no exactly zero pivot: solved and copied, as numpy solves them, each
        # copying pixels that are not zero (a renderer that took them for singular would copy none).
        make("path_collinear_a", verts=[(0.0, 0.0, 0.0), (1.0, 1.0, 0.0), (3.0, 2.0, 0.0), (5.0, 3.0, 0.0)],
             uv=[(0.0, 0.0), *src((0, 0), (3, 6), (6, 12))], subs=[(0, 3, 0)], idx=[1, 2, 3], **m),
        make("path_collinear_b", verts=[(0.0, 0.0, 0.0), (1.0, 0.0, 0.0), (4.0, 1.0, 0.0), (7.0, 2.0, 0.0)],
             uv=[(0.0, 0.0), (0.1, 0.1), (0.3, 0.2), (0.5, 0.6)], subs=[(0, 3, 0)], idx=[1, 2, 3], **m),
        # Source corners as numpy converts them: 2^63 (a uint64 array), 2^63 beside a negative (a float64 array),
        # -2^63 (an int64 array), the range's two ends; and 2^64, 2^65 and -2^64 (object arrays, which solve cannot
        # cast).
        make("corner_2_63", verts=tri(4.0), uv=corner(2.0**63, 0.0), **t, **m),
        make("corner_2_63_mixed", verts=tri(4.0), uv=[(2.0**59, 0.0), (-4 / 16, 0.0), (0.0, 4 / 16)], **t, **m),
        make("corner_minus_2_63", verts=tri(4.0), uv=corner(-(2.0**63), 0.0), **t, **m),
        make("corner_2_64", verts=tri(4.0), uv=corner(2.0**64, 0.0), **t, **m),
        make("corner_2_65", verts=tri(4.0), uv=corner(2.0**65, 0.0), **t, **m),
        make("corner_minus_2_64", verts=tri(4.0), uv=corner(-(2.0**64), 0.0), **t, **m),
        # Matching offsets far out (a flat triangle, x one value, which a float32 uv holds exactly): the copy's crop
        # refuses its box's edges, as a C long and as an int.
        make("corner_copy_c_long", verts=flat_x, uv=src((2.0**70, 0), (2.0**70, 4), (2.0**70, 2)), **t, **m),
        make("corner_copy_c_int", verts=flat_x, uv=src((3.0e9, 0), (3.0e9, 4), (3.0e9, 2)), **t, **m),
    ]  # fmt: skip


def seeded_cases() -> list[dict]:
    """Triangles at seeded positions: fans of five triangles masked (fractional corners on quarter and half pixels,
    every rounding of Pillow's edges) and rendered from seeded uvs (every path of the affine transform)."""
    out = []
    idx = [0, 1, 2, 0, 2, 3, 0, 3, 4, 0, 4, 5, 0, 5, 1]
    for k in range(8):
        xs, ys = coords(100 + k, 6, 9.0, 0.25), coords(200 + k, 6, 9.0, 0.25)
        verts = [(x, y, 0.0) for x, y in zip(xs, ys, strict=True)]
        tex = sheet("RGB24" if k % 2 else "RGBA32", 12, 12, 60 + k)
        out.append(make(f"mask_seeded_{k}", verts=verts, ppu=1.0 + k / 4, subs=[(0, 15, 0)], idx=idx, settings=TIGHT,
                        rect=(1.0, 1.0, 10.0, 10.0), tex=tex))  # fmt: skip
    for k in range(10):
        xs, ys = coords(300 + k, 6, 12.0, 0.5), coords(400 + k, 6, 12.0, 0.5)
        us, vs = coords(500 + k, 6, 1.0, 1 / 64), coords(600 + k, 6, 1.0, 1 / 64)
        verts = [(x, y, 0.0) for x, y in zip(xs, ys, strict=True)]
        tex = sheet("RGB24" if k % 3 == 0 else "RGBA32", 16, 16, 70 + k)
        out.append(make(f"mesh_seeded_{k}", verts=verts, uv=list(zip(us, vs, strict=True)), ppu=1.0,
                        subs=[(0, 15, 0)], idx=idx, settings=TIGHT, rect=(0.0, 0.0, 16.0, 16.0),
                        tex=tex))  # fmt: skip
    return out


def atlas_cases() -> list[dict]:
    other = ((9, 9, 9, 9), 7)
    hit = atlas("a", [(other, (0.0, 0.0, 2.0, 2.0), RECT, False), (KEY, (2.0, 1.0, 5.0, 4.0), 1 | RECT | 3 << 2, True)])
    miss = atlas("a", [(other, (0.0, 0.0, 2.0, 2.0), RECT, False)])
    # The key's GUID words reversed and the same (1, 2, 3, 4) in reverse: a match only where they are read by name.
    near = ((4, 3, 2, 1), 7)
    named = atlas("a", [(near, (0.0, 0.0, 2.0, 2.0), RECT, False), (KEY, (2.0, 1.0, 5.0, 4.0), RECT, False)])
    tight = atlas("a", [(KEY, (0.0, 0.0, 8.0, 8.0), TIGHT, False)])
    tri = [(0.0, 0.0, 0.0), (7.0, 1.0, 0.0), (2.0, 6.0, 0.0)]
    t = dict(subs=[(0, 3, 0)], idx=[0, 1, 2])
    return [
        make("atlas_pointer", atlas=hit, atlas_ptr=5),
        make("atlas_tag", atlas=hit, tags=["a"]),
        make("atlas_tag_other", atlas=hit, tags=["b"]),
        make("atlas_missing_key", atlas=miss, atlas_ptr=5),
        make("atlas_guid_by_name", atlas=named, atlas_ptr=5, atlas_tree="atlas_guid_reversed"),
        make("atlas_tight_mask", atlas=tight, atlas_ptr=5, verts=tri, **t),
        make("atlas_tight_mesh", atlas=tight, tags=["a"], verts=tri, uv=[(0.1, 0.2), (0.6, 0.3), (0.2, 0.7)], **t),
    ]


def cases() -> list[dict]:
    out = crop_cases() + rotation_cases() + alpha_cases() + mask_cases() + mesh_cases() + affine_cases()
    out += seeded_cases() + atlas_cases()
    names = [c["name"] for c in out]
    assert len(names) == len(set(names)), "case names repeat"
    return out
