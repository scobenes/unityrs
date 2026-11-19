"""Complete the optional root basis matrix fail-closed: each clip neutral but for its own basis flags, its
built start-at-origin flag on whatever its XZ flag (as real built clips store it), on a neutral Avatar."""

import unittest
from unittest import mock

import test_anim_replay_coverage as coverage
from anim_coverage_support import BASIS, BASIS_XZ, BASIS_Y, COVERAGE, NEUTRAL_BASIS, coverage_archives

import anim_fixture_coverage  # noqa: E402
import anim_replay as replay  # noqa: E402


class BasisMatrixTests(unittest.TestCase):
    # The coverage completion run, with the two basis clips recorded beside the probes.
    setUp = coverage.CoverageCompletionTests.setUp
    complete = coverage.CoverageCompletionTests.complete
    assert_refused = coverage.CoverageCompletionTests.assert_refused
    completed = coverage.CoverageCompletionTests.completed

    def build(self, **kwargs):
        self.statuses |= dict.fromkeys(BASIS, "recorded")
        self.coverage = coverage_archives(self.bundles, self.inspect, cases=COVERAGE + BASIS, **kwargs)

    def finish(self):
        return self.complete(written=COVERAGE + BASIS)

    def test_the_matrix_contract(self):
        cases = anim_fixture_coverage.BASIS_CASES
        self.assertEqual(len(cases), 8)
        self.assertEqual(cases["humanoid-root-basis-o1-y0-xz1"], (True, False, True))
        self.assertEqual({fixture.bundle for name, fixture in anim_fixture_coverage.COVERAGE_CASES.items()
                          if name in cases}, {"humanoid-root-basis"})
        # Optional on every version; only the neutral combination has no refusal label.
        self.assertFalse(set(cases) & anim_fixture_coverage.required_coverage("2022.3.62f3"))
        self.assertEqual([name for name in cases if name not in anim_fixture_coverage.REFUSALS], [NEUTRAL_BASIS])

    def test_completes_recorded_basis_clips_with_their_labels(self):
        self.build()
        result = self.finish()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Completed and validated 21 replay(s)", result.stdout)
        self.assertIn("humanoid-root-basis-o1-y1-xz1: not_covered", result.stdout)
        self.assertEqual(self.completed(BASIS_Y)["spec"]["evidence"]["expected_refusal"],
                         "unsupported_humanoid_root_basis")
        self.assertNotIn("expected_refusal", self.completed(NEUTRAL_BASIS)["spec"]["evidence"])
        self.assertEqual(self.completed(BASIS_XZ)["spec"]["evidence"]["expected_refusal"],
                         "unsupported_humanoid_root_basis")
        for name in BASIS:
            replay.validate(self.completed(name), self.bundles)

    def test_refuses_a_start_at_origin_flag_off(self):
        def edit(clip):
            clip["m_MuscleClip"]["m_StartAtOrigin"] = False
            return clip
        self.build(clips={NEUTRAL_BASIS: edit})
        self.assert_refused("built basis matrix clip flags differ from the authored combination",
                            written=COVERAGE + BASIS)

    def test_refuses_a_built_flag_off_the_authored_one(self):
        def edit(clip):
            clip["m_MuscleClip"]["m_KeepOriginalPositionY"] = False
            return clip
        self.build(clips={BASIS_Y: edit})
        self.assert_refused("basis matrix clip has another serialized clip setting", written=COVERAGE + BASIS)

    def test_refuses_a_non_neutral_built_setting(self):
        for field, value in (("m_Mirror", True), ("m_Level", .125), ("m_CycleOffset", .5), ("m_LoopBlend", True)):
            def edit(clip, field=field, value=value):
                clip["m_MuscleClip"][field] = value
                return clip
            with self.subTest(field):
                self.build(clips={NEUTRAL_BASIS: edit})
                # The serialized import row or, for a field with none, the built clip refuses it.
                self.assert_refused("basis matrix clip has", written=COVERAGE + BASIS)

    def test_a_run_without_matrix_rows_completes(self):
        # As a round-3 run: no matrix rows in run.json, no matrix bundle or recordings.
        older = tuple(name for name in coverage.DEFERRED if name not in anim_fixture_coverage.BASIS_CASES)
        with mock.patch.object(coverage, "DEFERRED", older):
            self.coverage = coverage_archives(self.bundles, self.inspect)
            result = self.complete()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("humanoid-root-basis", result.stdout)

    def test_refuses_another_captured_setting(self):
        self.build()
        self.coverage[BASIS_Y]["capture"]["clip_settings"]["mirror"] = True
        self.assert_refused("basis matrix clip has another clip setting", written=COVERAGE + BASIS)

    def test_refuses_a_non_neutral_avatar(self):
        def edit(case, value):
            if case in BASIS:
                value["m_Avatar"]["m_Human"]["data"]["m_LegStretch"] = .05
            return value
        self.build(avatar=edit)
        self.assert_refused("built basis matrix Avatar is not neutral", written=COVERAGE + BASIS)

    def test_refuses_a_wrong_refusal_label(self):
        self.build()
        self.coverage[NEUTRAL_BASIS]["spec"]["evidence"]["editor"]["expected_refusal"] = (
            "unsupported_humanoid_root_basis")
        self.assert_refused(f"fixture refusal label mismatch for {NEUTRAL_BASIS}", written=COVERAGE + BASIS)


if __name__ == "__main__":
    unittest.main()
