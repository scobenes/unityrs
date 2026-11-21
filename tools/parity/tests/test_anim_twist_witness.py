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
    return dict(rig=rig, toes={"Left": toes, "Right": toes}, signs=signs, reach=model.x_reach(rig),
                case=f"synthetic-{seed}")


class RowTests(unittest.TestCase):
    def test_only_exploration_and_selection(self):
        rows = witness.rows()
        groups = {}
        for case, sample, rig, name, vector, group in rows:
            groups[group] = groups.get(group, 0) + 1
            self.assertNotEqual(tables.cases()[case][3], "confirmation")
            # Capture sample indices: the anchors at 0, .5 and 1 are never a vector's.
            count = len(tables.SETS[tables.cases()[case][2]]())
            self.assertNotIn(sample, (0, (count + 1) // 2 + 1, count + 2))
        self.assertEqual(groups, {"exploration all-equal core": 339, "exploration single-field core": 452,
                                  "exploration full-set extras": 294, "exploration toe-absent": 29, "selection": 633})
        order = [row[5] for row in rows]
        self.assertEqual(order, sorted(order, key=["exploration all-equal core", "exploration single-field core",
                                                   "exploration full-set extras", "exploration toe-absent",
                                                   "selection"].index))

    def test_the_allowlist_names_one_identity_root_clip_per_rig(self):
        import anim_fixture_round11 as r11

        cases = {rig: witness.allowlisted(rig) for rig in witness.SCOPED_RIGS}
        self.assertEqual(len(set(cases.values())), 27)
        for rig, case in cases.items():
            self.assertTrue(case.startswith(f"body-clip-{rig}-scoped-") and not case.endswith("-offset"), case)
            self.assertEqual(r11.ROUND11_INFO[case]["root"], "identity")
        for rig in ("fingers", "holdout", "compact-scoped", "eyes-jaw"):
            with self.assertRaises(ValueError):
                witness.allowlisted(rig)

    def test_a_rest_must_be_a_unit_quaternion(self):
        for q in ((0., 0., 0., 1.), (0., 0., 0., 1. + 2. ** -22), (.5, .5, .5, .5)):
            witness.unit_quaternion(q, "rest")
        for q in ((0., 0., 0., 2. ** -100), (0., 0., 0., 2. ** -80), (0., 0., 0., 2.), (0., 0., 0., 1e30), (0., 0., float("nan"), 1.), (0., 1.)):
            with self.assertRaises(ValueError, msg=q):
                witness.unit_quaternion(q, "rest")

    def test_pre_and_post_rotations_must_be_unit_quaternions(self):
        unit, tiny = dict(x=0., y=0., z=0., w=1.), dict(x=0., y=0., z=0., w=2. ** -80)
        limit = dict(m_Min=dict(x=-1., y=-1., z=-1.), m_Max=dict(x=1., y=1., z=1.))
        sign = dict(x=1., y=1., z=1., w=1.)
        axes = witness.axes_of(dict(m_PreQ=unit, m_PostQ=unit, m_Sgn=sign, m_Limit=limit), "bone")
        self.assertEqual((axes["pre"], axes["max"]), ([0., 0., 0., 1.], [1., 1., 1.]))
        for pre, post in ((tiny, unit), (unit, tiny)):
            with self.assertRaises(ValueError, msg=(pre, post)):
                witness.axes_of(dict(m_PreQ=pre, m_PostQ=post, m_Sgn=sign, m_Limit=limit), "bone")

    def test_run_files_refuse_symlinks_and_escapes(self):
        with scratch.folder() as root:
            run, outside = root / "run", root / "outside"
            (run / "replays" / "body").mkdir(parents=True)
            outside.mkdir()
            (outside / "sealed.pending.json").write_text("{}", encoding="utf-8")
            (run / "replays" / "body" / "real.pending.json").write_text("{}", encoding="utf-8")
            (run / "replays" / "body" / "link.pending.json").symlink_to(outside / "sealed.pending.json")
            (run / "replays" / "inner.pending.json").write_text("{}", encoding="utf-8")
            (run / "replays" / "body" / "near.pending.json").symlink_to(run / "replays" / "inner.pending.json")
            (run / "bundles").symlink_to(outside, target_is_directory=True)
            self.assertEqual(witness.run_file(run, "replays", "body", "real.pending.json").name, "real.pending.json")
            for parts in (("replays", "body", "link.pending.json"), ("replays", "body", "near.pending.json"),
                          ("bundles", "sealed.pending.json"), ("replays", "..", "outside"),
                          ("replays", "body/real.pending.json"), ("replays", "body", "missing.pending.json")):
                with self.assertRaises(ValueError, msg=parts):
                    witness.run_file(run, *parts)
            # run.json too, and the run folder itself.
            (run / "run.json").symlink_to(outside / "sealed.pending.json")
            with self.assertRaises(ValueError):
                witness.run_file(run, "run.json")
            (root / "linked").symlink_to(run, target_is_directory=True)
            with self.assertRaises(ValueError):
                witness.run_file(root / "linked", "replays", "body", "real.pending.json")
            # A symlinked ancestor of a plain run folder: /parent/alias/run.
            (root / "alias").symlink_to(root, target_is_directory=True)
            with self.assertRaises(ValueError):
                witness.run_file(root / "alias" / "run", "replays", "body", "real.pending.json")
            with self.assertRaises(ValueError):
                witness.run(run, "unityrs", log=lambda *_: None)

    def test_apart_needs_more_than_both_bounds(self):
        q = (0., 0., 0., 1.)
        turned = (2 ** -20, 0., 0., 1.)
        self.assertFalse(witness._apart((q, 0.), (q, 0.)))
        self.assertTrue(witness._apart((q, 0.), (turned, 0.)))
        self.assertFalse(witness._apart((q, 2 ** -19), (turned, 0.)))
        self.assertAlmostEqual(witness.margin((q, 0.), (turned, 0.)) + 2 * witness.bound.angle_bound(0.),
                               2 * 2 ** -20, delta=1e-12)
        # Near the identity the separation is atan2's, never acos's: 2^-40 rad apart is apart.
        tiny = (2 ** -41, 0., 0., 1.)
        self.assertTrue(witness._apart((q, 0.), (tiny, 0.)))
        self.assertLess(witness.margin((q, 0.), (tiny, 0.)), 2 ** -40)


class RuleTests(unittest.TestCase):
    """Two limb candidates one equation at w = 0 (S1, S2) and a third apart (S3): rows at t = 0 alone witness S3
    against both, never S1 against S2, so a cell at t = .25 (where they differ) refuses and one at t = 0 does not."""

    LIMBS = (model.Limb(1, 1, 2, "+", 0), model.Limb(1, 2, 2, "+", 0), model.Limb(1, 3, 2, "+", 0))
    FEET = (model.Foot(1, 0), model.Foot(2, 1), model.Foot(3, 1))

    def run_rule(self, admission):
        rigs = {"compact": rig_entry(11), "tall": rig_entry(12), "no-toes": rig_entry(13, toes=False)}
        twist = {m: .5 for m in model.LIMB_TWISTS}
        plan = [("body-twist-compact-t0", 1, "compact", "t0", twist, "exploration all-equal core"),
                ("body-twist-compact-t0", 2, "compact", "t0", {41: -1., 39: .5, 23: .5, 27: .5},
                 "exploration all-equal core")]
        with mock.patch.multiple(model, LIMBS=self.LIMBS, FEET=self.FEET), \
                mock.patch.object(witness, "rows", lambda: plan), mock.patch.object(witness, "ADMISSION", admission):
            limb_found, foot_found, joint_found, joint_open, credits = witness.search(rigs, log=lambda *_: None)
            # Every joint pair is credited (with its component's row), searched, or open: each exactly once.
            searched = set(joint_found) | joint_open
            assert not set(credits) & searched and len(credits) + len(searched) == 9 * 8 // 2, credits
            for (l1, l2), (f1, f2) in credits:
                kind, component = credits[((l1, l2), (f1, f2))]
                assert component in (limb_found if kind == "limb" else foot_found)
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
        for case, sample, side, group, found in limb_found.values():
            self.assertEqual((case, group), ("body-twist-compact-t0", "exploration all-equal core"))
            self.assertIn(sample, (1, 2))
            self.assertIn(side, ("Left", "Right"))
            self.assertGreater(found, 0)

    def test_a_component_witness_needs_both_joint_candidates_to_predict(self):
        # A pole on a row refuses that candidate's whole side: a limb witness found there with F1 cannot credit a
        # joint pair whose other foot candidate refuses on it, so that pair is searched on every chain bone.
        real = witness.predict

        def refusing(rig, vector, settings, limb, foot, side):
            return None if foot == self.FEET[2] else real(rig, vector, settings, limb, foot, side)
        with mock.patch.object(witness, "predict", refusing):
            limb_found, _, joint_open, _ = self.run_rule((("t0", ("compact",)),))
        self.assertIn((0, 2), limb_found)
        self.assertIn(((0, 2), (2, 2)), joint_open)
        self.assertNotIn(((0, 2), (0, 0)), joint_open)


if __name__ == "__main__":
    unittest.main()
