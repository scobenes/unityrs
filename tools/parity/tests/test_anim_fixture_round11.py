"""Round 11's body fixtures (anim_fixture_round11, anim_fixture_muscles): E5's clip and chain rigs and B5a's
muscle sweeps mirror the builder, stay out of B2, the freeze and the hold-out, and complete only as a declared
round 11 run with the whole inventory; freeze v8's provenance holds on this checkout."""

import json
import re
import unittest
import zlib
from contextlib import ExitStack, contextmanager
from pathlib import Path
from unittest import mock

import test_anim_replay_coverage as coverage  # noqa: F401  (puts tools/parity on the path)
import anim_body_support as support
from anim_fixture_support import RUN, write
import test_anim_body_fixtures as fixtures
from test_anim_body_fixtures import source

import anim_body_runs as runs  # noqa: E402
import anim_body_seal as seal  # noqa: E402
import anim_fixture_body as body  # noqa: E402
import anim_fixture_full as full  # noqa: E402
import anim_fixture_muscles as muscles  # noqa: E402
import anim_fixture_round11 as r11  # noqa: E402
import anim_fixture_scope as scope  # noqa: E402
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
            AXES=responding_axes(), limits=limits, RIG_FEATURES=r11.RIG_FEATURES))
        yield


class Built:
    """A built bundle's view: the clip, each node's serialized Transform (its recorded rest, the Animator node at
    the offset placement for an offset root) and the Avatar, its m_Scale the capture's human scale."""

    def __init__(self, name, doc, clip=None):
        self.clip, self.transforms = clip, {}
        self.avatar_path = doc["spec"].setdefault("avatar_asset", "assets/unityrsfixtures/body/avatar.asset")
        self.human_scale = replay.f32_from_bits(doc["capture"]["humanoid"]["human_scale"])
        self.skeleton(doc)
        offset = (r11.ROUND11_INFO | body.BODY_INFO).get(name, {}).get("root") == "offset"
        for k, (node, rest) in enumerate(zip(doc["spec"]["nodes"], doc["capture"]["rest"]["transforms"])):
            node["id"] = dict(bundle=0, file_index=0, file="CAB", path_id=str(k + 1))
            values = {key: [replay.f32_from_bits(v) for v in rest[key]] for key in ("translation", "rotation", "scale")}
            if offset and node["address"] == "0":
                values["translation"], values["rotation"] = map(list, r11.OFFSET_PLACEMENT)
            self.transforms[(0, k + 1)] = {field: dict(zip(axes, values[key])) for field, key, axes in (
                ("m_LocalPosition", "translation", "xyz"), ("m_LocalRotation", "rotation", "xyzw"),
                ("m_LocalScale", "scale", "xyz"))}

    # The Avatar's (file index, path id), as BundleView.asset returns a key, never the value.
    AVATAR = (0, -90)

    def value(self, key):
        if key == self.AVATAR:
            return self.avatar()
        return self.clip if key is None else self.transforms[key]

    def asset(self, path, class_id):
        assert (path, class_id) == (self.avatar_path, 90)
        return self.AVATAR

    def avatar(self):
        return {"m_Avatar": {"m_Human": {"data": {"m_Scale": self.human_scale, "m_Skeleton": {"data": dict(
            m_ID=self.ids, m_Node=[dict(m_ParentId=parent) for parent in self.parents])},
            "m_HumanBoneIndex": self.bone_index}}}, "m_TOS": self.tos}

    def skeleton(self, doc):
        """The Avatar's skeleton as Unity builds it: the Animator node and every node below it, parents first,
        each `m_ID` its path's CRC-32, with the path in `m_TOS`, and the body slots in m_HumanBoneIndex."""
        spec = doc["spec"]
        names = {node["address"]: node["name"] for node in spec["nodes"]}
        base = spec["binding_address"]
        order = [base] + [node["address"] for node in spec["nodes"] if node["address"].startswith(base + "/")]
        at = {address: k for k, address in enumerate(order)}
        self.parents = [at.get(address.rpartition("/")[0], -1) if k else -1 for k, address in enumerate(order)]
        paths = ["/".join(names[base + "/" + "/".join(address[len(base) + 1:].split("/")[:n + 1])]
                          for n in range(address.count("/") - base.count("/"))) for address in order]
        self.ids = [zlib.crc32(path.encode("utf-8")) for path in paths]
        self.tos = [[node_id, path] for node_id, path in zip(self.ids, paths)]
        nodes = {row["name"]: row["node"] for row in doc["capture"]["humanoid"]["bones"]}
        self.bone_index = [at[spec["nodes"][nodes[slot]]["address"]] if nodes.get(slot, -1) >= 0 else -1
                           for slot in runs.SLOTS]

    def at(self, doc, address):
        return self.transforms[(0, 1 + [node["address"] for node in doc["spec"]["nodes"]].index(address))]


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
        self.assertIn(".Concat(ClipRigs.Concat(FullRigs).Concat(ScopedRigs).SelectMany(rig => ClipCases(rig)))", builder)
        self.assertIn("foreach (var rig in ClipRigs.Concat(FullRigs).Concat(ScopedRigs))", builder)
        # The full-body clip rigs and variants, and each full curve's values.
        table = clips[clips.index("BodyRig[] FullRigs => new[] {"):]
        parsed = {}
        for name, fields in re.findall(r'new BodyRig \{ Name = "([a-z-]+)"([^}]*)\}', table[:table.index("};")]):
            self.assertIn("Full = true, Extras = FullExtras", fields)
            size = re.search(r"Size = ([\d.]+)f", fields)
            parsed[name] = ("armature" if "Armature = true" in fields else "", float(size.group(1)) if size else 1)
        self.assertEqual(parsed, full.FULL_RIGS)
        self.assertEqual(tuple(re.findall(r'"([a-z0-9-]+)"', re.search(r"FullVariants = \{(.*?)\};", clips).group(1))),
                         full.FULL_VARIANTS)
        self.assertIn("static string[] FullExtras => Face.Concat(FingerBones).ToArray();", clips)
        self.assertEqual(set(full.FULL_EXTRAS), {"LeftEye", "RightEye", "Jaw", *muscles.FINGER_BONES})
        # The in-scope rigs: features, sizes, topologies and variants.
        table = clips[clips.index("BodyRig[] ScopedRigs => new[] {"):]
        parsed = {}
        for name, fields in re.findall(r'new BodyRig \{ Name = "([a-z-]+)"([^}]*)\}', table[:table.index("}.Concat(")]):
            self.assertIn("Full = true, Scoped = true, Extras = FullExtras", fields)
            size = re.search(r"Size = ([\d.]+)f", fields)
            variants = re.search(r"Variants = (\w+|new\[\] \{ ChainVariant.Name)", fields).group(1)
            parsed[name] = ("tall" if "Tall = true" in fields else "compact",
                            next((v for k, v in features.items() if k in fields), ""),
                            float(size.group(1)) if size else 1, {"E5Admitted": full.SCOPED_ADMITTED,
                                                                 "FullVariants": full.FULL_VARIANTS}.get(
                                variants, (body.CHAIN_VARIANT,)))
        topologies = re.search(r"ScopedTopologies = \{(.*?)\};", clips, re.S).group(1)
        self.assertEqual(tuple(re.findall(r'"([a-z-]+)"', topologies)), full.SCOPED_TOPOLOGIES)
        self.assertIn("Noncoplanar = topology.Noncoplanar, Variants = E5Admitted", clips)
        # The face configurations, on the compact and tall rigs, before the topologies (the builder's order).
        self.assertIn('("no-face", new string[0]), ("eyes", new[] { "LeftEye", "RightEye" }), ("jaw", new[] { "Jaw" })',
                      clips)
        self.assertEqual(full.FACES, {"no-face": (), "eyes": ("LeftEye", "RightEye"), "jaw": ("Jaw",)})
        self.assertEqual(tuple(re.findall(r'"([a-z0-9-]+)"', re.search(r"FaceVariants = \{(.*?)\};", clips).group(1))),
                         full.FACE_VARIANTS)
        self.assertIn('}.Concat(new[] { "compact", "tall" }.SelectMany(rig => FaceRigs.Select(face => new BodyRig {\n'
                      '        Name = rig + "-" + face.name + "-scoped", Full = true, Scoped = true, '
                      'Extras = face.bones.Concat(FingerBones).ToArray(),\n'
                      '        Tall = rig == "tall", Variants = FaceVariants\n    }))).Concat(ScopedTopologies', clips)
        parsed |= {f"{rig}-{face}-scoped": (rig, "", 1, full.FACE_VARIANTS) for rig in ("compact", "tall")
                   for face in full.FACES}
        parsed |= {t + "-scoped": (t, "", 1, full.SCOPED_ADMITTED) for t in full.SCOPED_TOPOLOGIES}
        self.assertEqual(list(parsed), list(full.SCOPED_RIGS))
        self.assertEqual(parsed, full.SCOPED_RIGS)
        self.assertEqual(full.mapped_bones("tall-jaw-scoped") - full.mapped_bones("tall-no-face-scoped"), {"Jaw"})
        self.assertEqual(full.expected_absent("compact-eyes-scoped", NAMES),
                         {"Jaw Close", "Jaw Left-Right"})
        self.assertEqual(full.SCOPED_ADMITTED, r11.E5_ADMITTED)
        # Each F5 feature in scope at every scale a chain rig plays it at, but the armature's at F5's own (B4's
        # human-scale range's lower end, which a full-body Avatar must not cross), which plays at 1.25 instead.
        chains = {(f, size) for f, size in r11.CHAIN_FEATURES.values() if f} - {("armature", 1)}
        self.assertEqual({(f, size) for _, f, size, _ in full.SCOPED_RIGS.values() if f}, chains | {("armature", 1.25)})
        # Expected-absent muscles: a muscle whose bone the rig maps is curved, no other.
        self.assertIn(".Where(i => mapped.Contains(HumanTrait.BoneName[HumanTrait.BoneFromMuscle(i)])", clips)
        # Every topology B3 admits but the compact rig's own, as body topology (the face is a leaf B3 ignores).
        self.assertEqual({t for t in r11.E5_TOPOLOGIES if t not in ("eyes", "jaw", "eyes-jaw")}, set(full.SCOPED_TOPOLOGIES))
        amplitudes = re.search(r"ScopedAmplitudes = \{(.*?)\n    \};", clips, re.S).group(1)
        rows = re.findall(r"new\[\] \{ (.*?) \}, (\d+) / (\d+)f", amplitudes)
        self.assertEqual(tuple((tuple(re.findall(r'"([^"]+)"', names)), int(a) / int(b)) for names, a, b in rows),
                         full.SCOPED_AMPLITUDES)
        self.assertIn("static readonly int[] Moduli = { 17, 13, 11, 13, 17 };", clips)
        self.assertIn("((muscle * 7 + 3 + key * 5) % Moduli[key] - (Moduli[key] - 1) / 2) / 8f;", clips)
        self.assertIn("DefaultIfEmpty(.5f)", clips)
        self.assertIn('? parts[0] + "Hand." + parts[1] + "." + parts[2] : name;', clips)
        self.assertIn("BuildMuscleSweeps(build, report);", builder)

    def test_the_recorders_reference_is_each_clips_own_authored_values(self):
        recorder, motion = source("UnityrsFixtureRecording.cs"), source("UnityrsHumanoidMotion.cs")
        self.assertIn("SampleMuscles(animator, UnityrsFixtureBuilder.AuthoredMuscles(authoredClip, time));", recorder)
        self.assertIn("var authoredClip = AssetDatabase.LoadAssetAtPath<AnimationClip>(fixture.Clip);", recorder)
        self.assertNotIn("KnownHumanoidMuscles(time)", recorder)
        self.assertIn('EditorCurveBinding.FloatCurve("", typeof(Animator), MuscleProperty(names[i])));', motion)
        self.assertIn("if (curve != null) vector[i] = curve.Evaluate(time);", motion)
        spine = ("Spine Front-Back", "Spine Left-Right", "Spine Twist Left-Right")
        with round11_docs():
            for rig in full.SCOPED_RIGS:
                name = next(case for case, info in r11.ROUND11_INFO.items() if info["rig"] == rig)
                doc = full_clip(support.body_doc(name, RUN), name)[0]
                authored = body._authored(doc["spec"]["evidence"]["editor"], replay)
                for time, known in ((0, 0), (1, 2)):
                    reference = full.authored_muscles(NAMES, rig, time)
                    with self.subTest(rig=rig, time=time):
                        # The reference is the clip's own key at that time, for every curve it authors.
                        self.assertEqual({prop: replay.f32_from_bits(next(
                            key["value_f32"] for key in authored[prop]["keys"]
                            if replay.f32_from_bits(key["time_f32"]) == time)) for prop in reference}, reference)
                        # The in-scope Spine is not KnownHumanoidMuscles', which the recorder read before.
                        self.assertNotEqual([reference[prop] for prop in spine],
                                            [HUMANOID_SPINE[prop][known] for prop in spine])
        self.assertEqual(full.authored_muscles(NAMES, "compact-scoped", 0)["Spine Front-Back"], -.3125)

    def test_the_admitted_variants_are_freeze_v8s_e4_passing_set(self):
        passing = json.loads(FREEZE.read_text(encoding="utf-8"))["e4"]["passing"]
        compact = {entry.split("/")[0] for entry in passing if entry.split("/")[1] == "compact"}
        self.assertEqual(set(r11.E5_ADMITTED), {variant.removesuffix("-start") for variant in compact})
        self.assertEqual(len(r11.E5_VARIANTS), 21)

    def test_the_cases_are_round_11s_own(self):
        # 15 clip rigs x 21 variants x 2 roots, each new chain rig's targets and clip, the refusal full-body clips
        # (three plain rigs x 4 variants x 2 roots, the armature's chain variant), the in-scope ones (the compact,
        # large, small and tall rigs and nine topologies, each 11 admitted variants x 2 roots; eight F5 feature
        # rigs' chain variant; three face configurations on the compact and tall rigs, each 2 variants x 2 roots)
        # and the four sweeps, in the builder's order.
        scoped = 13 * 11 * 2 + 8 + 2 * 3 * 2 * 2
        self.assertEqual(len(r11.ROUND11_CASES), 15 * 21 * 2 + 5 * 2 + 25 + scoped + 4)
        self.assertEqual(len(r11.E5_CASES), 665 + scoped)
        kinds = [r11.ROUND11_INFO[name]["kind"] for name in r11.ROUND11_CASES]
        self.assertEqual(kinds[:10], ["targets", "clip"] * 5)
        self.assertEqual(kinds[10:], ["clip"] * (655 + scoped) + ["probes"] * 4)
        self.assertEqual(len(r11.ROUND11_BUNDLES), 5 + 15 + 4 + 27 + 4)
        # docs/anim_reference.md's case table accounts for every body case, round 11's included.
        table = (HERE.parents[1] / "docs" / "anim_reference.md").read_text(encoding="utf-8").split("| Cases |")[1]
        counts = re.findall(r"^\| `body-[^|]*\((\d+)\) \|", table.split("\n\n")[0], re.M)
        self.assertEqual(sum(map(int, counts)), len(set(body.BODY_CASES) | set(r11.ROUND11_CASES)))
        self.assertEqual(len(set(body.BODY_CASES) | set(r11.ROUND11_CASES)), 1111)
        # Only the in-scope clips declare the frozen scope; every other clip is refusal evidence.
        declared = {name: info.get("scope") for name, info in r11.ROUND11_INFO.items()}
        self.assertEqual({name for name, value in declared.items() if value == "in"},
                         {name for name in r11.ROUND11_CASES if r11.ROUND11_INFO[name]["rig"] in full.SCOPED_RIGS})
        self.assertEqual({name for name, value in declared.items() if value is None},
                         {*muscles.MUSCLE_SWEEPS, *(f"body-chain-{rig}-targets" for rig in r11.CHAIN_RIGS)})
        self.assertEqual([name for name in r11.ROUND11_CASES if name.startswith("body-clip-armature-scoped")],
                         ["body-clip-armature-scoped-" + body.CHAIN_VARIANT])
        rows = [r11.ROUND11_INFO[name] | dict(name=name) for name in list(r11.ROUND11_CASES)[640:665]]
        self.assertEqual({row["rig"] for row in rows}, set(full.FULL_RIGS))
        self.assertEqual([row["name"] for row in rows if row["rig"] == "armature-large-full"],
                         ["body-clip-armature-large-full-" + body.CHAIN_VARIANT])
        self.assertTrue(set(full.FULL_VARIANTS) <= set(r11.E5_ADMITTED) and body.CHAIN_VARIANT in full.FULL_VARIANTS)
        self.assertEqual(r11.ROUND11_CASES["body-clip-armature-large-full-" + body.CHAIN_VARIANT].probe[0],
                         "HumanoidRig/Armature/Hips/Spine")
        for rig in r11.CLIP_RIGS:
            for variant in r11.E5_VARIANTS:
                for root, suffix in (("identity", ""), ("offset", "-offset")):
                    name = f"body-clip-{rig}-{variant}{suffix}"
                    fixture = r11.ROUND11_CASES[name]
                    self.assertEqual((fixture.bundle, fixture.prefab, fixture.clip, fixture.playback),
                                     (f"body-clips-{rig}", root, f"body-clip-{rig}-{variant}", "animator"))
                    self.assertEqual(r11.ROUND11_INFO[name], dict(kind="clip", rig=rig, variant=variant, root=root,
                                                                  scope="refusal"))
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
        # Only a recorded Built-in run declares the round. Only the Built-in build-only run's dialog says not
        # recorded; the URP and Android runs, which record nothing by design, say ready and to send them.
        builder = source("UnityrsFixtureBuilder.cs")
        self.assertIn('if (run.variant == "builtin" && variant.Record) WriteRound(staging, run.run_id);', builder)
        self.assertIn('bool buildOnly = run.variant == "builtin" && !variant.Record;\n'
                      '            if (failures.Length == 0 && buildOnly)\n'
                      '                EditorUtility.DisplayDialog("unityrs fixtures built, not recorded",', builder)
        self.assertIn('else if (failures.Length == 0 && !variant.Record)\n'
                      '                EditorUtility.DisplayDialog("unityrs fixtures ready",\n'
                      '                    "The " + run.variant + " bundles were built and checked. '
                      'Zip this folder and send it.', builder)
        variants = source("UnityrsFixtureVariants.cs")
        self.assertEqual(re.findall(r'Name = "(\w+)"', variants), ["builtin", "urp", "android"])
        self.assertNotIn("Record = true", variants)
        # Both humanoid refusal probes are required, as completion requires them.
        self.assertIn('report.AddDeferred(name, "The coverage fixture was not prepared.", '
                      'name == TwistCase || name == SettingsCase);', source("UnityrsCoverageFixtures.cs"))
        self.assertIn('return rows.Where(row => row.status == "failed" || required.Contains(row.name) '
                      '&& row.status == "not_covered")', builder)

    def test_the_editor_runs_completions_response_and_twist_checks(self):
        # A sweep whose single-muscle sample leaves its bone still, or an in-scope clip whose recorded pose
        # twists the shoulders past the freeze's limit, fails in the Editor, before it is marked recorded.
        recording = source("UnityrsBodyRecording.cs")
        limit = json.loads(FREEZE.read_text(encoding="utf-8"))["bounds"]["twist_limit"]
        lateral = json.loads(FREEZE.read_text(encoding="utf-8"))["scope"]["lateral_limit"]
        self.assertIn(f"const double LeastTurn = {r11.LEAST_TURN}, TwistLimit = {limit!r}, LateralLimit = {lateral!r};",
                      recording)
        self.assertEqual(tuple(re.findall(r'"([\w -]+)"', re.search(r"LateralMuscles = \{(.*?)\};", recording).group(1))),
                         scope.LATERAL_MUSCLES)
        self.assertIn("var played = Values(body.Samples[k].readback_muscles);", recording)
        # An in-scope clip's curves are authored with both tangents in Unity's Linear mode.
        self.assertIn("scoped ? LinearCurve(BodyTimes, FullValues(muscle, true))", clips_source := source("UnityrsBodyClips.cs"))
        for side in ("Left", "Right"):
            self.assertIn(f"AnimationUtility.SetKey{side}TangentMode(curve, i, AnimationUtility.TangentMode.Linear);",
                          clips_source)
        self.assertEqual(tuple(re.findall(r'"(\w+)"', re.search(r"FrameBones = \{(.*?)\};", recording).group(1))),
                         scope.FRAME_BONES)
        # Fatal for every case but a round 12 one, whose motion checks are diagnostics (Check is Need otherwise).
        self.assertIn('if (fixture.Body.Variant == "muscle" || fixture.Body.Variant == "table")', recording)
        self.assertIn("        if (diagnostics == null)\n        {\n            Need(passed, message);",
                      source("UnityrsRound12Recording.cs"))
        self.assertIn('if (fixture.Body.Kind == "clip" && fixture.Body.Spec.Scoped)', recording)
        self.assertIn("            CheckBodyResponse(capture as HumanCapture, fixture, body);\n"
                      "            SetBodyEvidence(", source("UnityrsFixtureRecording.cs"))
        # A hidden m_IndexArray records with a note, and completion verifies the index offline; a wrong one fails.
        clips, builder = source("UnityrsBodyClips.cs"), source("UnityrsFixtureBuilder.cs")
        self.assertIn("|| indexed && (track.attribute >= index.arraySize", clips)
        self.assertIn('const string IndexUnverified = "index unverified in Editor";', builder)
        self.assertIn("if (!outcome.indexed) report.Note(replay.OutputName, IndexUnverified);",
                      source("UnityrsCoverageFixtures.cs"))

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


# The Spine's curves every humanoid clip keys (UnityrsHumanoidMotion.cs AuthorHumanoidClip): at 0, .5 and 1.
HUMANOID_SPINE = {"Spine Front-Back": (0, .5, -.25), "Spine Left-Right": (0, 0, .375),
                  "Spine Twist Left-Right": (0, 0, -.5)}


def full_clip(doc, name):
    """A full-body clip case's authored curves and built clip: its full curves under their Animator
    properties, and a refusal clip's humanoid Spine curves besides, authored and built."""
    rig = r11.ROUND11_INFO[name]["rig"]
    groups = full.full_groups(list(NAMES), rig)
    clip = support.clip_case(doc, r11.ROUND11_INFO[name]["variant"],
                             groups + ([] if rig in full.SCOPED_RIGS else spine_groups()))
    return doc, clip, groups


def spine_groups():
    """The humanoid Spine curves every humanoid clip keys, as curve groups (authored and compiled)."""
    return [[(prop, body.MUSCLE_ATTRIBUTE + NAMES.index(prop), (0, .5, 1), values)]
            for prop, values in HUMANOID_SPINE.items()]


def spine_clip(doc, variant):
    """A Spine-only clip case (E5's clip rigs, the chain clips): its body curves and the humanoid Spine's."""
    return support.clip_case(doc, variant, spine_groups())


class Round11CheckTests(unittest.TestCase):
    def check(self, name, doc, view=None):
        return r11.check_body(name, body.BODY_CASES[name], doc, view or Built(name, doc), None, replay)

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
        # B4 reads the serialized Avatar's m_Scale (chain.rs `scale`); the captured human scale is not B4's input.
        name = "body-chain-armature-large-targets"
        low, high = r11.H_RANGE
        self.assertEqual([replay.f32_bits(v) for v in r11.H_RANGE], ["3f47f9bb", "3fb7a311"])
        with round11_docs():
            for scale, ok in ((low, True), (high, True), (replay.f32_from_bits("3f47f9ba"), False),
                              (replay.f32_from_bits("3fb7a312"), False)):
                doc = self.doc(name)
                view = Built(name, doc)
                view.human_scale = scale
                doc["capture"]["humanoid"]["human_scale"] = replay.f32_bits(1)
                with self.subTest(scale=scale):
                    if ok:
                        self.assertIsNone(self.check(name, doc, view))
                    else:
                        with self.assertRaisesRegex(ValueError, "outside B4's range"):
                            self.check(name, doc, view)
            # An earlier case and a muscle sweep keep their own human scale.
            for name in ("body-chain-large-targets", "body-probes-muscles-tall"):
                doc = self.doc(name)
                doc["capture"]["humanoid"]["human_scale"] = replay.f32_bits(2)
                self.assertIsNone(self.check(name, doc))


class Round11BuiltTests(unittest.TestCase):
    check = Round11CheckTests.check
    doc = Round11CheckTests.doc

    def test_refuses_a_built_chain_b4_refuses(self):
        # B4's rule on the bundle's serialized Transforms (chain.rs animator_chain), not the captured rest alone.
        def ulp(value, k=1):
            return replay.f32_from_bits(f"{int(replay.f32_bits(value), 16) + k:08x}")

        name = "body-chain-armature-large-targets"
        with round11_docs():
            self.assertIsNone(self.check(name, self.doc(name)))
            for address, field, axis, edit in (
                    ("0/0", "m_LocalPosition", "y", ulp), ("0/0", "m_LocalRotation", "w", ulp),
                    ("0/0", "m_LocalScale", "z", ulp), ("0", "m_LocalPosition", "x", lambda v: v + 2.),
                    ("0", "m_LocalScale", "x", lambda v: 1.25), ("", "m_LocalPosition", "z", lambda v: .5),
                    ("", "m_LocalRotation", "y", lambda v: .1), ("", "m_LocalScale", "y", ulp)):
                doc = self.doc(name)
                view = Built(name, doc)
                row = view.at(doc, address)[field]
                row[axis] = edit(row[axis])
                with self.subTest(address=address, field=field), self.assertRaisesRegex(
                        ValueError, f"the built chain of {name} is not one B4 admits"):
                    self.check(name, doc, view)
            # Either zero's sign is B4's zero.
            doc = self.doc(name)
            view = Built(name, doc)
            view.at(doc, "0/0")["m_LocalPosition"]["x"] = -0.
            view.at(doc, "")["m_LocalRotation"]["y"] = -0.
            self.assertIsNone(self.check(name, doc, view))

    def test_only_a_plain_chain_plays_at_the_offset_placement(self):
        # (A plain chain's offset root passes: test_full_body_clips_carry_every_muscle.)
        with round11_docs():
            for name in ("body-chain-scaled-animator-large-targets", "body-chain-hips-scale-small-targets"):
                doc = self.doc(name)
                view, rig = Built(name, doc), r11.ROUND11_INFO[name]["rig"]
                self.assertIsNone(r11.serialized_chain(name, rig, "identity", doc["spec"], view, replay))
                view.at(doc, "0")["m_LocalPosition"].update(zip("xyz", r11.OFFSET_PLACEMENT[0]))
                view.at(doc, "0")["m_LocalRotation"].update(zip("xyzw", r11.OFFSET_PLACEMENT[1]))
                for root in ("identity", "offset"):
                    with self.subTest(name=name, root=root), self.assertRaisesRegex(ValueError, "not one B4 admits"):
                        r11.serialized_chain(name, rig, root, doc["spec"], view, replay)

    def full_clip(self, name, **kwargs):
        return full_clip(self.doc(name), name, **kwargs)

    def test_full_body_clips_carry_every_muscle(self):
        with round11_docs():
            for name in ("body-clip-compact-full-keep-o0-y0-xz0", "body-clip-large-full-level-offset",
                         "body-clip-armature-large-full-" + body.CHAIN_VARIANT):
                doc, clip, groups = self.full_clip(name)
                self.assertEqual(len(groups), 92)
                with self.subTest(name=name):
                    self.assertIsNotNone(self.check(name, doc, Built(name, doc, clip)))

    def test_refuses_a_full_body_clip_that_drops_or_moves_a_channel(self):
        name = "body-clip-small-full-bake-o1-y1-xz1-offset"
        with round11_docs():
            doc, clip, groups = self.full_clip(name)
            # A finger muscle's slot dropped from the built clip.
            prop, attribute = groups[-1][0][:2]
            clip["m_MuscleClip"]["m_IndexArray"][attribute] = -1
            with self.assertRaisesRegex(ValueError, f"built clip lost its full-body {prop} slot"):
                self.check(name, doc, Built(name, doc, clip))
            # A twist muscle's authored key moved.
            doc, clip, groups = self.full_clip(name)
            prop = "Left Arm Twist In-Out"
            row = next(row for row in doc["spec"]["evidence"]["editor"]["bindings"] if row["property"] == prop)
            row["keys"][2]["value_f32"] = replay.f32_bits(.75)
            with self.assertRaisesRegex(ValueError, f"authored full-body {prop} curve mismatch"):
                self.check(name, doc, Built(name, doc, clip))
            # A clip without the full curves (the Spine's alone) is not a full-body clip.
            doc = self.doc(name)
            clip = support.clip_case(doc, r11.ROUND11_INFO[name]["variant"])
            with self.assertRaisesRegex(ValueError, "authored full-body .* curve mismatch"):
                self.check(name, doc, Built(name, doc, clip))

    def test_the_response_is_from_the_zero_vector_sample(self):
        # A bone the zero vector already turns does not count as a muscle's response.
        name = "body-probes-face"
        with round11_docs():
            doc = self.doc(name)
            node = next(row["node"] for row in doc["capture"]["humanoid"]["bones"] if row["name"] == "Jaw")
            samples = doc["capture"]["samples"]
            turned = [replay.f32_bits(v) for v in support.euler((20, 0, 0), 4)]
            for sample in samples:
                sample["pose"]["transforms"][node]["rotation"] = list(turned)
            with self.assertRaisesRegex(ValueError, "turned no bone in body-probes-face"):
                self.check(name, doc)
            # And a sweep without the zero vector has no baseline.
            doc = self.doc(name)
            samples = doc["capture"]["samples"]
            for sample in samples:
                if all(v in ("00000000", "80000000") for v in sample["muscles_f32"]):
                    sample["muscles_f32"][0] = replay.f32_bits(.5)
            with self.assertRaisesRegex(ValueError, "records no zero-vector sample"):
                self.check(name, doc)


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
