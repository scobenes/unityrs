"""The NGUI cutter's reference (`ngui_cutter.py`) on unityrs-testkit's NGUI scenes, against expectations worked out
by hand from docs/BATCH_SPEC.md section 5, 2.1 to 2.3, 3 and 4.4.

Each scene's outputs, units, skips (in record order) and found sheets are written out here; a JSON in full where
it is short, else by field. Every image's pixels are checked in `test_ngui_pixels.py`, from the testkit's pixel
rule; a few here too, written out. The scenes are built by cargo (`ngui_scenes.py`); needs UnityPy 1.25.2 (the
oracle image), skipped elsewhere.
"""

import io
import json
import sys
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE / "tests"))

from rows_support import ORACLE  # noqa: E402

import child  # noqa: E402
import scratch  # noqa: E402

if ORACLE:
    from PIL import Image

    import ngui_cutter as nc
    import ngui_scenes

Z4, Z3 = (0, 0, 0, 0), (0, 0, 0)
KEYS = ["name", "texture_width", "texture_height", "pixel_size", "settings", "sprite_count", "sprites"]
MB, TEX = "MonoBehaviour", "Texture2D"


def px(base, x, y, rgb=False):
    p = ((base + x + 16 * y) & 0xFF, (0x40 + x) & 0xFF, (0x80 + y) & 0xFF, (0xFF - x - y) & 0xFF)
    return p[:3] if rgb else p


def image(data):
    with Image.open(io.BytesIO(data)) as im:
        return im.mode, im.size, list(im.getdata())


def skips(result):
    return [(s.serialized_file, s.path_id, s.cls, s.output, s.reason, s.sprite, s.detail) for s in result.skips]


def outputs(result):
    return [(o.kind, o.key, o.unit) for o in result.outputs]


def by_key(result):
    return {o.key: o for o in result.outputs}


def atlas_skip(cab, path_id, reason, detail=None):
    return (cab, path_id, MB, "atlas", reason, None, detail)


def texture_skip(cab, path_id, reason, detail=None):
    return (cab, path_id, TEX, "texture", reason, None, detail)


def unit_of(json_key, *sprites):
    return [("atlas_json", json_key, json_key)] + [("atlas_sprite", s, json_key) for s in sprites]


@unittest.skipUnless(ORACLE, "needs UnityPy 1.25.2 (the oracle image)")
class Cutter(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.scenes = ngui_scenes.scenes()

    def one(self, name):
        (result,) = ngui_scenes.run(self.scenes[name])
        return result

    def crop(self, result, key, mode, size, pixels):
        self.assertEqual(image(by_key(result)[key].data), (mode, size, pixels), key)

    def test_the_material_names_the_sheet_and_crops_pad_past_its_edges(self):
        r = self.one("own_sheet_by_material_name")
        self.assertEqual(outputs(r), unit_of("ui.json", "ui/a.png", "ui/b.png", "ui/edge.png", "ui/neg.png"))
        # Two empty rectangles (a width of 0, a width of -1), sprites 4 and 5.
        self.assertEqual(skips(r), [("CAB-x1", 5, MB, "atlas_sprite", "empty_rect", i, None) for i in (4, 5)])
        doc = json.loads(by_key(r)["ui.json"].data)
        # The sheet is `sheet_a` (4x4), the material's, not the texture named `ui` that comes first.
        self.assertEqual(r.sheets, [("CAB-x1", 5, "material_name", "CAB-x1", 3)])
        self.assertEqual((doc["texture_width"], doc["texture_height"], doc["sprite_count"]), (4, 4, 6))
        first = doc["sprites"][0]
        self.assertEqual(
            [first[k] for k in ("borderLeft", "borderBottom", "paddingLeft", "paddingBottom")], [1, 4, 5, 8]
        )
        self.assertIs(first["rotated"], True)
        # Borders, paddings and `rotated` are not applied: `a` is its rectangle.
        a = [(1, 64, 128, 255), (2, 65, 128, 254), (17, 64, 129, 254), (18, 65, 129, 253)]
        self.crop(r, "ui/a.png", "RGBA", (2, 2), a)
        self.crop(r, "ui/edge.png", "RGBA", (2, 2), [(52, 67, 131, 249), Z4, Z4, Z4])
        self.crop(r, "ui/neg.png", "RGBA", (2, 2), [Z4, Z4, Z4, (1, 64, 128, 255)])

    def test_a_struct_maintex_read_in_a_dependency_names_an_own_sheet(self):
        r = self.one("struct_maintex_in_dependency")
        self.assertEqual(outputs(r), unit_of("far.json", "far/s.png"))
        self.assertEqual(r.sheets, [("CAB-x2", 4, "material_name", "CAB-x2", 3)])
        self.assertEqual(skips(r), [])
        self.assertEqual(
            by_key(r)["far.json"].data.decode("ascii"),
            """{
  "name": "far",
  "texture_width": 3,
  "texture_height": 3,
  "pixel_size": 1.0,
  "settings": null,
  "sprite_count": 1,
  "sprites": [
    {
      "name": "s",
      "x": 0,
      "y": 0,
      "width": 3,
      "height": 3,
      "borderLeft": 0,
      "borderRight": 0,
      "borderTop": 0,
      "borderBottom": 0,
      "paddingLeft": 0,
      "paddingRight": 0,
      "paddingTop": 0,
      "paddingBottom": 0
    }
  ]
}""",
        )
        # The own `shared_sheet` (base 0x30), not the dependency's texture of that name nor the own `far`.
        self.assertEqual(image(by_key(r)["far/s.png"].data)[2][-1], (82, 66, 130, 251))

    def test_a_name_no_own_texture_has_falls_back_to_the_atlas_name(self):
        r = self.one("material_in_dependency")
        self.assertEqual(outputs(r), unit_of("atlas_b.json", "atlas_b/wide.png", "atlas_b/in.png"))
        doc = json.loads(by_key(r)["atlas_b.json"].data)
        self.assertEqual((doc["texture_width"], doc["texture_height"]), (2, 2))
        self.assertEqual(r.sheets, [("CAB-x3", 2, "atlas_name", "CAB-x3", 22)])
        # An RGB24 sheet: the crops keep its mode, and the padding is RGB zeros.
        self.crop(r, "atlas_b/wide.png", "RGB", (3, 3), [(6, 65, 128), Z3, Z3, (22, 65, 129), Z3, Z3, Z3, Z3, Z3])

    def test_a_null_or_missing_material_a_reference_atlas_and_no_sheet(self):
        r = self.one("null_material_and_reference")
        self.assertEqual(outputs(r), unit_of("plain.json", "plain/p.png") + unit_of("nomat.json", "nomat/q.png"))
        self.assertEqual(
            skips(r), [atlas_skip("CAB-x4", 4, "reference_atlas"), atlas_skip("CAB-x4", 5, "sheet_not_found")]
        )
        # `nomat` has no `material` field: no name, so the texture of its own name.
        self.assertEqual(r.sheets, [("CAB-x4", 3, "atlas_name", "CAB-x4", 2), ("CAB-x4", 6, "atlas_name", "CAB-x4", 7)])
        doc = json.loads(by_key(r)["nomat.json"].data)
        self.assertEqual((doc["pixel_size"], doc["settings"], doc["sprite_count"]), (1.0, None, 1))

    def test_every_way_step_one_gives_no_name_falls_to_the_atlas_name(self):
        r = self.one("missing_sheets")
        self.assertEqual(outputs(r), [o for n in range(1, 9) for o in unit_of(f"n{n}.json", f"n{n}/p.png")])
        self.assertEqual(skips(r), [])
        # n1 to n8 (path ids 10 to 17) each cut the own texture of its name (20 to 27).
        self.assertEqual(r.sheets, [("CAB-x5", p, "atlas_name", "CAB-x5", p + 10) for p in range(10, 18)])

    def test_names_and_keys_inside_one_input(self):
        r = self.one("duplicates")
        self.assertEqual(
            outputs(r), unit_of("dup.json", "dup/t.png", "dup/s.png") + unit_of("p.json", "p/q/r.png") + [
                ("atlas_json", "p/q.json", "p/q.json")]
        )  # fmt: skip
        self.assertEqual(
            skips(r),
            [
                ("CAB-x6", 3, MB, "atlas_sprite", "duplicate_key", 0, None),  # E30: the later `s` wins
                atlas_skip("CAB-x6", 5, "duplicate_name"),
                ("CAB-x6", 7, MB, "atlas_sprite", "duplicate_key", 0, None),  # `p`'s sprite came first
            ],
        )
        self.crop(r, "dup/s.png", "RGBA", (2, 2), [px(2, 2, 0), px(2, 3, 0), px(2, 2, 1), px(2, 3, 1)])
        # The JSON keeps every element, the losing duplicate's among them.
        self.assertEqual([s["name"] for s in json.loads(by_key(r)["dup.json"].data)["sprites"]], ["s", "t", "s"])

    def test_two_atlases_cutting_one_sheet_are_one_unit_with_its_texture(self):
        r = self.one("two_atlases_one_sheet")
        unit = [("texture", "shared.png", "shared.png")]
        unit += [(k, key, "shared.png") for k, key, _ in unit_of("left.json", "left/l.png")]
        unit += [(k, key, "shared.png") for k, key, _ in unit_of("right.json", "right/r.png")]
        # The texture no atlas cuts is a plain candidate, exported after the sheet's group.
        self.assertEqual(outputs(r), unit + [("texture", "other.png", None)])
        self.assertEqual(skips(r), [])

    def test_without_textures_each_atlas_is_its_own_unit(self):
        r = self.one("two_atlases_without_textures")
        self.assertEqual(outputs(r), unit_of("left.json", "left/l.png") + unit_of("right.json", "right/r.png"))

    def test_a_sheet_whose_texture_key_is_taken_makes_no_unit_of_it(self):
        r = self.one("sheet_texture_key_taken")
        self.assertEqual(skips(r), [texture_skip("CAB-x13", 3, "duplicate_key")])
        # The decoy (path id 2) took `sheet.png`: a plain texture output, after the groups.
        self.assertEqual(
            outputs(r),
            unit_of("left.json", "left/l.png")
            + unit_of("right.json", "right/r.png")
            + [("texture", "sheet.png", None)],
        )
        self.assertEqual(by_key(r)["sheet.png"].path_id, 2)

    def test_a_texture_key_equal_to_the_atlas_s_json_key(self):
        r = self.one("texture_key_is_json_key")
        # Textures come first in plan order: the sheet takes `tk.json`, the atlas's JSON is `duplicate_key`.
        self.assertEqual(outputs(r), [("texture", "tk.json", "tk.json"), ("atlas_sprite", "tk/s.png", "tk.json")])
        self.assertEqual(skips(r), [("CAB-x18", 3, MB, "atlas_json", "duplicate_key", None, None)])

    def test_sheet_groups_export_in_the_order_of_their_first_atlas(self):
        r = self.one("two_sheet_groups")
        # `s2`'s group (`ga`, `gc`) before `s1`'s (`gb`), though `s1` comes first in object order.
        self.assertEqual(
            outputs(r),
            [("atlas_sprite", "ga/x.png", None)]
            + unit_of("gc.json", "gc/x.png")
            + [("atlas_sprite", "gb/x.png", None)],
        )
        self.assertEqual(skips(r), [("CAB-x19", p, MB, "atlas_json", "json_unwritable", None, None) for p in (6, 7)])

    def test_bytes_skip_the_json_alone(self):
        r = self.one("json_bytes")
        self.assertEqual(outputs(r), [("atlas_sprite", "bin/c.png", None), ("atlas_sprite", "bin/d.png", None)])
        self.assertEqual(skips(r), [("CAB-x8", 3, MB, "atlas_json", "json_unwritable", None, None)])
        self.crop(r, "bin/d.png", "RGBA", (1, 1), [(26, 65, 129, 253)])
        r = self.one("json_bytes_with_textures")
        self.assertEqual(
            outputs(r),
            [("texture", "bin.png", "bin.png"), ("atlas_sprite", "bin/c.png", "bin.png"),
             ("atlas_sprite", "bin/d.png", "bin.png")],
        )  # fmt: skip
        self.assertEqual(skips(r), [("CAB-x8", 3, MB, "atlas_json", "json_unwritable", None, None)])

    def test_floats_and_a_pair_are_written_as_python_writes_them(self):
        r = self.one("floats")
        self.assertEqual(
            by_key(r)["f.json"].data.decode("ascii"),
            """{
  "name": "f",
  "texture_width": 1,
  "texture_height": 1,
  "pixel_size": 0.10000000149011612,
  "settings": {
    "d1": 1e-05,
    "d2": 1e+16,
    "d3": -0.0,
    "d4": NaN,
    "d5": Infinity,
    "d6": -Infinity,
    "f1": 9.999999747378752e-06,
    "f2": 16777216.0,
    "f3": -0.0,
    "f4": NaN,
    "f5": 3.4028234663852886e+38,
    "i": -7,
    "b": true,
    "list": [
      0.10000000149011612,
      2.5
    ],
    "pair": [
      3,
      "three"
    ]
  },
  "sprite_count": 1,
  "sprites": [
    {
      "name": "x",
      "x": 0,
      "y": 0,
      "width": 1,
      "height": 1,
      "borderLeft": 0,
      "borderRight": 0,
      "borderTop": 0,
      "borderBottom": 0,
      "paddingLeft": 0,
      "paddingRight": 0,
      "paddingTop": 0,
      "paddingBottom": 0,
      "scale": 0.10000000149011612,
      "weight": 0.1
    }
  ]
}""",
        )

    def test_names_outside_ascii(self):
        r = self.one("non_ascii")
        self.assertEqual(
            outputs(r), [("atlas_json", "_tlas.json", "_tlas.json"), ("atlas_sprite", "ätlas/精灵.png", "_tlas.json")]
        )
        self.assertEqual(
            skips(r),
            [
                ("CAB-x10", 3, MB, "atlas_sprite", "unwritable_key(not_utf8)", 1, None),
                ("CAB-x10", 3, MB, "atlas_sprite", "unwritable_key(control_char)", 2, None),
            ],
        )
        text = by_key(r)["_tlas.json"].data.decode("ascii")
        for written in (
            '"name": "\\u00e4tlas"',
            '"name": "\\u7cbe\\u7075"',
            '"name": "bad\\udcff"',
            '"name": "tab\\tname"',
        ):
            self.assertIn(written, text)

    def test_recognition_from_the_typetree(self):
        r = self.one("layouts")
        self.assertEqual(r.unresolved_scripts, 2)  # a null m_Script, and one to a missing object
        unsupported = [atlas_skip("CAB-x11a", p, "ngui_layout_unsupported") for p in range(17, 24)]
        self.assertEqual(
            skips(r),
            [atlas_skip("CAB-x11a", p, "ngui_layout_unsupported") for p in (13, 14)]
            + [atlas_skip("CAB-x11a", 15, "unnamed_atlas")]
            + unsupported  # strings, no `width`, a float, a bool, a map, an `int` name
            + [atlas_skip("CAB-x11b", 1, "no_typetree"), atlas_skip("CAB-x11c", 2, "no_typetree")],
        )
        # Rectangle fields of `UInt8`, `unsigned int`, `UInt64` and `int`: recognised, read as integers.
        self.assertEqual(outputs(r), unit_of("mixed.json", "mixed/m.png"))
        self.assertEqual(
            by_key(r)["mixed.json"].data.decode("ascii"),
            """{
  "name": "mixed",
  "texture_width": 1,
  "texture_height": 1,
  "pixel_size": null,
  "settings": null,
  "sprite_count": 1,
  "sprites": [
    {
      "name": "m",
      "x": 0,
      "y": 0,
      "width": 1,
      "height": 1
    }
  ]
}""",
        )

    def test_a_failed_texture_read_keeps_its_name_and_answers_for_its_atlas(self):
        r = self.one("failed_reads")
        self.assertEqual(
            skips(r),
            [
                texture_skip("CAB-x15", 2, "decode_error", "read"),  # cut short, read past its end
                texture_skip("CAB-x15", 3, "duplicate_name"),  # the first `ui`, peeked, took the name
                atlas_skip("CAB-x15", 4, "decode_error", "read"),  # its sheet's read: the sheet's answer
                atlas_skip("CAB-x15", 5, "decode_error", "read"),  # its own read, before its sheet
            ],
        )
        self.assertEqual(r.sheets, [("CAB-x15", 4, "atlas_name", "CAB-x15", 2)])
        # No atlas cuts `short` once its atlas failed: a plain texture output.
        self.assertEqual(outputs(r), [("texture", "short.png", None)])

    def test_a_sheet_s_skip_skips_its_atlas(self):
        r = self.one("sheet_skips")
        self.assertEqual(outputs(r), [])
        at_the_export = [
            (3, 6, "decode_error", "not_enough_data"),
            (8, 12, "limit(texture_pixels, 268435456)", None),  # 16388 x 16384 padded, 8 bytes of data
            (9, 13, "decode_error", "pillow_memory_error"),  # one pixel past the raw decoder's cap
            (10, 14, "decode_error", "negative_size"),
            (11, 15, "decode_error", "no_image_data"),
        ]
        self.assertEqual(
            skips(r),
            [
                texture_skip("CAB-x12", 2, "unsupported_format(Alpha8)"),
                texture_skip("CAB-x12", 4, "empty_image"),
                atlas_skip("CAB-x12", 5, "unsupported_format(Alpha8)"),
                atlas_skip("CAB-x12", 7, "empty_image"),
            ]
            # The decode's answers, at the export, in group order: the texture, then its atlas.
            + [s for t, a, reason, detail in at_the_export
               for s in (texture_skip("CAB-x12", t, reason, detail), atlas_skip("CAB-x12", a, reason, detail))],
        )  # fmt: skip

    def test_streamed_sheets_by_the_own_chain(self):
        r = self.one("streamed_sheets")
        units = []
        for name in ("own", "dep", "twin"):
            units += [("texture", f"{name}.png", f"{name}.png"), ("atlas_json", f"{name}.json", f"{name}.png"),
                      ("atlas_sprite", f"{name}/s.png", f"{name}.png")]  # fmt: skip
        self.assertEqual(outputs(r), units)
        gone = "outside_view(archive:/CAB-x16/CAB-missing.resS)"
        self.assertEqual(
            skips(r),
            [
                texture_skip("CAB-x16", 4, gone),
                atlas_skip("CAB-x16", 10, gone),
                texture_skip("CAB-x16", 6, "decode_error", "no_image_data"),  # an empty stream path
                atlas_skip("CAB-x16", 12, "decode_error", "no_image_data"),
                texture_skip("CAB-x16", 7, "decode_error", "negative_size"),
                atlas_skip("CAB-x16", 13, "decode_error", "negative_size"),
            ],
        )
        # The input's own top-level `CAB-twin.resS` (base 0x18), not the nested bundle's (0x19).
        self.crop(r, "twin.png", "RGBA", (2, 1), [px(0x18, 0, 0), px(0x18, 1, 0)])
        self.crop(r, "own.png", "RGBA", (2, 2), [px(0x17, x, y) for y in (0, 1) for x in (0, 1)])

    def test_crops_pillow_refuses_are_skipped_at_the_plan(self):
        r = self.one("crop_bounds")
        self.assertEqual(outputs(r), unit_of("cb.json", "cb/ok.png"))
        self.assertEqual(
            skips(r),
            [
                ("CAB-x17", 3, MB, "atlas_sprite", "decode_error", 1, "decompression_bomb"),  # 2 x 10^8 pixels
                ("CAB-x17", 3, MB, "atlas_sprite", "decode_error", 2, "crop_overflow"),  # a right edge of 2^31
            ],
        )
        self.assertEqual(json.loads(by_key(r)["cb.json"].data)["sprite_count"], 3)

    def test_every_json_has_section_five_s_keys_in_order(self):
        for name, scene in self.scenes.items():
            for result in ngui_scenes.run(scene):
                for o in result.outputs:
                    if o.kind == "atlas_json":
                        with self.subTest(scene=name, key=o.key):
                            self.assertEqual(list(json.loads(o.data)), KEYS)
                            self.assertEqual(o.data, json.dumps(json.loads(o.data), indent=2).encode("ascii"))

    def test_the_cli_prints_the_report_and_writes_the_outputs(self):
        tmp = scratch.for_case(self, "unityrs-ngui-")
        scene = self.scenes["struct_maintex_in_dependency"]
        args = [sys.executable, str(HERE / "ngui_cutter.py"), "--input", "x.bundle", "--out", str(tmp / "out")]
        for bid, data in scene.bundles.items():
            (tmp / bid).write_bytes(data)
            args += ["--bundle", f"{bid}={tmp / bid}"]
        proc = child.run(args, capture_output=True, text=True, check=True)
        r = self.one("struct_maintex_in_dependency")
        self.assertEqual(json.loads(proc.stdout), nc.report(r))
        self.assertEqual((tmp / "out/far.json").read_bytes(), by_key(r)["far.json"].data)


@unittest.skipUnless(ORACLE, "needs UnityPy 1.25.2 (the oracle image)")
class Keys(unittest.TestCase):
    def test_safe(self):
        cases = {
            "ätlas".encode(): b"_tlas",
            b"bad\xff": b"bad_",
            b"..": b"unnamed",
            b".a.b.": b"a.b",
            "精灵".encode(): b"__",
        }
        for value, want in cases.items():
            self.assertEqual(nc.safe(value), want, value)

    def test_expand(self):
        self.assertEqual(nc.expand("{{x}}/{atlas}-{safe_atlas}", "atlas_json", {"atlas": b"a b"}), b"{x}/a b-a_b")
        for template in ("{sprite}.png", "{nope}", "a}b"):
            with self.subTest(template=template), self.assertRaises(ValueError):
                nc.expand(template, "atlas_json", {"atlas": b"a"})

    def test_key_rules_in_section_2_3_s_order(self):
        cases = [
            (b"", "empty"),
            (b"\xff", "not_utf8"),
            (b"a\x7fb", "control_char"),
            (b"/a", "leading_slash"),
            (b"a/", "trailing_slash"),
            (b"a//b", "empty_segment"),
            (b"a/./b", "dot_segment"),
            (b"x" * 256, "segment_too_long"),
            (b"/".join([b"a" * 200] * 6), "too_long"),
            (b"_unityrs/x", "reserved_prefix"),
            (b"a/.unityrs-tmp-1", "reserved_name"),
            (b"a/b.png", None),
        ]
        for key, want in cases:
            self.assertEqual(nc.key_refusal(key), want, key)
        self.assertEqual(nc.key_refusal(b"x", b"_unityrs/"), "reserved_prefix")  # after the sink's prefix

    def test_a_view(self):
        self.assertEqual(nc.view_of({"x": ["b", "a"], "a": ["c"], "b": [], "z": ["x"]}, "x"), ["a", "b", "c", "x"])


if __name__ == "__main__":
    unittest.main()
