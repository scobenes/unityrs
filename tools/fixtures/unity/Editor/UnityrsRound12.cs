#if UNITY_EDITOR
using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Linq;
using System.Security.Cryptography;
using System.Text;
using UnityEditor;
using UnityEngine;
using UnityEngine.SceneManagement;

// Round 12 (round 12 design v4 section 3): a body-only run of its own. Every case comes from the generated tables
// (UnityrsRound12Tables.cs, written by tools/parity/anim_round12_tables.py), checked against their sha256 before a
// byte is read. The builder re-derives what its own code computes (each SetHumanPose schedule's times, the sweeps'
// B5a vectors, the clip cases' names) and refuses a difference before anything is built. The run:
//
// - writes to UnityrsFixturesOutput-round12, builds only body bundles and keeps its staging folder on any failure;
// - sets each Avatar's four twist settings (HumanDescription's upperArmTwist, lowerArmTwist, upperLegTwist and
//   lowerLegTwist, as UnityrsCoverageFixtures.cs does) and logs them, with their f32 bits, as built;
// - records motion checks as diagnostics beside each capture (UnityrsRound12Recording.cs);
// - ends with inventory.json (every table case's capture, sample count and times) and round.json (the round, the
//   run, the tables' sha256 and digest, and the sha256 of this Editor folder's sources).
static partial class UnityrsFixtureBuilder
{
    const string Round12Output = "UnityrsFixturesOutput-round12";

    [Serializable] sealed class R12Tuple { public string name; public string[] bits; }
    [Serializable] sealed class R12Name { public string name; public int ordinal; }
    [Serializable] sealed class R12Vector { public string[] muscles, bits; }
    [Serializable] sealed class R12Set { public string name; public R12Vector[] vectors; }
    [Serializable] internal sealed class R12Case { public string name, family, rig, tuple, set, partition; public string[] times; }
    [Serializable] sealed class R12Tables
    {
        public string format, tables_digest;
        public int version;
        public R12Tuple[] tuples;
        public R12Name[] names;
        public R12Set[] sets;
        public R12Case[] cases;
    }

    static R12Tables round12;

    static string Sha256(byte[] bytes)
    {
        using (var sha = SHA256.Create())
            return BitConverter.ToString(sha.ComputeHash(bytes)).Replace("-", "").ToLowerInvariant();
    }

    static R12Tables Round12
    {
        get
        {
            if (round12 != null) return round12;
            string text = string.Concat(Round12TablesParts);
            Require(Sha256(Encoding.UTF8.GetBytes(text)) == Round12TablesSha256,
                "UnityrsRound12Tables.cs does not match its sha256: regenerate it (anim_round12_tables.py --write).");
            var tables = JsonUtility.FromJson<R12Tables>(text);
            Require(tables != null && tables.format == "unityrs.round12_tables" && tables.version == 1,
                "UnityrsRound12Tables.cs is not round 12's tables.");
            var names = HumanTrait.MuscleName;
            Require(tables.names.All(row => row.ordinal >= 0 && row.ordinal < names.Length && names[row.ordinal] == row.name),
                "HumanTrait's muscle order differs from the round 12 tables' (anim_twist_tables.NAMES).");
            return round12 = tables;
        }
    }

    static float F32(string bits) => BitConverter.ToSingle(BitConverter.GetBytes(
        uint.Parse(bits, NumberStyles.HexNumber, CultureInfo.InvariantCulture)), 0);
    static string F32Bits(float value) =>
        BitConverter.ToUInt32(BitConverter.GetBytes(value), 0).ToString("x8", CultureInfo.InvariantCulture);

    static R12Case Round12Case(string name) => Round12.cases.Single(row => row.name == name);
    static float[] Round12Twist(string tuple) => Round12.tuples.Single(row => row.name == tuple).bits.Select(F32).ToArray();

    static List<float[]> Round12Vectors(string set)
    {
        var names = HumanTrait.MuscleName;
        return Round12.sets.Single(row => row.name == set).vectors.Select(row => {
            var vector = new float[names.Length];
            for (int i = 0; i < row.muscles.Length; i++) vector[MuscleIndex(names, row.muscles[i])] = F32(row.bits[i]);
            return AnimReplayRecorder.Canonical(vector);
        }).ToList();
    }

    // A table case's SetHumanPose schedule: its vectors on Scheduled's times, which must be the tables' own; a
    // sweep's vectors must also be B5a's (MuscleVectors) and then each limb group's joint vectors without its twist.
    static BodyProbe[] Round12Schedule(R12Case row, string[] mapped)
    {
        var vectors = Round12Vectors(row.set);
        if (row.family == "sweep")
        {
            string name = "body-probes-muscles-" + row.rig;
            var mirror = MuscleVectors(name, mapped);
            var twists = LimbTwists.Select(twist => twist.muscle).ToArray();
            var names = HumanTrait.MuscleName;
            foreach (var group in SweepGroups(name, mapped).Where(group => group.Intersect(twists).Any()))
                foreach (var values in MuscleCombined)
                {
                    var vector = new float[names.Length];
                    for (int k = 0; k < group.Length; k++)
                        if (!twists.Contains(group[k])) vector[MuscleIndex(names, group[k])] = values[k % values.Length];
                    mirror.Add(AnimReplayRecorder.Canonical(vector));
                }
            Require(mirror.Count == vectors.Count && mirror.Zip(vectors, (a, b) => a.SequenceEqual(b)).All(same => same),
                "The builder's sweep vectors for " + row.name + " differ from the round 12 tables'.");
        }
        var probes = Scheduled(vectors, false);
        Require(probes.Select(probe => F32Bits((float)probe.Time)).SequenceEqual(row.times),
            "The builder's schedule for " + row.name + " differs from the round 12 tables' times.");
        return probes;
    }

    // A clip-subset case's clip: key k at k/16 s (frameRate 16, so every key is on a frame) holding the set's
    // vector k, the zero vector first, every animated muscle on Linear tangents; no root curves.
    static AnimationClip AuthorSubsetClip(string dir, string name, R12Case row)
    {
        var vectors = Round12Vectors(row.set);
        var times = Enumerable.Range(0, vectors.Count).Select(k => k / 16f).ToArray();
        var names = HumanTrait.MuscleName;
        Require(vectors[0].All(value => value == 0), "The subset clip of " + row.name + " does not start at the zero vector.");
        var moved = Enumerable.Range(0, names.Length).Where(m => vectors.Any(vector => vector[m] != 0)).ToArray();
        var clip = Author(dir, name, authored => {
            foreach (int m in moved)
                authored.SetCurve("", typeof(Animator), MuscleProperty(names[m]),
                    LinearCurve(times, vectors.Select(vector => vector[m]).ToArray()));
        }, 0, times[times.Length - 1]);
        Require(clip.humanMotion && clip.frameRate == 16 && clip.length == times[times.Length - 1],
            "The subset clip of " + row.name + " is not a 16 frames per second human-motion clip of its keys.");
        return clip;
    }

    static HumanDescription WithTwist(HumanDescription description, float[] twist)
    {
        if (twist == null) return description;
        description.upperArmTwist = twist[0];
        description.lowerArmTwist = twist[1];
        description.upperLegTwist = twist[2];
        description.lowerLegTwist = twist[3];
        return description;
    }

    static string TwistLine(string label, HumanDescription description) => "twist settings " + label + ": "
        + string.Join(", ", new[] {
            ("upperArmTwist", description.upperArmTwist), ("lowerArmTwist", description.lowerArmTwist),
            ("upperLegTwist", description.upperLegTwist), ("lowerLegTwist", description.lowerLegTwist)
        }.Select(row => row.Item1 + " " + row.Item2.ToString("R", CultureInfo.InvariantCulture) + " (" + F32Bits(row.Item2) + ")"));

    // A round 12 Avatar keeps its four twist settings bit for bit, and the log records them as built.
    static void CheckTwist(Avatar avatar, float[] twist, string name, RunReport report)
    {
        if (twist == null) return;
        var built = avatar.humanDescription;
        report.Log(TwistLine(name, built));
        Require(new[] { built.upperArmTwist, built.lowerArmTwist, built.upperLegTwist, built.lowerLegTwist }
            .Select(F32Bits).SequenceEqual(twist.Select(F32Bits)), "The " + name + " Avatar did not keep its twist settings.");
    }

    // The defaults probe: a default-initialized HumanDescription's twist settings, with their f32 bits.
    [MenuItem("Tools/unityrs/Print humanoid twist defaults")]
    static void PrintTwistDefaults() => Debug.Log("Unity " + Application.unityVersion + " "
        + TwistLine("new HumanDescription()", new HumanDescription()));

    [MenuItem("Tools/unityrs/Build and record round 12 (body)")]
    static void BuildRound12()
    {
        var target = EditorUserBuildSettings.activeBuildTarget;
        Run(new Variant {
            Name = "round12", Root = Round12Output, Folder = target.ToString(), Target = target, Record = true,
            BodyOnly = true, KeepStaging = true, Cases = Round12.cases.Select(row => row.name).ToArray(),
            Prepare = PrepareRound12, Finish = FinishRound12
        });
    }

    static Build PrepareRound12(RunReport report, Scene scene, string stamp)
    {
        var tables = Round12;
        var build = new Build();
        report.Log("Unity " + Application.unityVersion + " " + TwistLine("new HumanDescription()", new HumanDescription()));
        // The full-body clip cases: each in-scope rig as a twist rig and a twin, whose ClipCases must be the tables'.
        var clipRigs = ScopedRigs.SelectMany(rig => new[] {
            (family: "twist-clip", spec: rig.With(rig.Name + "-twist", Round12Twist("t5"))),
            (family: "twin", spec: rig.With(rig.Name + "-twin", Round12Twist("t0"), noLimbTwist: true))
        }).ToArray();
        foreach (var family in new[] { "twist-clip", "twin" })
            Require(clipRigs.Where(row => row.family == family).SelectMany(row => ClipCases(row.spec))
                .OrderBy(name => name, StringComparer.Ordinal).SequenceEqual(tables.cases.Where(row => row.family == family)
                    .Select(row => row.name).OrderBy(name => name, StringComparer.Ordinal)),
                "The builder's " + family + " cases differ from the round 12 tables'.");
        foreach (var row in tables.cases.Where(row => row.family != "twist-clip" && row.family != "twin"))
        {
            var rig = (row.family == "sweep" ? MuscleRig(row.rig) : ProbeRigs.Single(spec => spec.Name == row.rig))
                .With(row.rig, Round12Twist(row.tuple));
            string sweep = row.family == "subset" ? "subset" : "table";
            report.Prepare(build, "Prepare " + row.name, new[] { row.name },
                () => BuildBodyRig(build, report, rig, false, (row.name, row.rig, null), sweep));
        }
        foreach (var (_, spec) in clipRigs)
            report.Prepare(build, "Prepare " + ClipBundle(spec), ClipCases(spec).ToArray(), () => BuildBodyClips(build, report, spec));
        var prepared = new HashSet<string>(build.Replays.Select(replay => replay.OutputName), StringComparer.Ordinal);
        report.FailUnreached(tables.cases.Select(row => row.name).Where(name => !prepared.Contains(name)),
            "No preparation step produced this round 12 case.");
        return build;
    }

    [Serializable] sealed class R12Sample { public double local; }
    [Serializable] sealed class R12Capture { public R12Sample[] samples; }
    [Serializable] sealed class R12Pending { public R12Capture capture; }
    [Serializable] sealed class R12Row { public string name, status, reason; public int samples; }
    [Serializable] sealed class R12Inventory
    {
        public string format = "unityrs.round12_inventory", run_id, tables_sha256;
        public int cases, matched;
        public R12Row[] rows;
    }
    [Serializable] sealed class R12Round
    {
        public string format = "unityrs.fixture_round";
        public int round = 12;
        public string run_id, tables_sha256, tables_digest, editor_sources_sha256;
    }

    // The end of the session: every table case's capture against its table row (sample count and times), into
    // inventory.json, a mismatch failing its case; then round.json. Captures are only read, never removed.
    static string FinishRound12(RunReport report, string staging, RunIdentity run)
    {
        var rows = new List<R12Row>();
        foreach (var row in Round12.cases)
        {
            string path = Path.Combine(staging, "replays", "body", row.name + ".pending.json");
            var found = new R12Row { name = row.name, status = "missing", reason = "no capture", samples = -1 };
            if (File.Exists(path))
            {
                var samples = JsonUtility.FromJson<R12Pending>(File.ReadAllText(path))?.capture?.samples;
                found.samples = samples?.Length ?? -1;
                bool same = samples != null && samples.Select(sample => F32Bits((float)sample.local)).SequenceEqual(row.times);
                found.status = same ? "matched" : "mismatch";
                found.reason = same ? "" : "its samples are not the tables' " + row.times.Length + " times";
                if (!same) report.Set(row.name, "failed", "The capture's samples differ from the round 12 tables' (inventory.json).");
            }
            rows.Add(found);
        }
        var inventory = new R12Inventory {
            run_id = run.run_id, tables_sha256 = Round12TablesSha256, cases = rows.Count,
            matched = rows.Count(row => row.status == "matched"), rows = rows.ToArray()
        };
        File.WriteAllText(Path.Combine(staging, "inventory.json"), JsonUtility.ToJson(inventory, true));
        File.WriteAllText(Path.Combine(staging, "round.json"), JsonUtility.ToJson(new R12Round {
            run_id = run.run_id, tables_sha256 = Round12TablesSha256, tables_digest = Round12.tables_digest,
            editor_sources_sha256 = EditorSourcesSha256()
        }, true));
        return "Inventory: " + inventory.matched + " of " + inventory.cases
            + " round 12 cases recorded with the tables' samples and times (inventory.json).";
    }

    // The sha256 of this Editor folder's C# sources, by file name and content in ordinal order.
    static string EditorSourcesSha256()
    {
        var self = Directory.GetFiles(Application.dataPath, "UnityrsRound12.cs", SearchOption.AllDirectories);
        Require(self.Length == 1, "Expected one UnityrsRound12.cs under Assets.");
        var text = new StringBuilder();
        foreach (var file in Directory.GetFiles(Path.GetDirectoryName(self[0]), "*.cs").OrderBy(path => Path.GetFileName(path), StringComparer.Ordinal))
            text.Append(Path.GetFileName(file)).Append('\n').Append(Sha256(File.ReadAllBytes(file))).Append('\n');
        return Sha256(Encoding.UTF8.GetBytes(text.ToString()));
    }
}
#endif
