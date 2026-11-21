"""Round 12's witness rule (anim_twist_witness): the rows it may search, the allowlist it reads, and the rule itself
on synthetic rigs: a pair the rows cannot separate stays unwitnessed, and refuses exactly the cells whose keys
separate it."""

import unittest
from unittest import mock

import test_anim_replay_coverage  # noqa: F401  (puts tools/parity on the path)

import anim_twist_model as model  # noqa: E402
import anim_twist_tables as tables  # noqa: E402
import anim_twist_witness as witness  # noqa: E402
import scratch  # noqa: E402
from test_anim_twist_model import synthetic_rig  # noqa: E402


def rig_entry(seed, toes=True):
    rig = synthetic_rig(seed, toes=toes)
    signs = {side: [rig[s + k]["axes"]["sign"][0] * rig[r + k]["axes"]["sign"][0] for s, r in witness.PAIR_BONES]
             for side, k in (("Left", 0), ("Right", 1))}
    return dict(rig=rig, toes={"Left": toes, "Right": toes}, signs=signs, case=f"synthetic-{seed}")


class RowTests(unittest.TestCase):
    def test_only_exploration_and_selection(self):
        rows = witness.rows()
        groups = {}
        for case, index, rig, name, vector, group in rows:
            groups[group] = groups.get(group, 0) + 1
            self.assertNotEqual(tables.cases()[case][3], "confirmation")
        self.assertEqual(groups, {"exploration all-equal core": 339, "exploration single-field core": 452,
                                  "exploration full-set extras": 294, "exploration toe-absent": 29, "selection": 633})
        order = [row[5] for row in rows]
        self.assertEqual(order, sorted(order, key=["exploration all-equal core", "exploration single-field core",
                                                   "exploration full-set extras", "exploration toe-absent",
                                                   "selection"].index))

    def test_the_allowlist_reads_an_identity_root_clip(self):
        with scratch.folder() as folder:
            body = folder / "replays" / "body"
            body.mkdir(parents=True)
            for name in ("body-clip-tall-scoped-level-offset", "body-clip-tall-scoped-level",
                         "body-clip-tall-scoped-bake-o0-y0-xz1-offset", "body-probes-fingers"):
                (body / f"{name}.pending.json").write_text("{}", encoding="utf-8")
            self.assertEqual(witness.allowlisted(folder, "tall").name, "body-clip-tall-scoped-level.pending.json")
            with self.assertRaises(ValueError):
                witness.allowlisted(folder, "compact")

    def test_apart_needs_more_than_both_bounds(self):
        q = (0., 0., 0., 1.)
        turned = (2 ** -20, 0., 0., 1.)
        self.assertFalse(witness._apart((q, 0.), (q, 0.)))
        self.assertTrue(witness._apart((q, 0.), (turned, 0.)))
        self.assertFalse(witness._apart((q, 2 ** -19), (turned, 0.)))
        self.assertAlmostEqual(witness.margin((q, 0.), (turned, 0.)) + 2 * witness.bound.angle_bound(0.),
                               2 * 2 ** -20, delta=1e-12)


class RuleTests(unittest.TestCase):
    """Two limb candidates one equation at w = 0 (S1, S2) and a third apart (S3): rows at t = 0 alone witness S3
    against both, never S1 against S2, so a cell at t = .25 (where they differ) refuses and one at t = 0 does not."""

    LIMBS = (model.Limb(1, 1, 2, "+", 0), model.Limb(1, 2, 2, "+", 0), model.Limb(1, 3, 2, "+", 0))
    FEET = (model.Foot(1, 0), model.Foot(2, 1), model.Foot(3, 1))

    def run_rule(self, admission):
        rigs = {"compact": rig_entry(11), "tall": rig_entry(12), "no-toes": rig_entry(13, toes=False)}
        twist = {m: .5 for m in model.LIMB_TWISTS}
        plan = [("body-twist-compact-t0", 0, "compact", "t0", twist, "exploration all-equal core"),
                ("body-twist-compact-t0", 1, "compact", "t0", {41: -1., 39: .5, 23: .5, 27: .5},
                 "exploration all-equal core")]
        with mock.patch.multiple(model, LIMBS=self.LIMBS, FEET=self.FEET), \
                mock.patch.object(witness, "rows", lambda: plan), mock.patch.object(witness, "ADMISSION", admission):
            limb_found, foot_found, joint_found, joint_open = witness.search(rigs, log=lambda *_: None)
            return limb_found, foot_found, joint_open, witness.cells(rigs, joint_open)

    def test_a_missed_pair_refuses_only_where_its_keys_differ(self):
        limb_found, foot_found, joint_open, cells = self.run_rule((("t0", ("compact",)), ("t25", ("compact",))))
        self.assertEqual(sorted(limb_found), [(0, 2), (1, 2)])
        # F2 and F3 of one sign are one with toes: no Toes row separates them, the Foot alone cannot either.
        self.assertEqual(sorted(foot_found), [(0, 1), (0, 2)])
        self.assertIn(((0, 1), (0, 0)), joint_open)
        self.assertIn(((0, 0), (1, 2)), joint_open)
        by_tuple = {cell["tuple"]: cell for cell in cells}
        self.assertFalse(by_tuple["t0"]["refused"])
        self.assertTrue(by_tuple["t25"]["refused"])
        self.assertGreater(by_tuple["t25"]["unwitnessed"], 0)

    def test_witness_rows_are_named(self):
        limb_found, _, _, _ = self.run_rule((("t0", ("compact",)),))
        for case, index, group, found in limb_found.values():
            self.assertEqual((case, group), ("body-twist-compact-t0", "exploration all-equal core"))
            self.assertIn(index, (0, 1))
            self.assertGreater(found, 0)


if __name__ == "__main__":
    unittest.main()
