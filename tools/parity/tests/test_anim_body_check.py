"""B2's checks E0-E4 on synthetic recordings that follow the f64 model, and on one value perturbed."""

import copy
import json
import math
import unittest
from unittest import mock

import anim_body_support as support
from anim_body_model_support import BITS, CLIPS, F32, avatar, coefficients, recording, write_run

import anim_body_adequacy as adequacy  # noqa: E402  (test_anim_replay puts it on the path)
import anim_body_check as check  # noqa: E402
import anim_body_e4 as e4  # noqa: E402
import anim_body_freeze as freeze  # noqa: E402
import anim_body_model as model  # noqa: E402
import anim_body_runs as runs  # noqa: E402
import anim_body_tolerance as tolerance  # noqa: E402
import anim_fixture_body as body  # noqa: E402
import scratch  # noqa: E402
from anim_reference import euler, multiply  # noqa: E402

CASES = ("body-probes-compact", "body-probes-no-upper-chest", "body-targets-tall", "body-chain-armature-targets",
         "body-chain-hips-scale-targets", "body-chain-scaled-animator-targets", "body-clip-keep-o0-y0-xz0",
         "body-clip-keep-o0-y0-xz0-start", "body-clip-bake-o1-y1-xz1-offset", "body-clip-level",
         "body-clip-keep-o1-y1-xz1-motion", "body-clip-keep-o1-y0-xz0", "body-clip-height-from-feet",
         "body-chain-armature-clip")


def hips_index(doc):
    return next(row["node"] for row in doc["capture"]["humanoid"]["bones"] if row["name"] == "Hips")


def avatars(run, doc):
    return avatar(doc)


def clips(run, doc):
    return CLIPS[doc["spec"]["evidence"]["editor"]["fixture_case"]]


def config(report, variant, root="identity", rig="compact"):
    """E4's entry for one configuration of the synthetic run."""
    return report["configurations"][f"{variant}/{rig}/{write_run.__defaults__[0]}/{root}"]


def checked(mode, folder, docs, avatar_of=avatars, clip_of=clips):
    """run_mode on a run of `docs`, whose training cases are the required ones."""
    write_run(folder, docs)
    required = frozenset(name for name in docs if runs.role_of(name) == "training")
    with mock.patch.object(runs, "REQUIRED", required):
        return freeze.run_mode(mode, [folder], avatar_of=avatar_of, clip_of=clip_of)


class Checks(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.docs = {name: recording(name, samples=12) for name in CASES}

    def setUp(self):
        # One Editor family per run here; test_e4_needs_a_comparison_in_each_version... checks the code's two.
        family = mock.patch.object(e4, "EDITORS", (write_run.__defaults__[0].split(".")[0],))
        family.start()
        self.addCleanup(family.stop)

    def run_mode(self, mode, docs=None, avatar_of=avatars, clip_of=clips):
        return checked(mode, scratch.for_case(self), docs if docs is not None else self.docs, avatar_of, clip_of)

    def edited(self, name, edit):
        docs = copy.deepcopy(self.docs)
        edit(docs[name])
        return docs

    def test_every_check_passes_on_the_model(self):
        for mode in ("e0", "e2", "e3a", "e4"):
            with self.subTest(mode=mode):
                report = self.run_mode(mode)
                self.assertEqual(report["failures"], [])
                self.assertTrue(report["passed"])
        report = self.run_mode("e4")
        self.assertEqual(config(report, "keep-o0-y0-xz0"), dict(status="pass", samples=12, no_readback=0, topology=0,
                                                                start_offset="pass"))
        self.assertEqual(config(report, "keep-o1-y1-xz1-motion")["status"], "pass")
        self.assertEqual(config(report, "keep-o1-y0-xz0")["status"], "refused")
        self.assertEqual(config(report, "height-from-feet")["status"], "undecided")
        self.assertEqual(report["start_pairs"], 6)
        # Per configuration (variant, rig, Editor version, root placement), naming exactly the passing ones: the
        # F5 chain its own, apart from the compact clip of its variant.
        version = write_run.__defaults__[0]
        self.assertEqual(report["passing"], [f"{variant}/{rig}/{version}/{root}" for variant, rig, root in (
            ("bake-o1-y1-xz1", "compact", "offset"), ("keep-o0-y0-xz0", "compact", "identity"),
            ("keep-o1-y1-xz1-motion", "armature", "identity"), ("keep-o1-y1-xz1-motion", "compact", "identity"),
            ("level", "compact", "identity"))])
        self.assertEqual(report["excluded"], [])

    def test_e4_excludes_a_configuration_without_read_back(self):
        def unread(doc):
            del doc["spec"]["evidence"]["editor"]["body_samples"]
        report = self.run_mode("e4", self.edited("body-clip-level", unread))
        entry = config(report, "level")
        self.assertEqual((entry["status"], entry["samples"]), ("excluded", 0))
        self.assertGreater(entry["no_readback"], 0)
        self.assertEqual(report["excluded"], [f"level/compact/{write_run.__defaults__[0]}/identity"])
        self.assertNotIn(report["excluded"][0], report["passing"])
        # A chain without a comparison is excluded, never carried by its variant's passing compact clip.
        report = self.run_mode("e4", self.edited("body-chain-armature-clip", unread))
        self.assertEqual(config(report, "keep-o1-y1-xz1-motion", rig="armature")["status"], "excluded")
        self.assertEqual(config(report, "keep-o1-y1-xz1-motion")["status"], "pass")

    def test_e0_refuses_or_fails_a_wrong_avatar(self):
        def edited(change):
            def read(run, doc):
                value = avatar(doc)
                change(check.human_of(value))
                return value
            return read

        def mass(human, slot, value):
            masses = [F32(human["m_HumanBoneMass"]["v"][i:i + 8]) for i in range(0, 200, 8)]
            masses[slot] = value
            human["m_HumanBoneMass"]["v"] = "".join(BITS(v) for v in masses)

        report = self.run_mode("e0", avatar_of=edited(lambda human: mass(human, 7, F32(BITS(.051)))))
        e0b = [line for line in report["failures"] if "E0b" in line]
        self.assertTrue(e0b and all("E0b Spine" in line for line in e0b))
        # An absent slot must hold no mass; a leaf's then refuses its Avatar too (the fit keeps the others).
        def heavy_eye(run, doc):
            value = avatar(doc)
            if doc["spec"]["evidence"]["editor"]["fixture_case"] == "body-probes-compact":
                mass(check.human_of(value), 22, 1e-3)
            return value
        report = self.run_mode("e0", avatar_of=heavy_eye)
        self.assertTrue(any("E0b LeftEye" in line for line in report["failures"]))
        # E0 alone on an Avatar the fit did not read: a mapping the capture lacks refuses the fit's rows.
        folder = write_run(scratch.for_case(self), self.docs)
        with mock.patch.object(runs, "REQUIRED", frozenset(self.docs)):
            loaded = runs.load([folder], "training")
        design = runs.Design(runs.observe(loaded, avatar_of=avatars))
        coefficients = runs.solve(design)

        def e0(change):
            return check.e0(loaded, edited(change), design, coefficients)

        report = e0(lambda human: human["m_HumanBoneIndex"].__setitem__(22, 3))
        self.assertTrue(any("E0a LeftEye on" in line for line in report["failures"]))

        def swap(human):
            index = human["m_HumanBoneIndex"]
            index[14], index[15] = index[15], index[14]

        # Two slots swapped keep the mapped set; their paths do not match the capture.
        report = e0(swap)
        self.assertTrue(any("E0a LeftUpperArm on" in line for line in report["failures"]))
        self.assertTrue(any("E0a RightUpperArm on" in line for line in report["failures"]))
        # Two slots on one node.
        report = e0(lambda human: human["m_HumanBoneIndex"].__setitem__(15, human["m_HumanBoneIndex"][14]))
        self.assertTrue(any("two slots share one node in the Avatar" in line for line in report["failures"]))

        def moved(human):
            human["m_SkeletonPose"]["data"]["m_X"][3]["t"]["x"] += 1e-3

        report = e0(moved)
        self.assertTrue(any("E0d" in line and "off m_SkeletonPose" in line for line in report["failures"]))
        report = self.run_mode("e0", avatar_of=edited(
            lambda human: human["m_RootX"]["t"].update(y=human["m_RootX"]["t"]["y"] + 1e-4)))
        self.assertTrue(any("E0c m_RootX.t" in line for line in report["failures"]))
        report = self.run_mode("e0", avatar_of=edited(lambda human: human.update(m_Scale=human["m_Scale"] * 1.001)))
        self.assertTrue(any("E0c m_Scale" in line for line in report["failures"]))
        for malformed in ({"t": "f32s", "v": "00000000"}, {"t": "f32s", "v": "7f800000" * 25}):
            with self.assertRaisesRegex(ValueError, "m_HumanBoneMass"):
                self.run_mode("e0", avatar_of=edited(
                    lambda human, value=malformed: human.update(m_HumanBoneMass=value)))
        with self.assertRaisesRegex(ValueError, "give --unityrs"):
            self.run_mode("e0", avatar_of=None)

    def test_a_twist_sweep_is_evaluated_within_the_scope_and_never_fitted(self):
        name = "body-probes-twist-compact"
        docs = dict(self.docs, **{name: recording(name)})
        before, after = self.run_mode("e2"), self.run_mode("e2", docs)
        self.assertTrue(after["passed"], after["failures"])
        # Its twists up to the scope's limit compare, with the neutral and single-axis anchors; the larger ones
        # refuse, as does the noncommuting anchor's spine twist.
        within = sum(abs(d) <= 4 for d in body.TWIST_DEGREES) * len(body.TWIST_TILTS) + 2
        total = len(body.TWIST_DEGREES) * len(body.TWIST_TILTS) + 3
        self.assertEqual(after["refused"] - before["refused"], total - within)
        self.assertGreater(after["residuals"], before["residuals"])
        folder = scratch.for_case(self)
        write_run(folder, docs)
        with mock.patch.object(runs, "REQUIRED", frozenset()):
            rows = runs.observe(runs.load([folder], "training"), avatar_of=avatars)
        self.assertEqual({row["fit"] for row in rows if name in row["name"]}, {False})

    def test_e2_fails_a_turned_read_back(self):
        def turn(doc):
            row = doc["spec"]["evidence"]["editor"]["body_samples"][3]
            rotation = multiply(euler((0, .1, 0), 4), [F32(b) for b in row["readback_rotation"]])
            row["readback_rotation"] = [BITS(v) for v in rotation]

        report = self.run_mode("e2", self.edited("body-targets-tall", turn))
        self.assertEqual(len(report["failures"]), 3)
        self.assertIn("model to read-back", report["failures"][0])
        self.assertIn("request to read-back", report["failures"][1])
        # The sample bends no spine sideways: no lateral domain is left, and every pose is compared.
        self.assertIn("no validated spine lateral domain", report["failures"][2])
        self.assertIsNone(report["lateral_limit"])
        # A clip has no request: only the model residual.
        report = self.run_mode("e2", self.edited("body-clip-level", turn))
        self.assertEqual(len(report["failures"]), 2)

    def test_e3a_fails_a_moved_or_turned_hips(self):
        def move(doc):
            pose = doc["capture"]["samples"][4]["pose"]["transforms"][hips_index(doc)]
            pose["translation"][0] = BITS(F32(pose["translation"][0]) + 5e-4)

        report = self.run_mode("e3a", self.edited("body-probes-compact", move))
        self.assertEqual(len(report["failures"]), 2)
        self.assertIn("hips translation off", report["failures"][0])
        self.assertIn("no validated spine lateral domain", report["failures"][1])
        self.assertGreater(report["worst_ratio"], 1)

        def turn(doc):
            # Sample 3 is untwisted, inside 2.2's twist scope (samples 1, 2, 4 and 5 are refused).
            pose = doc["capture"]["samples"][3]["pose"]["transforms"][hips_index(doc)]
            pose["rotation"] = [BITS(v) for v in multiply([F32(b) for b in pose["rotation"]], euler((.1, 0, 0), 4))]

        report = self.run_mode("e3a", self.edited("body-chain-armature-targets", turn))
        self.assertTrue(any("hips rotation off" in line for line in report["failures"]))
        with self.assertRaisesRegex(ValueError, "give --unityrs"):
            self.run_mode("e3a", avatar_of=None)

    def test_e4_fails_an_off_read_back_and_a_moved_start(self):
        def shift(index, delta):
            def edit(doc):
                row = doc["spec"]["evidence"]["editor"]["body_samples"][index]
                row["readback_position"][2] = BITS(F32(row["readback_position"][2]) + delta)
            return edit

        report = self.run_mode("e4", self.edited("body-clip-level", shift(2, 1e-3)))
        self.assertEqual(config(report, "level")["status"], "fail")
        self.assertEqual(len(report["failures"]), 1)
        # The start variant moved alone: its own sample and its twin pair both fail.
        report = self.run_mode("e4", self.edited("body-clip-keep-o0-y0-xz0-start", shift(1, 1e-3)))
        self.assertEqual([config(report, "keep-o0-y0-xz0")[key] for key in ("status", "start_offset")], ["fail", "fail"])
        self.assertTrue(any("start offset changed" in line for line in report["failures"]))
        # An unsupported combination's pair only informs.
        docs = dict(self.docs, **{"body-clip-keep-o1-y0-xz0-start": recording("body-clip-keep-o1-y0-xz0-start", 12)})
        docs = copy.deepcopy(docs)
        shift(1, 1e-2)(docs["body-clip-keep-o1-y0-xz0-start"])
        report = self.run_mode("e4", docs)
        self.assertTrue(report["passed"])
        self.assertEqual(config(report, "keep-o1-y0-xz0")["start_offset"], "fail")

    def test_e4_needs_a_comparison_in_each_version_and_names_topology_refusals(self):
        def unread(doc):
            doc["spec"]["evidence"]["editor"].pop("body_samples", None)
        root = scratch.for_case(self)
        quiet = copy.deepcopy(self.docs)
        for name in quiet:
            if body.BODY_INFO[name]["kind"] == "clip":
                unread(quiet[name])
        folders = [write_run(root / "a", self.docs), write_run(root / "b", quiet, "6000.6.9f1")]
        with mock.patch.object(runs, "REQUIRED", frozenset()), mock.patch.object(e4, "EDITORS", runs.EDITORS):
            report = freeze.run_mode("e4", folders, avatar_of=avatars, clip_of=clips)
            self.assertEqual(report["failures"], ["E4: no comparison in Editor 6000"])
            # The families come from the code (E3b's), not the rows: one version alone fails too.
            report = freeze.run_mode("e4", folders[:1], avatar_of=avatars, clip_of=clips)
            self.assertEqual(report["failures"], ["E4: no comparison in Editor 6000"])
        self.assertIs(runs.EDITORS, freeze.EDITORS)
        # A configuration whose every sample the topology scope refused is a named refusal, never omitted.
        with mock.patch.object(runs, "REQUIRED", frozenset()):
            loaded = runs.load(folders[:1], "training")
        rows = freeze.observed(loaded, avatars, clips)
        for row in rows:
            if row["case"] == "body-clip-level":
                row["refused"] = "topology: outside"
        report = e4.e4(rows, {f"{run.version}/{case}": clips(run, doc) for run in loaded
                              for case, doc in run.documents.items() if body.BODY_INFO[case]["kind"] == "clip"})
        self.assertEqual((config(report, "level")["status"], config(report, "level")["reason"]),
                         ("refused", "topology: every sample refused"))
        self.assertNotIn(f"level/compact/{write_run.__defaults__[0]}/identity", report["passing"])

    def test_e4_refuses_a_built_clip_without_body_slots(self):
        def stripped(run, doc):
            clip = copy.deepcopy(clips(run, doc))
            clip["m_MuscleClip"]["m_IndexArray"][7:14] = [-1] * 7
            return clip

        with self.assertRaisesRegex(ValueError, "no body slots"):
            self.run_mode("e4", clip_of=stripped)

    def test_e4_fails_a_start_variant_without_its_twin(self):
        docs = {name: doc for name, doc in self.docs.items() if name != "body-clip-keep-o0-y0-xz0"}
        report = self.run_mode("e4", docs)
        self.assertTrue(any("no start-offset twin" in line for line in report["failures"]))
        self.assertEqual(config(report, "keep-o0-y0-xz0")["start_offset"], "fail")

    def test_e4_refuses_an_unbounded_yaw(self):
        with mock.patch("anim_body_e4.yaw_error", return_value=math.inf):
            report = self.run_mode("e4")
        self.assertEqual({row["status"] for row in report["configurations"].values()}, {"refused", "undecided"})
        self.assertIn("E4: no comparison", report["failures"])

    def test_no_comparison_or_a_missing_case_fails(self):
        self.assertIn("E3: no comparison", check.e3([], None, {})["failures"])
        self.assertIn("E2: no comparison", check.e2([])["failures"])
        # A body case's sample without a target fails; a P2b case's is excluded by its role, and listed.
        rows = [dict(name="v/body-probes-compact", case="body-probes-compact", local=0, state=dict(target_kind=None)),
                dict(name="v/humanoid-compact-muscles", case="humanoid-compact-muscles", local=0,
                     state=dict(target_kind=None))]
        report = check.e3(rows, None, {})
        self.assertIn("v/body-probes-compact@0: a body case's sample without a target", report["failures"])
        self.assertEqual(report["excluded"], ["v/humanoid-compact-muscles"])
        # A required case the run does not record fails every check.
        folder = scratch.for_case(self)
        write_run(folder, self.docs)
        with mock.patch.object(runs, "REQUIRED", frozenset({*self.docs, "body-probes-tall"})):
            report = freeze.run_mode("e2", [folder], avatar_of=avatars, clip_of=clips)
        self.assertIn("2022.3.62f3: required case body-probes-tall not recorded", report["failures"])

    def test_e3_fails_an_unbounded_uncertainty_or_a_moved_rest(self):
        with mock.patch.object(runs.Design, "hips_uncertainty", return_value=math.inf):
            report = self.run_mode("e3a")
        self.assertTrue(report["failures"] and all("over E_unity,s" in line or "T_s" in line
                                                   for line in report["failures"][:-1]))
        self.assertIn("no validated spine lateral domain", report["failures"][-1])

        def grow(doc):
            pose = doc["capture"]["samples"][3]["pose"]["transforms"]
            spine = next(row["node"] for row in doc["capture"]["humanoid"]["bones"] if row["name"] == "Spine")
            pose[spine]["scale"] = [BITS(1.001)] * 3

        report = self.run_mode("e3a", self.edited("body-chain-armature-targets", grow))
        self.assertTrue(any("off the rest" in line for line in report["failures"]))

    def test_the_lateral_limit_is_the_largest_passing_value_a_step_below_every_failing_one(self):
        def row(name, lateral):
            muscles = dict(zip(model.LATERAL_MUSCLES, (lateral / 2, -lateral, 0.)))
            return dict(name=name, local=0, state=dict(target_kind="known", muscle_names=list(muscles),
                                                       sample=dict(muscles_f32=[BITS(v) for v in muscles.values()])))

        values = (0, .046875, .109375, .125, .3125, .3203125, .5)
        rows = [row(f"s{k}", value) for k, value in enumerate(values)]

        def limit(failing, unnamed=False, refused=()):
            lines = [f"s{k}@0: off" for k, value in enumerate(values) if value in failing]
            lines += ["E3: no comparison"] * unnamed
            def e2(rows, compared):
                compared.update(f"s{k}@0" for k, value in enumerate(values) if value not in refused)
                return dict(failures=lines)

            with mock.patch.object(check, "e2", e2), \
                    mock.patch.object(check, "e3", return_value=dict(failures=[])):
                return check.lateral_limit(rows, None, None)

        def bracket(failing, refused=()):
            found = limit(failing, refused=refused)
            return found["limit"], (found["failing"] or {}).get("value"), (found["passing"] or {}).get("sample")

        self.assertEqual(bracket(()), (.5, None, "s6@0"))
        # The largest compared passing value at least a step below every failing one: an observed value, never
        # the failing one less a step (.2890625 here, which no sample tested).
        self.assertEqual(bracket((.3203125, .5)), (.125, .3203125, "s3@0"))
        self.assertEqual(bracket((.125, .5)), (.046875, .125, "s1@0"))
        self.assertEqual(bracket((.5,)), (.3203125, .5, "s5@0"))
        # A passing value above a failing one widens nothing.
        self.assertEqual(bracket((.3125,)), (.125, .3125, "s3@0"))
        # Exactly a step below counts.
        def e2(rows, compared):
            compared.update(("a@0", "b@0"))
            return dict(failures=["b@0: off"])

        with mock.patch.object(check, "e2", e2), mock.patch.object(check, "e3", return_value=dict(failures=[])):
            found = check.lateral_limit([row("a", .125), row("b", .125 + check.LATERAL_STEP)], None, None)
        self.assertEqual(found, dict(limit=.125, failing=dict(sample="b@0", value=.15625),
                                     passing=dict(sample="a@0", value=.125)))
        # A sample the scope refused (never compared) validates nothing.
        self.assertEqual(bracket((.3203125, .5), refused=(.125,)), (.109375, .3203125, "s2@0"))
        # A failure at no sample, or at an unbent spine, leaves no domain.
        self.assertEqual(limit((), unnamed=True), dict(limit=0., failing=dict(sample="E3", value=0.), passing=None))
        self.assertLess(limit((0,))["limit"], check.LATERAL_FLOOR)
        # The model's refusal, named, past the limit, inclusive at it.
        model.lateral_scope({"Chest Left-Right": -.25, "Spine Front-Back": 1}, .25)
        with self.assertRaisesRegex(model.Refused, "^spine_lateral_range: spine Left-Right 0.3 over 0.25"):
            model.lateral_scope({"UpperChest Left-Right": .3}, .25)

    def test_no_lateral_limit_without_every_band_sweep(self):
        # A required band sweep unrecorded sets no limit, even with the rig's own probe case missing too: E2 and
        # E3a fail and compare every pose.
        folder = scratch.for_case(self)
        write_run(folder, self.docs)
        required = {name for name in self.docs if runs.role_of(name) == "training"}
        version = runs.load([folder], "training")[0].version
        with mock.patch.object(runs, "REQUIRED", frozenset({*required, "body-probes-band-tall", "body-probes-tall"})):
            for mode in ("e2", "e3a"):
                report = freeze.run_mode(mode, [folder], avatar_of=avatars, clip_of=clips)
                self.assertIsNone(report["lateral_limit"])
                self.assertIn("no spine lateral limit: the band sweeps that validate it are not recorded "
                              f"({version}: body-probes-band-tall)", report["failures"])
                self.assertFalse(report["passed"])

    def test_e2_and_e3_refuse_a_pose_past_the_lateral_limit(self):
        folder = scratch.for_case(self)
        write_run(folder, self.docs)
        with mock.patch.object(runs, "REQUIRED", frozenset()):
            rows = freeze.observed(runs.load([folder], "training"), avatars, clips)
        design = runs.Design(rows)
        fit = runs.solve(design)
        compared = [row for row in rows if not row.get("refused") and row["state"]["target_kind"] is not None]
        values = sorted({check.lateral_of(row) for row in compared})
        self.assertGreater(len(values), 2)
        found = check.lateral_limit(rows, design, fit)
        self.assertEqual((found["limit"], found["failing"], found["passing"]["value"]), (values[-1], None, values[-1]))
        cut = values[len(values) // 2]
        past = sum(check.lateral_of(row) > cut for row in compared)
        self.assertGreater(past, 0)
        # Past the cut a pose refuses, unless the frame's twist scope already refused it.
        for refused in (lambda limit: check.e3(rows, design, fit, limit)["refused"],
                        lambda limit: check.e2(rows, limit)["refused"]):
            self.assertTrue(0 < refused(cut) - refused(None) <= past)
            self.assertEqual(refused(values[-1]), refused(None))

    def test_a_clip_sample_takes_its_lateral_value_from_its_built_clip(self):
        # A built clip's spine Left-Right slot, sampled as playback does; an unbound muscle is 0.
        attribute = body.MUSCLE_ATTRIBUTE + support.MUSCLES.index("Spine Left-Right")
        stream = support.compiled([[("Spine Left-Right", attribute, (0, .5, 1), (0, 0, .375))]])
        clip = {"m_MuscleClip": dict(m_Clip=stream["m_Clip"], m_IndexArray=stream["m_IndexArray"]),
                "m_ClipBindingConstant": stream["m_ClipBindingConstant"]}
        self.assertEqual(e4.clip_muscles(clip, list(support.MUSCLES), 1.),
                         {"Spine Left-Right": .375, "Chest Left-Right": 0., "UpperChest Left-Right": 0.})
        self.assertEqual(e4.clip_muscles(clip, list(support.MUSCLES), .5)["Spine Left-Right"], 0.)
        self.assertAlmostEqual(e4.clip_muscles(clip, list(support.MUSCLES), .75)["Spine Left-Right"], .1875, 6)
        # An index naming no slot, or none at all, refuses rather than reading 0.
        for index in ([*clip["m_MuscleClip"]["m_IndexArray"][:attribute], 99, *range(200)], [0] * 3):
            broken = copy.deepcopy(clip)
            broken["m_MuscleClip"]["m_IndexArray"] = index
            with self.assertRaisesRegex(ValueError, "invalid slot index"):
                e4.clip_muscles(broken, list(support.MUSCLES), 1.)
        # A clip sample records no applied muscles: unread, its lateral value refuses rather than reading 0.
        folder = scratch.for_case(self)
        write_run(folder, self.docs)
        with mock.patch.object(runs, "REQUIRED", frozenset()):
            loaded = runs.load([folder], "training")
        clip_row = next(row for row in runs.observe(loaded, avatar_of=avatars) if row["case"] == "body-clip-level")
        with self.assertRaisesRegex(ValueError, "a clip sample's muscles are unread"):
            check.lateral_of(clip_row)
        # Sampled at the clock playback used: the effective source where it differs from the nominal one.
        times = []

        def spy(clip, names, time):
            times.append(time)
            return dict.fromkeys(model.LATERAL_MUSCLES, 0.)

        docs = self.edited("body-clip-level", lambda doc: doc["capture"]["samples"][1].update(effective_source=.875))
        write_run(folder, docs)
        with mock.patch.object(runs, "REQUIRED", frozenset()), mock.patch.object(e4, "clip_muscles", spy):
            freeze.observed(runs.load([folder], "training"), avatars, clips)
        self.assertIn(.875, times)
        write_run(folder, self.docs)
        # Read from the clip, it decides the lateral scope as a known vector's does.
        bent = dict.fromkeys(model.LATERAL_MUSCLES, 0.) | {"Spine Left-Right": .5}
        with mock.patch.object(e4, "clip_muscles", return_value=bent):
            rows = freeze.observed(loaded, avatars, clips)
        fit = runs.solve(runs.Design(rows))
        clipped = [row for row in rows if not row.get("refused") and "clip_muscles" in row["state"]
                   and row["state"]["target_kind"] is not None]
        self.assertTrue(clipped and all(check.lateral_of(row) == .5 for row in clipped))
        design = runs.Design(rows)
        compared = set()
        check.e3(rows, design, fit, compared=compared)
        within = [row for row in clipped if f"{row['name']}@{row['local']}" in compared]
        self.assertTrue(within)
        added = check.e3(rows, design, fit, .25)["refused"] - check.e3(rows, design, fit)["refused"]
        self.assertTrue(len(within) <= added <= len(clipped))

    def test_e3a_takes_the_translations_below_the_hips_from_the_avatar(self):
        # Unity's playback writes m_SkeletonPose's translations, which the prefab's can miss by f32 rounding:
        # every sample at the Avatar's translation passes, and at the prefab's then fails.
        name = "body-chain-armature-targets"
        doc = self.docs[name]
        spine = next(row["node"] for row in doc["capture"]["humanoid"]["bones"] if row["name"] == "Spine")
        y = doc["capture"]["rest"]["transforms"][spine]["translation"][1]
        self.assertGreater(F32(y), 0)
        nudged = F32(f"{int(y, 16) + 1:08x}")  # one f32 ulp up

        def nudge(run, recorded):
            value = avatar(recorded)
            if recorded["spec"]["evidence"]["editor"]["fixture_case"] == name:
                path = runs.node_paths(recorded)[spine]
                k = next(k for k, (_, p) in enumerate(value["m_TOS"]) if p == path)
                check.human_of(value)["m_SkeletonPose"]["data"]["m_X"][k]["t"]["y"] = nudged
            return value

        def played(recorded):
            for sample in recorded["capture"]["samples"]:
                sample["pose"]["transforms"][spine]["translation"][1] = BITS(nudged)

        docs = self.edited(name, played)
        self.assertEqual(runs.rest_translations(nudge(None, docs[name]), docs[name])[spine][1], BITS(nudged))
        report = self.run_mode("e3a", docs, avatar_of=nudge)
        self.assertFalse(any("off the rest" in line for line in report["failures"]), report["failures"])
        report = self.run_mode("e3a", docs)
        self.assertTrue(any(f"{name}@" in line and "off the rest" in line for line in report["failures"]))
        # A node below the hips the Avatar's skeleton lacks refuses.
        bare = avatar(doc)
        bare["m_TOS"] = [row for row in bare["m_TOS"] if row[1] != runs.node_paths(doc)[spine]]
        with self.assertRaisesRegex(ValueError, "not in the Avatar's skeleton"):
            runs.rest_translations(bare, doc)

    def test_a_leaf_with_mass_refuses_before_any_kinematics(self):
        def heavy(run, doc):
            value = avatar(doc)
            human = check.human_of(value)
            masses = [F32(human["m_HumanBoneMass"]["v"][8 * k:8 * k + 8]) for k in range(25)]
            masses[24] = 1e-3  # the Jaw, absent from the compact rig
            human["m_HumanBoneMass"]["v"] = "".join(BITS(m) for m in masses)
            return value

        docs = {"body-probes-compact": self.docs["body-probes-compact"]}
        folder = scratch.for_case(self)
        write_run(folder, docs)
        with mock.patch.object(adequacy, "samples", side_effect=AssertionError("kinematics")):
            rows = runs.observe(runs.load([folder], "training"), avatar_of=heavy)
        self.assertTrue(rows and all(row["refused"] == "topology: nonzero mass on a zero-mass leaf (['Jaw'])"
                                     for row in rows))

    def test_the_topology_comes_from_the_avatar_before_any_kinematics(self):
        def lone_eye(run, doc):
            value = avatar(doc)
            check.human_of(value)["m_HumanBoneIndex"][22] = 3
            return value

        docs = {"body-probes-compact": self.docs["body-probes-compact"]}
        folder = scratch.for_case(self)
        write_run(folder, docs)
        with mock.patch.object(adequacy, "samples", side_effect=AssertionError("kinematics")):
            rows = runs.observe(runs.load([folder], "training"), avatar_of=lone_eye)
        self.assertTrue(rows and all(row["refused"] == "the Avatar maps ['LeftEye'] unlike the capture"
                                     for row in rows))


class SmokeCauses(unittest.TestCase):
    """The two input conventions the first run on owner recordings exposed, on synthetic recordings."""

    def test_the_recorded_masses_are_hierarchy_masses(self):
        doc = recording("body-probes-no-upper-chest", samples=1)
        rows = doc["spec"]["evidence"]["editor"]["human_bones"]
        hips = next(row for row in rows if row["name"] == "Hips")
        # GetBoneDefaultHierarchyMass: the Hips hold the whole body's default mass.
        self.assertEqual(F32(hips["mass"]), sum(support.MASSES[name] for name in tolerance.BODY_SLOTS))
        parents, masses = adequacy.bone_table([doc])
        self.assertEqual(masses, {name: support.MASSES[name] for name in tolerance.BODY_SLOTS})
        # The finger bones are not read (Unity 6 returns unspecified values for them).
        next(row for row in rows if row["name"] == "Finger 0")["mass"] = "7fc00000"
        self.assertEqual(adequacy.bone_table([doc])[1], masses)
        # Two tables that differ only in the fingers are one table; in a body slot, two.
        other = copy.deepcopy(doc)
        next(row for row in other["spec"]["evidence"]["editor"]["human_bones"]
             if row["name"] == "Finger 3")["mass"] = "7f800000"
        self.assertEqual(adequacy.bone_table([doc, other])[1], masses)
        next(row for row in other["spec"]["evidence"]["editor"]["human_bones"]
             if row["name"] == "Spine")["mass"] = BITS(.5)
        with self.assertRaisesRegex(ValueError, "expected one HumanTrait bone table"):
            adequacy.bone_table([doc, other])
        # A table whose Hips hold less than their children is not a hierarchy table.
        hips["mass"] = BITS(support.MASSES["Hips"])
        with self.assertRaisesRegex(ValueError, "Hips is below its children"):
            adequacy.bone_table([doc])

    def test_a_scaled_animators_read_back_is_scaled_back(self):
        doc = recording("body-chain-scaled-animator-targets", samples=4)
        parents, masses = adequacy.bone_table([doc])
        rows = list(adequacy.samples("body-chain-scaled-animator-targets", doc, parents, masses))
        for row in rows:
            state = row["state"]
            self.assertEqual(state["root"].scale, 1.25)
            points = {name: frame.position for name, frame in state["bones"].items()}
            want = model.centre(points, state["weights"], state["links"], coefficients(state["links"]))
            recorded = tolerance.decode(state["readback"]["readback_position"], 3, F32)
            read = tolerance.readback_position(state["readback"], state["root"], F32)
            self.assertLess(max(abs(state["scale"] * a - b) for a, b in zip(read, want)), 1e-6)
            self.assertGreater(max(abs(state["scale"] * a - b) for a, b in zip(recorded, want)), .1)
            # So E1's right-hand side is the model's coefficient columns at the true coefficients.
            a = coefficients(state["links"])
            fitted = [sum(a[key] * column[k] for key, column in row["columns"].items()) for k in range(3)]
            self.assertLess(math.dist(fitted, runs.rhs(row)), 1e-6)


class FitCheck(unittest.TestCase):
    """E1 on a design that reaches every training rig's edges. The synthetic poses are a stand-in for Unity's
    muscle map, so the range and band sweeps, whose adequacy design-only predicts from the recorded probes, are
    left out of this design and of its plan."""

    @classmethod
    def setUpClass(cls):
        names = [name for name, info in body.BODY_INFO.items()
                 if info["kind"] == "probes" and info["rig"] != body.HOLDOUT
                 and name not in body.RANGE_CASES | body.BAND_CASES]
        full = {"body-probes-compact", *body.EXTRA_CASES}
        cls.docs = {name: recording(name, samples=None if name in full else 10) for name in names}
        cls.docs["body-targets-tall-offset"] = recording("body-targets-tall-offset")
        cls.docs["body-chain-armature-clip"] = recording("body-chain-armature-clip")

    def e1(self, docs):
        with mock.patch.object(adequacy, "RANGE_CASES", frozenset()), \
                mock.patch.object(adequacy, "BAND_CASES", frozenset()):
            return checked("e1", scratch.for_case(self), docs)

    def test_e1_passes_and_recovers_the_coefficients(self):
        report = self.e1(self.docs)
        self.assertEqual(report["failures"], [])
        self.assertEqual(report["rank"], len(report["coefficients"]))
        self.assertLess(report["condition"], 1e6)
        self.assertTrue(all(abs(a - .4) < 1e-4 for a in report["coefficients"].values()))
        self.assertGreater(report["terminal_samples"], 5)
        self.assertLess(report["worst_residual"], 1)

    def test_e1_fails_an_off_read_back_a_large_coefficient_and_a_moving_terminal(self):
        docs = copy.deepcopy(self.docs)
        row = docs["body-probes-tall"]["spec"]["evidence"]["editor"]["body_samples"][5]
        row["readback_position"][0] = BITS(F32(row["readback_position"][0]) + 1e-3)
        report = self.e1(docs)
        self.assertTrue(any("fit residual" in line for line in report["failures"]))
        # Coefficients of 1.5 are outside the centre-of-mass bound's |a| <= 1.
        docs = {name: recording(name, samples=6, value=1.5) for name in ("body-probes-compact", "body-probes-tall")}
        report = self.e1(docs)
        self.assertTrue(any("outside |a| <= 1.0" in line for line in report["failures"]))
        # A sample turning only the left hand moved the hips.
        docs = copy.deepcopy(self.docs)
        doc = docs["body-probes-compact"]
        names = [m["name"] for m in doc["capture"]["humanoid"]["muscles"]]
        index = next(i for i, sample in enumerate(doc["capture"]["samples"])
                     if {names[k] for k, b in enumerate(sample["muscles_f32"]) if F32(b)} == {"Left Hand Down-Up"})
        pose = doc["capture"]["samples"][index]["pose"]["transforms"][hips_index(doc)]
        pose["translation"][1] = BITS(F32(pose["translation"][1]) + 1e-3)
        report = self.e1(docs)
        self.assertTrue(any("terminal ['LeftHand'] moved the hips" in line for line in report["failures"]))

    def test_e1_reports_a_short_design(self):
        report = self.e1({name: self.docs[name] for name in ("body-probes-compact",)})
        self.assertFalse(report["passed"])
        self.assertIn("the adequacy checkpoint is short", report["failures"])
        self.assertTrue(report["checkpoint"]["coverage"]["missing"])
        json.dumps(freeze.clean(report), allow_nan=False)
        self.assertTrue(math.isfinite(report["worst_residual"]))


if __name__ == "__main__":
    unittest.main()
