#if UNITY_EDITOR
using System.Collections.Generic;
using System.Linq;
using UnityEngine;

// B5a's muscle sweeps (anim_fixture_muscles.py MUSCLE_SWEEPS): known vectors
// for the muscles the body probes leave out, so the Rust muscle pose can be
// checked on every muscle a clip animates. The ten limb twist muscles on the
// compact and tall rigs, and the forty finger muscles on the compact rig with
// mapped fingers (FingersRig). Each muscle is probed alone at every
// MuscleValues value, both signs out to its limits (a twist sweep probes only
// its twist muscles alone, the body probes having the others), then each
// group (a twist muscle with its bone's probe muscles, a finger's four
// muscles) is set jointly at MuscleCombined's two vectors. Each sweep is a
// bundle of its own. B2 never reads them.
static partial class UnityrsFixtureBuilder
{
    static readonly float[] MuscleValues = { 1, -1, .75f, -.75f, .5f, -.5f, .25f, -.25f, .125f, -.125f };
    static readonly float[][] MuscleCombined = { new[] { .6f, -.5f, .4f, -.3f }, new[] { -.6f, .5f, -.4f, .3f } };

    // Each limb twist muscle and the bone whose probe muscles it is combined with.
    static readonly (string muscle, string bone)[] LimbTwists = {
        ("Left Upper Leg Twist In-Out", "LeftUpperLeg"), ("Left Lower Leg Twist In-Out", "LeftLowerLeg"),
        ("Left Foot Twist In-Out", "LeftFoot"), ("Right Upper Leg Twist In-Out", "RightUpperLeg"),
        ("Right Lower Leg Twist In-Out", "RightLowerLeg"), ("Right Foot Twist In-Out", "RightFoot"),
        ("Left Arm Twist In-Out", "LeftUpperArm"), ("Left Forearm Twist In-Out", "LeftLowerArm"),
        ("Right Arm Twist In-Out", "RightUpperArm"), ("Right Forearm Twist In-Out", "RightLowerArm"),
    };

    static readonly string[] Fingers = { "Thumb", "Index", "Middle", "Ring", "Little" };
    static readonly string[] Phalanges = { "Proximal", "Intermediate", "Distal" };
    // Each finger's phalanx offsets on the left hand (x mirrored on the right).
    static readonly Vector3[][] FingerLayout = {
        new[] { new Vector3(.02f, -.01f, .025f), new Vector3(.025f, 0, .015f), new Vector3(.02f, 0, .01f) },
        new[] { new Vector3(.075f, 0, .022f), new Vector3(.03f, 0, 0), new Vector3(.022f, 0, 0) },
        new[] { new Vector3(.078f, 0, .007f), new Vector3(.033f, 0, 0), new Vector3(.024f, 0, 0) },
        new[] { new Vector3(.074f, 0, -.008f), new Vector3(.03f, 0, 0), new Vector3(.022f, 0, 0) },
        new[] { new Vector3(.066f, 0, -.022f), new Vector3(.024f, 0, 0), new Vector3(.018f, 0, 0) },
    };

    static IEnumerable<(string side, int sign)> Sides => new[] { ("Left", -1), ("Right", 1) };

    static string[] FingerBones => Sides.SelectMany(side => Fingers.SelectMany(finger =>
        Phalanges.Select(phalanx => side.side + " " + finger + " " + phalanx))).ToArray();

    // Each finger's muscles, in HumanTrait order.
    static string[][] FingerGroups => Sides.SelectMany(side => Fingers.Select(finger => new[] {
        side.side + " " + finger + " 1 Stretched", side.side + " " + finger + " Spread",
        side.side + " " + finger + " 2 Stretched", side.side + " " + finger + " 3 Stretched" })).ToArray();

    static string[][] TwistGroups => LimbTwists.Select(row =>
        new[] { row.muscle }.Concat(ProbeGroups.Single(group => group[0] == row.bone).Skip(1)).ToArray()).ToArray();

    // The compact rig with all thirty finger bones mapped.
    static readonly BodyRig FingersRig = new BodyRig { Name = "fingers", Extras = FingerBones };

    static readonly (string name, string rig)[] MuscleSweeps = {
        ("body-probes-limb-twist-compact", "compact"), ("body-probes-limb-twist-tall", "tall"),
        ("body-probes-fingers", "fingers"),
    };

    static BodyRig MuscleRig(string rig) => rig == FingersRig.Name ? FingersRig : ProbeRigs.Single(row => row.Name == rig);

    // The fingers below each hand, for a rig whose Extras name them.
    static void AddFingers(BodyRig rig, System.Action<string, string, Vector3> add)
    {
        foreach (var (side, sign) in Sides)
            for (int f = 0; f < Fingers.Length; f++)
            {
                string parent = side + "Hand";
                for (int k = 0; k < Phalanges.Length; k++)
                {
                    string name = side + " " + Fingers[f] + " " + Phalanges[k];
                    if (!rig.Extras.Contains(name)) break;
                    var offset = FingerLayout[f][k];
                    add(name, parent, new Vector3(sign * offset.x, offset.y, offset.z));
                    parent = name;
                }
            }
    }

    static BodyProbe[] MuscleSchedule(string name)
    {
        var names = HumanTrait.MuscleName;
        var groups = name == "body-probes-fingers" ? FingerGroups : TwistGroups;
        // A twist sweep probes its twist muscles alone (the body probes have the rest); the finger sweep every muscle.
        var singles = name == "body-probes-fingers" ? groups.SelectMany(group => group) : groups.Select(group => group[0]);
        var vectors = new List<float[]>();
        foreach (string muscle in singles)
            foreach (float value in MuscleValues)
            {
                var vector = new float[names.Length];
                vector[MuscleIndex(names, muscle)] = value;
                vectors.Add(vector);
            }
        foreach (var group in groups)
            foreach (var values in MuscleCombined)
            {
                var vector = new float[names.Length];
                for (int k = 0; k < group.Length; k++) vector[MuscleIndex(names, group[k])] = values[k % values.Length];
                vectors.Add(vector);
            }
        return Scheduled(vectors, false);
    }

    // The body generation a main run declares beside run.json: round 11 adds E5's
    // clip and chain rigs and these sweeps (anim_fixture_round11.py completes it).
    const string BodyRound = "11";

    static void WriteRound(string directory, string runId) => System.IO.File.WriteAllText(
        System.IO.Path.Combine(directory, "round.json"),
        "{\"format\": \"unityrs.fixture_round\", \"round\": " + BodyRound + ", \"run_id\": \"" + runId + "\"}\n");

    static void BuildMuscleSweeps(Build build, RunReport report)
    {
        // Each sweep is required: Unity refusing its Avatar is an error.
        foreach (var sweep in MuscleSweeps)
            report.Prepare(build, "Prepare " + sweep.name, new[] { sweep.name },
                () => BuildBodyRig(build, report, MuscleRig(sweep.rig), false, (sweep.name, sweep.rig, null), sweep: "muscle"));
    }
}
#endif
