"""Round 11's body fixtures (anim_fixture_round11, anim_fixture_muscles): E5's clip and chain rigs and B5a's
muscle sweeps mirror the builder, stay out of B2, the freeze and the hold-out, and complete only as a declared
round 11 run with the whole inventory; freeze v8's provenance holds on this checkout."""

import json
import re
import unittest
from contextlib import ExitStack, contextmanager
from pathlib import Path
from unittest import mock

import test_anim_replay_coverage as coverage  # noqa: F401  (puts tools/parity on the path)
import anim_body_support as support
from anim_fixture_support import RUN, write
import test_anim_body_fixtures as fixtures
from test_anim_body_fixtures import View, source

import anim_body_runs as runs  # noqa: E402
import anim_body_seal as seal  # noqa: E402
import anim_fixture_body as body  # noqa: E402
import anim_fixture_muscles as muscles  # noqa: E402
import anim_fixture_round11 as r11  # noqa: E402
import anim_replay as replay  # noqa: E402

HERE = Path(__file__).resolve().parents[1]
FREEZE = HERE / "anim_body_freeze_v8.json"
CASES = sorted(r11.ROUND11_CASES)
EVERY = [*body.BODY_CASES, *CASES]
TWIST_MUSCLES = tuple(muscle for muscle, _ in muscles.LIMB_TWISTS)
FINGER_MUSCLES = tuple(muscle for group in muscles.FINGER_GROUPS for muscle in group)
NAMES = (*support.PROBE_MUSCLES, *TWIST_MUSCLES, *FINGER_MUSCLES)


def responding_axes():
    """The synthetic pose generator's axes, with each limb twist muscle turning its bone about x and each finger
    muscle its phalanx (a stretch about z, a spread about y)."""
    axes = dict(support.AXES)
    axes.update({muscle: (bone, 0) for muscle, bone in muscles.LIMB_TWISTS})
    axes.update({muscle: (muscles.muscle_bone(muscle, body.PROBE_GROUPS), 1 if "Spread" in muscle else 2)
                 for muscle in FINGER_MUSCLES})
    return axes


def limits(muscle):
    """The synthetic limits, the twist and finger muscles' at 40 degrees."""
    return (-40, 40) if muscle in TWIST_MUSCLES or muscle in FINGER_MUSCLES else LIMITS(muscle)


LIMITS = support.limits


def floats(text):
    return tuple(float(v.rstrip("f")) for v in re.findall(r"-?[\d.]+f?", text))


@contextmanager
def round11_docs():
    """Round 11's tables in completion, and the synthetic recordings' (with real limb twist and finger names)."""
    with ExitStack() as stack:
        stack.enter_context(r11.round11_tables())
        bones = tuple(b for b in support.BONES if not b.startswith("Finger "))
        stack.enter_context(mock.patch.multiple(
            support, BODY_INFO=body.BODY_INFO, probe_schedule=r11.probe_schedule, schedule_name=r11.schedule_name,
            layout=r11.rig_layout, BONES=(*bones[:-1], *muscles.FINGER_BONES, bones[-1]), MUSCLES=NAMES,
            AXES=responding_axes(), limits=limits))
        yield


class Round11InventoryTests(unittest.TestCase):
    def test_the_clip_and_chain_rigs_mirror_the_builder(self):
        clips = source("UnityrsBodyClips.cs")
        admitted = re.search(r"E5Admitted = \{(.*?)\};", clips, re.S).group(1)
        self.assertEqual(tuple(re.findall(r'"([a-z0-9-]+)"', admitted)), r11.E5_ADMITTED)
        self.assertIn('SmallRig = new BodyRig { Name = "small", Size = .82f };', clips)
        topologies = re.search(r"E5Topologies = \{(.*?)\};", clips, re.S).group(1)
        self.assertEqual(tuple(re.findall(r'"([a-z-]+)"', topologies)), r11.E5_TOPOLOGIES)
        self.assertIn('ChainRigs.Single(rig => rig.Name == "large"), ProbeRigs.Single(rig => rig.Name == "tall"),\n'
                      '        SmallRig }.Concat(E5Topologies.Select(', clips)
        self.assertEqual(r11.CLIP_RIGS, ("large", "tall", "small", *r11.E5_TOPOLOGIES))
        # Every training topology B3 admits but the compact rig's: none it refuses, never a hold-out rig.
        refused = {"no-chest", "no-neck", "no-left-shoulder", "no-right-shoulder"}
        self.assertEqual(set(r11.E5_TOPOLOGIES),
                         set(body.PROBE_RIGS) - refused - {"compact", "tall", *body.HOLDOUT_RIGS})
        builder = source("UnityrsBodyFixtures.cs")
        features = {"ScaledAnimator = true": "scaled-animator", "Armature = true": "armature",
                    "HipsScale = 1.2f": "hips-scale"}

        def chain_table(text, table):
            text = text[text.index(f"BodyRig[] {table} = {{"):]
            parsed = {}
            for name, fields in re.findall(r'new BodyRig \{ Name = "([a-z-]+)"([^}]*)\}', text[:text.index("};")]):
                size = re.search(r"Size = ([\d.]+)f", fields)
                parsed[name] = (next((v for k, v in features.items() if k in fields), ""),
                                float(size.group(1)) if size else 1)
            return parsed

        # The builder's ChainRigs stay freeze v8's four, which B4's tests read; round 11's are E5ChainRigs.
        v8, e5 = chain_table(builder, "ChainRigs"), chain_table(clips, "E5ChainRigs")
        self.assertEqual(tuple(v8), tuple(body.CHAIN_RIGS))
        self.assertEqual(tuple(e5), r11.CHAIN_RIGS)
        self.assertEqual(v8 | e5, r11.CHAIN_FEATURES)
        self.assertIn(".Concat(ChainRigs.Concat(E5ChainRigs).SelectMany(rig => RigCases(rig, true)))", builder)
        self.assertIn("foreach (var rig in ChainRigs.Concat(E5ChainRigs))", builder)
        # Size scales the hips' offset in an armature, never the armature's own transform (B4 admits F5's only).
        self.assertIn('Child(root, "Armature", new Vector3(0, .05f, .1f));', builder)
        self.assertIn("hips.localPosition = new Vector3(0, .125f, .8375f) * rig.Size;", builder)
        self.assertIn(".Concat(ClipRigs.SelectMany(rig => ClipCases(rig))).Concat(MuscleSweeps.Select(sweep => sweep.name));",
                      builder)
        self.assertIn("BuildMuscleSweeps(build, report);", builder)

    def test_the_admitted_variants_are_freeze_v8s_e4_passing_set(self):
        passing = json.loads(FREEZE.read_text(encoding="utf-8"))["e4"]["passing"]
        compact = {entry.split("/")[0] for entry in passing if entry.split("/")[1] == "compact"}
        self.assertEqual(set(r11.E5_ADMITTED), {variant.removesuffix("-start") for variant in compact})
        self.assertEqual(len(r11.E5_VARIANTS), 21)

    def test_the_cases_are_round_11s_own(self):
        # 15 clip rigs x 21 variants x 2 roots, each new chain rig's targets and clip, and the four sweeps, in the
        # builder's order: the chains, the clips, the sweeps.
        self.assertEqual(len(r11.ROUND11_CASES), 15 * 21 * 2 + 5 * 2 + 4)
        self.assertEqual(len(r11.E5_CASES), 640)
        kinds = [r11.ROUND11_INFO[name]["kind"] for name in r11.ROUND11_CASES]
        self.assertEqual(kinds[:10], ["targets", "clip"] * 5)
        self.assertEqual(kinds[10:], ["clip"] * 630 + ["probes"] * 4)
        self.assertEqual(len(r11.ROUND11_BUNDLES), 5 + 15 + 4)
        for rig in r11.CLIP_RIGS:
            for variant in r11.E5_VARIANTS:
                for root, suffix in (("identity", ""), ("offset", "-offset")):
                    name = f"body-clip-{rig}-{variant}{suffix}"
                    fixture = r11.ROUND11_CASES[name]
                    self.assertEqual((fixture.bundle, fixture.prefab, fixture.clip, fixture.playback),
                                     (f"body-clips-{rig}", root, f"body-clip-{rig}-{variant}", "animator"))
                    self.assertEqual(r11.ROUND11_INFO[name], dict(kind="clip", rig=rig, variant=variant, root=root))
        for rig in r11.CHAIN_RIGS:
            self.assertEqual(r11.ROUND11_INFO[f"body-chain-{rig}-clip"]["variant"], body.CHAIN_VARIANT)
            probe = r11.ROUND11_CASES[f"body-chain-{rig}-targets"].probe[0]
            self.assertEqual(probe, "HumanoidRig/Armature/Hips/Spine" if rig.startswith("armature") else body.SPINE_PROBE)
        for name, rig in muscles.MUSCLE_SWEEPS.items():
            self.assertEqual(r11.ROUND11_INFO[name], dict(kind="probes", rig=rig, variant="muscle", root="identity"))
            self.assertEqual(r11.ROUND11_CASES[name].bundle, name.replace("probes-", ""))
        # None is an earlier case or names a hold-out rig, so the sealed cases are the hold-out's alone.
        self.assertFalse(r11.ROUND11_CASES.keys() & body.BODY_CASES.keys())
        self.assertFalse({info["rig"] for info in r11.ROUND11_INFO.values()} & set(body.HOLDOUT_RIGS))
        self.assertEqual(seal.HOLDOUT_CASES, ("body-probes-holdout-2",))

    def test_b2_and_the_freeze_never_read_them(self):
        self.assertFalse(r11.ROUND11_CASES.keys() & runs.CASES.keys())
        self.assertFalse(r11.ROUND11_CASES.keys() & runs.REQUIRED)
        files = seal.sources()
        self.assertNotIn("anim_fixture_round11.py", files)
        self.assertNotIn("anim_fixture_muscles.py", files)
        # The tables are extended only while round 11's completion runs.
        with r11.round11_tables():
            self.assertIn("body-probes-fingers", body.BODY_CASES)
        self.assertNotIn("body-probes-fingers", body.BODY_CASES)
        self.assertIs(body.check_chain, r11._BASE["check_chain"])

    def test_freeze_v8s_provenance_holds_on_this_checkout(self):
        code = json.loads(FREEZE.read_text(encoding="utf-8"))["code"]
        self.assertEqual(code["files"], seal.sources())
        self.assertEqual(code["digest"], seal.source_digest(HERE, code["files"]))
        # The whole binding where the checkout has the freeze's commit (CI's parity job checks out one commit).
        if seal.git(HERE, "cat-file", "-e", code["commit"] + "^{commit}")[0] == 0:
            seal.bind_code(code)


class MuscleSweepTests(unittest.TestCase):
    def test_the_muscle_tables_mirror_the_builder(self):
        text = source("UnityrsBodyMuscles.cs")
        self.assertEqual(floats(re.search(r"MuscleValues = \{(.*?)\};", text).group(1)), muscles.MUSCLE_VALUES)
        combined = re.search(r"MuscleCombined = \{(.*?)\};", text).group(1)
        self.assertEqual(tuple(floats(row) for row in re.findall(r"\{([^{}]*)\}", combined)), muscles.MUSCLE_COMBINED)
        self.assertEqual(tuple(re.findall(r'\("([A-Z][A-Za-z -]+)", "([A-Z][A-Za-z]+)"\)', text)), muscles.LIMB_TWISTS)
        layout = re.search(r"FingerLayout = \{(.*?)\};", text, re.S).group(1)
        vectors = [floats(v) for v in re.findall(r"new Vector3\(([^)]*)\)", layout)]
        self.assertEqual(tuple(tuple(vectors[3 * f:3 * f + 3]) for f in range(5)), muscles.FINGER_LAYOUT)
        for table in ("Fingers", "Phalanges"):
            row = re.search(table + r" = \{(.*?)\};", text).group(1)
            self.assertEqual(tuple(re.findall(r'"(\w+)"', row)), getattr(muscles, table.upper()))
        sweeps = re.search(r"MuscleSweeps = \{(.*?)\};", text, re.S).group(1)
        self.assertEqual(dict(re.findall(r'\("([a-z-]+)", "([a-z-]+)"\)', sweeps)), muscles.MUSCLE_SWEEPS)
        self.assertIn('const string BodyRound = "11";', text)
        self.assertIn('"{\\"format\\": \\"unityrs.fixture_round\\", \\"round\\": " + BodyRound + ", \\"run_id\\": \\"" '
                      '+ runId + "\\"}\\n"', text)
        self.assertIn('if (run.variant == "builtin") WriteRound(staging, run.run_id);',
                      source("UnityrsFixtureBuilder.cs"))

    def test_every_muscle_has_the_same_schedule(self):
        probed = [muscle for _, group in body.PROBE_GROUPS for muscle in group]
        self.assertEqual((len(probed), len(set(NAMES))), (45, 95))
        index = {name: i for i, name in enumerate(NAMES)}
        covered = []
        for name, rig, count, groups in (("body-probes-muscles-compact", "compact", 49, 21),
                                         ("body-probes-muscles-tall", "tall", 46, 20),
                                         ("body-probes-fingers", "fingers", 40, 10),
                                         ("body-probes-face", "eyes-jaw", 6, 3)):
            mapped = r11.rig_bones(rig)
            rows = muscles.groups(name, body.PROBE_GROUPS, mapped)
            alone = [muscle for _, group in rows for muscle in group]
            self.assertEqual((len(alone), len(rows)), (count, groups), name)
            self.assertTrue(all(muscles.muscle_bone(m, body.PROBE_GROUPS) in mapped for m in alone))
            covered += alone
            vectors = muscles.muscle_vectors(name, index, body.PROBE_GROUPS, mapped)
            self.assertEqual(len(vectors), 10 * count + 2 * groups)
            # Each muscle alone at both endpoints and four interior values per sign, each exact in f32.
            singles = [(NAMES[i], v) for vector in vectors[:10 * count] for i, v in vector.items()]
            self.assertEqual(singles, [(m, v) for m in alone for v in muscles.MUSCLE_VALUES])
            # Then each bone's muscles jointly, at two opposite vectors.
            joint = vectors[10 * count:]
            self.assertEqual([set(vector) for vector in joint[::2]], [{index[m] for m in group} for _, group in rows])
            self.assertTrue(all(a == {k: -v for k, v in b.items()} for a, b in zip(joint[::2], joint[1::2])))
            self.assertEqual(len(r11.probe_schedule(rig, NAMES, False, name)), len(vectors) + 3)
        # The compact rig's sweep and the fingers' and face's cover all 95 (the tall rig's repeats its own).
        self.assertEqual(sorted(set(covered)), sorted(NAMES))
        self.assertTrue(set({1, -1, .125, -.125}) <= set(muscles.MUSCLE_VALUES))
        self.assertEqual(len(muscles.MUSCLE_VALUES), 10)
        self.assertTrue(all(replay.f32_from_bits(replay.f32_bits(v)) == v for v in muscles.MUSCLE_VALUES))
        self.assertEqual(r11.schedule_name("body-probes-fingers"), "body-probes-fingers")
        self.assertIsNone(r11.schedule_name("body-probes-compact"))

    def test_the_fingers_rig_maps_every_finger_bone_below_its_hand(self):
        self.assertEqual(r11.rig_bones("fingers"), body.rig_bones("compact") | set(muscles.FINGER_BONES))
        self.assertEqual(len(muscles.FINGER_BONES), 30)
        rows = r11.rig_layout("fingers")
        compact = r11.rig_layout("compact")
        self.assertEqual(rows[:len(compact)], compact)
        fingers = {name: (parent, offset) for name, parent, offset in rows[len(compact):]}
        self.assertEqual(fingers["Left Thumb Proximal"], ("LeftHand", (-.02, -.01, .025)))
        self.assertEqual(fingers["Right Little Distal"], ("Right Little Intermediate", (.018, 0, 0)))
        self.assertEqual(r11.rig_layout("small")[0], ("Hips", "", (0, .82 * .88, 0)))


class Round11CheckTests(unittest.TestCase):
    def check(self, name, doc):
        return r11.check_body(name, body.BODY_CASES[name], doc, View(None), None, replay)

    def doc(self, name):
        return support.body_doc(name, RUN)

    def test_accepts_each_chain_rig_and_muscle_sweep(self):
        with round11_docs():
            for name in (*(f"body-chain-{rig}-targets" for rig in r11.CHAIN_RIGS), *muscles.MUSCLE_SWEEPS):
                with self.subTest(name=name):
                    self.assertIsNone(self.check(name, self.doc(name)))

    def test_refuses_a_sweep_whose_muscle_turns_no_bone(self):
        with round11_docs():
            for name, bone in (("body-probes-fingers", "Left Ring Distal"),
                               ("body-probes-muscles-compact", "RightLowerArm"), ("body-probes-face", "Jaw")):
                doc = self.doc(name)
                node = next(row["node"] for row in doc["capture"]["humanoid"]["bones"] if row["name"] == bone)
                rest = doc["capture"]["rest"]["transforms"][node]["rotation"]
                for sample in doc["capture"]["samples"]:
                    sample["pose"]["transforms"][node]["rotation"] = list(rest)
                with self.subTest(name=name), self.assertRaisesRegex(ValueError, "turned no bone in " + name):
                    self.check(name, doc)

    def test_refuses_a_muscle_sweep_off_its_schedule(self):
        with round11_docs():
            name = "body-probes-fingers"
            doc = self.doc(name)
            doc["spec"]["schedule"][5]["muscles"][-1] = .5
            with self.assertRaisesRegex(ValueError, "body known-vector schedule mismatch"):
                self.check(name, doc)
            doc = self.doc(name)
            doc["capture"]["humanoid"]["bones"][-2]["node"] = -1
            with self.assertRaisesRegex(ValueError, "captured Avatar topology differs from the fingers rig"):
                self.check(name, doc)

    def test_refuses_a_chain_rig_at_another_size(self):
        with round11_docs():
            for rig in ("hips-scale-small", "armature-large", "scaled-animator-large", "large"):
                name = f"body-chain-{rig}-targets"
                doc = self.doc(name)
                transforms = doc["capture"]["rest"]["transforms"]
                hips = next(i for i, node in enumerate(doc["spec"]["nodes"]) if node["name"] == "Hips")
                transforms[hips]["translation"] = [replay.f32_bits(.9 * replay.f32_from_bits(v))
                                                   for v in transforms[hips]["translation"]]
                with self.subTest(rig=rig), self.assertRaisesRegex(ValueError, "body rig chain differs"):
                    self.check(name, doc)

    def test_refuses_any_chain_feature_b4_refuses(self):
        # B4's comparison, bit for bit: the armature the builder scaled before 0025d26e, an ulp off in any stored
        # component, the same rotation's other sign, and the Animator node's or the hips' scale an ulp off.
        def ulp(bits, k=1):
            return f"{int(bits, 16) + k:08x}"

        with round11_docs():
            for name, node, field, edit in (
                    ("armature-large", 2, "translation", lambda v: [replay.f32_bits(x) for x in (0, .09, .18)]),
                    ("armature-large", 2, "translation", lambda v: [v[0], ulp(v[1]), v[2]]),
                    ("armature-large", 2, "rotation", lambda v: [ulp(v[0]), *v[1:]]),
                    ("armature-large", 2, "rotation", lambda v: [replay.f32_bits(-replay.f32_from_bits(x)) for x in v]),
                    ("armature-large", 2, "scale", lambda v: [ulp(v[0], -1), *v[1:]]),
                    ("scaled-animator-small", 1, "scale", lambda v: [*v[:2], ulp(v[2])]),
                    ("hips-scale-large", 2, "scale", lambda v: [ulp(v[0]), *v[1:]])):
                doc = self.doc(f"body-chain-{name}-targets")
                transforms = doc["capture"]["rest"]["transforms"]
                transforms[node][field] = edit(transforms[node][field])
                with self.subTest(name=name, field=field), self.assertRaisesRegex(ValueError, "body rig chain differs"):
                    self.check(f"body-chain-{name}-targets", doc)
            # A stored zero of either sign is B4's zero.
            doc = self.doc("body-chain-armature-large-targets")
            doc["capture"]["rest"]["transforms"][2]["translation"][0] = "80000000"
            doc["capture"]["rest"]["transforms"][2]["rotation"][1] = "80000000"
            self.assertIsNone(self.check("body-chain-armature-large-targets", doc))

    def test_refuses_a_human_scale_outside_b4s_range(self):
        name = "body-chain-armature-large-targets"
        low, high = r11.H_RANGE
        self.assertEqual([replay.f32_bits(v) for v in r11.H_RANGE], ["3f47f9bb", "3fb7a311"])
        with round11_docs():
            for scale, ok in ((low, True), (high, True), (replay.f32_from_bits("3f47f9ba"), False),
                              (replay.f32_from_bits("3fb7a312"), False)):
                doc = self.doc(name)
                doc["capture"]["humanoid"]["human_scale"] = replay.f32_bits(scale)
                with self.subTest(scale=scale):
                    if ok:
                        self.assertIsNone(self.check(name, doc))
                    else:
                        with self.assertRaisesRegex(ValueError, "outside B4's range"):
                            self.check(name, doc)
            # An earlier case and a muscle sweep keep their own human scale.
            for name in ("body-chain-large-targets", "body-probes-muscles-tall"):
                doc = self.doc(name)
                doc["capture"]["humanoid"]["human_scale"] = replay.f32_bits(2)
                self.assertIsNone(self.check(name, doc))


class Round11RunTests(unittest.TestCase):
    setUp = fixtures.BodyRunTests.setUp
    complete = fixtures.BodyRunTests.complete
    run_with = fixtures.BodyRunTests.run_with

    def round11(self, names, statuses=None, declaration=True):
        """Complete the run through anim_fixture_round11.py, with its round.json (`declaration`, or none)."""
        if declaration:
            declared = dict(format="unityrs.fixture_round", round=11, run_id=RUN["run_id"])
            write(self.folder / "round.json", declared if declaration is True else declared | declaration)
        run = coverage.child.run

        def swapped(args, **kwargs):
            return run([*args[:2], str(HERE / "anim_fixture_round11.py"), *args[3:]], **kwargs)

        with mock.patch.object(coverage.child, "run", swapped):
            return self.run_with(names, statuses)

    def test_the_plain_completion_refuses_a_round_11_run(self):
        result = self.run_with(EVERY)
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("unknown or duplicate run.json case: body-chain-armature-large-clip", result.stderr)

    def test_a_run_must_declare_round_11(self):
        result = self.round11(EVERY, declaration=False)
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("not a round 11 run: no round.json", result.stderr)
        result = self.round11(EVERY, declaration=dict(run_id="2" * 32))
        self.assertIn("round.json does not declare this round 11 run", result.stderr)
        result = self.round11(EVERY, declaration=dict(round=10))
        self.assertIn("round.json does not declare this round 11 run", result.stderr)

    def test_a_declared_run_lists_the_whole_inventory(self):
        # Zero round 11 rows, or the compact rig's clips missing, is not a round 11 run.
        result = self.round11(list(body.BODY_CASES))
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn(f"round 11 run lacks {len(CASES)} body case(s): ", result.stderr)
        clips = [name for name in body.BODY_CASES if name.startswith("body-clip-")]
        result = self.round11([name for name in EVERY if name not in clips])
        self.assertIn(f"round 11 run lacks {len(clips)} body case(s): body-clip-", result.stderr)
        result = self.round11(CASES)
        self.assertIn(f"round 11 run lacks {len(body.BODY_CASES)} body case(s): ", result.stderr)

    def test_a_declared_run_records_every_body_case(self):
        # A partial run fails: every case a round 11 run expects must be recorded, the earlier ones too.
        result = self.round11(EVERY)
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn(f"round 11 body cases not recorded ({len(EVERY)}): body-chain-armature-clip (not_covered)",
                      result.stderr)
        clips = [name for name in body.BODY_CASES if name.startswith("body-clip-")]
        # (run_with keeps earlier statuses: each call states every one it changes.)
        result = self.round11(EVERY, dict.fromkeys(EVERY, "recorded") | dict.fromkeys(clips, "not_covered"))
        self.assertIn(f"round 11 body cases not recorded ({len(clips)}): body-clip-", result.stderr)
        result = self.round11(EVERY, dict.fromkeys(EVERY, "recorded") | {"body-chain-armature-clip": "not_covered"})
        self.assertIn("round 11 body cases not recorded (1): body-chain-armature-clip (not_covered)", result.stderr)
        # Every one recorded passes the inventory, and completion then checks each with round 11's tables.
        name = "body-probes-fingers"
        result = self.round11(EVERY, dict.fromkeys(EVERY, "recorded"))
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertNotIn("body cases not recorded", result.stderr)
        self.assertIn(f"{name}: missing replays/body/{name}.pending.json", result.stderr)


if __name__ == "__main__":
    unittest.main()
