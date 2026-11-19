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
        new BodyRig { Name = "eyes", Extras = new[] { "LeftEye", "RightEye" } },
        new BodyRig { Name = "jaw", Extras = new[] { "Jaw" } },
        new BodyRig { Name = "eyes-jaw", Extras = new[] { "LeftEye", "RightEye", "Jaw" } },
        new BodyRig { Name = "noncoplanar", Noncoplanar = true },
        new BodyRig { Name = "holdout", Arms = 1.3f, Legs = .8f },
    };
    // F5: each plays F3's targets and the F4 all-off clip.
    static readonly BodyRig[] ChainRigs = {
        new BodyRig { Name = "scaled-animator", ScaledAnimator = true },
        new BodyRig { Name = "large", Size = 1.5f },
        new BodyRig { Name = "armature", Armature = true },
        new BodyRig { Name = "hips-scale", HipsScale = 1.2f },
    };
    static readonly string[] TargetRigs = { "compact", "tall" };
    const float AnimatorScale = 1.25f, ArmatureScale = .8f;

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
    // A probe sets one muscle to the value for its place in the group.
    static readonly float[] ProbeValues = { .6f, -.5f, .4f };
    // Three noncommuting vectors over every probe muscle of the rig, cycling these values.
    static readonly float[][] CombinedValues = {
        new[] { .3f, -.2f, .25f, -.35f, .15f }, new[] { -.25f, .4f, -.15f, .2f, -.3f }, new[] { .2f, .2f, -.3f, -.1f, .35f }
    };
    // F3 targets, alternating: a normalized offset and a tilt (Euler degrees).
    static readonly Vector3[] TargetOffsets = { new Vector3(.05f, -.03f, .04f), new Vector3(-.04f, .02f, -.06f) };
    static readonly Vector3[] TargetTilts = { new Vector3(12, -20, 7), new Vector3(-9, 35, -11) };

    internal sealed class BodyCase { public string Kind, Rig, Variant = "", Root = "identity"; }

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
        .Concat(ClipCases()).Concat(ChainRigs.SelectMany(rig => RigCases(rig, true)));

    // Each body case fails until a step prepares it, so none can vanish unseen.
    static void AddBodyCases(RunReport report) => report.AddPending(BodyCases());

    static void BuildBody(Build build, RunReport report)
    {
        foreach (var rig in ProbeRigs)
            report.Prepare(build, "Prepare body-" + rig.Name, RigCases(rig, false).ToArray(),
                () => BuildBodyRig(build, rig, chain: false), true);
        foreach (var rig in ChainRigs)
            report.Prepare(build, "Prepare body-chain-" + rig.Name, RigCases(rig, true).ToArray(),
                () => BuildBodyRig(build, rig, chain: true), true);
        report.Prepare(build, "Prepare " + BodyClipBundle, ClipCases().ToArray(), () => BuildBodyClips(build, report), true);
        var prepared = new HashSet<string>(build.Replays.Select(replay => replay.OutputName), StringComparer.Ordinal);
        report.FailUnreached(BodyCases().Where(name => !prepared.Contains(name)),
            "No preparation step produced this body fixture.");
    }

    sealed class BodyNode { public string Name, Parent; public Vector3 Offset; }

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

    static Transform BuildBodySkeleton(Transform root, BodyRig rig)
    {
        var layout = BodyLayout(rig);
        var byName = layout.ToDictionary(node => node.Name, StringComparer.Ordinal);
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
        foreach (var node in layout)
        {
            if (rig.Omit.Contains(node.Name)) continue;
            // An omitted bone's children keep their place from its parent.
            var offset = node.Offset;
            string parent = node.Parent;
            while (rig.Omit.Contains(parent))
            {
                offset += byName[parent].Offset;
                parent = byName[parent].Parent;
            }
            created[node.Name] = Child(parent.Length == 0 ? top : created[parent], node.Name, offset);
        }
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
    static BodyProbe[] BodySchedule(IEnumerable<string> mapped, bool targets)
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
        });
        var vectors = (targets ? combined : singles.Concat(combined)).ToList();
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
    static void BuildBodyRig(Build build, BodyRig rig, bool chain)
    {
        string bundle = (chain ? "body-chain-" : "body-") + rig.Name;
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
            if (chain) clips.Add(AuthorBodyClip(dir, bundle + "-clip", ChainVariant));
            animator.runtimeAnimatorController = Controller(assets[assets.Length - 1], clips.ToArray());

            root = new GameObject("HumanoidFixture");
            rigObject.transform.SetParent(root.transform, false);
            if (rig.ScaledAnimator) rigObject.transform.localScale = Vector3.one * AnimatorScale;
            string prefab = SavePrefab(root, dir + "/identity.prefab");
            bool targets = !chain && TargetRigs.Contains(rig.Name);
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
                BodyReplay(bundle, path, assets[0], clip, name, new BodyCase { Kind = kind, Rig = rig.Name, Root = rootKind }, probe);
            if (chain)
            {
                AddProbeReplay(build, Replay(bundle + "-targets", prefab, clipPaths[0], "targets", "identity"),
                    BodySchedule(mapped, targets: true));
                var clipReplay = Replay(bundle + "-clip", prefab, clipPaths[1], "clip", "identity");
                clipReplay.Body.Variant = ChainVariant.Name;
                clipReplay.BodyClip = ChainVariant;
                build.Replays.Add(clipReplay);
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
