"""Round 11's E5 body fixtures: the clip rigs and the new F5 chain rigs mirror the builder, stay out of B2 and
the hold-out, record only the variants B4 admits, and are required as a generation once a run has them."""

import json
import re
import unittest
from pathlib import Path

import test_anim_replay_coverage as coverage  # noqa: F401  (puts tools/parity on the path)
from anim_body_support import body_doc
from anim_fixture_support import RUN
import test_anim_body_fixtures as fixtures
from test_anim_body_fixtures import EARLIER, View, source

import anim_body_runs as runs  # noqa: E402
import anim_body_seal as seal  # noqa: E402
import anim_fixture_body as body  # noqa: E402
import anim_fixture_rigs as rigs_module  # noqa: E402
import anim_replay as replay  # noqa: E402

FREEZE = Path(__file__).resolve().parents[1] / "anim_body_freeze_v8.json"


class E5InventoryTests(unittest.TestCase):
    def test_the_clip_rigs_and_variants_mirror_the_builder(self):
        clips = source("UnityrsBodyClips.cs")
        admitted = re.search(r"E5Admitted = \{(.*?)\};", clips, re.S).group(1)
        self.assertEqual(tuple(re.findall(r'"([a-z0-9-]+)"', admitted)), body.E5_ADMITTED)
        self.assertIn('SmallRig = new BodyRig { Name = "small", Size = .82f };', clips)
        self.assertEqual(body.RIG_SIZES["small"], .82)
        self.assertIn('static readonly string[] SealedRigs = { "holdout", HoldoutRig };', clips)
        self.assertIn('ChainRigs.Single(rig => rig.Name == "large"), ProbeRigs.Single(rig => rig.Name == "tall"),\n'
                      '        SmallRig }.Concat(ProbeRigs.Where(rig => rig.Name != "compact" && rig.Name != "tall" '
                      '&& !SealedRigs.Contains(rig.Name)))', clips)
        self.assertEqual(body.CLIP_RIGS[:3], ("large", "tall", "small"))
        self.assertEqual(len(body.CLIP_RIGS), 19)
        self.assertTrue(set(body.CLIP_RIGS).isdisjoint(body.HOLDOUT_RIGS))
        # The F5 chain rigs: each feature and size, entry for entry.
        builder = source("UnityrsBodyFixtures.cs")
        table = builder[builder.index("BodyRig[] ChainRigs = {"):]
        rows = re.findall(r'new BodyRig \{ Name = "([a-z-]+)"([^}]*)\}', table[:table.index("};")])
        features = {"ScaledAnimator = true": "scaled-animator", "Armature = true": "armature",
                    "HipsScale = 1.2f": "hips-scale"}
        parsed = {}
        for name, fields in rows:
            size = re.search(r"Size = ([\d.]+)f", fields)
            parsed[name] = (next((v for k, v in features.items() if k in fields), ""), float(size.group(1)) if size else 1)
        self.assertEqual(parsed, body.CHAIN_RIGS)
        self.assertEqual(body.E5_CHAIN_RIGS, ("scaled-animator-large", "scaled-animator-small", "hips-scale-large",
                                              "hips-scale-small", "armature-large"))
        # Size scales the hips' offset in an armature, exactly so at Size 1, never the armature's own transform,
        # which B4 admits only at F5's value.
        self.assertIn('Child(root, "Armature", new Vector3(0, .05f, .1f));', builder)
        self.assertIn("hips.localPosition = new Vector3(0, .125f, .8375f) * rig.Size;", builder)
        self.assertIn(".Concat(ClipRigs.SelectMany(rig => ClipCases(rig)));", builder)
        self.assertIn("() => BuildBodyClips(build, report, rig));", builder)
        self.assertEqual(rigs_module.RIG_SIZES, body.RIG_SIZES)

    def test_the_admitted_variants_are_freeze_v8s_e4_passing_set(self):
        passing = json.loads(FREEZE.read_text(encoding="utf-8"))["e4"]["passing"]
        compact = {entry.split("/")[0] for entry in passing if entry.split("/")[1] == "compact"}
        self.assertEqual(set(body.E5_ADMITTED), {variant.removesuffix("-start") for variant in compact})
        self.assertEqual(len(body.E5_VARIANTS), 21)
        self.assertEqual({v for v in body.E5_VARIANTS if not v.endswith("-start")}, set(body.E5_ADMITTED))

    def test_the_cases_are_round_11s(self):
        # 19 clip rigs x 21 variants x 2 roots, and each new chain rig's targets and clip.
        self.assertEqual(len(body.E5_CASES), 19 * 21 * 2 + 5 * 2)
        kinds = [body.BODY_INFO[name]["kind"] for name in body.E5_CASES]
        self.assertEqual((kinds.count("clip"), kinds.count("targets"), kinds.count("probes")), (803, 5, 0))
        self.assertEqual(len(body.E5_BUNDLES), 24)
        for rig in body.CLIP_RIGS:
            for variant in body.E5_VARIANTS:
                for root, suffix in (("identity", ""), ("offset", "-offset")):
                    name = f"body-clip-{rig}-{variant}{suffix}"
                    fixture, info = body.BODY_CASES[name], body.BODY_INFO[name]
                    self.assertEqual((fixture.bundle, fixture.prefab, fixture.clip, fixture.playback),
                                     (f"body-clips-{rig}", root, f"body-clip-{rig}-{variant}", "animator"))
                    self.assertEqual(info, dict(kind="clip", rig=rig, variant=variant, root=root))
        for rig in body.E5_CHAIN_RIGS:
            self.assertEqual(body.BODY_INFO[f"body-chain-{rig}-clip"]["variant"], body.CHAIN_VARIANT)
            probe = body.BODY_CASES[f"body-chain-{rig}-targets"].probe[0]
            self.assertEqual(probe, "HumanoidRig/Armature/Hips/Spine" if rig.startswith("armature")
                             else body.SPINE_PROBE)
        # No E5 case names a hold-out rig, so the sealed cases are the hold-out's alone, as before.
        self.assertFalse({body.BODY_INFO[name]["rig"] for name in body.E5_CASES} & set(body.HOLDOUT_RIGS))
        self.assertEqual(seal.HOLDOUT_CASES, ("body-probes-holdout-2",))
        # The older generations are unchanged.
        self.assertFalse(body.E5_CASES & set().union(*body.LATER_CASES))
        self.assertFalse({name for name in body.E5_CASES if not name.startswith(("body-clip-", "body-chain-"))})

    def test_b2_never_reads_them(self):
        self.assertTrue(all(runs.role_of(name) == "e5" for name in body.E5_CASES))
        self.assertFalse(runs.REQUIRED & body.E5_CASES)
        self.assertEqual(runs.role_of("body-chain-large-clip"), "training")
        self.assertEqual(runs.role_of("body-clip-level"), "training")
        self.assertEqual(runs.role_of("body-probes-holdout-2"), "holdout")


class E5CheckTests(unittest.TestCase):
    def check(self, name, doc):
        return body.check_body(name, body.BODY_CASES[name], doc, View(None), None, replay)

    def test_accepts_each_new_chain_rig_and_a_clip_rig(self):
        for rig in body.E5_CHAIN_RIGS:
            with self.subTest(rig=rig):
                self.assertIsNone(self.check(f"body-chain-{rig}-targets", body_doc(f"body-chain-{rig}-targets", RUN)))

    def test_refuses_a_chain_rig_at_another_size(self):
        for rig in ("hips-scale-small", "armature-large", "scaled-animator-large", "large"):
            name = f"body-chain-{rig}-targets"
            doc = body_doc(name, RUN)
            transforms = doc["capture"]["rest"]["transforms"]
            hips = next(i for i, node in enumerate(doc["spec"]["nodes"]) if node["name"] == "Hips")
            transforms[hips]["translation"] = [replay.f32_bits(.9 * replay.f32_from_bits(v))
                                               for v in transforms[hips]["translation"]]
            with self.subTest(rig=rig), self.assertRaisesRegex(ValueError, "body rig chain differs"):
                self.check(name, doc)

    def test_refuses_a_human_scale_outside_b4s_range(self):
        name = "body-chain-armature-large-targets"
        low, high = body.H_RANGE
        self.assertEqual([replay.f32_bits(v) for v in body.H_RANGE], ["3f47f9bb", "3fb7a311"])
        for scale, ok in ((low, True), (high, True), (1.2685639, True),
                          (replay.f32_from_bits("3f47f9ba"), False), (replay.f32_from_bits("3fb7a312"), False)):
            doc = body_doc(name, RUN)
            doc["capture"]["humanoid"]["human_scale"] = replay.f32_bits(scale)
            with self.subTest(scale=scale):
                if ok:
                    self.assertIsNone(self.check(name, doc))
                else:
                    with self.assertRaisesRegex(ValueError, "outside B4's range"):
                        self.check(name, doc)
        # An older case keeps its own human scale, whatever it is.
        doc = body_doc("body-chain-large-targets", RUN)
        doc["capture"]["humanoid"]["human_scale"] = replay.f32_bits(2)
        self.assertIsNone(self.check("body-chain-large-targets", doc))


class E5RunTests(unittest.TestCase):
    setUp = fixtures.BodyRunTests.setUp
    complete = fixtures.BodyRunTests.complete
    run_with = fixtures.BodyRunTests.run_with

    def test_a_run_before_e5_completes_and_one_with_it_requires_every_case(self):
        result = self.run_with(EARLIER)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn("E5 cases", result.stderr)
        # With E5's generation, a case Unity did not cover is an error, each named.
        result = self.run_with([*EARLIER, *sorted(body.E5_CASES)])
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn(f"E5 cases not recorded ({len(body.E5_CASES)}): body-chain-armature-large-clip (not_covered)",
                      result.stderr)
        # One missing from run.json is from no generation at all.
        missing = "body-clip-small-level-start-offset"
        result = self.run_with([*EARLIER, *sorted(body.E5_CASES - {missing})])
        self.assertIn(f"missing run.json case: {missing}", result.stderr)

    def test_a_recorded_e5_case_needs_its_recording(self):
        name = "body-clip-tall-orientation-offset-offset"
        result = self.run_with([*EARLIER, *sorted(body.E5_CASES)], dict.fromkeys(body.E5_CASES, "recorded"))
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn(f"{name}: missing replays/body/{name}.pending.json", result.stderr)
        self.assertNotIn("E5 cases not recorded", result.stderr)


if __name__ == "__main__":
    unittest.main()
