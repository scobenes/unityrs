"""The NGUI cutter's reference (`ngui_cutter.py`, `ngui_winners.py`) on unityrs-testkit's NGUI scenes, against
expectations worked out by hand from docs/BATCH_SPEC.md section 5, 2.1 to 2.3, 4.4 and 4.5.

A scene's sheets are the testkit's `ngui::sheet`s: the pixel at (x, y), y from the image's top, is
`(base + x + 16 y, 0x40 + x, 0x80 + y, 0xFF - x - y)` (RGB24 drops the last), each byte wrapping. So every crop's
pixels below are written from that rule and the rectangle, with E23's zero pixels past the sheet's edge, and
compared with the crop's PNG as Pillow reads it back. Each JSON is written out in full where it is short, and
otherwise checked by field. The scenes are built by cargo (`ngui_scenes.py`); needs UnityPy 1.25.2 (the oracle
image), skipped elsewhere.
"""

import io
import json
import sys
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE / "tests"))

import child  # noqa: E402
import scratch  # noqa: E402
from rows_support import ORACLE  # noqa: E402

if ORACLE:
    import ngui_cutter as nc
    import ngui_scenes
    import ngui_winners as nw
    from PIL import Image

Z4, Z3 = (0, 0, 0, 0), (0, 0, 0)
KEYS = ["name", "texture_width", "texture_height", "pixel_size", "settings", "sprite_count", "sprites"]


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
        self.assertEqual(
            outputs(r),
            [("atlas_json", "ui.json", "ui.json")]
            + [("atlas_sprite", f"ui/{n}.png", "ui.json") for n in ("a", "b", "edge", "neg")],
        )
        # Two empty rectangles (a width of 0, a width of -1), sprites 4 and 5.
        self.assertEqual(
            skips(r),
            [("CAB-x1", 5, "MonoBehaviour", "atlas_sprite", "empty_rect", i, None) for i in (4, 5)],
        )
        doc = json.loads(by_key(r)["ui.json"].data)
        # The sheet is `sheet_a` (4x4), the material's, not the texture named `ui` that comes first.
        self.assertEqual(r.sheets, [("CAB-x1", 5, "material_name", "CAB-x1", 3)])
        self.assertEqual((doc["texture_width"], doc["texture_height"], doc["sprite_count"]), (4, 4, 6))
        first = doc["sprites"][0]
        self.assertEqual([first[k] for k in ("borderLeft", "borderBottom", "paddingLeft", "paddingBottom")], [1, 4, 5, 8])
        self.assertIs(first["rotated"], True)
        # Borders, paddings and `rotated` are not applied: `a` is its rectangle.
        self.crop(r, "ui/a.png", "RGBA", (2, 2), [(1, 64, 128, 255), (2, 65, 128, 254), (17, 64, 129, 254), (18, 65, 129, 253)])
        self.crop(r, "ui/b.png", "RGBA", (3, 2), [px(1, x, y) for y in (2, 3) for x in (1, 2, 3)])
        self.crop(r, "ui/edge.png", "RGBA", (2, 2), [(52, 67, 131, 249), Z4, Z4, Z4])
        self.crop(r, "ui/neg.png", "RGBA", (2, 2), [Z4, Z4, Z4, (1, 64, 128, 255)])

    def test_a_struct_maintex_read_in_a_dependency_names_an_own_sheet(self):
        r = self.one("struct_maintex_in_dependency")
        self.assertEqual(outputs(r), [("atlas_json", "far.json", "far.json"), ("atlas_sprite", "far/s.png", "far.json")])
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
        self.crop(r, "far/s.png", "RGBA", (3, 3), [px(0x30, x, y) for y in range(3) for x in range(3)])
        self.assertEqual(image(by_key(r)["far/s.png"].data)[2][-1], (82, 66, 130, 251))

    def test_a_name_no_own_texture_has_falls_back_to_the_atlas_name(self):
        r = self.one("material_in_dependency")
        self.assertEqual(
            outputs(r),
            [("atlas_json", "atlas_b.json", "atlas_b.json")]
            + [("atlas_sprite", f"atlas_b/{n}.png", "atlas_b.json") for n in ("wide", "in")],
        )
        doc = json.loads(by_key(r)["atlas_b.json"].data)
        self.assertEqual((doc["texture_width"], doc["texture_height"]), (2, 2))
        self.assertEqual(r.sheets, [("CAB-x3", 2, "atlas_name", "CAB-x3", 22)])
        # An RGB24 sheet: the crops keep its mode, and the padding is RGB zeros.
        self.crop(r, "atlas_b/wide.png", "RGB", (3, 3), [(6, 65, 128), Z3, Z3, (22, 65, 129), Z3, Z3, Z3, Z3, Z3])
        self.crop(r, "atlas_b/in.png", "RGB", (2, 1), [(21, 64, 129), (22, 65, 129)])

    def test_a_null_material_a_reference_atlas_and_no_sheet(self):
        r = self.one("null_material_and_reference")
        self.assertEqual(outputs(r), [("atlas_json", "plain.json", "plain.json"), ("atlas_sprite", "plain/p.png", "plain.json")])
        self.assertEqual(
            skips(r),
            [
                ("CAB-x4", 4, "MonoBehaviour", "atlas", "reference_atlas", None, None),
                ("CAB-x4", 5, "MonoBehaviour", "atlas", "sheet_not_found", None, None),
            ],
        )
        self.crop(r, "plain/p.png", "RGBA", (2, 1), [(7, 64, 128, 255), (8, 65, 128, 254)])
        self.assertEqual(r.sheets, [("CAB-x4", 3, "atlas_name", "CAB-x4", 2)])

    def test_every_way_step_one_gives_no_name(self):
        r = self.one("missing_sheets")
        self.assertEqual(outputs(r), [])
        self.assertEqual(
            skips(r), [("CAB-x5", p, "MonoBehaviour", "atlas", "sheet_not_found", None, None) for p in range(10, 16)]
        )

    def test_names_and_keys_inside_one_input(self):
        r = self.one("duplicates")
        self.assertEqual(
            outputs(r),
            [
                ("atlas_json", "dup.json", "dup.json"),
                ("atlas_sprite", "dup/t.png", "dup.json"),
                ("atlas_sprite", "dup/s.png", "dup.json"),
                ("atlas_json", "p.json", "p.json"),
                ("atlas_sprite", "p/q/r.png", "p.json"),
                ("atlas_json", "p/q.json", "p/q.json"),
            ],
        )
        self.assertEqual(
            skips(r),
            [
                ("CAB-x6", 3, "MonoBehaviour", "atlas_sprite", "duplicate_key", 0, None),  # E30: the later `s` wins
                ("CAB-x6", 5, "MonoBehaviour", "atlas", "duplicate_name", None, None),
                ("CAB-x6", 7, "MonoBehaviour", "atlas_sprite", "duplicate_key", 0, None),  # `p`'s sprite came first
            ],
        )
        self.crop(r, "dup/s.png", "RGBA", (2, 2), [px(2, 2, 0), px(2, 3, 0), px(2, 2, 1), px(2, 3, 1)])
        self.crop(r, "p/q/r.png", "RGBA", (1, 1), [(2, 64, 128, 255)])
        # The JSON keeps every element, the losing duplicate's among them.
        self.assertEqual([s["name"] for s in json.loads(by_key(r)["dup.json"].data)["sprites"]], ["s", "t", "s"])

    def test_two_atlases_cutting_one_sheet_are_one_unit_with_its_texture(self):
        r = self.one("two_atlases_one_sheet")
        self.assertEqual(
            outputs(r),
            [
                ("texture", "shared.png", "shared.png"),
                ("atlas_json", "left.json", "shared.png"),
                ("atlas_sprite", "left/l.png", "shared.png"),
                ("atlas_json", "right.json", "shared.png"),
                ("atlas_sprite", "right/r.png", "shared.png"),
            ],
        )
        self.assertEqual(skips(r), [])
        self.assertEqual(r.uncut_textures, [("CAB-x7", 3, "other.png")])
        self.crop(r, "shared.png", "RGBA", (3, 2), [px(0x11, x, y) for y in range(2) for x in range(3)])
        self.crop(r, "left/l.png", "RGBA", (1, 2), [px(0x11, 0, 0), px(0x11, 0, 1)])
        self.crop(r, "right/r.png", "RGBA", (2, 2), [px(0x11, 1, 0), px(0x11, 2, 0), px(0x11, 1, 1), px(0x11, 2, 1)])

    def test_without_textures_each_atlas_is_its_own_unit(self):
        r = self.one("two_atlases_without_textures")
        self.assertEqual(
            outputs(r),
            [
                ("atlas_json", "left.json", "left.json"),
                ("atlas_sprite", "left/l.png", "left.json"),
                ("atlas_json", "right.json", "right.json"),
                ("atlas_sprite", "right/r.png", "right.json"),
            ],
        )

    def test_a_sheet_whose_texture_key_is_taken_makes_no_unit_of_it(self):
        r = self.one("sheet_texture_key_taken")
        self.assertEqual(skips(r), [("CAB-x13", 3, "Texture2D", "texture", "duplicate_key", None, None)])
        self.assertEqual([o[2] for o in outputs(r)], ["left.json", "left.json", "right.json", "right.json"])
        self.assertEqual(r.uncut_textures, [("CAB-x13", 2, "sheet.png")])

    def test_bytes_skip_the_json_alone(self):
        r = self.one("json_bytes")
        self.assertEqual(outputs(r), [("atlas_sprite", "bin/c.png", None), ("atlas_sprite", "bin/d.png", None)])
        self.assertEqual(skips(r), [("CAB-x8", 3, "MonoBehaviour", "atlas_json", "json_unwritable", None, None)])
        self.crop(r, "bin/d.png", "RGBA", (1, 1), [(26, 65, 129, 253)])
        r = self.one("json_bytes_with_textures")
        self.assertEqual(
            outputs(r),
            [("texture", "bin.png", "bin.png"), ("atlas_sprite", "bin/c.png", "bin.png"), ("atlas_sprite", "bin/d.png", "bin.png")],
        )
        self.assertEqual(skips(r), [("CAB-x8", 3, "MonoBehaviour", "atlas_json", "json_unwritable", None, None)])

    def test_floats_are_written_as_python_writes_them(self):
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
                ("CAB-x10", 3, "MonoBehaviour", "atlas_sprite", "unwritable_key(not_utf8)", 1, None),
                ("CAB-x10", 3, "MonoBehaviour", "atlas_sprite", "unwritable_key(control_char)", 2, None),
            ],
        )
        text = by_key(r)["_tlas.json"].data.decode("ascii")
        for written in ('"name": "\\u00e4tlas"', '"name": "\\u7cbe\\u7075"', '"name": "bad\\udcff"', '"name": "tab\\tname"'):
            self.assertIn(written, text)

    def test_recognition(self):
        r = self.one("layouts")
        self.assertEqual(outputs(r), [])
        self.assertEqual(r.unresolved_scripts, 2)  # a null m_Script, and one to a missing object
        self.assertEqual(
            skips(r),
            [
                ("CAB-x11a", 13, "MonoBehaviour", "atlas", "ngui_layout_unsupported", None, None),
                ("CAB-x11a", 14, "MonoBehaviour", "atlas", "ngui_layout_unsupported", None, None),
                ("CAB-x11a", 15, "MonoBehaviour", "atlas", "unnamed_atlas", None, None),
                ("CAB-x11b", 1, "MonoBehaviour", "atlas", "no_typetree", None, None),
            ],
        )

    def test_a_sheet_s_skip_skips_its_atlas(self):
        r = self.one("sheet_skips")
        self.assertEqual(outputs(r), [])
        self.assertEqual(
            skips(r),
            [
                ("CAB-x12", 2, "Texture2D", "texture", "unsupported_format(Alpha8)", None, None),
                ("CAB-x12", 4, "Texture2D", "texture", "empty_image", None, None),
                ("CAB-x12", 5, "MonoBehaviour", "atlas", "unsupported_format(Alpha8)", None, None),
                ("CAB-x12", 7, "MonoBehaviour", "atlas", "empty_image", None, None),
                # The decode's failure, at the export: the texture, then its atlas.
                ("CAB-x12", 3, "Texture2D", "texture", "decode_error", None, "not_enough_data"),
                ("CAB-x12", 6, "MonoBehaviour", "atlas", "decode_error", None, "not_enough_data"),
            ],
        )

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
        self.assertEqual(json.loads(proc.stdout), nc.report(self.one("struct_maintex_in_dependency")))
        self.assertEqual((tmp / "out/far.json").read_bytes(), by_key(self.one("struct_maintex_in_dependency"))["far.json"].data)


@unittest.skipUnless(ORACLE, "needs UnityPy 1.25.2 (the oracle image)")
class Keys(unittest.TestCase):
    def test_safe(self):
        cases = {"ätlas".encode(): b"_tlas", b"bad\xff": b"bad_", b"..": b"unnamed", b".a.b.": b"a.b", "精灵".encode(): b"__"}
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

    def test_a_rectangle_section_5_does_not_cover_is_a_spec_gap(self):
        for element in ({"x": 0, "y": 0, "width": 1}, {"x": 0, "y": 0, "width": 1, "height": "1"}, [0, 0, 1, 1]):
            with self.subTest(element=element), self.assertRaises(nc.SpecGap):
                nc.rect(element)

    def test_a_view(self):
        self.assertEqual(nc.view_of({"x": ["b", "a"], "a": ["c"], "b": [], "z": ["x"]}, "x"), ["a", "b", "c", "x"])


def decisions(results, **kw):
    return {
        k: (d.status, d.reason, d.flip, d.winner, [(c.input, c.rule, c.winner) for c in d.candidates])
        for k, d in nw.winners(results, **kw).items()
    }


@unittest.skipUnless(ORACLE, "needs UnityPy 1.25.2 (the oracle image)")
class Winners(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.results = ngui_scenes.run(ngui_scenes.scenes()["winners"])
        cls.four = [r for r in cls.results if r.input != "e.bundle"]

    def test_the_inputs_units(self):
        units = {r.input: sorted({(o.key, o.unit) for o in r.outputs}) for r in self.results}
        self.assertEqual(units["a.bundle"], [("j.png", "shared.json"), ("k.png", "shared.json"), ("shared.json", "shared.json")])
        self.assertEqual(units["d.bundle"], [("h.png", None), ("k.png", None)])  # its JSON unwritable: no unit
        self.assertEqual(units["e.bundle"], [("k.png", "other.json"), ("other.json", "other.json")])

    def test_two_winning_units_claiming_one_key(self):
        conflict = lambda w: [("a.bundle", "unit_conflict", w), ("b.bundle", "unit_conflict", False), ("c.bundle", "unit_conflict", False)]  # noqa: E731
        self.assertEqual(
            decisions(self.results),
            {
                "h.png": ("publish", None, False, "d.bundle", [("d.bundle", "first_input", True)]),
                "i.png": ("no_rule", None, False, None, [("c.bundle", "first_input", False)]),
                "j.png": ("withheld", "unit_conflict", True, "a.bundle", conflict(True)),
                "k.png": (
                    "publish", None, False, "e.bundle",
                    conflict(False) + [("d.bundle", "unit", False), ("e.bundle", "first_input", True)],
                ),
                "other.json": ("publish", None, False, "e.bundle", [("e.bundle", "first_input", True)]),
                "shared.json": ("withheld", "unit_conflict", True, "a.bundle", conflict(True)),
            },
        )  # fmt: skip

    def unit(self, rule, winner="a.bundle"):
        return [(i, rule, i == winner) for i in ("a.bundle", "b.bundle", "c.bundle")]

    def test_rule_three_and_a_flip(self):
        got = decisions(self.four)
        self.assertEqual(got["shared.json"], ("publish", None, True, "a.bundle", self.unit("first_input")))
        self.assertEqual(got["j.png"], ("publish", None, True, "a.bundle", self.unit("first_input")))
        self.assertEqual(
            got["k.png"], ("publish", None, True, "a.bundle", self.unit("first_input") + [("d.bundle", "unit", False)])
        )
        self.assertEqual(got["i.png"], ("no_rule", None, False, None, [("c.bundle", "first_input", False)]))

    def test_the_preferred_input_and_md5(self):
        c_json = next(o.data for r in self.four if r.input == "c.bundle" for o in r.outputs if o.key == "shared.json")
        for rule, entry in (("prefer_input", {"input": "c.bundle", "md5": None}),
                            ("prefer_md5", {"input": None, "md5": nw.md5(c_json)})):  # fmt: skip
            with self.subTest(rule=rule):
                got = decisions(self.four, prefer={"shared.json": entry})
                self.assertEqual(got["shared.json"], ("publish", None, False, "c.bundle", self.unit(rule, "c.bundle")))
                self.assertEqual(got["i.png"], ("publish", None, False, "c.bundle", [("c.bundle", rule, True)]))
                self.assertEqual(got["k.png"][3], "c.bundle")

    def test_an_entry_for_another_key_of_a_unit_is_not_consulted(self):
        got = decisions(self.four, prefer={"k.png": {"input": "d.bundle", "md5": None}})
        self.assertEqual(got, decisions(self.four))

    def test_withholding(self):
        got = decisions(self.four, prefer={"shared.json": {"input": "z.bundle", "md5": None}}, failed=["z.bundle"])
        self.assertEqual(got["shared.json"], ("withheld", "withheld", True, "a.bundle", self.unit("withheld")))
        self.assertEqual(got["k.png"][:4], ("withheld", "withheld", True, "a.bundle"))
        self.assertEqual(got["i.png"], ("withheld", "withheld", False, None, [("c.bundle", "withheld", False)]))
        self.assertEqual(got["h.png"], ("publish", None, False, "d.bundle", [("d.bundle", "first_input", True)]))
        got = decisions(self.four, prefer={"h.png": {"input": "z.bundle", "md5": None}}, failed=["z.bundle"])
        self.assertEqual(got["h.png"], ("withheld", "withheld", False, "d.bundle", [("d.bundle", "withheld", True)]))
        self.assertEqual(got["shared.json"][0], "publish")

    def test_publish_incomplete_false(self):
        got = decisions(self.four, failed=["z.bundle"], publish_incomplete=False)
        self.assertEqual({k: d[:2] for k, d in got.items()}, {
            "h.png": ("withheld", "publish_incomplete"), "i.png": ("no_rule", None),
            "j.png": ("withheld", "publish_incomplete"), "k.png": ("withheld", "publish_incomplete"),
            "shared.json": ("withheld", "publish_incomplete"),
        })  # fmt: skip


if __name__ == "__main__":
    unittest.main()
