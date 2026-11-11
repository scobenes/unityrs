"""Every image the NGUI cutter's reference makes on the testkit's scenes, held to pixels written from the scenes'
builders by hand (`crates/unityrs-testkit/src/ngui/`), never from the reference's output.

A scene's sheets are the testkit's `ngui::sheet`s: the pixel at (x, y), y from the image's top, is
`(base + x + 16 y, 0x40 + x, 0x80 + y, 0xFF - x - y)` (RGB24 drops the last), each byte wrapping. So each image
below is its sheet's base, size and mode, and the rectangle cut from it (none for a texture output, its whole
sheet), with E23's zero pixels past the sheet's edge; the test checks that every image output is in the table, and
that every entry of the table is made. Needs UnityPy 1.25.2 (the oracle image), skipped elsewhere.
"""

import io
import sys
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE / "tests"))

from rows_support import ORACLE  # noqa: E402

if ORACLE:
    from PIL import Image

    import ngui_scenes


def sheet(base, w, h, rgb=False):
    return {"base": base, "size": (w, h), "rgb": rgb}


def cut(s, rect=None):
    return (s, rect)


def winner_input(base, sprites):
    """`winner_input`'s sheet (2 x 1) and its sprites at x 0, 1, 0, ..."""
    s = sheet(base, 2, 1)
    return {f"{n}.png": cut(s, (i % 2, 0, 1, 1)) for i, n in enumerate(sprites)}


def one_by_one(bases):
    """Each streamed 1 x 1 sheet of `bases` as its texture output and its atlas's one crop."""
    out = {}
    for name, base in bases.items():
        out[f"{name}.png"] = cut(sheet(base, 1, 1))
        out[f"{name}/s.png"] = cut(sheet(base, 1, 1), (0, 0, 1, 1))
    return out


def table():
    x1, x7, x8 = sheet(1, 4, 4), sheet(0x11, 3, 2), sheet(9, 2, 2)
    dup, x16 = sheet(2, 4, 2), sheet(0x17, 2, 2)
    rgb = sheet(5, 2, 2, rgb=True)
    left_right = {"left/l.png": cut(x7, (0, 0, 1, 2)), "right/r.png": cut(x7, (1, 0, 2, 2))}
    t = {
        ("own_sheet_by_material_name", "x.bundle"): {
            "ui/a.png": cut(x1, (0, 0, 2, 2)), "ui/b.png": cut(x1, (1, 2, 3, 2)),
            "ui/edge.png": cut(x1, (3, 3, 2, 2)), "ui/neg.png": cut(x1, (-1, -1, 2, 2)),
        },
        ("struct_maintex_in_dependency", "x.bundle"): {"far/s.png": cut(sheet(0x30, 3, 3), (0, 0, 3, 3))},
        ("material_in_dependency", "x.bundle"): {
            "atlas_b/wide.png": cut(rgb, (1, 0, 3, 3)), "atlas_b/in.png": cut(rgb, (0, 1, 2, 1)),
        },
        ("null_material_and_reference", "x.bundle"): {
            "plain/p.png": cut(sheet(7, 2, 1), (0, 0, 2, 1)), "nomat/q.png": cut(sheet(0x0B, 1, 1), (0, 0, 1, 1)),
        },
        ("missing_sheets", "x.bundle"): {
            **{f"n{n}/p.png": cut(sheet(0x30 + n, 1, 1), (0, 0, 1, 1)) for n in range(1, 9)},
            "n9/p.png": cut(sheet(0x3A, 1, 1), (0, 0, 1, 1)),  # the own `far`, by n9's material
        },
        ("duplicates", "x.bundle"): {
            "dup/t.png": cut(dup, (1, 0, 1, 1)), "dup/s.png": cut(dup, (2, 0, 2, 2)),
            "p/q/r.png": cut(dup, (0, 0, 1, 1)),
        },
        ("two_atlases_one_sheet", "x.bundle"): {
            "shared.png": cut(x7), **left_right, "other.png": cut(sheet(0x21, 1, 1)),
        },
        ("two_atlases_without_textures", "x.bundle"): dict(left_right),
        ("sheet_texture_key_taken", "x.bundle"): {**left_right, "sheet.png": cut(sheet(0x61, 1, 1))},
        ("texture_key_is_json_key", "x.bundle"): {
            "tk.json": cut(sheet(0x1C, 2, 1)), "tk/s.png": cut(sheet(0x1C, 2, 1), (0, 0, 2, 1)),
        },
        ("two_sheet_groups", "x.bundle"): {
            "ga/x.png": cut(sheet(0x1E, 1, 1), (0, 0, 1, 1)), "gc/x.png": cut(sheet(0x1E, 1, 1), (0, 0, 1, 1)),
            "gb/x.png": cut(sheet(0x1D, 1, 1), (0, 0, 1, 1)),
        },
        ("json_bytes", "x.bundle"): {"bin/c.png": cut(x8, (0, 0, 1, 1)), "bin/d.png": cut(x8, (1, 1, 1, 1))},
        ("json_bytes_with_textures", "x.bundle"): {
            "bin.png": cut(x8), "bin/c.png": cut(x8, (0, 0, 1, 1)), "bin/d.png": cut(x8, (1, 1, 1, 1)),
        },
        ("floats", "x.bundle"): {"f/x.png": cut(sheet(4, 1, 1), (0, 0, 1, 1))},
        ("non_ascii", "x.bundle"): {"ätlas/精灵.png": cut(sheet(6, 2, 2), (0, 0, 1, 1))},
        ("layouts", "x.bundle"): {"mixed/m.png": cut(sheet(0x0C, 1, 1), (0, 0, 1, 1))},
        ("failed_reads", "x.bundle"): {
            "short.png": cut(sheet(0x15, 1, 1)), "via.png": cut(sheet(0x16, 1, 1)),
            "extra.png": cut(sheet(0x1E, 1, 1)), "mtx.png": cut(sheet(0x19, 1, 1)),
            "tsx.png": cut(sheet(0x2E, 1, 1)), "tmx.png": cut(sheet(0x2C, 1, 1)),
            **one_by_one({"mt": 0x1A, "ts": 0x2F, "tl": 0x2B, "tm": 0x2D, "tq": 0x31}),
        },
        ("streamed_sheets", "x.bundle"): {
            "own.png": cut(x16), "own/s.png": cut(x16, (0, 0, 2, 2)),
            "dep.png": cut(sheet(0x1A, 1, 2)), "dep/s.png": cut(sheet(0x1A, 1, 2), (0, 0, 1, 2)),
            "twin.png": cut(sheet(0x18, 2, 1)), "twin/s.png": cut(sheet(0x18, 2, 1), (0, 0, 2, 1)),
            **one_by_one({"res": 0x41, "as": 0x42, "st": 0x43, "bb": 0x45, "cc": 0x48, "pp": 0x49}),
        },
        ("repeated_rows", "x.bundle"): one_by_one({"rep": 0x53, "nest": 0x54}),
        ("fallback_rows", "x.bundle"): {
            "mx.png": cut(sheet(0x84, 1, 1)), "nx.png": cut(sheet(0x86, 1, 1)),
            "wx.png": cut(sheet(0x94, 1, 1)), "zx.png": cut(sheet(0x96, 1, 1)),
            **one_by_one({"nn": 0x83, "am": 0x85, "an": 0x87, "wq": 0x88, "qq": 0x89, "aw": 0x95, "az": 0x97}),
        },
        ("crop_bounds", "x.bundle"): {"cb/ok.png": cut(sheet(0x1B, 1, 1), (0, 0, 1, 1))},
        ("failed_sheet_rows", "x.bundle"): {"sheet.png": cut(sheet(0x61, 1, 1))},
        ("winners", "a.bundle"): winner_input(1, ["k", "j"]),
        ("winners", "b.bundle"): winner_input(1, ["k", "j"]),
        ("winners", "c.bundle"): winner_input(0x31, ["k", "j", "i"]),
        ("winners", "d.bundle"): winner_input(0x41, ["k", "h"]),
        ("winners", "e.bundle"): winner_input(0x51, ["k"]),
        ("winners", "f.bundle"): winner_input(0x61, ["i"]),
        ("winners", "g.bundle"): winner_input(0x71, ["k", "j", "i"]),
    }  # fmt: skip
    for input_id, base in (("p.bundle", 0x21), ("q.bundle", 0x22)):
        s = sheet(base, 2, 1)
        t[("textured_winners", input_id)] = {"sheet.png": cut(s), "at/s.png": cut(s, (0, 0, 1, 1))}
    t[("textured_winners", "r.bundle")] = {"sheet.png": cut(sheet(0x23, 2, 1))}
    return t


def px(base, x, y, rgb):
    p = ((base + x + 16 * y) & 0xFF, (0x40 + x) & 0xFF, (0x80 + y) & 0xFF, (0xFF - x - y) & 0xFF)
    return p[:3] if rgb else p


def expected(s, rect):
    """The image `rect` cuts from sheet `s` (its whole for None): mode, size and pixels, zero past the edge."""
    (w, h), rgb = s["size"], s["rgb"]
    x0, y0, cw, ch = rect or (0, 0, w, h)
    zero = (0,) * (3 if rgb else 4)
    pixels = [px(s["base"], x, y, rgb) if 0 <= x < w and 0 <= y < h else zero
              for y in range(y0, y0 + ch) for x in range(x0, x0 + cw)]  # fmt: skip
    return ("RGB" if rgb else "RGBA", (cw, ch), pixels)


@unittest.skipUnless(ORACLE, "needs UnityPy 1.25.2 (the oracle image)")
class Pixels(unittest.TestCase):
    def test_every_image_output_is_its_sheet_s_pixels(self):
        want = table()
        seen = set()
        for name, scene in ngui_scenes.scenes().items():
            for r in ngui_scenes.run(scene):
                for o in r.outputs:
                    if o.kind == "atlas_json":
                        continue
                    with self.subTest(scene=name, input=r.input, key=o.key):
                        s, rect = want[(name, r.input)][o.key]
                        with Image.open(io.BytesIO(o.data)) as im:
                            got = (im.mode, im.size, list(im.getdata()))
                        self.assertEqual(got, expected(s, rect))
                        seen.add((name, r.input, o.key))
        self.assertEqual(seen, {(n, i, k) for (n, i), keys in want.items() for k in keys})


if __name__ == "__main__":
    unittest.main()
