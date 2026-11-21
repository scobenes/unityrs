#if UNITY_EDITOR
using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Text;
using UnityEditor;
using UnityEngine;

// What round 12 checks of its clips and writes at the end of its session (round 12 design v4 section 3):
//
// - every clip it authors (the clip subset, the twist clips and their twins) has exactly its case's Animator muscle
//   bindings from the tables (a twin no limb-twist curve), or its cases fail; each clip's curves (every Animator
//   binding's key times, values and tangents, as f32 bits) go to authored.json, which
//   tools/parity/anim_round12_tables.py --authored checks against the tables;
// - the built bundle's twin keeps no compiled limb-twist track, and a subset clip exactly its muscles' tracks
//   (UnityrsBodyClips.cs CheckTracks), fatally;
// - each built Avatar, loaded back from its bundle, is logged with its four twist settings against the authored
//   tuple;
// - inventory.json walks replays/body: a table case without a readable capture of the tables' samples fails, and
//   any file there that is not a table case's is listed; then authored.json and round.json.
static partial class UnityrsFixtureBuilder
{
    [Serializable] sealed class R12Curve { public string path, type, property; public string[] times, values, in_tangents, out_tangents; }
    [Serializable] sealed class R12Clip { public string clip; public string[] cases; public R12Curve[] bindings; }
    [Serializable] sealed class R12Authored
    {
        public string format = "unityrs.round12_authored", run_id;
        public string[] muscle_names;
        public R12Clip[] clips;
    }

    static List<R12Clip> round12Clips = new List<R12Clip>();
    static Action<string> round12Log;

    // A round 12 clip's Animator muscle bindings against its cases' list, its curves kept for authored.json: null,
    // or why its cases fail.
    static string Round12Authored(AnimationClip clip, string[] cases)
    {
        var row = Round12Case(cases[0]);
        var want = Round12.bindings.Single(list => list.name == row.bindings).properties;
        var muscles = new HashSet<string>(HumanTrait.MuscleName.Select(MuscleProperty), StringComparer.Ordinal);
        var bindings = AnimationUtility.GetCurveBindings(clip).Where(binding => binding.path == "" && binding.type == typeof(Animator))
            .OrderBy(binding => binding.propertyName, StringComparer.Ordinal).ToArray();
        round12Clips.Add(new R12Clip {
            clip = clip.name, cases = cases, bindings = bindings.Select(binding => {
                var keys = AnimationUtility.GetEditorCurve(clip, binding).keys;
                return new R12Curve {
                    path = binding.path, type = binding.type.FullName, property = binding.propertyName,
                    times = keys.Select(key => F32Bits(key.time)).ToArray(), values = keys.Select(key => F32Bits(key.value)).ToArray(),
                    in_tangents = keys.Select(key => F32Bits(key.inTangent)).ToArray(),
                    out_tangents = keys.Select(key => F32Bits(key.outTangent)).ToArray()
                };
            }).ToArray()
        });
        var have = bindings.Select(binding => binding.propertyName).Where(muscles.Contains).ToArray();
        return have.SequenceEqual(want) ? null : "The authored clip's muscle curves differ from the round 12 tables' "
            + row.bindings + " list (a twin authors no limb-twist curve).";
    }

    // A subset clip's compiled tracks: exactly its moved muscles', each at its Animator attribute.
    static (string track, int attribute)[] SubsetTracks(R12Case row)
    {
        var names = HumanTrait.MuscleName;
        return Round12.bindings.Single(list => list.name == row.bindings).properties
            .Select(property => (property, MuscleAttribute + MuscleIndex(names, property))).ToArray();
    }

    // The limb-twist muscles' Animator attributes, which a twin's built clip must not bind.
    static int[] LimbTwistAttributes => LimbTwists.Select(row => MuscleAttribute + MuscleIndex(HumanTrait.MuscleName, row.muscle)).ToArray();

    // A round 12 Avatar loaded back from its bundle: its twist settings logged against the authored tuple.
    static void LogBuiltTwist(Avatar avatar, ReplayCase replay)
    {
        var twist = replay.Body?.Spec?.Twist;
        if (twist == null || round12Log == null) return;
        var built = avatar.humanDescription;
        bool same = new[] { built.upperArmTwist, built.lowerArmTwist, built.upperLegTwist, built.lowerLegTwist }
            .Select(F32Bits).SequenceEqual(twist.Select(F32Bits));
        round12Log(TwistLine("built " + replay.Bundle + " (loaded from its bundle)", built)
            + (same ? "; matches the authored tuple" : "; DIFFERS from the authored tuple "
                + string.Join(", ", twist.Select(F32Bits))));
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
        public string[] extra;
    }
    [Serializable] sealed class R12Round
    {
        public string format = "unityrs.fixture_round";
        public int round = 12;
        public string run_id, tables_sha256, tables_digest, editor_sources_sha256, importer_default;
    }

    // The end of the session. Captures are only read, never moved or removed; one that cannot be read fails its case
    // and the rest are still checked.
    static string FinishRound12(RunReport report, string staging, RunIdentity run)
    {
        const string Pending = ".pending.json", Diagnostics = ".diagnostics.json";
        string folder = Path.Combine(staging, "replays", "body");
        var files = Directory.Exists(folder)
            ? Directory.GetFiles(folder).Select(path => Path.GetFileName(path)).ToArray() : Array.Empty<string>();
        var present = new HashSet<string>(files, StringComparer.Ordinal);
        var rows = new List<R12Row>();
        foreach (var row in Round12.cases)
        {
            var found = new R12Row { name = row.name, status = "missing", reason = "no capture in replays/body", samples = -1 };
            if (present.Contains(row.name + Pending))
            {
                try
                {
                    var samples = JsonUtility.FromJson<R12Pending>(File.ReadAllText(Path.Combine(folder, row.name + Pending)))?.capture?.samples;
                    Require(samples != null, "the capture has no samples");
                    found.samples = samples.Length;
                    bool same = samples.Select(sample => F32Bits((float)sample.local)).SequenceEqual(row.times);
                    found.status = same ? "matched" : "mismatch";
                    found.reason = same ? "" : "its samples are not the tables' " + row.times.Length + " times";
                }
                catch (Exception error)
                {
                    found.status = "unreadable";
                    found.reason = error.Message;
                }
            }
            if (found.status != "matched")
                report.Fail(row.name, "inventory.json: " + found.status + ", " + found.reason);
            rows.Add(found);
        }
        var expected = new HashSet<string>(Round12.cases.SelectMany(row => new[] { row.name + Pending, row.name + Diagnostics }),
            StringComparer.Ordinal);
        var extra = files.Where(name => !expected.Contains(name)).OrderBy(name => name, StringComparer.Ordinal).ToArray();
        foreach (string name in extra) report.Log("inventory.json: replays/body/" + name + " is not a round 12 table case's.");
        var inventory = new R12Inventory {
            run_id = run.run_id, tables_sha256 = Round12TablesSha256, cases = rows.Count,
            matched = rows.Count(row => row.status == "matched"), rows = rows.ToArray(), extra = extra
        };
        File.WriteAllText(Path.Combine(staging, "inventory.json"), JsonUtility.ToJson(inventory, true));
        File.WriteAllText(Path.Combine(staging, "authored.json"), JsonUtility.ToJson(new R12Authored {
            run_id = run.run_id, muscle_names = HumanTrait.MuscleName, clips = round12Clips.ToArray()
        }, true));
        File.WriteAllText(Path.Combine(staging, "round.json"), JsonUtility.ToJson(new R12Round {
            run_id = run.run_id, tables_sha256 = Round12TablesSha256, tables_digest = Round12.tables_digest,
            editor_sources_sha256 = EditorSourcesSha256(), importer_default = ImporterDefault
        }, true));
        return "Inventory: " + inventory.matched + " of " + inventory.cases + " round 12 cases recorded with the tables' "
            + "samples and times" + (extra.Length == 0 ? "" : ", and " + extra.Length + " other file(s) in replays/body")
            + " (inventory.json).";
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
