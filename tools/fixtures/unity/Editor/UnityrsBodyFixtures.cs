#if UNITY_EDITOR
using System;
using System.Collections.Generic;
using System.Linq;
using UnityEditor;
using UnityEngine;
using Object = UnityEngine.Object;

// Optional coverage for the humanoid body motion plan (docs/plans/humanoid-body.md
// section 4). F1 probe vectors run on both P2b rigs and every F6 topology, F3
// body targets on both P2b rigs and their offset roots, F4 body-motion clips
// (UnityrsBodyClips.cs) and the F5 scale and parent chains. The recorder adds
// F2's GetHumanPose read-back, the root rows and the default bone masses
// (UnityrsBodyRecording.cs). Every case is built in its own bundle build and
// tools/parity/anim_fixture_body.py mirrors these tables.
static partial class UnityrsFixtureBuilder
{
    internal sealed class BodyRig
    {
        public string Name;
        public bool Tall, Noncoplanar, Armature, ScaledAnimator;
        public string[] Omit = Array.Empty<string>(), Extras = Array.Empty<string>();
        public float Arms = 1, Legs = 1, Size = 1, HipsScale = 1;
    }

    // F1 rigs: the two P2b rigs, then F6's topologies on the compact rig.
    // The last is the hold-out rig, never used to fit.
    static readonly BodyRig[] ProbeRigs = {
        new BodyRig { Name = "compact" }, new BodyRig { Name = "tall", Tall = true },
        // Spine's three children are its own edges here alone: off one plane, so each separates.
        new BodyRig { Name = "no-chest", Omit = new[] { "Chest", "UpperChest" }, Noncoplanar = true },
        new BodyRig { Name = "no-upper-chest", Omit = new[] { "UpperChest" } },
        new BodyRig { Name = "no-neck", Omit = new[] { "Neck" } },
        new BodyRig { Name = "no-left-shoulder", Omit = new[] { "LeftShoulder" } },
        new BodyRig { Name = "no-right-shoulder", Omit = new[] { "RightShoulder" } },
        new BodyRig { Name = "no-shoulders", Omit = new[] { "LeftShoulder", "RightShoulder" } },
        new BodyRig { Name = "no-left-toes", Omit = new[] { "LeftToes" } },
        new BodyRig { Name = "no-right-toes", Omit = new[] { "RightToes" } },
        new BodyRig { Name = "no-toes", Omit = new[] { "LeftToes", "RightToes" } },
        new BodyRig { Name = "no-upper-chest-toes", Omit = new[] { "UpperChest", "LeftToes", "RightToes" } },
        new BodyRig { Name = "no-upper-chest-left-toes", Omit = new[] { "UpperChest", "LeftToes" } },
        new BodyRig { Name = "no-upper-chest-right-toes", Omit = new[] { "UpperChest", "RightToes" } },
        new BodyRig { Name = "eyes", Extras = new[] { "LeftEye", "RightEye" } },
        new BodyRig { Name = "jaw", Extras = new[] { "Jaw" } },
        new BodyRig { Name = "eyes-jaw", Extras = new[] { "LeftEye", "RightEye", "Jaw" } },
        new BodyRig { Name = "noncoplanar", Noncoplanar = true },
        new BodyRig { Name = "holdout", Arms = 1.3f, Legs = .8f },
    };
    // F6 rigs that came with the range sweeps (anim_fixture_body.py LATER_RIGS), each required.
    static readonly string[] LaterRigs = { "no-upper-chest-toes", "no-upper-chest-left-toes", "no-upper-chest-right-toes" };
    // F5: each plays F3's targets and the F4 all-off clip.
    static readonly BodyRig[] ChainRigs = {
        new BodyRig { Name = "scaled-animator", ScaledAnimator = true },
        new BodyRig { Name = "large", Size = 1.5f },
        new BodyRig { Name = "armature", Armature = true },
        new BodyRig { Name = "hips-scale", HipsScale = 1.2f },
    };
    static readonly string[] TargetRigs = { "compact", "tall" };
    // Extra probe cases, each on its own bundle of a probe rig (tools/parity/anim_fixture_body.py
    // EXTRA_PROBES): independent poses where B1's adequacy checkpoint found the rig short.
    static readonly (string name, string rig, (string bone, int source)[][] plan)[] ExtraProbes = {
        ("body-probes-tall-2", "tall", new[]
        {
            new[] { ("Chest", 2), ("RightUpperLeg", 1) },
            new[] { ("LeftUpperLeg", 2), ("RightUpperLeg", 1) },
            new[] { ("Chest", -3), ("RightUpperLeg", -2) },
            new[] { ("LeftUpperLeg", 1), ("RightUpperLeg", -2) },
            new[] { ("LeftUpperLeg", 1), ("RightUpperLeg", 2) },
            new[] { ("Chest", 1), ("LeftUpperLeg", 2) },
            new[] { ("LeftUpperLeg", 2), ("LeftUpperArm", -3) },
            new[] { ("LeftUpperLeg", 2), ("RightUpperLeg", -2) },
            new[] { ("Chest", 3), ("LeftUpperLeg", 2) },
            new[] { ("LeftUpperLeg", -3), ("RightUpperLeg", 1) },
            new[] { ("Chest", -1), ("LeftUpperArm", 2) },
            new[] { ("Chest", 2), ("LeftUpperArm", 2) },
            new[] { ("LeftUpperLeg", -3), ("RightUpperLeg", 2) },
            new[] { ("Chest", -1), ("LeftUpperLeg", -3) },
            new[] { ("Chest", 2), ("LeftUpperLeg", -2) },
            new[] { ("LeftUpperLeg", -3), ("RightUpperArm", 1) },
            new[] { ("Spine", 3), ("LeftUpperLeg", 2) },
            new[] { ("LeftUpperArm", 2), ("RightUpperArm", -3) },
            new[] { ("Chest", -1), ("RightUpperLeg", 1) },
            new[] { ("Chest", -3), ("LeftUpperArm", 2) },
            new[] { ("Neck", 2), ("LeftUpperLeg", -3) },
            new[] { ("Chest", -2), ("RightUpperLeg", 1) },
            new[] { ("Chest", -3), ("RightUpperArm", 2) },
            new[] { ("Chest", 1), ("LeftUpperLeg", -1) },
            new[] { ("LeftUpperLeg", -3), ("RightUpperLeg", -2) },
            new[] { ("LeftUpperLeg", -3), ("RightUpperArm", -3) },
        }),
    };
    // Twist sweeps, each on its own bundle of a training rig (anim_fixture_body.py TWIST_RIGS): the
    // Spine and Chest twist muscles together at degrees / TwistRange (HumanTrait's default limits,
    // +-40 degrees each), crossed with an upright body and three pitch and roll tilts of the body
    // target from F3's tilts. They measure the read-back frame against the shoulder-hip twist.
    // The degree values only label the muscle settings: a rig with its own limits (the tall
    // rig's Spine is -18/+42) realizes a different twist, so the analysis measures the twist
    // from the recorded upper and lower laterals, never from these labels.
    static readonly (string name, string rig)[] TwistProbes = {
        ("body-probes-twist-compact", "compact"), ("body-probes-twist-tall", "tall"),
    };
    static readonly int[] TwistDegrees = { 0, 2, -2, 4, -4, 6, -6, 8, -8, 10, -10, 15, -15, 20, -20, 30, -30, 45, -45, 60, -60 };
    const double TwistRange = 80;
    static readonly string[] BodyTwistMuscles = { "Spine Twist Left-Right", "Chest Twist Left-Right" };
    static readonly Vector3[] TwistTilts = { Vector3.zero, new Vector3(12, 0, 7), new Vector3(-9, 0, -11), new Vector3(12, 0, -11) };
    // Full-range spine sweeps, each on its own bundle of a training rig (anim_fixture_body.py RANGE_RIGS):
    // every mapped spine bone's three muscles over the whole cube [-1, 1]^3, jointly. The vectors are the
    // first RangePoints points of a 9-d Sobol sequence (Joe and Kuo's direction numbers, unscrambled), each
    // coordinate x as 2 x - 1: Spine Front-Back, Left-Right and Twist, then Chest's, then UpperChest's (a rig
    // without UpperChest takes the first six). Then the joint corners, every mapped spine bone at the same
    // corner, but (-1, -1, -1), the sequence's first point. Every other muscle is zero. The body target takes
    // TwistTilts, each over an aligned block of RangePoints / 4 Sobol points (a net filling the whole cube;
    // every fourth point would not), the corners cycling through them, the spine anchors upright.
    static readonly (string name, string rig)[] RangeProbes = {
        ("body-probes-range-compact", "compact"), ("body-probes-range-tall", "tall"),
        ("body-probes-range-noncoplanar", "noncoplanar"),
    };
    const int RangePoints = 256;
    static readonly string[] RangeBones = { "Spine", "Chest", "UpperChest" };
    // Lateral band sweeps (anim_fixture_body.py BAND_RIGS): the range sweeps' Sobol points with Front-Back over
    // [-1, 1], Left-Right over +-BandLateral and Twist over +-BandTwist, then the pure side bends, every mapped
    // spine bone at one BandLevels Left-Right value of either sign under each tilt. The tilts are the range sweeps'.
    static readonly (string name, string rig)[] BandProbes = {
        ("body-probes-band-compact", "compact"), ("body-probes-band-tall", "tall"),
        ("body-probes-band-noncoplanar", "noncoplanar"),
    };
    const double BandLateral = 3.0 / 8, BandTwist = 1.0 / 32;
    static readonly double[] BandLevels = { 1.0 / 8, 3.0 / 16, 1.0 / 4, 5.0 / 16, 3.0 / 8 };
    // (degree s, coefficients a, initial m_1..m_s) of dimensions 2-9.
    static readonly (int degree, int a, int[] m)[] SobolDirections = {
        (1, 0, new[] { 1 }), (2, 1, new[] { 1, 3 }), (3, 1, new[] { 1, 3, 1 }), (3, 2, new[] { 1, 1, 1 }),
        (4, 1, new[] { 1, 1, 3, 3 }), (4, 4, new[] { 1, 3, 5, 13 }), (5, 2, new[] { 1, 1, 5, 5, 17 }),
        (5, 4, new[] { 1, 1, 5, 5, 5 }),
    };
    const int SobolBits = 16;
    const float AnimatorScale = 1.25f, ArmatureScale = .8f;
    internal const float BodyAnimatorScale = AnimatorScale, BodyArmatureScale = ArmatureScale;

    // Each bone's probe muscles, in this order; twist and stretch of the
    // limbs stay out (section 8). A group runs only where its bone is mapped.
    static readonly string[][] ProbeGroups = {
        new[] { "Spine", "Spine Front-Back", "Spine Left-Right", "Spine Twist Left-Right" },
        new[] { "Chest", "Chest Front-Back", "Chest Left-Right", "Chest Twist Left-Right" },
        new[] { "UpperChest", "UpperChest Front-Back", "UpperChest Left-Right", "UpperChest Twist Left-Right" },
        new[] { "Neck", "Neck Nod Down-Up", "Neck Tilt Left-Right", "Neck Turn Left-Right" },
        new[] { "Head", "Head Nod Down-Up", "Head Tilt Left-Right", "Head Turn Left-Right" },
        new[] { "LeftUpperLeg", "Left Upper Leg Front-Back", "Left Upper Leg In-Out" },
        new[] { "RightUpperLeg", "Right Upper Leg Front-Back", "Right Upper Leg In-Out" },
        new[] { "LeftLowerLeg", "Left Lower Leg Stretch" },
        new[] { "RightLowerLeg", "Right Lower Leg Stretch" },
        new[] { "LeftFoot", "Left Foot Up-Down" },
        new[] { "RightFoot", "Right Foot Up-Down" },
        new[] { "LeftToes", "Left Toes Up-Down" },
        new[] { "RightToes", "Right Toes Up-Down" },
        new[] { "LeftShoulder", "Left Shoulder Down-Up", "Left Shoulder Front-Back" },
        new[] { "RightShoulder", "Right Shoulder Down-Up", "Right Shoulder Front-Back" },
        new[] { "LeftUpperArm", "Left Arm Down-Up", "Left Arm Front-Back" },
        new[] { "RightUpperArm", "Right Arm Down-Up", "Right Arm Front-Back" },
        new[] { "LeftLowerArm", "Left Forearm Stretch" },
        new[] { "RightLowerArm", "Right Forearm Stretch" },
        new[] { "LeftHand", "Left Hand Down-Up", "Left Hand In-Out" },
        new[] { "RightHand", "Right Hand Down-Up", "Right Hand In-Out" },
        new[] { "LeftEye", "Left Eye Down-Up", "Left Eye In-Out" },
        new[] { "RightEye", "Right Eye Down-Up", "Right Eye In-Out" },
        new[] { "Jaw", "Jaw Close", "Jaw Left-Right" },
    };
    // A probe sets one muscle to the value for its place in the group: near
    // the muscle's limits, so each probe turns its edges as far as it can.
    static readonly float[] ProbeValues = { 1f, -.9f, .8f };
    // Three noncommuting vectors over every probe muscle of the rig, cycling these values.
    static readonly float[][] CombinedValues = {
        new[] { .3f, -.2f, .25f, -.35f, .15f }, new[] { -.25f, .4f, -.15f, .2f, -.3f }, new[] { .2f, .2f, -.3f, -.1f, .35f }
    };
    // F3 targets, alternating: a normalized offset and a tilt (Euler degrees).
    static readonly Vector3[] TargetOffsets = { new Vector3(.05f, -.03f, .04f), new Vector3(-.04f, .02f, -.06f) };
    static readonly Vector3[] TargetTilts = { new Vector3(12, -20, 7), new Vector3(-9, 35, -11) };

    // A case's rig and the body bones its schedule assumes the built Avatar maps.
    internal sealed class BodyCase
    {
        public string Kind, Rig, Variant = "", Root = "identity";
        public BodyRig Spec;
        public string[] Mapped;
    }

    // One known-vector point: its time, muscles and, for F3, its body target.
    internal sealed class BodyProbe
    {
        public double Time;
        public float[] Muscles;
        public bool Targeted;
        public Vector3 Offset;
        public Vector3 Tilt;
    }

    internal sealed partial class ReplayCase
    {
        public BodyCase Body;
        public BodyProbe[] BodyProbes;
        public BodyClipVariant BodyClip;
    }

    sealed partial class Build
    {
        public readonly List<AssetBundleBuild> BodyBundles = new List<AssetBundleBuild>();

        public void AddBodyBundle(string name, params string[] assets)
        {
            BodyBundles.Add(new AssetBundleBuild {
                assetBundleName = name, assetNames = assets.OrderBy(path => path, StringComparer.Ordinal).ToArray()
            });
        }
    }

    static IEnumerable<string> RigCases(BodyRig rig, bool chain) => chain
        ? new[] { "body-chain-" + rig.Name + "-targets", "body-chain-" + rig.Name + "-clip" }
        : new[] { "body-probes-" + rig.Name }.Concat(TargetRigs.Contains(rig.Name)
            ? new[] { "body-targets-" + rig.Name, "body-targets-" + rig.Name + "-offset" } : Array.Empty<string>());

    static IEnumerable<string> BodyCases() => ProbeRigs.SelectMany(rig => RigCases(rig, false))
        .Concat(ExtraProbes.Select(extra => extra.name)).Concat(TwistProbes.Select(twist => twist.name))
        .Concat(RangeProbes.Select(range => range.name)).Concat(BandProbes.Select(band => band.name))
        .Concat(ClipCases()).Concat(ChainRigs.SelectMany(rig => RigCases(rig, true)));

    // Each body case fails until a step prepares it, so none can vanish unseen.
    static void AddBodyCases(RunReport report) => report.AddPending(BodyCases());

    static void BuildBody(Build build, RunReport report)
    {
        var builtRigs = new HashSet<string>(StringComparer.Ordinal);
        // A rig that came with the range sweeps is a required training rig: Unity refusing its Avatar is an error.
        foreach (var rig in ProbeRigs)
            if (report.Prepare(build, "Prepare body-" + rig.Name, RigCases(rig, false).ToArray(),
                    () => BuildBodyRig(build, report, rig, chain: false), !LaterRigs.Contains(rig.Name)))
                builtRigs.Add(rig.Name);
        // Once its rig builds, an extra probe case is required: Unity refusing it is an error.
        foreach (var extra in ExtraProbes)
            report.Prepare(build, "Prepare " + extra.name, new[] { extra.name },
                () => BuildBodyRig(build, report, ProbeRigs.Single(rig => rig.Name == extra.rig), false, extra),
                !builtRigs.Contains(extra.rig));
        foreach (var twist in TwistProbes)
            report.Prepare(build, "Prepare " + twist.name, new[] { twist.name },
                () => BuildBodyRig(build, report, ProbeRigs.Single(rig => rig.Name == twist.rig), false,
                    (twist.name, twist.rig, null)),
                !builtRigs.Contains(twist.rig));
        foreach (var range in RangeProbes)
            report.Prepare(build, "Prepare " + range.name, new[] { range.name },
                () => BuildBodyRig(build, report, ProbeRigs.Single(rig => rig.Name == range.rig), false,
                    (range.name, range.rig, null), sweep: "range"),
                !builtRigs.Contains(range.rig));
        foreach (var band in BandProbes)
            report.Prepare(build, "Prepare " + band.name, new[] { band.name },
                () => BuildBodyRig(build, report, ProbeRigs.Single(rig => rig.Name == band.rig), false,
                    (band.name, band.rig, null), sweep: "band"),
                !builtRigs.Contains(band.rig));
        foreach (var rig in ChainRigs)
            report.Prepare(build, "Prepare body-chain-" + rig.Name, RigCases(rig, true).ToArray(),
                () => BuildBodyRig(build, report, rig, chain: true), true);
        report.Prepare(build, "Prepare " + BodyClipBundle, ClipCases().ToArray(), () => BuildBodyClips(build, report), true);
        var prepared = new HashSet<string>(build.Replays.Select(replay => replay.OutputName), StringComparer.Ordinal);
        report.FailUnreached(BodyCases().Where(name => !prepared.Contains(name)),
            "No preparation step produced this body fixture.");
    }

    internal sealed class BodyNode { public string Name, Parent; public Vector3 Offset; }

    // The P2b rigs' T-pose (BuildHumanoidSkeleton) with F6's and F5's changes.
    // Unchanged, it repeats the P2b offsets exactly and in the same sibling order.
    static List<BodyNode> BodyLayout(BodyRig rig)
    {
        bool tall = rig.Tall;
        var nodes = new List<BodyNode>();
        void Add(string name, string parent, Vector3 offset) =>
            nodes.Add(new BodyNode { Name = name, Parent = parent, Offset = offset * rig.Size });
        Add("Hips", "", Vector3.up * ((tall ? 1.18f : .88f) - (1 - rig.Legs) * (tall ? 1.06f : .73f)));
        Add("Spine", "Hips", Vector3.up * (tall ? .21f : .16f));
        Add("Chest", "Spine", Vector3.up * (tall ? .27f : .13f));
        string torso = "Chest";
        if (!tall)
        {
            Add("UpperChest", "Chest", Vector3.up * .08f);
            torso = "UpperChest";
        }
        Add("Neck", torso, Vector3.up * .14f);
        Add("Head", "Neck", Vector3.up * .12f);
        if (rig.Extras.Contains("LeftEye")) Add("LeftEye", "Head", new Vector3(-.03f, .06f, .08f));
        if (rig.Extras.Contains("RightEye")) Add("RightEye", "Head", new Vector3(.03f, .06f, .08f));
        if (rig.Extras.Contains("Jaw")) Add("Jaw", "Head", new Vector3(0, -.03f, .05f));
        float hipZ = rig.Noncoplanar ? .03f : 0, shoulderZ = rig.Noncoplanar ? -.025f : 0;
        foreach (var (side, sign) in new[] { ("Left", -1), ("Right", 1) })
        {
            Add(side + "UpperLeg", "Hips", new Vector3(sign * (tall ? .10f : .13f), -.05f, hipZ));
            Add(side + "LowerLeg", side + "UpperLeg", new Vector3(0, (tall ? -.58f : -.36f) * rig.Legs, .04f));
            Add(side + "Foot", side + "LowerLeg", new Vector3(0, (tall ? -.48f : -.37f) * rig.Legs, -.04f));
            Add(side + "Toes", side + "Foot", new Vector3(0, -.04f, .14f));
            Add(side + "Shoulder", torso, new Vector3(sign * .08f, .08f, shoulderZ));
            Add(side + "UpperArm", side + "Shoulder", Vector3.right * (sign * (tall ? .10f : .14f)));
            Add(side + "LowerArm", side + "UpperArm", Vector3.right * (sign * (tall ? .34f : .26f) * rig.Arms));
            Add(side + "Hand", side + "LowerArm", Vector3.right * (sign * (tall ? .26f : .23f) * rig.Arms));
        }
        return nodes;
    }

    // The rig's bones in creation order, each with its parent and offset: an
    // omitted bone's children keep their place from its parent.
    internal static List<BodyNode> RigNodes(BodyRig rig)
    {
        var layout = BodyLayout(rig);
        var byName = layout.ToDictionary(node => node.Name, StringComparer.Ordinal);
        var nodes = new List<BodyNode>();
        foreach (var node in layout)
        {
            if (rig.Omit.Contains(node.Name)) continue;
            var offset = node.Offset;
            string parent = node.Parent;
            while (rig.Omit.Contains(parent))
            {
                offset += byName[parent].Offset;
                parent = byName[parent].Parent;
            }
            nodes.Add(new BodyNode { Name = node.Name, Parent = parent, Offset = offset });
        }
        return nodes;
    }

    static Transform BuildBodySkeleton(Transform root, BodyRig rig)
    {
        var created = new Dictionary<string, Transform>(StringComparer.Ordinal);
        var top = root;
        if (rig.Armature)
        {
            // An FBX-like armature: scaled, translated and turned, with the hips
            // turned back so the T-pose stays upright and facing +Z.
            top = Child(root, "Armature", new Vector3(0, .05f, .1f));
            top.localRotation = Quaternion.Euler(-90, 0, 0);
            top.localScale = Vector3.one * ArmatureScale;
        }
        foreach (var node in RigNodes(rig))
            created[node.Name] = Child(node.Parent.Length == 0 ? top : created[node.Parent], node.Name, node.Offset);
        var hips = created["Hips"];
        if (rig.Armature)
        {
            // Hips at (0, .72, 0) in the rig: the armature's inverse turn of (0, .67, -.1), over its scale.
            hips.localPosition = new Vector3(0, .125f, .8375f);
            hips.localRotation = Quaternion.Euler(90, 0, 0);
        }
        if (rig.HipsScale != 1)
        {
            // The scaled legs are longer: raise the hips by the same factor to keep the feet above the root.
            hips.localScale = Vector3.one * rig.HipsScale;
            hips.localPosition *= rig.HipsScale;
        }
        return hips;
    }

    // Neutral, the spine single-axis vector and the spine noncommuting vector at
    // 0, .5 and 1, as the P2b known vectors, so the Spine endpoint checks hold.
    // The probes (or, for targets, the three combined vectors) fill each half.
    // The rig's single-muscle probes and its three combined vectors over every probe muscle.
    static (List<float[]> singles, List<float[]> combined) ProbeVectors(IEnumerable<string> mapped)
    {
        var names = HumanTrait.MuscleName;
        var present = new HashSet<string>(mapped, StringComparer.Ordinal);
        var singles = new List<float[]>();
        var muscles = new List<int>();
        foreach (var group in ProbeGroups.Where(group => present.Contains(group[0])))
        {
            for (int k = 1; k < group.Length; k++)
            {
                int index = MuscleIndex(names, group[k]);
                muscles.Add(index);
                var vector = new float[names.Length];
                vector[index] = ProbeValues[k - 1];
                singles.Add(vector);
            }
        }
        var combined = CombinedValues.Select(values => {
            var vector = new float[names.Length];
            for (int g = 0; g < muscles.Count; g++) vector[muscles[g]] = values[g % values.Length];
            return vector;
        }).ToList();
        return (singles, combined);
    }

    static BodyProbe[] BodySchedule(IEnumerable<string> mapped, bool targets)
    {
        var (singles, combined) = ProbeVectors(mapped);
        return Scheduled((targets ? combined : singles.Concat(combined)).ToList(), targets);
    }

    // An extra probe case: each listed bone takes its muscles from one of the
    // rig's own probe vectors (single probe k from 1, or combined vector c as
    // -1 - c), every other muscle zero, so each bone's local rotation is one
    // the rig's probe case records.
    static BodyProbe[] ExtraSchedule(IEnumerable<string> mapped, (string bone, int source)[][] plan)
    {
        var names = HumanTrait.MuscleName;
        var (_, combined) = ProbeVectors(mapped);
        var vectors = plan.Select(sources => {
            var vector = new float[names.Length];
            foreach (var (bone, source) in sources)
            {
                var group = ProbeGroups.Single(row => row[0] == bone);
                for (int k = 1; k < group.Length; k++)
                {
                    int index = MuscleIndex(names, group[k]);
                    vector[index] = source > 0 ? (k == source ? ProbeValues[k - 1] : 0) : combined[-1 - source][index];
                }
            }
            return vector;
        }).ToList();
        return Scheduled(vectors, false);
    }

    // A twist sweep: each tilt, then each twist, every point a body target
    // without offset (the spine anchors upright).
    static BodyProbe[] TwistSchedule()
    {
        var names = HumanTrait.MuscleName;
        var vectors = new List<float[]>();
        var tilts = new List<Vector3>();
        foreach (var tilt in TwistTilts)
        {
            foreach (int degrees in TwistDegrees)
            {
                var vector = new float[names.Length];
                foreach (var muscle in BodyTwistMuscles) vector[MuscleIndex(names, muscle)] = (float)(degrees / TwistRange);
                vectors.Add(vector);
                tilts.Add(tilt);
            }
        }
        var probes = Scheduled(vectors, false);
        foreach (var probe in probes)
        {
            int k = vectors.IndexOf(probe.Muscles);
            probe.Targeted = true;
            probe.Tilt = k < 0 ? Vector3.zero : tilts[k];
        }
        return probes;
    }

    // The first count points of the unscrambled Sobol sequence in dims (at most 9) dimensions, in [0, 1):
    // dimension 1 is the van der Corput sequence in base 2, the others SobolDirections', by Gray code.
    static List<double[]> Sobol(int count, int dims)
    {
        var directions = new uint[dims][];
        for (int d = 0; d < dims; d++)
        {
            var v = directions[d] = new uint[SobolBits];
            for (int k = 0; k < SobolBits; k++)
            {
                if (d == 0) { v[k] = 1u << (31 - k); continue; }
                var (degree, a, m) = SobolDirections[d - 1];
                if (k < degree) { v[k] = (uint)m[k] << (31 - k); continue; }
                uint x = v[k - degree] ^ (v[k - degree] >> degree);
                for (int j = 1; j < degree; j++)
                    if (((a >> (degree - 1 - j)) & 1) != 0) x ^= v[k - j];
                v[k] = x;
            }
        }
        var state = new uint[dims];
        var points = new List<double[]>();
        for (int i = 0; i < count; i++)
        {
            points.Add(state.Select(value => value / 4294967296.0).ToArray());
            int low = 0;
            while (((i >> low) & 1) != 0) low++;
            if (i + 1 < count)
                for (int d = 0; d < dims; d++) state[d] ^= directions[d][low];
        }
        return points;
    }

    // A range sweep: the Sobol points over the rig's mapped spine bones, then the joint corners but the first.
    // A band sweep: the scaled Sobol points, then the side bends under each tilt.
    static BodyProbe[] RangeSchedule(IEnumerable<string> mapped, bool band = false)
    {
        var names = HumanTrait.MuscleName;
        var present = new HashSet<string>(mapped, StringComparer.Ordinal);
        var groups = ProbeGroups.Where(group => RangeBones.Contains(group[0]) && present.Contains(group[0]))
            .Select(group => group.Skip(1).Select(muscle => MuscleIndex(names, muscle)).ToArray()).ToList();
        var scales = band ? new[] { 1, BandLateral, BandTwist } : new[] { 1.0, 1, 1 };
        var points = Sobol(RangePoints, 3 * groups.Count)
            .Select(point => point.Select((x, d) => (float)((2 * x - 1) * scales[d % 3])).ToArray()).ToList();
        var tilts = new List<Vector3>();
        int block = RangePoints / TwistTilts.Length;
        for (int k = 0; k < RangePoints; k++) tilts.Add(TwistTilts[k / block]);
        if (band)
        {
            foreach (var level in BandLevels)
                foreach (var sign in new[] { 1, -1 })
                    foreach (var tilt in TwistTilts)
                    {
                        var bend = new float[3 * groups.Count];
                        for (int i = 1; i < bend.Length; i += 3) bend[i] = (float)(sign * level);
                        points.Add(bend);
                        tilts.Add(tilt);
                    }
        }
        else
        {
            for (int k = 1; k < 8; k++)
            {
                var corner = new float[3 * groups.Count];
                for (int i = 0; i < corner.Length; i++) corner[i] = ((k >> (2 - i % 3)) & 1) != 0 ? 1f : -1f;
                points.Add(corner);
                tilts.Add(TwistTilts[(k - 1) % TwistTilts.Length]);
            }
        }
        var vectors = points.Select(point => {
            var vector = new float[names.Length];
            for (int g = 0; g < groups.Count; g++)
                for (int k = 0; k < 3; k++) vector[groups[g][k]] = point[3 * g + k];
            return vector;
        }).ToList();
        var probes = Scheduled(vectors, false);
        foreach (var probe in probes)
        {
            int k = vectors.IndexOf(probe.Muscles);
            probe.Targeted = true;
            probe.Tilt = k < 0 ? Vector3.zero : tilts[k];
        }
        return probes;
    }

    static BodyProbe[] Scheduled(List<float[]> vectors, bool targets)
    {
        int first = (vectors.Count + 1) / 2, second = vectors.Count - first;
        // Times on a 2^-16 grid, so every one is exact in the JSON and as an f32.
        BodyProbe Point(double time, float[] vector) => new BodyProbe { Time = Math.Round(time * 65536) / 65536, Muscles = vector };
        var probes = new List<BodyProbe> { Point(0, KnownHumanoidMuscles(0)) };
        for (int k = 0; k < first; k++) probes.Add(Point(.5 * (k + 391.0 / 1024) / first, vectors[k]));
        probes.Add(Point(.5, KnownHumanoidMuscles(.5f)));
        for (int k = 0; k < second; k++) probes.Add(Point(.5 + .5 * (k + 391.0 / 1024) / second, vectors[first + k]));
        probes.Add(Point(1, KnownHumanoidMuscles(1)));
        if (targets)
        {
            for (int i = 0; i < probes.Count; i++)
            {
                probes[i].Targeted = true;
                probes[i].Offset = TargetOffsets[i % 2];
                probes[i].Tilt = TargetTilts[i % 2];
            }
        }
        return probes.ToArray();
    }

    static ReplayCase BodyReplay(string bundle, string prefab, string avatar, string clip, string name, BodyCase body,
        string probe)
    {
        var replay = HumanoidReplay(bundle, prefab, avatar, clip, name);
        replay.Optional = true;
        replay.ProbePath = probe;
        replay.Body = body;
        return replay;
    }

    static void AddProbeReplay(Build build, ReplayCase replay, BodyProbe[] probes)
    {
        replay.BodyProbes = probes;
        replay.KnownMuscles = time => probes.Single(point => (float)point.Time == time).Muscles;
        build.Replays.Add(replay);
    }

    // A probe rig (F1, with F3 on the P2b rigs) or a chain rig (F5).
    static void BuildBodyRig(Build build, RunReport report, BodyRig rig, bool chain,
        (string name, string rig, (string bone, int source)[][] plan) extra = default, string sweep = "twist")
    {
        // An extra probe case or sweep has its own bundle, so a rerun leaves the rig's other cases unchanged.
        string bundle = extra.name != null ? extra.name.Replace("probes-", "") : (chain ? "body-chain-" : "body-") + rig.Name;
        var rigObject = new GameObject("HumanoidRig");
        GameObject root = null;
        Avatar avatar = null;
        try
        {
            var hips = BuildBodySkeleton(rigObject.transform, rig);
            var skeleton = rigObject.GetComponentsInChildren<Transform>();
            avatar = AvatarBuilder.BuildHumanAvatar(rigObject, DescribeHumanoid(skeleton, rig.Tall));
            Unavailable(avatar != null && avatar.isValid && avatar.isHuman,
                "AvatarBuilder.BuildHumanAvatar did not accept the " + rig.Name + " rig as a valid Humanoid Avatar.");
            avatar.name = bundle;
            avatar.hideFlags = HideFlags.None;

            string dir = Folder(Assets + "/" + bundle);
            var animator = SaveCoverageRig(rigObject, hips, skeleton, avatar, dir, out var assets);
            var muscles = AuthorHumanoidClip(dir, bundle + "-muscles", bodyMotion: false);
            SetNeutralSettings(muscles, 0);
            var clips = new List<AnimationClip> { muscles };
            // The chain clip counts only with its motion curves; without them its
            // case is not covered and the chain's targets still record.
            string chainMissing = null;
            if (chain)
            {
                AnimationClip chainClip = null;
                try { chainClip = AuthorBodyClip(dir, bundle + "-clip", ChainVariant); }
                catch (CoverageUnavailable error) { chainMissing = error.Message; }
                if (chainMissing == null && !KeepsCurves(chainClip, "MotionT.x", "MotionQ.w"))
                    chainMissing = "The Editor dropped the chain clip's MotionT and MotionQ curves.";
                if (chainMissing == null) clips.Add(chainClip);
            }
            animator.runtimeAnimatorController = Controller(assets[assets.Length - 1], clips.ToArray());

            root = new GameObject("HumanoidFixture");
            rigObject.transform.SetParent(root.transform, false);
            if (rig.ScaledAnimator) rigObject.transform.localScale = Vector3.one * AnimatorScale;
            string prefab = SavePrefab(root, dir + "/identity.prefab");
            bool targets = !chain && extra.name == null && TargetRigs.Contains(rig.Name);
            string offsetPrefab = null;
            if (targets)
            {
                rigObject.transform.localPosition = new Vector3(2, .25f, -1);
                rigObject.transform.localRotation = Quaternion.Euler(0, 35, 0);
                offsetPrefab = SavePrefab(root, dir + "/offset.prefab");
            }
            var clipPaths = clips.Select(AssetDatabase.GetAssetPath).ToArray();
            var prefabs = offsetPrefab == null ? new[] { prefab } : new[] { prefab, offsetPrefab };
            build.AddBodyBundle(bundle, assets.Concat(clipPaths).Concat(prefabs).ToArray());

            var mapped = skeleton.Select(bone => bone.name).Where(HumanTrait.BoneName.Contains).ToArray();
            string probe = rig.Armature ? "HumanoidRig/Armature/Hips/Spine" : "HumanoidRig/Hips/Spine";
            ReplayCase Replay(string name, string path, string clip, string kind, string rootKind) =>
                BodyReplay(bundle, path, assets[0], clip, name,
                    new BodyCase { Kind = kind, Rig = rig.Name, Root = rootKind, Spec = rig, Mapped = mapped }, probe);
            if (chain)
            {
                AddProbeReplay(build, Replay(bundle + "-targets", prefab, clipPaths[0], "targets", "identity"),
                    BodySchedule(mapped, targets: true));
                if (chainMissing != null)
                {
                    report.Set(bundle + "-clip", "not_covered", chainMissing);
                    return;
                }
                var clipReplay = Replay(bundle + "-clip", prefab, clipPaths[1], "clip", "identity");
                clipReplay.Body.Variant = ChainVariant.Name;
                clipReplay.BodyClip = ChainVariant;
                build.Replays.Add(clipReplay);
                return;
            }
            if (extra.name != null)
            {
                // A null plan is a twist, range or band sweep.
                var extraReplay = Replay(extra.name, prefab, clipPaths[0], "probes", "identity");
                extraReplay.Body.Variant = extra.plan == null ? sweep : "extra";
                AddProbeReplay(build, extraReplay, extra.plan != null ? ExtraSchedule(mapped, extra.plan)
                    : sweep == "range" ? RangeSchedule(mapped) : sweep == "band" ? RangeSchedule(mapped, band: true)
                    : TwistSchedule());
                return;
            }
            AddProbeReplay(build, Replay("body-probes-" + rig.Name, prefab, clipPaths[0], "probes", "identity"),
                BodySchedule(mapped, targets: false));
            if (!targets) return;
            foreach (var (path, suffix, rootKind) in new[] { (prefab, "", "identity"), (offsetPrefab, "-offset", "offset") })
                AddProbeReplay(build, Replay("body-targets-" + rig.Name + suffix, path, clipPaths[0], "targets", rootKind),
                    BodySchedule(mapped, targets: true));
        }
        finally
        {
            Object.DestroyImmediate(root == null ? rigObject : root);
            if (avatar != null && !AssetDatabase.Contains(avatar)) Object.DestroyImmediate(avatar);
        }
    }
}
#endif
