"""B5a's round 12 verdict (anim_twist_b5a): its twist-free rows, its 25 refusal cases with the runtime's exact
reasons (humanoid_refusal's precedence), and the exact verdict."""

import collections
import unittest

import test_anim_replay_coverage  # noqa: F401  (puts tools/parity on the path)

import anim_fixture_round11 as r11  # noqa: E402
import anim_fixture_round12 as round12  # noqa: E402
import anim_muscle_confirm as confirm  # noqa: E402
import anim_replay as replay  # noqa: E402
import anim_twist_b5a as b5a  # noqa: E402

VERSIONS = ("2022", "6000")
TWIN, TWIST_CLIP = "body-clip-compact-scoped-twin-level", "body-clip-compact-scoped-twist-level"


def observation(rows_, refused, editors=VERSIONS):
    seen = confirm.Observation()
    seen.runs = {editor: "r" for editor in editors}
    seen.rows = {key: (0., 1.) for key in rows_}
    seen.refused = {key: (0., 1., reason) for key, reason in refused.items()}
    return seen


class RefusalTests(unittest.TestCase):
    def test_the_pinned_reasons_follow_the_runtimes_precedence(self):
        pinned = b5a.pinned_refusals()
        self.assertEqual(collections.Counter(pinned.values()), {
            "unsupported_animator_root": 12, "unsupported_body_motion(body_slots)": 9, "unsupported_locomotion": 4})
        self.assertTrue(all(r11.ROUND11_INFO[case]["scope"] == "refusal" for case in pinned))
        # The review's example: an offset root is refused before any clip check, the limb twist never reached.
        self.assertEqual(pinned["body-clip-compact-full-level-offset"], "unsupported_animator_root")
        self.assertEqual(pinned["body-clip-compact-full-keep-o1-y1-xz1-motion"], "unsupported_locomotion")
        self.assertEqual(pinned["body-clip-compact-full-level"], "unsupported_body_motion(body_slots)")
        self.assertNotIn("unsupported_humanoid_limb_twist(23)", pinned.values())


class B5aTests(unittest.TestCase):
    def test_the_twist_free_rows(self):
        free = b5a.twist_free()
        doc = round12.tables()[0]
        families = {row["name"]: row["family"] for row in doc["cases"]}
        self.assertEqual({families.get(case, "round 11") for case in free}, {"sweep", "twin", "round 11"})
        self.assertEqual(sum(families.get(case) == "twin" for case in free), 318)
        self.assertEqual(sorted(case for case in free if case not in families), ["body-probes-face",
                                                                                "body-probes-fingers"])
        self.assertEqual(free[TWIN], tuple(round12.rows()[TWIN]["times"]))
        # A sweep keeps exactly its vectors whose ten limb-twist values are 0.
        sets = {row["name"]: row["vectors"] for row in doc["sets"]}
        twists = {r["name"] for r in doc["names"] if r["ordinal"] in b5a.LIMB_TWIST_ORDINALS}
        self.assertEqual(len(twists), 10)
        vectors = sets["sweep-compact"]
        kept = [k for k, v in enumerate(vectors) if all(replay.f32_from_bits(b) == 0
                                                        for m, b in zip(v["muscles"], v["bits"]) if m in twists)]
        self.assertEqual(free["body-twist-sweep-compact"], tuple(kept))
        self.assertLess(len(kept), len(vectors))

    def test_the_verdict_is_exact(self):
        rows_ = {(e, TWIN, 7, sid) for e in VERSIONS for sid in round12.rows()[TWIN]["times"][:2]}
        case = "body-clip-compact-full-level-offset"
        refused = {(e, case, 7, "00000000") for e in VERSIONS}
        reason = "unsupported_animator_root"
        expected = (rows_, refused)
        self.assertEqual(b5a.b5a_failures(observation(rows_, dict.fromkeys(refused, reason)), expected), [])
        some = sorted(rows_)[1:]
        for seen, pattern in (
                (observation(some, dict.fromkeys(refused, reason)), "lack 1 row"),
                (observation(rows_ | {("2022", TWIN, 8, "00000000")}, dict.fromkeys(refused, reason)), "beyond"),
                (observation(rows_ | {("2022", TWIST_CLIP, 7, "00000000")}, dict.fromkeys(refused, reason)),
                 "twist-active"),
                (observation(rows_, dict.fromkeys(refused, "unsupported_humanoid_limb_twist(23)")),
                 "refuses as unsupported_humanoid_limb_twist(23), not unsupported_animator_root"),
                (observation(rows_, dict.fromkeys(sorted(refused)[:1], reason)), "lack refusal 1"),
                (observation(rows_, dict.fromkeys(refused, reason), editors=("2022",)), "no B5a runs of Editor 6000")):
            with self.subTest(pattern=pattern):
                self.assertTrue(any(pattern in line for line in b5a.b5a_failures(seen, expected)),
                                b5a.b5a_failures(seen, expected))
        outside = observation(rows_, dict.fromkeys(refused, reason))
        outside.rows[sorted(rows_)[0]] = (2., 1.)
        self.assertEqual(b5a.b5a_failures(outside, expected), [
            f"1 B5a row(s) outside their bound: {[sorted(rows_)[0]]}"])

    def test_the_expected_rows_never_hold_a_twist_active_case(self):
        rows_, refused = b5a.b5a_expected()
        families = {row["name"]: row["family"] for row in round12.tables()[0]["cases"]}
        self.assertFalse({key[1] for key in rows_ | refused if families.get(key[1]) in round12.TWIST_ONLY})
        self.assertEqual({key[1] for key in refused}, set(b5a.pinned_refusals()))
        self.assertEqual({key[0] for key in rows_ | refused}, set(VERSIONS))


if __name__ == "__main__":
    unittest.main()
