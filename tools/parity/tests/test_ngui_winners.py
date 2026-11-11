"""The NGUI reference's winners across inputs (`ngui_winners.py`) on the testkit's winners scenes, against each key's
decision worked out by hand from docs/BATCH_SPEC.md 4.5 and 7.3 (E31, E32, E61).

`winners`: inputs `a` to `f`, each an atlas cutting its own sheet, sprite key `{sprite}.png`. `a` and `b` are alike
(unit `shared.json`: `k.png`, `j.png`); `c` has other pixels and one more sprite, `i.png`; `d`'s and `f`'s JSONs are
unwritable, so their crops are plain candidates (`k.png`, `h.png`; `i.png`); `e` is a unit `other.json` whose
`k.png` meets `shared.json`'s. `textured_winners`: `p` and `q` units keyed by their sheet's texture key `sheet.png`,
other pixels in `q`, and `r` a plain `sheet.png`. Needs UnityPy 1.25.2 (the oracle image), skipped elsewhere.
"""

import sys
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE / "tests"))

from rows_support import ORACLE  # noqa: E402

if ORACLE:
    import ngui_scenes
    import ngui_winners as nw

Z = ["z.bundle"]


def decisions(results, **kw):
    return {
        k: (d.status, d.reason, d.flip, d.winner, [(c.input, c.rule, c.winner) for c in d.candidates])
        for k, d in nw.winners(results, **kw).items()
    }


def rows(rule, winner, *inputs):
    """Each input's row with one rule, `winner` the row the key publishes."""
    return [(f"{i}.bundle", rule, i == winner) for i in inputs]


def unit(rule, winner="a", status=("publish", None), flip=True):
    """A key of unit `shared.json` decided for `winner` among `a`, `b`, `c` by `rule`."""
    return (*status, flip, f"{winner}.bundle", rows(rule, winner, "a", "b", "c"))


@unittest.skipUnless(ORACLE, "needs UnityPy 1.25.2 (the oracle image)")
class Winners(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        scenes = ngui_scenes.scenes()
        cls.by = {r.input[0]: r for name in ("winners", "textured_winners") for r in ngui_scenes.run(scenes[name])}

    def pick(self, ids):
        return [self.by[i] for i in ids]

    def test_the_inputs_units(self):
        units = {i: sorted({(o.key, o.unit) for o in r.outputs}) for i, r in self.by.items()}
        self.assertEqual(
            units["a"], [("j.png", "shared.json"), ("k.png", "shared.json"), ("shared.json", "shared.json")]
        )
        self.assertEqual(units["d"], [("h.png", None), ("k.png", None)])  # its JSON unwritable: no unit
        self.assertEqual(units["e"], [("k.png", "other.json"), ("other.json", "other.json")])
        self.assertEqual(units["f"], [("i.png", None)])
        self.assertEqual(units["p"], [("at.json", "sheet.png"), ("at/s.png", "sheet.png"), ("sheet.png", "sheet.png")])
        self.assertEqual(units["r"], [("sheet.png", None)])

    def test_two_winning_units_claiming_one_key(self):
        # `other.json` keeps `k.png` (first by unit key); `shared.json`'s winner is withheld whole, its losers keep
        # the rule that beat them; `c`'s `i.png`, a losing unit's, loses to `f`'s plain candidate.
        conflict = [("a.bundle", "unit_conflict", True), ("b.bundle", "first_input", False),
                    ("c.bundle", "first_input", False)]  # fmt: skip
        k_rows = [(i, r, False) for i, r, _ in conflict] + [
            ("d.bundle", "unit", False),
            ("e.bundle", "first_input", True),
        ]
        self.assertEqual(
            decisions(self.pick("abcdef")),
            {
                "h.png": ("publish", None, False, "d.bundle", rows("first_input", "d", "d")),
                "i.png": ("publish", None, False, "f.bundle", rows("first_input", "f", "c", "f")),
                "j.png": ("withheld", "unit_conflict", True, "a.bundle", conflict),
                "k.png": ("publish", None, False, "e.bundle", k_rows),
                "other.json": ("publish", None, False, "e.bundle", rows("first_input", "e", "e")),
                "shared.json": ("withheld", "unit_conflict", True, "a.bundle", conflict),
            },
        )

    def test_rule_three_and_a_flip(self):
        got = decisions(self.pick("abcdf"))
        self.assertEqual(got["shared.json"], unit("first_input"))
        self.assertEqual(got["j.png"], unit("first_input"))
        k = unit("first_input")
        self.assertEqual(got["k.png"], (*k[:4], k[4] + [("d.bundle", "unit", False)]))
        self.assertEqual(got["i.png"], ("publish", None, False, "f.bundle", rows("first_input", "f", "c", "f")))

    def test_a_key_only_losing_units_hold_is_withheld(self):
        got = decisions(self.pick("abcd"))
        self.assertEqual(got["i.png"], ("withheld", "unit_lost", False, "c.bundle", rows("first_input", "c", "c")))
        self.assertEqual(got["shared.json"], unit("first_input"))

    def test_rule_three_among_equal_md5s_is_no_flip(self):
        got = decisions(self.pick("ab"))
        for key in ("shared.json", "j.png", "k.png"):
            self.assertEqual(got[key], ("publish", None, False, "a.bundle", rows("first_input", "a", "a", "b")))

    def test_the_preferred_input_and_md5(self):
        c_json = next(o.data for o in self.by["c"].outputs if o.key == "shared.json")
        for rule, entry in (("prefer_input", {"input": "c.bundle", "md5": None}),
                            ("prefer_md5", {"input": None, "md5": nw.md5(c_json)})):  # fmt: skip
            with self.subTest(rule=rule):
                got = decisions(self.pick("abcdf"), prefer={"shared.json": entry})
                self.assertEqual(got["shared.json"], unit(rule, "c", flip=False))
                # `c`'s unit now holds `i.png`: reserved, so `f`'s plain candidate loses.
                self.assertEqual(
                    got["i.png"],
                    ("publish", None, False, "c.bundle", [("c.bundle", rule, True), ("f.bundle", "unit", False)]),
                )
                self.assertEqual(got["k.png"][3], "c.bundle")

    def test_an_entry_for_another_key_of_a_unit_is_not_consulted(self):
        plain = self.pick("abcdf")
        self.assertEqual(decisions(plain, prefer={"k.png": {"input": "d.bundle", "md5": None}}), decisions(plain))

    def test_a_withheld_unit_key_reserves_every_unit_s_keys(self):
        got = decisions(self.pick("abcdf"), prefer={"shared.json": {"input": "z.bundle", "md5": None}}, failed=Z)
        held = ("withheld", "withheld")
        self.assertEqual(got["shared.json"], unit("withheld", status=held))
        self.assertEqual(got["k.png"][:4], (*held, True, "a.bundle"))
        # `i.png`: reserved though the chosen `a` does not hold it; its row the first holder among the units, `c`.
        self.assertEqual(
            got["i.png"], (*held, False, "c.bundle", [("c.bundle", "withheld", True), ("f.bundle", "unit", False)])
        )
        self.assertEqual(got["h.png"], ("publish", None, False, "d.bundle", rows("first_input", "d", "d")))

    def test_a_withheld_plain_key(self):
        got = decisions(self.pick("abcdf"), prefer={"h.png": {"input": "z.bundle", "md5": None}}, failed=Z)
        self.assertEqual(got["h.png"], ("withheld", "withheld", False, "d.bundle", rows("withheld", "d", "d")))
        self.assertEqual(got["shared.json"][0], "publish")

    def test_a_withheld_unit_key_keeps_a_key_in_step_two(self):
        got = decisions(self.pick("abcde"), prefer={"other.json": {"input": "z.bundle", "md5": None}}, failed=Z)
        held = ("withheld", "withheld")
        self.assertEqual(got["other.json"], (*held, False, "e.bundle", rows("withheld", "e", "e")))
        conflict = [
            ("a.bundle", "unit_conflict", False),
            ("b.bundle", "first_input", False),
            ("c.bundle", "first_input", False),
            ("d.bundle", "unit", False),
            ("e.bundle", "withheld", True),
        ]
        self.assertEqual(got["k.png"], (*held, False, "e.bundle", conflict))
        self.assertEqual(got["shared.json"][:4], ("withheld", "unit_conflict", True, "a.bundle"))
        self.assertEqual(got["i.png"][:2], ("withheld", "unit_lost"))

    def test_publish_incomplete_false(self):
        got = decisions(self.pick("abcdf"), failed=Z, publish_incomplete=False)
        held = {k: ("withheld", "publish_incomplete") for k in ("h.png", "i.png", "j.png", "k.png", "shared.json")}
        self.assertEqual({k: d[:2] for k, d in got.items()}, held)

    def test_a_unit_keyed_by_its_sheet_s_texture(self):
        textured = self.pick("pqr")
        sheet = rows("first_input", "p", "p", "q") + [("r.bundle", "unit", False)]
        want = {
            "sheet.png": ("publish", None, True, "p.bundle", sheet),
            # The JSONs are alike, the crops differ: each member key's flip is its own candidates'.
            "at.json": ("publish", None, False, "p.bundle", rows("first_input", "p", "p", "q")),
            "at/s.png": ("publish", None, True, "p.bundle", rows("first_input", "p", "p", "q")),
        }
        self.assertEqual(decisions(textured), want)
        # An entry naming the plain candidate's input is not honoured in step 1.
        self.assertEqual(decisions(textured, prefer={"sheet.png": {"input": "r.bundle", "md5": None}}), want)

    def test_rule_two_on_a_png_s_md5(self):
        textured = self.pick("pqr")
        q_png = next(o.data for o in self.by["q"].outputs if o.key == "sheet.png")
        got = decisions(textured, prefer={"sheet.png": {"input": None, "md5": nw.md5(q_png)}})
        self.assertEqual(
            got["sheet.png"],
            ("publish", None, False, "q.bundle", rows("prefer_md5", "q", "p", "q") + [("r.bundle", "unit", False)]),
        )
        self.assertEqual(got["at/s.png"], ("publish", None, False, "q.bundle", rows("prefer_md5", "q", "p", "q")))


if __name__ == "__main__":
    unittest.main()
