"""Round 12's generated tables (anim_round12_tables) and the C# file that carries them (UnityrsRound12Tables.cs).

The builder reads the document from the C# file and re-derives its schedules from its own code; these tests require
the committed C# file to be byte-identical to the generator's output, its pieces to concatenate to the canonical
document, and the document to be what the twist tables, B5a's sweeps and round 11's clips give."""

import hashlib
import struct
import unittest

import test_anim_replay_coverage  # noqa: F401  (puts tools/parity/tests' parent on the path)

import anim_fixture_body as body  # noqa: E402
import anim_fixture_round11 as r11  # noqa: E402
import anim_muscle_confirm as confirm  # noqa: E402
import anim_round12_tables as r12  # noqa: E402
import anim_twist_tables as tables  # noqa: E402
from anim_fixture_muscles import LIMB_TWISTS, groups, muscle_vectors  # noqa: E402

EDITOR = r12.CS_PATH.parent


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

    def test_the_builder_reads_them_by_their_sha256(self):
        builder = (EDITOR / "UnityrsRound12.cs").read_text(encoding="utf-8")
        for line in ("string text = string.Concat(Round12TablesParts);",
                     "Require(Sha256(Encoding.UTF8.GetBytes(text)) == Round12TablesSha256,",
                     "var times = Enumerable.Range(0, vectors.Count).Select(k => k / 16f).ToArray();",
                     "var probes = Scheduled(vectors, false);",
                     "Require(probes.Select(probe => F32Bits((float)probe.Time)).SequenceEqual(row.times),"):
            self.assertIn(line, builder)
        # Scheduled's formula, which `scheduled` mirrors.
        fixtures = (EDITOR / "UnityrsBodyFixtures.cs").read_text(encoding="utf-8")
        self.assertIn("Time = Math.Round(time * 65536) / 65536", fixtures)
        self.assertIn("probes.Add(Point(.5 * (k + 391.0 / 1024) / first, vectors[k]));", fixtures)


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


if __name__ == "__main__":
    unittest.main()
