"""Round 12's generated tables (anim_round12_tables) and the C# file that carries them (UnityrsRound12Tables.cs).

The builder reads the document from the C# file and re-derives its schedules from its own code; these tests require
the committed C# file to be byte-identical to the generator's output, its pieces to concatenate to the canonical
document, and the document to be what the twist tables, B5a's sweeps and round 11's clips give. A run's authored.json
(the builder's curves, as it authored them) is checked against the tables by `check_authored`: a run authoring what
the tables say passes, and a wrong key value, another interpolation, a weighted key, another tangent mode, a duplicate,
missing or extra binding, a wrong body curve, or a limb twist back in a twin is a problem. The tables carry the
curves themselves, so the committed C# file pins their values and slopes."""

import hashlib
import json
import struct
import unittest

import test_anim_replay_coverage  # noqa: F401  (puts tools/parity/tests' parent on the path)

import anim_fixture_body as body  # noqa: E402
import anim_fixture_round11 as r11  # noqa: E402
import anim_muscle_confirm as confirm  # noqa: E402
import anim_round12_tables as r12  # noqa: E402
import anim_twist_tables as tables  # noqa: E402
from anim_fixture_full import full_groups  # noqa: E402
from anim_fixture_muscles import LIMB_TWISTS, groups, muscle_vectors  # noqa: E402


def f32(bits):
    return struct.unpack(">f", bytes.fromhex(bits))[0]


class GeneratedFileTests(unittest.TestCase):
    def test_the_committed_csharp_tables_are_the_generators(self):
        source = r12.CS_PATH.read_text(encoding="utf-8")
        data = r12.canonical()
        self.assertEqual(source, r12.render_cs(data))
        self.assertEqual(r12.parse_cs(source), data)
        self.assertIn(f'Round12TablesSha256 = "{hashlib.sha256(data).hexdigest()}"', source)
        self.assertTrue(data.isascii())


class DocumentTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.doc = r12.document()
        cls.cases = {row["name"]: row for row in cls.doc["cases"]}
        cls.sets = {row["name"]: row["vectors"] for row in cls.doc["sets"]}

    def test_the_families_and_their_samples(self):
        counts, samples = {}, {}
        for row in self.doc["cases"]:
            counts[row["family"]] = counts.get(row["family"], 0) + 1
            samples[row["family"]] = samples.get(row["family"], 0) + len(row["times"])
        self.assertEqual(counts, {"sweep": 2, "probe": 22, "toe-absent": 2, "subset": 6, "twist-clip": 318, "twin": 318})
        self.assertEqual(samples, {"sweep": 1078, "probe": 3532, "toe-absent": 64, "subset": 1350, "twist-clip": 2862,
                                   "twin": 2862})
        self.assertEqual(len(self.cases), len(self.doc["cases"]))
        self.assertEqual(self.doc["tables_digest"], tables.digest())

    def test_every_time_is_a_distinct_increasing_f32(self):
        for row in self.doc["cases"]:
            times = [f32(bits) for bits in row["times"]]
            self.assertEqual(times, sorted(set(times)), row["name"])
            self.assertEqual(times[0], 0., row["name"])

    def test_the_setHumanPose_schedules(self):
        # Scheduled's times (anim_fixture_body._scheduled), with each vector at anim_twist_tables.sample_index.
        for row in self.doc["cases"]:
            if row["family"] in ("sweep", "probe", "toe-absent"):
                vectors = self.sets[row["set"]]
                times = [f32(bits) for bits in row["times"]]
                self.assertEqual(times, r12.scheduled(len(vectors)))
                first = (len(vectors) + 1) // 2
                self.assertEqual([times[0], times[first + 1], times[-1]], [0., .5, 1.])
                inner = [t for t, _ in body._scheduled([{}] * len(vectors), {m: m for m in body.SPINE}, 95)][1:-1]
                self.assertEqual([times[tables.sample_index(k, len(vectors))] for k in range(len(vectors))],
                                 [t for t in inner if t != .5])

    def test_the_probe_vectors_are_the_twist_tables(self):
        names = {m: n for n, m in tables.NAMES.items()}
        for name, make in tables.SETS.items():
            want = [sorted((names[m], v) for m, v in vector.items()) for vector in make()]
            got = [sorted(zip(row["muscles"], (f32(b) for b in row["bits"]))) for row in self.sets[name]]
            self.assertEqual(got, want, name)
        for case, (rig, tuple_name, kind, part) in tables.cases().items():
            row = self.cases[case]
            self.assertEqual((row["rig"], row["tuple"], row["set"], row["partition"]), (rig, tuple_name, kind, part))
        self.assertEqual({row["name"]: [f32(b) for b in row["bits"]] for row in self.doc["tuples"]},
                         {name: list(values) for name, values in tables.TUPLES.items()})

    def test_the_sweeps_are_b5a_and_twist_free_joint_vectors(self):
        twists = {muscle for muscle, _ in LIMB_TWISTS}
        for rig, total in (("compact", 552), ("tall", 520)):
            rows = [dict(zip(row["muscles"], map(f32, row["bits"]))) for row in self.sets[f"sweep-{rig}"]]
            self.assertEqual(len(rows), total)
            name = f"body-probes-muscles-{rig}"
            index = {m: m for _, group in body.PROBE_GROUPS for m in group} | {m: m for m in twists}
            b5a = muscle_vectors(name, index, body.PROBE_GROUPS, r11.rig_bones(rig))
            self.assertEqual(rows[:len(b5a)], [{m: r12.f32(v) for m, v in vector.items()} for vector in b5a])
            extras = rows[len(b5a):]
            self.assertEqual(len(extras), 20)
            self.assertFalse(any(twists & set(vector) for vector in extras))
            limb_groups = [group for _, group in groups(name, body.PROBE_GROUPS, r11.rig_bones(rig)) if twists & set(group)]
            self.assertEqual([set(vector) for vector in extras[::2]], [set(group) - twists for group in limb_groups])
            self.assertEqual(self.cases[f"body-twist-sweep-{rig}"]["tuple"], "t0")

    def test_the_clip_subset(self):
        vectors = self.sets["subset"]
        self.assertEqual(vectors[0], {"muscles": [], "bits": []})
        self.assertEqual(len(vectors), 113)
        keys = [k / 16 for k in range(113)]
        for rig, tuple_name in r12.SUBSET:
            row = self.cases[f"body-twist-clip-{rig}-{tuple_name}"]
            times = [f32(bits) for bits in row["times"]]
            self.assertEqual(times, list(confirm.clip_times(keys)))
            self.assertEqual(len(times), 225)
            self.assertIn(.5, times)
            self.assertTrue(set(keys) <= set(times))

    def test_the_full_body_clips_and_their_twins(self):
        scoped = [case for case, info in r11.ROUND11_INFO.items() if info["kind"] == "clip" and info.get("scope") == "in"]
        for family, tag, tuple_name in (("twist-clip", "twist", "t5"), ("twin", "twin", "t0")):
            rows = [row for row in self.doc["cases"] if row["family"] == family]
            self.assertEqual(len(rows), 318)
            for row, case in zip(rows, scoped):
                info = r11.ROUND11_INFO[case]
                self.assertEqual(row["name"], case.replace(f"body-clip-{info['rig']}-", f"body-clip-{info['rig']}-{tag}-"))
                self.assertEqual((row["rig"], row["tuple"], row["set"]), (info["rig"], tuple_name, ""))
                self.assertEqual([f32(b) for b in row["times"]],
                                 list(confirm.clip_times(confirm.clip_keys(info["rig"], info["variant"]))))


def authored(doc, names):
    """authored.json as a run authoring the tables' curves writes it: muscle curves on Linear tangents, body and
    motion curves on PolyLine's (Free) tangents, every key unweighted."""
    rows = [row for row in doc["cases"] if row["bindings"]]
    variants = {name: variant for name, _, _, variant, _ in r12.full_clips()}
    clips, done = [], set()
    for row in rows:
        if row["name"] in done:
            continue
        cases = [row["name"]] + ([row["name"] + "-offset"] if any(r["name"] == row["name"] + "-offset" for r in rows)
                                 else [])
        done |= set(cases)
        bindings = []
        for prop, (times, values, slopes, mode) in sorted(r12.expected_curves(row, doc, variants.get(row["name"])).items()):
            slopes = slopes or r12.slope_bits([f32(t) for t in times], [f32(v) for v in values])
            n = len(times)
            bindings.append(dict(path="", type=r12.ANIMATOR, property=prop, times=list(times), values=list(values),
                                 in_tangents=[r12.bits(0.)] + slopes, out_tangents=slopes + [r12.bits(0.)],
                                 in_weights=[r12.UNWEIGHTED] * n, out_weights=[r12.UNWEIGHTED] * n,
                                 weighted_modes=["None"] * n, left_modes=[mode] * n, right_modes=[mode] * n))
        clips.append(dict(clip=row["name"], cases=cases, bindings=bindings, object_bindings=[]))
    return dict(format="unityrs.round12_authored", run_id="r", muscle_names=names, clips=clips)


class AuthoredTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.doc = r12.document()
        cls.names = r12.humantrait_order()

    def fresh(self):
        return json.loads(json.dumps(authored(self.doc, self.names)))

    def clip(self, run, family):
        rows = {row["name"]: row for row in self.doc["cases"]}
        return next(clip for clip in run["clips"] if rows[clip["cases"][0]]["family"] == family)

    def test_the_tables_curves_pass(self):
        run = self.fresh()
        self.assertEqual(r12.check_authored(run, self.doc), [])
        self.assertEqual(len(run["clips"]), 6 + 2 * len({(row[2], row[3]) for row in r12.full_clips()}))
        twin = {b["property"] for b in self.clip(run, "twin")["bindings"]}
        self.assertFalse(twin & {m for m, _ in LIMB_TWISTS})
        self.assertTrue({m for m, _ in LIMB_TWISTS} <= {b["property"] for b in self.clip(run, "twist-clip")["bindings"]})
        self.assertTrue({"RootT.x", "RootQ.w"} <= {b["property"] for b in self.clip(run, "twist-clip")["bindings"]})

    def test_a_wrong_value_another_interpolation_or_a_twin_twist_is_a_problem(self):
        def broken(family, change):
            run = self.fresh()
            change(self.clip(run, family))
            return r12.check_authored(run, self.doc)

        def muscle(clip):
            return next(b for b in clip["bindings"] if not b["property"].startswith(("Root", "Motion")))

        def value(clip):
            curve = muscle(clip)
            curve["values"][3] = r12.bits(r12.f32(.5) if curve["values"][3] != r12.bits(.5) else .25)

        def constant(clip):
            curve = muscle(clip)
            curve["out_tangents"] = [r12.bits(float("inf"))] * len(curve["out_tangents"])

        def weighted(clip):
            muscle(clip)["weighted_modes"][1] = "Both"

        def mode(clip):
            muscle(clip)["left_modes"][2] = "Free"

        def root_mode(clip):
            next(b for b in clip["bindings"] if b["property"] == "RootT.y")["right_modes"][0] = "Linear"

        def twice(clip):
            clip["bindings"].append(dict(muscle(clip)))

        def root_value(clip):
            curve = next(b for b in clip["bindings"] if b["property"] == "RootQ.y")
            curve["values"][2] = r12.bits(r12.f32(r12._value(curve["values"][2]) + .001))

        def no_root(clip):
            clip["bindings"] = [b for b in clip["bindings"] if b["property"] != "RootT.z"]

        def elsewhere(clip):
            clip["bindings"].append(dict(muscle(clip), path="HumanoidRig/Hips", type="UnityEngine.Transform"))

        def weight(clip):
            muscle(clip)["out_weights"][1] = r12.bits(1 / 3)

        def reference(clip):
            clip["object_bindings"].append(dict(path="HumanoidRig", type="UnityEngine.MeshFilter", property="m_Mesh",
                                                times=[r12.bits(0.)]))

        def flipped(clip):
            # An interior RootQ key at the other sign, its neighbours' tangents recomputed to match: the same keys
            # up to sign, but playback between them takes the long way round.
            curves = [b for b in clip["bindings"] if b["property"].startswith("RootQ.")]
            for curve in curves:
                curve["values"][2] = r12.bits(-r12._value(curve["values"][2]))
                slopes = r12.slope_bits([f32(t) for t in curve["times"]], [f32(v) for v in curve["values"]])
                curve["in_tangents"], curve["out_tangents"] = [r12.bits(0.)] + slopes, slopes + [r12.bits(0.)]

        def twist(clip):
            twin_rig = clip["bindings"][0]
            clip["bindings"].append(dict(twin_rig, property="Left Arm Twist In-Out"))
        for family, change in (("subset", value), ("subset", constant), ("twist-clip", value), ("twist-clip", constant),
                               ("subset", weighted), ("twist-clip", mode), ("twin", root_mode), ("subset", twice),
                               ("twist-clip", root_value), ("twin", no_root), ("subset", elsewhere), ("twin", twist),
                               ("subset", weight), ("twin", reference), ("twist-clip", flipped)):
            self.assertTrue(broken(family, change), (family, change.__name__))
        run = self.fresh()
        run["clips"].pop()
        self.assertTrue(any("no authored clip" in p for p in r12.check_authored(run, self.doc)))
        run = self.fresh()
        run["muscle_names"] = list(reversed(run["muscle_names"]))
        self.assertTrue(r12.check_authored(run, self.doc))

    def test_the_tables_carry_the_curves_round_12_authors(self):
        # The full-body muscle curves are full_groups' on every round 12 rig (an in-scope rig's values do not depend
        # on the rig), the subset's its vectors, and the body curves anim_fixture_body's; each slope the f32 one.
        from anim_fixture_body import BODY_PATH, BODY_TIMES

        sets = {curve["name"]: curve for curve in self.doc["curves"]}
        full = {row["property"]: row for row in sets["full"]["rows"]}
        self.assertEqual([row["name"] for row in sets["full"]["rows"]], self.names)
        self.assertEqual(sets["full"]["times"], [r12.bits(t) for t in BODY_TIMES])
        for rig in sorted({row[2] for row in r12.full_clips()}):
            for (prop, _, times, values), in full_groups(self.names, rig):
                self.assertEqual(full[prop]["values"], [r12.bits(v) for v in values], (rig, prop))
        for curve in sets.values():
            times = [f32(t) for t in curve["times"]]
            for row in curve["rows"]:
                if "slopes" in row:
                    self.assertEqual(row["slopes"], r12.slope_bits(times, [f32(v) for v in row["values"]]))
        subset = self.sets_of("subset")
        for row in sets["subset"]["rows"]:
            name = full[row["property"]]["name"]
            self.assertEqual(row["values"], [dict(zip(v["muscles"], v["bits"])).get(name, r12.bits(0.)) for v in subset])
        self.assertEqual(sets["root"]["rows"][0]["values"], [r12.bits(p[0]) for p in BODY_PATH])
        self.assertEqual({row["ordinal"]: row["name"] for row in self.doc["names"]},
                         {m: n for m, n in enumerate(self.names) if n in tables.NAMES})

    def test_every_clip_case_names_its_binding_list(self):
        lists = {row["name"]: row["properties"] for row in self.doc["bindings"]}
        twists = {m for m, _ in LIMB_TWISTS}
        for row in self.doc["cases"]:
            family, name = row["family"], row["bindings"]
            self.assertEqual(bool(name), family in ("subset", "twist-clip", "twin"), row["name"])
            if family == "twin":
                self.assertEqual(lists[name], [p for p in lists[row["rig"]] if p not in twists])
                self.assertTrue(twists <= set(lists[row["rig"]]))
        subset = {m for vector in self.sets_of("subset") for m in vector["muscles"]}
        self.assertEqual(lists["subset"], sorted(subset))

    def sets_of(self, name):
        return next(row["vectors"] for row in self.doc["sets"] if row["name"] == name)


if __name__ == "__main__":
    unittest.main()
