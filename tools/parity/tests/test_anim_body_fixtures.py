"""The optional body-motion fixtures: their inventory mirrors the builder, their rows are checked
fail-closed, and a run without them, or with all of them unrecorded, still completes."""

import copy
import hashlib
import json
import math
import re
import unittest
from pathlib import Path
from unittest import mock

import test_anim_replay_coverage as coverage
from anim_body_support import MUSCLES, body_doc, clip_case
from anim_coverage_support import COVERAGE
from anim_fixture_support import RUN, write

import anim_body_model as model  # noqa: E402  (test_anim_replay puts it on the path)
import anim_fixture_body as body  # noqa: E402
from anim_fixture_round11 import CHAIN_FEATURES  # noqa: E402
import anim_fixture_coverage  # noqa: E402
import anim_fixture_rigs as rigs_module  # noqa: E402
import anim_replay as replay  # noqa: E402
import anim_body_seal as seal  # noqa: E402
import anim_replay_complete as complete  # noqa: E402
from anim_replay_complete import OPTIONAL_CASES, complete_folder  # noqa: E402

EDITOR = Path(__file__).resolve().parents[2] / "fixtures" / "unity" / "Editor"
# A run's body cases from before the range sweeps and their topology rigs.
EARLIER = [name for name in body.BODY_CASES if name not in set().union(*body.LATER_CASES[2:])]


class View:
    def __init__(self, clip):
        self.clip = clip

    def value(self, key):
        return self.clip


def source(name):
    return (EDITOR / name).read_text(encoding="utf-8")


def yaw_gap(doc, k):
    """A recorded sample's arm-leg yaw gap in degrees: the signed angle about 2.2's up from the upper-leg lateral to
    the upper-arm lateral, from the read-back's world positions."""
    nodes = {row["name"]: row["node"] for row in doc["capture"]["humanoid"]["bones"]}
    rows = doc["spec"]["evidence"]["editor"]["body_samples"][k]["nodes"]
    ul, ur, ll, lr = ([replay.f32_from_bits(v) for v in rows[nodes[bone]]["position"]] for bone in
                      ("LeftUpperArm", "RightUpperArm", "LeftUpperLeg", "RightUpperLeg"))
    up = [(a + b - c - d) / 2 for a, b, c, d in zip(ul, ur, ll, lr)]
    up = [v / math.sqrt(sum(u * u for u in up)) for v in up]

    def flat(v):
        along = sum(a * b for a, b in zip(v, up))
        return [a - along * b for a, b in zip(v, up)]

    arm, leg = flat([b - a for a, b in zip(ul, ur)]), flat([b - a for a, b in zip(ll, lr)])
    cross = [leg[1] * arm[2] - leg[2] * arm[1], leg[2] * arm[0] - leg[0] * arm[2], leg[0] * arm[1] - leg[1] * arm[0]]
    return math.degrees(math.atan2(sum(a * b for a, b in zip(cross, up)), sum(a * b for a, b in zip(leg, arm))))


class BodyInventoryTests(unittest.TestCase):
    def test_cases_mirror_the_builder(self):
        self.assertEqual(len(body.BODY_CASES), 124)
        kinds = [info["kind"] for info in body.BODY_INFO.values()]
        self.assertEqual((kinds.count("probes"), kinds.count("targets"), kinds.count("clip")), (34, 8, 82))
        self.assertEqual(body.EXTRA_CASES, {"body-probes-tall-2", "body-probes-tall-3",
                                            *(f"body-probes-compact-{k}" for k in range(2, 6))})
        # The fifth generation: the cases added with the second hold-out rig, absent from older runs.
        self.assertEqual(body.LATER_CASES[-1], {"body-probes-holdout-2", *body.EXTRA_CASES - {"body-probes-tall-2"}})
        self.assertEqual(body.TWIST_CASES, {"body-probes-twist-compact", "body-probes-twist-tall"})
        self.assertEqual(body.RANGE_CASES, {"body-probes-range-compact", "body-probes-range-tall",
                                            "body-probes-range-noncoplanar"})
        self.assertEqual(body.BAND_CASES, {"body-probes-band-compact", "body-probes-band-tall",
                                           "body-probes-band-noncoplanar"})
        self.assertEqual(body.LATER_CASES[3], body.BAND_CASES)
        self.assertLessEqual(set(body.LATER_RIGS), set(body.PROBE_RIGS))
        self.assertEqual(len(body.CLIP_VARIANTS), 39)
        self.assertLessEqual(body.BODY_CASES.keys(), OPTIONAL_CASES)
        self.assertTrue(body.BODY_CASES.keys().isdisjoint(anim_fixture_coverage.COVERAGE_CASES))
        builder = source("UnityrsBodyFixtures.cs")
        rigs = re.findall(r'new BodyRig \{ Name = "([a-z0-9-]+)"', builder)
        self.assertEqual(rigs, [*body.PROBE_RIGS, *CHAIN_FEATURES])  # freeze v8's chain rigs, then round 11's
        # Each hold-out rig, by its topology in the builder's table alone, is inside the frozen scope, with the
        # proportions anim_fixture_rigs gives it.
        for rig in body.HOLDOUT_RIGS:
            holdout = re.search(r'new BodyRig \{ Name = "' + rig + r'"([^}]*)\}', builder).group(1)
            self.assertFalse(re.search(r"Omit|Extras", holdout))
            self.assertEqual(tuple(float(v) for v in re.findall(r"(?:Arms|Legs) = ([\d.]+)f", holdout)),
                             rigs_module.RIG_PROPORTIONS[rig])
            self.assertEqual(body.PROBE_RIGS[rig], ("compact", (), ()))
            model.topology_scope(body.rig_bones(rig))
        self.assertEqual(body.HOLDOUT, body.HOLDOUT_RIGS[-1])
        self.assertIn(f'const string HoldoutRig = "{body.HOLDOUT}";', builder)
        table = builder[builder.index("HoldoutPlan = {"):]
        table = table[:table.index("\n    };")]
        self.assertEqual([tuple((bone, int(source)) for bone, source in re.findall(r'\("([A-Za-z]+)", (-?\d)\)', line))
                          for line in re.findall(r'new\[\] \{ (\(.*?\)) \}', table)],
                         list(rigs_module.holdout_plan(body.HOLDOUT)))
        table = builder[builder.index("ProbeGroups = {"):]
        groups = re.findall(r'new\[\] \{ "([A-Za-z]+)", ((?:"[^"]+"(?:, )?)+) \}', table[:table.index("};")])
        self.assertEqual([(bone, tuple(re.findall(r'"([^"]+)"', muscles))) for bone, muscles in groups],
                         [(bone, tuple(muscles)) for bone, muscles in body.PROBE_GROUPS])
        # The extra probe cases, entry for entry.
        table = builder[builder.index("ExtraProbes = {"):]
        table = table[:table.index("\n    };")]
        cases = re.findall(r'\("([a-z0-9-]+)", "([a-z-]+)", new\[\]\s*\{(.*?)\n        \}\)', table, re.S)
        self.assertEqual({name: rig for name, rig, _ in cases}, body.EXTRA_RIGS)
        for name, _, plan in cases:
            vectors = [[(bone, int(source)) for bone, source in re.findall(r'\("([A-Za-z]+)", (-?\d)\)', line)]
                       for line in re.findall(r'new\[\] \{ (\(.*?\)) \}', plan)]
            self.assertEqual(vectors, [list(sources) for sources in body.EXTRA_PROBES[name]], name)
        # The twist sweeps: their cases, degrees, range, muscles and tilts.
        self.assertEqual(dict(re.findall(r'\("(body-probes-twist-[a-z]+)", "([a-z]+)"\)', builder)), body.TWIST_RIGS)
        degrees = re.search(r"TwistDegrees = \{ ([^}]*) \}", builder).group(1)
        self.assertEqual(tuple(int(v) for v in degrees.split(", ")), body.TWIST_DEGREES)
        self.assertIn(f"const double TwistRange = {body.TWIST_RANGE};", builder)
        muscles = re.search(r"BodyTwistMuscles = \{ ([^}]*) \}", builder).group(1)
        self.assertEqual(tuple(re.findall(r'"([^"]+)"', muscles)), body.TWIST_MUSCLES)
        tilts = re.search(r"TwistTilts = \{ Vector3\.zero, (.*) \};", builder).group(1)
        self.assertEqual(((0, 0, 0), *((int(x), int(y), int(z)) for x, y, z in
                                       re.findall(r"new Vector3\((-?\d+), (-?\d+), (-?\d+)\)", tilts))),
                         body.TWIST_TILTS)
        # The range sweeps: their cases, point count, bones and Sobol direction numbers.
        self.assertEqual(dict(re.findall(r'\("(body-probes-range-[a-z]+)", "([a-z]+)"\)', builder)), body.RANGE_RIGS)
        self.assertIn(f"const int RangePoints = {body.RANGE_POINTS};", builder)
        self.assertIn(f"const int SobolBits = {body.SOBOL_BITS};", builder)
        self.assertIn("int block = RangePoints / TwistTilts.Length;", builder)
        self.assertIn("for (int k = 0; k < RangePoints; k++) tilts.Add(TwistTilts[k / block]);", builder)
        self.assertIn("tilts.Add(TwistTilts[(k - 1) % TwistTilts.Length]);", builder)
        # The band sweeps: their cases, scales and bends, the side bends under each tilt, each required.
        self.assertEqual(dict(re.findall(r'\("(body-probes-band-[a-z]+)", "([a-z]+)"\)', builder)), body.BAND_RIGS)
        self.assertIn("const double BandLateral = 3.0 / 8, BandTwist = 1.0 / 32;", builder)
        self.assertEqual((body.BAND_LATERAL, body.BAND_TWIST), (3 / 8, 1 / 32))
        bends = builder[builder.index("BandBends = {"):]
        bends = re.findall(r"\(([0-9.]+) / ([0-9.]+), (-?)(BandTwist|0)\)", bends[:bends.index("};")])
        self.assertEqual(tuple((float(a) / float(b), (-1 if sign else 1) * (body.BAND_TWIST if twist != "0" else 0))
                               for a, b, sign, twist in bends), body.BAND_BENDS)
        self.assertIn("var scales = band ? new[] { 1, BandLateral, BandTwist } : new[] { 1.0, 1, 1 };", builder)
        self.assertIn("bend[i] = (float)(sign * level);", builder)
        self.assertIn("bend[i + 1] = (float)(sign * twist);", builder)
        self.assertIn('(band.name, band.rig, null), sweep: "band"));', builder)
        self.assertIn('sweep == "band" ? RangeSchedule(mapped, band: true)', builder)
        later = re.search(r"LaterRigs = \{ ([^}]*) \}", builder).group(1)
        self.assertEqual(tuple(re.findall(r'"([^"]+)"', later)), body.LATER_RIGS)
        self.assertIn("() => BuildBodyRig(build, report, rig, chain: false), !LaterRigs.Contains(rig.Name) "
                      "&& rig.Name != HoldoutRig))", builder)
        bones = re.search(r"RangeBones = \{ ([^}]*) \}", builder).group(1)
        self.assertEqual(tuple(re.findall(r'"([^"]+)"', bones)), body.RANGE_BONES)
        table = builder[builder.index("SobolDirections = {"):]
        table = table[:table.index("};")]
        self.assertEqual(tuple((int(d), int(a), tuple(int(v) for v in m.split(", ")))
                               for d, a, m in re.findall(r"\((\d+), (\d+), new\[\] \{ ([\d, ]+) \}\)", table)),
                         body.SOBOL_DIRECTIONS)
        clips = source("UnityrsBodyClips.cs")
        for name in body.CLIP_VARIANTS:
            stem = re.sub(r"-start$", "", name)
            self.assertTrue(stem.startswith(("keep-", "bake-")) or f'"{stem}"' in clips, name)
        self.assertIn('BodyClipBundle = "body-clips"', clips)

    def test_schedules_keep_the_spine_anchors_on_an_exact_grid(self):
        names = list(MUSCLES)
        spine = [names.index(muscle) for muscle in body.SPINE]
        for rig in body.PROBE_RIGS:
            for targets in (False, True):
                points = body.probe_schedule(rig, names, targets)
                times = [t for t, _ in points]
                self.assertEqual(times, sorted(set(times)))
                self.assertTrue(all(t * 65536 == int(t * 65536) for t in times))
                anchors = {t: [vector[i] for i in spine] for t, vector in points if t in (0, .5, 1)}
                self.assertEqual(anchors, {t: list(v) for t, v in body.ANCHORS.items()})
                probes = sum(len(group) for bone, group in body.PROBE_GROUPS if bone in body.rig_bones(rig))
                if rig == body.HOLDOUT and not targets:
                    probes = rigs_module.HOLDOUT_DRAWS - 3
                self.assertEqual(len(points), 6 if targets else probes + 6)
        self.assertNotIn("Chest", body.rig_bones("no-chest"))
        self.assertEqual(body.rig_bones("eyes-jaw") - body.rig_bones("compact"), {"LeftEye", "RightEye", "Jaw"})

    def test_the_hold_out_draws_are_fresh_and_in_scope(self):
        names = list(MUSCLES)
        plan = rigs_module.holdout_plan(body.HOLDOUT)
        self.assertEqual(len(plan), rigs_module.HOLDOUT_DRAWS)
        self.assertTrue(all(len({bone for bone, _ in draw}) == len(draw) in (2, 3) for draw in plan))
        self.assertEqual(plan, rigs_module.holdout_plan(body.HOLDOUT))
        self.assertNotEqual(plan, rigs_module.holdout_plan(body.HOLDOUT, seed=0))
        drawn = [vector for _, vector in body.probe_schedule(body.HOLDOUT, names, False)]
        spent = {tuple(vector) for rig in body.HOLDOUT_RIGS[:-1] for _, vector in body.probe_schedule(rig, names, False)}
        self.assertEqual(len(drawn), len(plan) + 3)
        # No draw repeats a spent hold-out's pose (beyond the shared anchors), bends the spine sideways, or sets
        # a muscle the frame scope refuses.
        anchors = {tuple(vector) for t, vector in body.probe_schedule(body.HOLDOUT, names, False) if t in (0, .5, 1)}
        self.assertFalse(({tuple(vector) for vector in drawn} - anchors) & spent)
        sideways = [names.index(m) for m in names if "Left-Right" in m and m.split()[0] in ("Spine", "Chest", "UpperChest")]
        refused = [names.index(m) for m in seal.FRAME_REFUSED if m in names]
        for vector in set(map(tuple, drawn)) - anchors:
            self.assertFalse(any(vector[i] for i in sideways + refused), vector)

    def test_extra_probes_take_each_bone_from_a_recorded_probe_vector(self):
        names = list(MUSCLES)
        spine = [names.index(muscle) for muscle in body.SPINE]
        for name in body.EXTRA_CASES:
            rig = body.EXTRA_RIGS[name]
            own = [vector for _, vector in body.probe_schedule(rig, names, False)]
            points = body.probe_schedule(rig, names, False, name)
            times = [t for t, _ in points]
            self.assertEqual(times, sorted(set(times)))
            self.assertTrue(all(t * 65536 == int(t * 65536) for t in times))
            self.assertEqual({t: [vector[i] for i in spine] for t, vector in points if t in (0, .5, 1)},
                             {t: list(v) for t, v in body.ANCHORS.items()})
            self.assertEqual(len(points), len(body.EXTRA_PROBES[name]) + 3)
            for t, vector in points:
                for bone, group in body.PROBE_GROUPS:
                    if bone in body.rig_bones(rig):
                        muscles = [names.index(muscle) for muscle in group]
                        self.assertTrue(any([vector[i] for i in muscles] == [other[i] for i in muscles] for other in own),
                                        (name, t, bone))

    def test_twist_sweeps_cross_each_twist_with_each_tilt_on_training_rigs(self):
        names = list(MUSCLES)
        spine = [names.index(muscle) for muscle in body.SPINE]
        twist = [names.index(muscle) for muscle in body.TWIST_MUSCLES]
        self.assertNotIn(body.HOLDOUT, body.TWIST_RIGS.values())
        self.assertEqual(sorted(set(body.TWIST_DEGREES)), sorted(body.TWIST_DEGREES))
        for name, rig in body.TWIST_RIGS.items():
            info = body.BODY_INFO[name]
            self.assertEqual((info["kind"], info["variant"], body.BODY_CASES[name].bundle),
                             ("probes", "twist", name.replace("probes-", "")))
            points = body.probe_schedule(rig, names, False, body.schedule_name(name))
            requests = body.body_requests(name, len(points))
            self.assertEqual(len(points), len(body.TWIST_DEGREES) * len(body.TWIST_TILTS) + 3)
            self.assertLess(len(points), 300)
            times = [t for t, _ in points]
            self.assertEqual(times, sorted(set(times)))
            self.assertTrue(all(t * 65536 == int(t * 65536) for t in times))
            self.assertEqual({t: [vector[i] for i in spine] for t, vector in points if t in (0, .5, 1)},
                             {t: list(v) for t, v in body.ANCHORS.items()})
            swept = set()
            for (t, vector), (offset, tilt) in zip(points, requests, strict=True):
                self.assertEqual(offset, (0, 0, 0))
                if t in (0, .5, 1):
                    self.assertEqual(tilt, (0, 0, 0))
                    continue
                # Both twist muscles together, at degrees over their summed default limits, and nothing else.
                value = vector[twist[0]]
                self.assertEqual(vector[twist[1]], value)
                self.assertTrue(all(v == 0 for i, v in enumerate(vector) if i not in twist))
                swept.add((round(value * body.TWIST_RANGE), tilt))
            self.assertEqual(swept, {(d, tilt) for d in body.TWIST_DEGREES for tilt in body.TWIST_TILTS})


    def test_range_sweeps_fill_each_spine_bones_muscle_cube_on_training_rigs(self):
        # The published start of Joe and Kuo's sequence, then the design: corpus-independent, on the cube.
        self.assertEqual(body.sobol(8, 3)[4:], [(.375, .375, .625), (.875, .875, .125), (.625, .125, .875),
                                                (.125, .625, .375)])
        points = body.sobol(body.RANGE_POINTS, 9)
        self.assertEqual(len(set(points)), body.RANGE_POINTS)
        for d in range(9):
            # Each dimension's first 2^k points take each of the 2^k equal cells once.
            for k in range(1, 9):
                self.assertEqual(sorted(int(point[d] * 2 ** k) for point in points[:2 ** k]), list(range(2 ** k)))
        names = list(MUSCLES)
        spine = [names.index(muscle) for muscle in body.SPINE]
        self.assertNotIn(body.HOLDOUT, body.RANGE_RIGS.values())
        for name, rig in body.RANGE_RIGS.items():
            info = body.BODY_INFO[name]
            self.assertEqual((info["kind"], info["variant"], body.BODY_CASES[name].bundle),
                             ("probes", "range", name.replace("probes-", "")))
            groups = [[names.index(muscle) for muscle in group] for bone, group in body.PROBE_GROUPS
                      if bone in body.RANGE_BONES and bone in body.rig_bones(rig)]
            self.assertEqual(len(groups), 2 if rig == "tall" else 3)
            points = body.probe_schedule(rig, names, False, body.schedule_name(name))
            self.assertEqual(len(points), body.RANGE_POINTS + 7 + 3)
            self.assertLess(len(points), 400)
            times = [t for t, _ in points]
            self.assertEqual(times, sorted(set(times)))
            self.assertTrue(all(t * 65536 == int(t * 65536) for t in times))
            self.assertEqual({t: [vector[i] for i in spine] for t, vector in points if t in (0, .5, 1)},
                             {t: list(v) for t, v in body.ANCHORS.items()})
            moved = [i for group in groups for i in group]
            vectors = [vector for t, vector in points if t not in (0, .5, 1)]
            # Each tilt takes an aligned quarter of the Sobol points, which itself fills the cube (each pair of
            # dimensions reaching every quadrant), and the corners cycle; the anchors stay upright.
            requests = body.body_requests(name, len(points))
            self.assertTrue(all(offset == (0, 0, 0) for offset, _ in requests))
            self.assertEqual([tilt for (t, _), (_, tilt) in zip(points, requests) if t in (0, .5, 1)], [(0, 0, 0)] * 3)
            tilts = [tilt for (t, _), (_, tilt) in zip(points, requests) if t not in (0, .5, 1)]
            self.assertEqual(tilts, body.range_tilts())
            block = body.RANGE_POINTS // 4
            for k, tilt in enumerate(body.TWIST_TILTS):
                self.assertEqual(tilts[k * block:(k + 1) * block], [tilt] * block)
                cell = [[vector[i] > 0 for i in moved] for vector in vectors[k * block:(k + 1) * block]]
                for a in range(len(moved)):
                    for c in range(a + 1, len(moved)):
                        self.assertEqual(len({(row[a], row[c]) for row in cell}), 4, (name, tilt, a, c))
            self.assertEqual(tilts[body.RANGE_POINTS:], [body.TWIST_TILTS[k % 4] for k in range(7)])
            for vector in vectors:
                self.assertTrue(all(v == 0 for i, v in enumerate(vector) if i not in moved))
                self.assertTrue(all(-1 <= vector[i] <= 1 and replay.f32_from_bits(replay.f32_bits(vector[i])) == vector[i]
                                    for i in moved))
            # Every joint corner, every bone at the same corner of its cube.
            corners = {tuple(vector[i] for i in groups[0]) for vector in vectors
                       if all(abs(vector[i]) == 1 for i in moved)
                       and all([vector[i] for i in group] == [vector[i] for i in groups[0]] for group in groups)}
            self.assertEqual(corners, {(a, b, c) for a in (-1, 1) for b in (-1, 1) for c in (-1, 1)})
            self.assertEqual(len(vectors), len({tuple(vector) for vector in vectors}))


class CheckBodyTests(unittest.TestCase):
    def check(self, name, doc, clip=None):
        return body.check_body(name, body.BODY_CASES[name], doc, View(clip), None, replay)

    def assert_refused(self, message, name, doc, clip=None):
        with self.assertRaisesRegex(ValueError, message):
            self.check(name, doc, clip)

    def test_band_sweeps_cross_the_lateral_band_with_tilts_and_yaw_gaps(self):
        names = list(MUSCLES)
        spine = [names.index(muscle) for muscle in body.SPINE]
        self.assertEqual(set(body.BAND_RIGS.values()), set(body.RANGE_RIGS.values()))
        for name, rig in body.BAND_RIGS.items():
            info = body.BODY_INFO[name]
            self.assertEqual((info["kind"], info["variant"], body.BODY_CASES[name].bundle),
                             ("probes", "band", name.replace("probes-", "")))
            groups = [[names.index(muscle) for muscle in group] for bone, group in body.PROBE_GROUPS
                      if bone in body.RANGE_BONES and bone in body.rig_bones(rig)]
            points = body.probe_schedule(rig, names, False, body.schedule_name(name))
            self.assertEqual(len(points), body.RANGE_POINTS + 40 + 3)
            self.assertLessEqual(len(points), 300)
            times = [t for t, _ in points]
            self.assertEqual(times, sorted(set(times)))
            self.assertEqual({t: [vector[i] for i in spine] for t, vector in points if t in (0, .5, 1)},
                             {t: list(v) for t, v in body.ANCHORS.items()})
            requests = body.body_requests(name, len(points))
            self.assertEqual([tilt for (t, _), (_, tilt) in zip(points, requests) if t in (0, .5, 1)], [(0, 0, 0)] * 3)
            pairs = [(vector, tilt) for (t, vector), (_, tilt) in zip(points, requests) if t not in (0, .5, 1)]
            moved = {i for group in groups for i in group}
            for vector, _ in pairs:
                self.assertTrue(all(v == 0 for i, v in enumerate(vector) if i not in moved))
                for group in groups:
                    for k, limit in enumerate((1, body.BAND_LATERAL, body.BAND_TWIST)):
                        value = vector[group[k]]
                        self.assertTrue(abs(value) <= limit and replay.f32_from_bits(replay.f32_bits(value)) == value)
            sobol, bends = pairs[:body.RANGE_POINTS], pairs[body.RANGE_POINTS:]
            self.assertEqual([tilt for _, tilt in sobol], body.range_tilts()[:body.RANGE_POINTS])
            # The side bends: every bend at either sign, all bones alike, Front-Back zero, under each tilt.
            self.assertEqual(sorted((vector[groups[0][1]], vector[groups[0][2]], tilt) for vector, tilt in bends),
                             sorted((sign * level, sign * twist, tilt) for level, twist in body.BAND_BENDS
                                    for sign in (1, -1) for tilt in body.TWIST_TILTS))
            for vector, _ in bends:
                for k in (1, 2):
                    self.assertEqual({vector[group[k]] for group in groups}, {vector[groups[0][k]]})
                self.assertTrue(all(vector[group[0]] == 0 for group in groups))
            # Every bin of the band past the validated domain, [7/64, 1/8) and then each 1/32 up to 3/8, holds
            # samples by their largest Left-Right under every tilt, and their realized arm-leg yaw gaps (the
            # synthetic poses') reach half a degree of either sign there.
            doc = body_doc(name, dict(run_id="run"))
            gaps = [yaw_gap(doc, k) - yaw_gap(doc, 0) for k in range(len(points))]
            for tilt in body.TWIST_TILTS:
                for low, high in ((7 / 64, 1 / 8), *((k / 32, (k + 1) / 32) for k in range(4, 12))):
                    found = [gap for (t, vector), (_, at), gap in zip(points, requests, gaps)
                             if t not in (0, .5, 1) and at == tilt
                             and low < max(abs(vector[group[1]]) for group in groups) < high]
                    self.assertTrue(found and min(found) <= -.5 and max(found) >= .5, (name, tilt, low, found))

    def test_no_authored_muscle_value_is_a_negative_zero(self):
        # The recordings compare f32 bits, and -0 is not +0's. Every probe, target, twist, range and band schedule
        # holds +0 alone.
        names = list(MUSCLES)
        for name, info in body.BODY_INFO.items():
            if info["kind"] in ("probes", "targets"):
                for _, vector in body.probe_schedule(info["rig"], names, info["kind"] == "targets",
                                                     body.schedule_name(name)):
                    self.assertFalse(any(v == 0 and math.copysign(1, v) < 0 for v in vector), name)
        self.assertEqual(replay.f32_bits(body.canonical(-0.)), replay.f32_bits(0))
        # The builder writes a pure bend's twist as sign * 0 in double, -0 at the negative sign; its probe
        # points and every known vector's schedule, pose and bits go through AnimReplayRecorder.Canonical.
        builder = source("UnityrsBodyFixtures.cs")
        bends = builder[builder.index("BandBends = {"):]
        twists = [0. if twist == "0" else (-1. if sign else 1.) * body.BAND_TWIST for sign, twist in
                  re.findall(r"\([0-9.]+ / [0-9.]+, (-?)(BandTwist|0)\)", bends[:bends.index("};")])]
        raw = [sign * twist for twist in twists for sign in (1., -1.)]
        self.assertTrue(any(math.copysign(1, v) < 0 for v in raw if v == 0))
        self.assertFalse(any(math.copysign(1, body.canonical(v)) < 0 for v in raw if v == 0))
        self.assertIn("Muscles = AnimReplayRecorder.Canonical(vector) };", builder)
        recording = source("UnityrsFixtureRecording.cs")
        self.assertIn("if (values[i] == 0f) values[i] = 0f;", recording)
        self.assertIn("muscles = Canonical(fixture.KnownMuscles((float)point.local))", recording)

    def test_accepts_each_kind(self):
        for name in ("body-probes-compact", "body-probes-no-chest", "body-targets-tall-offset",
                     "body-probes-twist-tall", "body-probes-range-tall", "body-probes-no-upper-chest-toes",
                     "body-chain-armature-targets", "body-chain-large-targets", "body-chain-hips-scale-targets",
                     "body-chain-scaled-animator-targets"):
            with self.subTest(name=name):
                self.assertIsNone(self.check(name, body_doc(name, RUN)))
        for name in ("body-clip-keep-o1-y0-xz1-start-offset", "body-clip-height-from-feet-knee-goals",
                     "body-clip-keep-o1-y1-xz1-motion", "body-chain-armature-clip"):
            with self.subTest(name=name):
                doc = body_doc(name, RUN)
                built = clip_case(doc, body.BODY_INFO[name]["variant"])
                settings = self.check(name, doc, built)
                self.assertLessEqual(set(range(7, 14)), set(settings["animator_attributes"]))
                self.assertEqual(settings["serialized"]["m_StartAtOrigin"], True)

    def test_refuses_a_wrong_case_row_or_topology(self):
        doc = body_doc("body-probes-eyes", RUN)
        doc["spec"]["evidence"]["editor"]["body_case"][0]["rig"] = "jaw"
        self.assert_refused("body case row mismatch", "body-probes-eyes", doc)
        doc = body_doc("body-probes-eyes", RUN)
        next(row for row in doc["capture"]["humanoid"]["bones"] if row["name"] == "Jaw")["node"] = 3
        self.assert_refused("captured Avatar topology differs", "body-probes-eyes", doc)
        doc = body_doc("body-probes-eyes", RUN)
        doc["spec"]["evidence"]["editor"]["human_bones"][4]["mass"] = "7f800000"
        self.assert_refused("nonfinite human_bones mass of RightLowerLeg in body-probes-eyes", "body-probes-eyes", doc)

    def test_refuses_missing_or_malformed_rows(self):
        name = "body-probes-compact"
        doc = body_doc(name, RUN)
        doc["spec"]["evidence"]["editor"]["body_samples"].pop()
        self.assert_refused("body read-back rows mismatch", name, doc)
        doc = body_doc(name, RUN)
        doc["spec"]["evidence"]["editor"]["body_samples"][2]["readback_rotation"] = [replay.f32_bits(.5)] * 4 + []
        doc["spec"]["evidence"]["editor"]["body_samples"][2]["readback_rotation"][3] = replay.f32_bits(.9)
        self.assert_refused("not a unit quaternion", name, doc)
        doc = body_doc(name, RUN)
        doc["spec"]["evidence"]["editor"]["body_samples"][0]["root_position"] = ["00000000"] * 3
        self.assert_refused("body root rows mismatch", name, doc)

    def test_refuses_missing_muscles_or_node_rows(self):
        name = "body-probes-compact"
        for edit, message in (
                (lambda row: row["readback_muscles"].pop(), "body read-back muscles mismatch"),
                (lambda row: row.pop("nodes"), "body node rows mismatch"),
                (lambda row: row["nodes"].pop(), "body node rows mismatch"),
                (lambda row: row["nodes"][4].__setitem__("rotation", [replay.f32_bits(.6)] * 4),
                 "body node rotation is not a unit quaternion"),
                (lambda row: row["nodes"][4].__setitem__("scale", ["7f800000"] * 3), "nonfinite node scale of "),
                (lambda row: row["readback_position"].__setitem__(1, "7fc00000"), "nonfinite readback_position in "),
                (lambda row: row.pop("world_rotation"), "malformed world_rotation in "),
                (lambda row: row["nodes"][2].__setitem__("hips_position", [replay.f32_bits(.1)] * 3),
                 "hips are not at the origin")):
            with self.subTest(message=message):
                doc = body_doc(name, RUN)
                edit(doc["spec"]["evidence"]["editor"]["body_samples"][1])
                self.assert_refused(message, name, doc)

    def test_keeps_nonfinite_values_the_analysis_does_not_read(self):
        # Raw rows: a finger's hierarchy mass, the read-back muscles and the getter rows; and an older
        # recording's Animator.bodyPosition and bodyRotation rows, no longer recorded, are ignored.
        name = "body-clip-keep-o1-y1-xz1-motion"
        doc = body_doc(name, RUN)
        built = clip_case(doc, "keep-o1-y1-xz1-motion")
        editor = doc["spec"]["evidence"]["editor"]
        next(row for row in editor["human_bones"] if row["name"] == "Finger 0")["mass"] = "7fc00000"
        row = editor["body_samples"][1]
        row["readback_muscles"][3] = "ff800000"
        row["root_position"] = ["7fc00000"] * 3
        row["motion_delta_rotation"] = ["7fc00000"] * 4
        row["body_position"], row["body_rotation"] = ["7fc00000"] * 3, ["7fc00000"] * 4
        self.check(name, doc, built)
        # The root-motion transform is read, so it must be finite.
        row["motion_position"][0] = "7f800000"
        self.assert_refused("nonfinite motion_position in " + name, name, doc, built)

    def test_rig_identity_comes_from_the_case_table_not_the_name(self):
        # The orientation-offset setting on the identity rig, and its offset-rig twin.
        self.assertEqual(body.BODY_CASES["body-clip-orientation-offset"].prefab, "identity")
        self.assertEqual(body.BODY_CASES["body-clip-orientation-offset-offset"].prefab, "offset")
        self.assertEqual(body.BODY_CASES["body-clip-orientation-offset-start"].prefab, "identity")
        self.assertEqual(body.BODY_CASES["body-clip-orientation-offset-start-offset"].prefab, "offset")
        for module in ("anim_fixture_contract.py", "anim_humanoid_qualify.py", "anim_replay_complete.py",
                       "anim_fixture_body.py"):
            source = (Path(body.__file__).parent / module).read_text(encoding="utf-8")
            self.assertNotRegex(source, r'(endswith|removesuffix)\("-offset"\)', module)

    def test_restores_a_schedule_time_misparsed_by_one_ulp(self):
        from anim_replay_complete import restore_parsed_clock
        local = 61451 / 65536  # 0.9376678466796875: 2022.3's JsonUtility read it one ulp up
        spec = dict(start=0., stop=1., source_clock="widened", schedule=[dict(local=local), dict(local=.5)])

        def capture(recorded, f32=None):
            return dict(samples=[dict(local=recorded, source=recorded, local_f32=f32 or replay.f32_bits(local),
                                      effective_source=local), dict(local=.5, source=.5, local_f32=replay.f32_bits(.5),
                                                                    effective_source=.5)])
        misparsed = capture(math.nextafter(local, 2))
        restore_parsed_clock(spec, misparsed, replay)
        self.assertEqual((misparsed["samples"][0]["local"], misparsed["samples"][0]["source"]), (local, local))
        # Two ulps off, or a different f32 sample time, stays and is refused by the clock check.
        for recorded, f32 in ((math.nextafter(math.nextafter(local, 2), 2), None),
                              (math.nextafter(local, 2), replay.f32_bits(.9))):
            kept = capture(recorded, f32)
            restore_parsed_clock(spec, kept, replay)
            self.assertEqual(kept["samples"][0]["local"], recorded)

    def test_a_clip_needs_its_root_motion_rows(self):
        name = "body-clip-keep-o1-y1-xz1-motion"
        doc = body_doc(name, RUN)
        built = clip_case(doc, "keep-o1-y1-xz1-motion")
        self.check(name, doc, built)
        for field in ("motion_position", "motion_delta_rotation"):
            with self.subTest(field=field):
                changed = copy.deepcopy(doc)
                changed["spec"]["evidence"]["editor"]["body_samples"][0][field] = []
                self.assert_refused("body root rows mismatch", name, changed, built)
        # A known-vector case has no playback and so no root-motion rows.
        doc = body_doc("body-probes-compact", RUN)
        doc["spec"]["evidence"]["editor"]["body_samples"][0]["motion_position"] = ["00000000"] * 3
        self.assert_refused("body root rows mismatch", "body-probes-compact", doc)

    def test_refuses_a_schedule_or_target_off_the_contract(self):
        name = "body-targets-compact"
        doc = body_doc(name, RUN)
        doc["spec"]["schedule"][1]["muscles"][0] += .125
        self.assert_refused("body known-vector schedule mismatch", name, doc)
        doc = body_doc(name, RUN)
        doc["spec"]["evidence"]["editor"]["body_targets"][1]["position"][0] = replay.f32_bits(.5)
        self.assert_refused("applied body target differs", name, doc)
        doc = body_doc(name, RUN)
        doc["spec"]["evidence"]["editor"]["body_targets"][0]["tilt"] = ["00000000"] * 3
        self.assert_refused("body target request mismatch", name, doc)
        # A twist sweep's tilted point: untilted, or its applied body left upright.
        name = "body-probes-twist-compact"
        last = len(body.TWIST_DEGREES) * len(body.TWIST_TILTS) + 1
        doc = body_doc(name, RUN)
        targets = doc["spec"]["evidence"]["editor"]["body_targets"]
        self.assertNotEqual(targets[last]["tilt"], ["00000000"] * 3)
        targets[last]["tilt"] = ["00000000"] * 3
        self.assert_refused("body target request mismatch", name, doc)
        doc = body_doc(name, RUN)
        doc["spec"]["evidence"]["editor"]["body_targets"][last]["rotation"] = [replay.f32_bits(v) for v in (0, 0, 0, 1)]
        self.assert_refused("applied body target differs", name, doc)

    def test_refuses_a_chain_off_its_rig(self):
        name = "body-chain-hips-scale-targets"
        doc = body_doc(name, RUN)
        for pose in (doc["capture"]["rest"],):
            pose["transforms"][2]["scale"] = [replay.f32_bits(1)] * 3
        self.assert_refused("body rig chain differs", name, doc)

    def test_refuses_clip_settings_or_curves_off_the_variant(self):
        name = "body-clip-bake-o1-y0-xz0-offset"
        doc = body_doc(name, RUN)
        built = clip_case(doc, "bake-o1-y0-xz0")
        changed = copy.deepcopy(doc)
        changed["capture"]["clip_settings"]["loopBlendOrientation"] = False
        self.assert_refused("captured clip settings differ", name, changed, built)
        changed = copy.deepcopy(built)
        changed["m_MuscleClip"]["m_LoopBlendOrientation"] = 0
        self.assert_refused("built clip root settings differ", name, doc, changed)
        changed = copy.deepcopy(doc)
        changed["spec"]["evidence"]["editor"]["bindings"][0]["keys"][2]["value_f32"] = replay.f32_bits(.5)
        self.assert_refused("authored body curve mismatch", name, changed, built)
        changed = copy.deepcopy(doc)
        changed["spec"]["evidence"]["editor"]["body_targets"] = [{}]
        self.assert_refused("known-vector targets", name, changed, built)


    def test_a_dense_slot_interpolates_between_frames(self):
        # The last streamed slot (the right knee) moves to a dense clip at 1.5 frames per second, so the key
        # at .5 lies between frames 0 and 2/3 and only linear interpolation reproduces it.
        name = "body-clip-height-from-feet-knee"
        doc = body_doc(name, RUN)
        built = clip_case(doc, "height-from-feet-knee")
        stream = built["m_MuscleClip"]["m_Clip"]["data"]["m_StreamedClip"]
        last = stream["curveCount"] - 1
        words, kept, cursor = stream["data"], [], 0
        while cursor < len(words):
            time, count = words[cursor:cursor + 2]
            entries = [words[i:i + 5] for i in range(cursor + 2, cursor + 2 + 5 * count, 5) if words[i] != last]
            kept += [time, len(entries)] + [word for entry in entries for word in entry]
            cursor += 2 + 5 * count
        a, b, c = body.KNEE["Right Lower Leg Stretch"]
        second = a + (b - a) / .75
        frames = [a, second, 2 * c - second]
        stream.update(curveCount=last, data=kept)
        built["m_MuscleClip"]["m_Clip"]["data"]["m_DenseClip"] = dict(m_CurveCount=1, m_SampleArray=frames,
                                                                      m_BeginTime=0., m_SampleRate=1.5)
        self.assertTrue(self.check(name, doc, built))
        frames[1] += .01
        self.assert_refused("built clip lost its", name, doc, built)

    def test_a_variant_counts_only_with_its_built_distinguishing_slots(self):
        self.assertEqual(body.CHAIN_VARIANT, "keep-o1-y1-xz1-motion")
        self.assertTrue(body.CLIP_VARIANTS[body.CHAIN_VARIANT]["motion"])
        self.assertEqual(len({y for _, y, _ in body.MOTION_PATH}), len(body.MOTION_PATH))
        self.assertTrue(all(y for _, y, _ in body.MOTION_PATH))
        for name, attribute in (("body-clip-keep-o1-y1-xz1-motion", 1), ("body-clip-height-from-feet-knee-goals", 15),
                                ("body-clip-height-from-feet-knee", None)):
            doc = body_doc(name, RUN)
            built = clip_case(doc, body.BODY_INFO[name]["variant"])
            bindings = built["m_ClipBindingConstant"]["genericBindings"]
            if attribute is None:
                # Drop the left knee's stretch slot: the slot count no longer matches the bindings.
                attribute = 42 + list(MUSCLES).index("Left Lower Leg Stretch")
            lost = copy.deepcopy(built)
            lost["m_ClipBindingConstant"]["genericBindings"] = [row for row in bindings if row["attribute"] != attribute]
            self.assert_refused("built clip lost its", name, doc, lost)
            # An Editor curve copy does not stand in for the runtime slot.
            variant = body.CLIP_VARIANTS[body.BODY_INFO[name]["variant"]]
            prop, times, values = next((prop, times, values) for group in body.distinguishing_curves(variant, list(MUSCLES))
                                       for prop, slot, times, values in group if slot == attribute)
            lost["m_EditorCurves"] = lost["m_FloatCurves"] = [dict(path="", classID=95, attribute=prop, curve=dict(
                m_Curve=[dict(time=t, value=v) for t, v in zip(times, values)]))]
            self.assert_refused("built clip lost its", name, doc, lost)
            # The DOF absent from m_IndexArray, or pointing at another DOF's slot.
            for value in (-1, (built["m_MuscleClip"]["m_IndexArray"][attribute] + 1) % len(bindings)):
                moved = copy.deepcopy(built)
                moved["m_MuscleClip"]["m_IndexArray"][attribute] = value
                self.assert_refused("built clip lost its", name, doc, moved)
            # The same slot with another value at its middle key.
            slot = [row["attribute"] for row in bindings].index(attribute)
            changed = copy.deepcopy(built)
            data = changed["m_MuscleClip"]["m_Clip"]["data"]["m_StreamedClip"]["data"]
            cursor, keys = 0, 0
            while data[cursor] != 0x7f800000:
                count = data[cursor + 1]
                for i in range(cursor + 2, cursor + 2 + 5 * count, 5):
                    if data[i] == slot:
                        keys += 1
                        if keys == 2:
                            data[i + 4] = int(replay.f32_bits(.75), 16)
                cursor += 2 + 5 * count
            self.assert_refused("built clip lost its", name, doc, changed)
        name = "body-clip-keep-o1-y1-xz1-motion"
        doc = body_doc(name, RUN)
        built = clip_case(doc, body.BODY_INFO[name]["variant"])
        motion = next(row for row in doc["spec"]["evidence"]["editor"]["bindings"] if row["property"] == "MotionT.y")
        motion["keys"][2]["value_f32"] = replay.f32_bits(0)
        self.assert_refused("authored MotionT.x curve mismatch", name, doc, built)


class BodyRunTests(unittest.TestCase):
    # The coverage completion run, with or without the body rows.
    setUp = coverage.CoverageCompletionTests.setUp
    complete = coverage.CoverageCompletionTests.complete
    assert_refused = coverage.CoverageCompletionTests.assert_refused

    def run_with(self, names, statuses=None, **kwargs):
        self.coverage = coverage.coverage_archives(self.bundles, self.inspect)
        self.statuses |= statuses or {}
        with mock.patch.object(coverage, "DEFERRED", (*coverage.DEFERRED, *names)):
            return self.complete(**kwargs)

    def test_a_run_with_every_body_case_unrecorded_completes(self):
        # Every body case but the later training rigs may go unrecorded (from before them, none is listed).
        result = self.run_with(EARLIER)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("body-clip-level-offset: not_covered (Unity refused it)", result.stdout)

    def test_a_later_training_rig_unrecorded_fails(self):
        # The UpperChest-and-Toes rigs are required whenever the run has their generation: an Avatar Unity
        # refused is an error, even with their rig's own probe case unrecorded, and each is named.
        self.assertEqual({body.primary_probes(rig) for rig in body.LATER_RIGS} & set(EARLIER), set())
        result = self.run_with(body.BODY_CASES)
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("training rigs not recorded: body-probes-no-upper-chest-left-toes (not_covered), "
                      "body-probes-no-upper-chest-right-toes (not_covered), "
                      "body-probes-no-upper-chest-toes (not_covered)", result.stderr)
        self.assertNotIn("required once body-probes-no-upper-chest", result.stderr)
        result = self.run_with(body.BODY_CASES, {"body-probes-no-upper-chest-toes": "failed"})
        self.assertIn("body-probes-no-upper-chest-toes: failed", result.stderr)
        self.assertIn("training rigs not recorded: body-probes-no-upper-chest-left-toes (not_covered), "
                      "body-probes-no-upper-chest-right-toes (not_covered), body-probes-no-upper-chest-toes "
                      "(failed)", result.stderr)

    def test_a_run_before_the_extra_probes_completes(self):
        # A run with every other body row and none for the extra probe cases is from before them.
        result = self.run_with([name for name in EARLIER if name not in body.EXTRA_CASES])
        self.assertEqual(result.returncode, 0, result.stderr)
        missing = sorted(body.EXTRA_CASES)[0]
        result = self.run_with([name for name in body.BODY_CASES if name != "body-probes-holdout"])
        self.assertIn("missing run.json case: body-probes-holdout", result.stderr)
        self.assertNotIn(missing, result.stderr)

    def test_a_run_before_the_twist_sweeps_completes(self):
        # A run with the extra probe cases and none of the twist sweeps is from between them; one holding a
        # single twist sweep is from after, and lacks the other.
        result = self.run_with([name for name in EARLIER if name not in body.TWIST_CASES])
        self.assertEqual(result.returncode, 0, result.stderr)
        result = self.run_with([name for name in body.BODY_CASES if name != "body-probes-twist-tall"])
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("missing run.json case: body-probes-twist-tall", result.stderr)

    def test_a_run_before_the_range_sweeps_completes(self):
        # A run with the twist sweeps and none of the range sweeps or their topology rigs is from between them.
        later = body.LATER_CASES[2]
        self.assertEqual(later, body.RANGE_CASES | {"body-probes-" + rig for rig in body.LATER_RIGS})
        result = self.run_with([name for name in EARLIER if name not in later])
        self.assertEqual(result.returncode, 0, result.stderr)
        result = self.run_with([name for name in EARLIER + sorted(later) if name != "body-probes-no-upper-chest-toes"])
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("missing run.json case: body-probes-no-upper-chest-toes", result.stderr)

    def test_a_band_sweep_unrecorded_fails_whatever_its_rig_did(self):
        # Every band sweep is required once the run has them, even with its rig's own probe case unrecorded
        # (an Avatar Unity refused is an error), and each is named.
        result = self.run_with(body.BODY_CASES)
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertEqual({body.primary_probes(rig) for rig in body.BAND_RIGS.values()} & set(EARLIER),
                         {body.primary_probes(rig) for rig in body.BAND_RIGS.values()})
        self.assertIn("band sweeps not recorded: body-probes-band-compact (not_covered), "
                      "body-probes-band-noncoplanar (not_covered), body-probes-band-tall (not_covered)", result.stderr)
        self.assertNotIn("required once body-probes-compact", result.stderr)
        result = self.run_with(body.BODY_CASES, {"body-probes-band-tall": "failed"})
        self.assertIn("body-probes-band-tall (failed)", result.stderr)

    def test_a_run_before_the_band_sweeps_lists_none_of_them(self):
        # The band sweeps are a generation of their own: a run without them is from before them, and one holding
        # a single band sweep lacks the others.
        result = self.run_with([name for name in body.BODY_CASES if name not in body.BAND_CASES])
        self.assertNotIn("body-probes-band", result.stderr)
        result = self.run_with([name for name in body.BODY_CASES if name != "body-probes-band-tall"])
        self.assertIn("missing run.json case: body-probes-band-tall", result.stderr)

    def test_an_extra_probe_case_is_required_once_its_rig_records(self):
        for name in (sorted(body.EXTRA_CASES)[0], "body-probes-twist-compact", "body-probes-range-noncoplanar",
                     "body-probes-band-tall"):
            primary = body.primary_probes(body.BODY_INFO[name]["rig"])
            result = self.run_with(body.BODY_CASES, {primary: "recorded"})
            self.assertEqual(result.returncode, 1, result.stdout)
            self.assertIn(f"{name}: required once {primary} records, but not_covered", result.stderr)

    def test_a_run_lists_all_body_cases_or_none(self):
        missing = "body-probes-holdout"
        result = self.run_with([name for name in body.BODY_CASES if name != missing])
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn(f"missing run.json case: {missing}", result.stderr)

    def test_a_failed_or_unwritten_body_case_fails_the_run(self):
        name = "body-targets-tall"
        result = self.run_with(body.BODY_CASES, {name: "failed"})
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn(f"{name}: failed (Unity refused it)", result.stderr)
        self.statuses[name] = "recorded"
        result = self.run_with(body.BODY_CASES)
        self.assertIn(f"{name}: missing replays/body/{name}.pending.json", result.stderr)

    def test_refuses_an_unlisted_body_recording_or_bundle(self):
        (self.replays / "body").mkdir()
        write(self.replays / "body" / "body-probes-jaw.pending.json", body_doc("body-probes-jaw", RUN))
        result = self.run_with(EARLIER)
        self.assertIn("unexpected pending recording: replays/body/body-probes-jaw.pending.json", result.stderr)
        (self.replays / "body" / "body-probes-jaw.pending.json").unlink()
        (self.bundles / "body-jaw").write_bytes(b"UnityFS\0")
        self.assertEqual(self.run_with(EARLIER).returncode, 0)
        (self.bundles / "body-unknown").write_bytes(b"UnityFS\0")
        result = self.run_with(EARLIER)
        self.assertIn("unexpected fixture bundle inventory", result.stderr)

    def test_the_hold_out_stays_sealed_until_a_freeze_unseals_it(self):
        name, spent = body.primary_probes(body.HOLDOUT), body.primary_probes(body.HOLDOUT_RIGS[0])
        (self.replays / "body").mkdir()
        recording, bundle = self.replays / "body" / f"{name}.pending.json", self.bundles / body.BODY_CASES[name].bundle
        # No file of either hold-out can be read: completion that opens one fails. A spent hold-out stays
        # sealed for good, even once E3b claims the current one's shot.
        paths = [recording, bundle, self.replays / "body" / f"{spent}.pending.json", self.bundles / body.BODY_CASES[spent].bundle]
        for path in paths:
            path.write_bytes(b"{")
            path.chmod(0)
            self.addCleanup(path.chmod, 0o600)
        result = self.run_with([*EARLIER, *body.LATER_CASES[-1]], {name: "recorded", spent: "recorded"})
        self.assertEqual(result.returncode, 0, result.stderr)
        for case in (name, spent):
            self.assertIn(f"Sealed hold-out case, not opened: {case}", result.stdout)
        self.assertFalse(recording.with_name(f"{name}.json").exists())
        # Only E3b opens it, with its freeze's sha256 and only once that freeze's shot is claimed; never by a path.
        with self.assertRaisesRegex(ValueError, "only through E3b's claim"):
            complete_folder(self.folder, "unityrs", self.inspect, replay, self.folder / "freeze.json")
        ledger, key = self.folder.parent / "ledger", "a" * 64
        with mock.patch.object(seal, "LEDGER_DIR", ledger):
            with self.assertRaisesRegex(ValueError, "only after E3b claims its shot"):
                complete_folder(self.folder, "unityrs", self.inspect, replay, key)
            seal.claim(key)
            with self.assertRaisesRegex(ValueError, f"{name}.pending.json: .*Permission denied"):
                complete_folder(self.folder, "unityrs", self.inspect, replay, key)
            # Validated, the hold-out recording is returned with the sha256 of the bytes completion read.
            recording.chmod(0o600)
            doc = body_doc(name, RUN)
            data = json.dumps(doc).encode("utf-8")
            recording.write_bytes(data)
            original = complete.complete_recording

            def validated(case, *args):
                return copy.deepcopy(doc) if case == name else original(case, *args)

            with mock.patch.object(complete, "complete_recording", validated):
                result = complete_folder(self.folder, "unityrs", self.inspect, replay, key)
        self.assertEqual(result[2], [(name, hashlib.sha256(data).hexdigest(), doc)])
        self.assertFalse(recording.with_name(f"{name}.json").exists())

    def test_an_ordinary_case_claiming_body_rows_is_refused(self):
        self.coverage = coverage.coverage_archives(self.bundles, self.inspect)
        self.coverage[COVERAGE[0]]["spec"]["evidence"]["editor"]["body_case"] = [{"kind": "probes"}]
        self.assert_refused("claims body fixture evidence")


if __name__ == "__main__":
    unittest.main()
