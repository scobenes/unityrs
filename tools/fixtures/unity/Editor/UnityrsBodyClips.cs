#if UNITY_EDITOR
using System;
using System.Collections.Generic;
using System.Linq;
using UnityEditor;
using UnityEngine;
using Object = UnityEngine.Object;

// F4: body-motion clips on the compact rig, each on an identity and an offset
// root. Every clip has XZ and Y motion and a tilted RootQ whose yaw turns back
// and forth. The variants: the eight keep-original combinations with bake off,
// the seven other bake combinations with keep-original off, height from feet,
// a level offset and an orientation offset alone; each of those again starting
// at a nonzero XZ and yaw; motion curves apart from the body; and height from
// feet with a knee bend, with and without foot goal curves.
//
// E5: the variants B4 admits, each with its start-offset twin, on an identity
// and an offset root, on every clip rig (ClipRigs), each in its own bundle;
// and full-body clips, every muscle on a curve of its own, on rigs mapping
// every bone a muscle turns: refusal evidence over freeze v8's lateral limit
// (FullRigs), and clips inside its scope on every chain feature, body
// topology and admitted variant (ScopedRigs).
static partial class UnityrsFixtureBuilder
{
    const string BodyClipBundle = "body-clips";

    internal sealed class BodyClipVariant
    {
        public string Name;
        // Orientation, Y, XZ: bake into pose, and keep original.
        public bool[] Bake = new bool[3], Keep = new bool[3];
        public bool HeightFromFeet, Start, Motion, Knee, Goals;
        public float Level, Orientation;

        public BodyClipVariant Starting() => new BodyClipVariant {
            Name = Name + "-start", Bake = Bake, Keep = Keep, HeightFromFeet = HeightFromFeet, Level = Level,
            Orientation = Orientation, Start = true
        };
    }

    static readonly BodyClipVariant[] ClipVariants = ClipVariantTable();
    // F5's clip: the original basis on every axis with MotionT and MotionQ
    // curves that change, so the residual body translation and tilt change
    // through each scaled chain and the original-root rule has motion to keep.
    static BodyClipVariant ChainVariant => ClipVariants.Single(row => row.Name == "keep-o1-y1-xz1-motion");

    static string FlagName(int flags) => "-o" + (flags >> 2 & 1) + "-y" + (flags >> 1 & 1) + "-xz" + (flags & 1);

    static BodyClipVariant[] ClipVariantTable()
    {
        var rows = new List<BodyClipVariant>();
        for (int flags = 0; flags < 8; flags++)
            rows.Add(new BodyClipVariant { Name = "keep" + FlagName(flags), Keep = BasisFlags(flags) });
        for (int flags = 1; flags < 8; flags++)
            rows.Add(new BodyClipVariant { Name = "bake" + FlagName(flags), Bake = BasisFlags(flags) });
        rows.Add(new BodyClipVariant { Name = "height-from-feet", HeightFromFeet = true });
        rows.Add(new BodyClipVariant { Name = "level", Level = .125f });
        rows.Add(new BodyClipVariant { Name = "orientation-offset", Orientation = 30 });
        rows.AddRange(rows.ToArray().Select(row => row.Starting()));
        rows.Add(new BodyClipVariant { Name = "keep-o1-y1-xz1-motion", Keep = BasisFlags(7), Motion = true });
        rows.Add(new BodyClipVariant { Name = "height-from-feet-knee", HeightFromFeet = true, Knee = true });
        rows.Add(new BodyClipVariant { Name = "height-from-feet-knee-goals", HeightFromFeet = true, Knee = true, Goals = true });
        return rows.ToArray();
    }

    // The variants B4 admits (freeze v8's E4 passing set, anim_fixture_round11.py
    // E5_ADMITTED). Each records with its start-offset twin where it has one.
    static readonly string[] E5Admitted = {
        "bake-o0-y0-xz1", "bake-o0-y1-xz0", "bake-o0-y1-xz1", "bake-o1-y0-xz0", "bake-o1-y0-xz1", "bake-o1-y1-xz0",
        "bake-o1-y1-xz1", "keep-o0-y0-xz0", "keep-o1-y1-xz1-motion", "level", "orientation-offset"
    };
    static BodyClipVariant[] E5Variants => ClipVariants.Where(row => E5Admitted.Contains(
        row.Start ? row.Name.Substring(0, row.Name.Length - "-start".Length) : row.Name)).ToArray();

    // E5's clip rigs (anim_fixture_round11.py CLIP_RIGS): the plain chain at
    // B4's highest human scale (F5's large rig), the tall rig's and near its
    // lowest (SmallRig), then every training topology B3's topology scope
    // admits but the compact rig's, whose clips body-clips records. No rig B3
    // refuses (no-chest, no-neck, one shoulder) and never a hold-out rig.
    static readonly BodyRig SmallRig = new BodyRig { Name = "small", Size = .82f };
    // E5's chain rigs (anim_fixture_round11.py CHAIN_RIGS): each F5 feature at a
    // second human scale inside B4's range, built as ChainRigs are. They are E5
    // recordings, not B4's training rigs, so they stay out of ChainRigs, which
    // B4's tests read as freeze v8's set.
    static readonly BodyRig[] E5ChainRigs = {
        new BodyRig { Name = "scaled-animator-large", ScaledAnimator = true, Size = 1.5f },
        new BodyRig { Name = "scaled-animator-small", ScaledAnimator = true, Size = .82f },
        new BodyRig { Name = "hips-scale-large", HipsScale = 1.2f, Size = 1.2f },
        new BodyRig { Name = "hips-scale-small", HipsScale = 1.2f, Size = .7f },
        new BodyRig { Name = "armature-large", Armature = true, Size = 1.8f },
    };
    static readonly string[] E5Topologies = {
        "no-upper-chest", "no-shoulders", "no-left-toes", "no-right-toes", "no-toes", "no-upper-chest-toes",
        "no-upper-chest-left-toes", "no-upper-chest-right-toes", "eyes", "jaw", "eyes-jaw", "noncoplanar"
    };
    // E5's full-body clips (anim_fixture_full.py): every muscle on a curve, on
    // rigs that map every bone a muscle turns (the face and all thirty fingers
    // added), so a recording carries each bone's channels through a real clip.
    // The compact, large and small plain chains play four admitted variants at
    // both placements; the large armature plays the chain variant at the
    // identity, the one pairing B4 admits for an F5 feature.
    static readonly string[] FullVariants = { "keep-o0-y0-xz0", "bake-o1-y1-xz1", "level", "keep-o1-y1-xz1-motion" };
    static string[] FullExtras => Face.Concat(FingerBones).ToArray();
    static BodyRig[] FullRigs => new[] {
        new BodyRig { Name = "compact-full", Full = true, Extras = FullExtras },
        new BodyRig { Name = "large-full", Full = true, Extras = FullExtras, Size = 1.5f },
        new BodyRig { Name = "small-full", Full = true, Extras = FullExtras, Size = .82f },
        new BodyRig { Name = "armature-large-full", Full = true, Extras = FullExtras, Armature = true, Size = 1.8f },
    };
    // E5's in-scope full-body clips (anim_fixture_full.py SCOPED_RIGS): every
    // muscle on a curve, the Spine's three too, with the frozen-limited ones
    // inside freeze v8's scope (ScopedAmplitudes). The compact rig plays every
    // admitted variant, the large and small rigs FullVariants, each F5 feature
    // the chain variant at the identity, every other body topology B3 admits one
    // variant; the plain chains at both placements.
    static readonly string[] ScopedTopologies = {
        "no-upper-chest", "no-shoulders", "no-left-toes", "no-right-toes", "no-toes", "no-upper-chest-toes",
        "no-upper-chest-left-toes", "no-upper-chest-right-toes", "noncoplanar"
    };
    static BodyRig[] ScopedRigs => new[] {
        new BodyRig { Name = "compact-scoped", Full = true, Scoped = true, Extras = FullExtras, Variants = E5Admitted },
        new BodyRig { Name = "large-scoped", Full = true, Scoped = true, Extras = FullExtras, Size = 1.5f, Variants = FullVariants },
        new BodyRig { Name = "small-scoped", Full = true, Scoped = true, Extras = FullExtras, Size = .82f, Variants = FullVariants },
        new BodyRig { Name = "armature-scoped", Full = true, Scoped = true, Extras = FullExtras, Armature = true, Size = 1.25f,
            Variants = new[] { ChainVariant.Name } },
        new BodyRig { Name = "scaled-animator-scoped", Full = true, Scoped = true, Extras = FullExtras, ScaledAnimator = true,
            Variants = new[] { ChainVariant.Name } },
        new BodyRig { Name = "hips-scale-scoped", Full = true, Scoped = true, Extras = FullExtras, HipsScale = 1.2f,
            Variants = new[] { ChainVariant.Name } },
    }.Concat(ScopedTopologies.Select(name => ProbeRigs.Single(rig => rig.Name == name)).Select(topology => new BodyRig {
        Name = topology.Name + "-scoped", Full = true, Scoped = true, Extras = FullExtras, Omit = topology.Omit,
        Noncoplanar = topology.Noncoplanar, Variants = new[] { "keep-o0-y0-xz0" }
    })).ToArray();
    // Each muscle's curve shape at BodyTimes: eighths in [-1, 1], the muscle index
    // taken modulo 17, 13 and 11 at different keys, so no two of the 95 curves
    // are equal and a swapped channel changes the pose.
    static readonly int[] Moduli = { 17, 13, 11, 13, 17 };
    static float CurveUnit(int muscle, int key) => ((muscle * 7 + 3 + key * 5) % Moduli[key] - (Moduli[key] - 1) / 2) / 8f;
    // An in-scope clip's frozen-limited muscles: the spine Left-Right ones within
    // the lateral limit 0.193359375, and the spine twist and shoulder Front-Back
    // ones, which turn the shoulders about the body's up, well inside the
    // 4.25 degree shoulder-hip twist limit. Every other muscle's amplitude is 1/2.
    static readonly (string[] muscles, float amplitude)[] ScopedAmplitudes = {
        (new[] { "Spine Left-Right", "Chest Left-Right", "UpperChest Left-Right" }, 3 / 16f),
        (new[] { "Spine Twist Left-Right", "Chest Twist Left-Right", "UpperChest Twist Left-Right" }, 1 / 32f),
        (new[] { "Left Shoulder Front-Back", "Right Shoulder Front-Back" }, 1 / 8f),
    };
    static float Amplitude(string name, bool scoped) => scoped
        ? ScopedAmplitudes.Where(row => row.muscles.Contains(name)).Select(row => row.amplitude).DefaultIfEmpty(.5f).First() : .5f;
    static float[] FullValues(int muscle, bool scoped) =>
        BodyTimes.Select((_, key) => Amplitude(HumanTrait.MuscleName[muscle], scoped) * CurveUnit(muscle, key)).ToArray();
    // An in-scope clip curves every muscle; a refusal clip every muscle but the
    // Spine's three, which keep AuthorHumanoidClip's curves (Left-Right .375).
    static int[] FullMuscles(bool scoped) => Enumerable.Range(0, HumanTrait.MuscleCount)
        .Where(i => scoped || !SpineMuscles.Contains(HumanTrait.MuscleName[i])).ToArray();
    // The Animator curve property Unity binds for a HumanTrait muscle
    // (anim_fixture_full.py muscle_property): "LeftHand.Thumb.1 Stretched" for
    // "Left Thumb 1 Stretched", a body muscle's own name. VerifyMuscleBindings
    // fails the run where Unity does not bind one.
    static string MuscleProperty(string name)
    {
        var parts = name.Split(new[] { ' ' }, 3);
        return parts.Length == 3 && Sides.Any(row => row.side == parts[0]) && Fingers.Contains(parts[1])
            ? parts[0] + "Hand." + parts[1] + "." + parts[2] : name;
    }

    // A rig's clip variants, in ClipVariants order: every variant on the compact
    // rig, the admitted ones on an E5 clip rig, an in-scope rig's own, FullVariants
    // on a plain full rig and the chain variant on an F5 one.
    static BodyClipVariant[] RigVariants(BodyRig rig) => rig == null ? ClipVariants : !rig.Full ? E5Variants
        : rig.Variants != null ? ClipVariants.Where(row => rig.Variants.Contains(row.Name)).ToArray()
        : rig.Armature ? new[] { ChainVariant } : ClipVariants.Where(row => FullVariants.Contains(row.Name)).ToArray();
    // Only a plain chain plays at the offset placement too.
    static bool Placed(BodyRig rig) => rig == null || !rig.Armature && !rig.ScaledAnimator && rig.HipsScale == 1;

    static BodyRig[] ClipRigs => new[] { ChainRigs.Single(rig => rig.Name == "large"), ProbeRigs.Single(rig => rig.Name == "tall"),
        SmallRig }.Concat(E5Topologies.Select(name => ProbeRigs.Single(rig => rig.Name == name))).ToArray();

    // The compact rig's bundle body-clips, or a clip rig's own.
    static string ClipBundle(BodyRig rig) => rig == null ? BodyClipBundle : BodyClipBundle + "-" + rig.Name;
    static string ClipName(BodyRig rig, BodyClipVariant row) => "body-clip-" + (rig == null ? "" : rig.Name + "-") + row.Name;

    static IEnumerable<string> ClipCases(BodyRig rig = null) => RigVariants(rig).SelectMany(row =>
        Placed(rig) ? new[] { ClipName(rig, row), ClipName(rig, row) + "-offset" } : new[] { ClipName(rig, row) });

    static readonly float[] BodyTimes = { 0, .25f, .5f, .75f, 1 };
    // RootT in normalized units, and RootQ as Euler degrees (tilt, yaw, roll).
    static readonly Vector3[] BodyPath = {
        new Vector3(0, 1, 0), new Vector3(.08f, .97f, .1f), new Vector3(.2f, 1.02f, .12f),
        new Vector3(.26f, .95f, .25f), new Vector3(.3f, 1, .3f)
    };
    static readonly Vector3[] BodyTurn = {
        new Vector3(0, 0, 0), new Vector3(8, 25, -4), new Vector3(12, 40, -6), new Vector3(6, 15, 3), new Vector3(10, 30, -5)
    };
    static readonly Vector3 StartOffset = new Vector3(.6f, 0, -.4f);
    const float StartYaw = 50;
    // Original root motion apart from the body: MotionT, and MotionQ's yaw.
    // Its Y starts above zero and changes, unlike a zero or start-height rule.
    static readonly Vector3[] RootMotionPath = {
        new Vector3(0, .04f, 0), new Vector3(.1f, .07f, .05f), new Vector3(.15f, .02f, .2f), new Vector3(.3f, .09f, .2f),
        new Vector3(.4f, .05f, .35f)
    };
    static readonly float[] MotionYaw = { 0, 10, 5, 20, 12 };
    // At 0, .5 and 1: each knee bends in turn, so the lower foot changes.
    static readonly float[] KneeTimes = { 0, .5f, 1 };
    static readonly (string muscle, float[] values)[] KneeCurves = {
        ("Left Upper Leg Front-Back", new[] { 0, .5f, .1f }), ("Left Lower Leg Stretch", new[] { 0, -.7f, -.1f }),
        ("Right Upper Leg Front-Back", new[] { 0, .1f, .5f }), ("Right Lower Leg Stretch", new[] { 0, -.1f, -.7f }),
    };
    static readonly (string goal, Vector3[] path)[] FootGoals = {
        ("LeftFoot", new[] { new Vector3(-.1f, .05f, 0), new Vector3(-.1f, .2f, .05f), new Vector3(-.1f, .08f, .1f) }),
        ("RightFoot", new[] { new Vector3(.1f, .05f, 0), new Vector3(.1f, .1f, .1f), new Vector3(.1f, .25f, .05f) }),
    };

    // Linear segments between the keys.
    static AnimationCurve PolyLine(float[] times, float[] values)
    {
        var keys = new Keyframe[times.Length];
        for (int i = 0; i < times.Length; i++)
        {
            float before = i == 0 ? 0 : (values[i] - values[i - 1]) / (times[i] - times[i - 1]);
            float after = i == times.Length - 1 ? before : (values[i + 1] - values[i]) / (times[i + 1] - times[i]);
            keys[i] = new Keyframe(times[i], values[i], i == 0 ? after : before, after);
        }
        return new AnimationCurve(keys);
    }

    static void SetVectorCurves(AnimationClip clip, string prefix, float[] times, Vector3[] values)
    {
        const string axes = "xyz";
        for (int i = 0; i < 3; i++)
            clip.SetCurve("", typeof(Animator), prefix + "." + axes[i], PolyLine(times, values.Select(v => v[i]).ToArray()));
    }

    static void SetRotationCurves(AnimationClip clip, string prefix, float[] times, Quaternion[] values)
    {
        const string axes = "xyzw";
        for (int i = 0; i < 4; i++)
            clip.SetCurve("", typeof(Animator), prefix + "." + axes[i], PolyLine(times, values.Select(q => q[i]).ToArray()));
    }

    static void AddBodyCurves(AnimationClip clip, BodyClipVariant variant, bool full, bool scoped)
    {
        // The start variant moves the whole body path rigidly: turned about +Y, then offset in XZ.
        var start = Quaternion.Euler(0, variant.Start ? StartYaw : 0, 0);
        var offset = variant.Start ? StartOffset : Vector3.zero;
        SetVectorCurves(clip, "RootT", BodyTimes, BodyPath.Select(p => start * p + offset).ToArray());
        SetRotationCurves(clip, "RootQ", BodyTimes, BodyTurn.Select(e => start * Quaternion.Euler(e.x, e.y, e.z)).ToArray());
        if (variant.Motion)
        {
            SetVectorCurves(clip, "MotionT", BodyTimes, RootMotionPath);
            SetRotationCurves(clip, "MotionQ", BodyTimes, MotionYaw.Select(yaw => Quaternion.Euler(0, yaw, 0)).ToArray());
        }
        var names = HumanTrait.MuscleName;
        if (full)
            foreach (int muscle in FullMuscles(scoped))
                clip.SetCurve("", typeof(Animator), MuscleProperty(names[muscle]), PolyLine(BodyTimes, FullValues(muscle, scoped)));
        if (variant.Knee)
            foreach (var (muscle, values) in KneeCurves)
                clip.SetCurve("", typeof(Animator), names[MuscleIndex(names, muscle)], PolyLine(KneeTimes, values));
        if (variant.Goals)
        {
            foreach (var (goal, path) in FootGoals)
            {
                SetVectorCurves(clip, goal + "T", KneeTimes, path);
                SetRotationCurves(clip, goal + "Q", KneeTimes, KneeTimes.Select(_ => Quaternion.identity).ToArray());
            }
        }
        clip.EnsureQuaternionContinuity();
    }

    static AnimationClip AuthorBodyClip(string dir, string name, BodyClipVariant variant, bool full = false, bool scoped = false)
    {
        var clip = AuthorHumanoidClip(dir, name, bodyMotion: false,
            extra: authored => AddBodyCurves(authored, variant, full, scoped));
        SetBodySettings(clip, variant);
        return clip;
    }

    static bool BodySettingsMatch(AnimationClipSettings s, BodyClipVariant variant) =>
        s.startTime == 0 && s.stopTime == 1 && s.cycleOffset == 0 && s.additiveReferencePoseTime == 0
        && s.additiveReferencePoseClip == null && !s.loopTime && !s.loopBlend && !s.mirror && !s.hasAdditiveReferencePose
        && s.loopBlendOrientation == variant.Bake[0] && s.loopBlendPositionY == variant.Bake[1]
        && s.loopBlendPositionXZ == variant.Bake[2] && s.keepOriginalOrientation == variant.Keep[0]
        && s.keepOriginalPositionY == variant.Keep[1] && s.keepOriginalPositionXZ == variant.Keep[2]
        && s.heightFromFeet == variant.HeightFromFeet && s.level == variant.Level
        && s.orientationOffsetY == variant.Orientation;

    static void SetBodySettings(AnimationClip clip, BodyClipVariant variant)
    {
        var settings = AnimationUtility.GetAnimationClipSettings(clip);
        settings.loopBlendOrientation = variant.Bake[0];
        settings.loopBlendPositionY = variant.Bake[1];
        settings.loopBlendPositionXZ = variant.Bake[2];
        settings.keepOriginalOrientation = variant.Keep[0];
        settings.keepOriginalPositionY = variant.Keep[1];
        settings.keepOriginalPositionXZ = variant.Keep[2];
        settings.heightFromFeet = variant.HeightFromFeet;
        settings.level = variant.Level;
        settings.orientationOffsetY = variant.Orientation;
        AnimationUtility.SetAnimationClipSettings(clip, settings);
        EditorUtility.SetDirty(clip);
        Unavailable(BodySettingsMatch(AnimationUtility.GetAnimationClipSettings(clip), variant),
            "Unity did not keep the authored root settings of " + clip.name + ".");
    }

    // Whether the Editor keeps authored curves under these Animator property names.
    static bool KeepsCurves(AnimationClip clip, params string[] properties)
    {
        var bound = new HashSet<string>(AnimationUtility.GetCurveBindings(clip)
            .Where(binding => binding.path.Length == 0 && binding.type == typeof(Animator))
            .Select(binding => binding.propertyName), StringComparer.Ordinal);
        return properties.All(bound.Contains);
    }

    // The compact rig's clips (`spec` null, every variant), an E5 clip rig's (the
    // admitted variants) or an E5 full-body clip rig's (RigVariants).
    static void BuildBodyClips(Build build, RunReport report, BodyRig spec = null)
    {
        var rigSpec = spec ?? ProbeRigs[0];
        string bundle = ClipBundle(spec);
        Require(rigSpec.Full || !rigSpec.ScaledAnimator && rigSpec.HipsScale == 1 && !rigSpec.Armature,
            "A clip rig is a plain chain, or a full-body clip rig: " + rigSpec.Name + ".");
        var rig = new GameObject("HumanoidRig");
        GameObject root = null;
        Avatar avatar = null;
        try
        {
            var hips = BuildBodySkeleton(rig.transform, rigSpec);
            var skeleton = rig.GetComponentsInChildren<Transform>();
            avatar = AvatarBuilder.BuildHumanAvatar(rig, DescribeHumanoid(skeleton, rigSpec.Tall));
            Require(avatar != null && avatar.isValid && avatar.isHuman,
                "AvatarBuilder.BuildHumanAvatar returned an invalid Humanoid Avatar for " + bundle + ".");
            avatar.name = bundle;
            avatar.hideFlags = HideFlags.None;
            string dir = Folder(Assets + "/" + bundle);
            var animator = SaveCoverageRig(rig, hips, skeleton, avatar, dir, out var assets);
            var clips = new List<(AnimationClip clip, BodyClipVariant variant)>();
            foreach (var variant in RigVariants(spec))
            {
                // Unity may refuse a variant's settings, or motion or goal curves on a
                // human-motion clip: only that variant is then not covered.
                AnimationClip clip = null;
                string missing = null;
                try { clip = AuthorBodyClip(dir, ClipName(spec, variant), variant, rigSpec.Full, rigSpec.Scoped); }
                catch (CoverageUnavailable error) { missing = error.Message; }
                if (missing == null && rigSpec.Full && !KeepsCurves(clip, FullMuscles(rigSpec.Scoped)
                        .Select(i => MuscleProperty(HumanTrait.MuscleName[i])).ToArray()))
                    missing = "The Editor dropped a full-body muscle curve.";
                if (missing == null && rigSpec.Full)
                    VerifyMuscleBindings(rig, clip, FullMuscles(rigSpec.Scoped), ClipName(spec, variant));
                if (missing == null && variant.Motion && !KeepsCurves(clip, "MotionT.x", "MotionQ.w"))
                    missing = "The Editor dropped the authored MotionT and MotionQ curves.";
                if (missing == null && variant.Goals && !KeepsCurves(clip, "LeftFootT.x", "RightFootQ.w"))
                    missing = "The Editor dropped the authored foot goal curves.";
                if (missing != null)
                {
                    foreach (string suffix in Placed(spec) ? new[] { "", "-offset" } : new[] { "" })
                        report.Set(ClipName(spec, variant) + suffix, "not_covered", missing);
                    continue;
                }
                clips.Add((clip, variant));
            }
            animator.runtimeAnimatorController = Controller(assets[assets.Length - 1], clips.Select(row => row.clip).ToArray());

            root = new GameObject("HumanoidFixture");
            rig.transform.SetParent(root.transform, false);
            if (rigSpec.ScaledAnimator) rig.transform.localScale = Vector3.one * AnimatorScale;
            string prefab = SavePrefab(root, dir + "/identity.prefab");
            var placements = new List<(string path, string suffix, string root)> { (prefab, "", "identity") };
            if (Placed(spec))
            {
                rig.transform.localPosition = new Vector3(2, .25f, -1);
                rig.transform.localRotation = Quaternion.Euler(0, 35, 0);
                placements.Add((SavePrefab(root, dir + "/offset.prefab"), "-offset", "offset"));
            }
            var clipPaths = clips.Select(row => AssetDatabase.GetAssetPath(row.clip)).ToArray();
            var mapped = skeleton.Select(bone => bone.name).Where(HumanTrait.BoneName.Contains).ToArray();
            build.AddBodyBundle(bundle, assets.Concat(clipPaths).Concat(placements.Select(row => row.path)).ToArray());
            string probe = rigSpec.Armature ? "HumanoidRig/Armature/Hips/Spine" : "HumanoidRig/Hips/Spine";
            for (int i = 0; i < clips.Count; i++)
            {
                var variant = clips[i].variant;
                foreach (var (path, suffix, rootKind) in placements)
                {
                    var body = new BodyCase { Kind = "clip", Rig = rigSpec.Name, Variant = variant.Name, Root = rootKind,
                        Spec = rigSpec, Mapped = mapped };
                    var replay = BodyReplay(bundle, path, assets[0], clipPaths[i],
                        ClipName(spec, variant) + suffix, body, probe);
                    replay.BodyClip = variant;
                    build.Replays.Add(replay);
                }
            }
        }
        finally
        {
            Object.DestroyImmediate(root == null ? rig : root);
            if (avatar != null && !AssetDatabase.Contains(avatar)) Object.DestroyImmediate(avatar);
        }
    }

    // Before recording: the built clip keeps its playback mode and authored root
    // settings. Settings Unity changed in the build make the case not covered.
    static void CheckBuiltBody(ReplayCase replay, string path)
    {
        var bundle = AssetBundle.LoadFromFile(path);
        Require(bundle != null, "Cannot load the built body bundle " + replay.Bundle + ".");
        try
        {
            var keys = bundle.GetAllAssetNames();
            string Key(string asset) => keys.Single(key => string.Equals(key, asset, StringComparison.OrdinalIgnoreCase));
            var clip = bundle.LoadAsset<AnimationClip>(Key(replay.Clip));
            Require(clip != null && !clip.legacy && clip.humanMotion,
                "The built body clip lost its playback mode in " + replay.Bundle + ".");
            var avatar = bundle.LoadAsset<Avatar>(Key(replay.Avatar));
            Require(avatar != null && avatar.isValid && avatar.isHuman, "The built body Avatar is invalid in " + replay.Bundle + ".");
            if (replay.BodyClip != null)
            {
                Unavailable(BodySettingsMatch(AnimationUtility.GetAnimationClipSettings(clip), replay.BodyClip),
                    "The built clip did not keep its authored root settings in " + replay.Bundle + ".");
                CheckCompiledTracks(clip, replay.BodyClip, replay.OutputName, replay.Body.Spec.Full, replay.Body.Spec.Scoped);
            }
        }
        finally { bundle.Unload(true); }
    }

    // Animator attributes of the compiled humanoid tracks (as tools/parity/anim_fixture_body.py):
    // MotionT and MotionQ 0 to 6, RootT and RootQ 7 to 13, the foot goals 14 to 27, muscles from 42.
    const int MotionAttribute = 0, RootAttribute = 7, LeftFootGoal = 14, RightFootGoal = 21, MuscleAttribute = 42;

    static IEnumerable<(string track, int attribute)> CompiledTracks(BodyClipVariant variant, bool full, bool scoped)
    {
        IEnumerable<(string, int)> Span(string prefix, int first, string axes) =>
            axes.Select((axis, i) => (prefix + "." + axis, first + i));
        var tracks = Span("RootT", RootAttribute, "xyz").Concat(Span("RootQ", RootAttribute + 3, "xyzw"));
        if (variant.Motion)
            tracks = tracks.Concat(Span("MotionT", MotionAttribute, "xyz")).Concat(Span("MotionQ", MotionAttribute + 3, "xyzw"));
        if (full)
            tracks = tracks.Concat(FullMuscles(scoped).Select(i => (MuscleProperty(HumanTrait.MuscleName[i]), MuscleAttribute + i)));
        if (variant.Knee)
        {
            var names = HumanTrait.MuscleName;
            tracks = tracks.Concat(KneeCurves.Select(row => (row.muscle, MuscleAttribute + MuscleIndex(names, row.muscle))));
        }
        if (variant.Goals)
            foreach (var (goal, first) in new[] { ("LeftFoot", LeftFootGoal), ("RightFoot", RightFootGoal) })
                tracks = tracks.Concat(Span(goal + "T", first, "xyz")).Concat(Span(goal + "Q", first + 3, "xyzw"));
        return tracks;
    }

    // Before an F4 or F5 clip case records: every track it relies on is compiled
    // into the built clip, bound to the Animator root (path 0, Animator class 95,
    // humanoid custom type 8) in m_ClipBindingConstant, the runtime track table.
    // Where Unity exposes m_MuscleClip.m_IndexArray on the bundle-loaded clip,
    // the track's attribute must also index that same slot, as playback reads it.
    // Otherwise the case fails with the missing tracks; completion then checks
    // the slots' values in the runtime data.
    static void CheckCompiledTracks(AnimationClip clip, BodyClipVariant variant, string name, bool full, bool scoped)
    {
        using (var serialized = new SerializedObject(clip))
        {
            var slots = CompiledSlots(serialized);
            // The Editor may not serialize a bundle-loaded clip's runtime data. Offline
            // completion reads the compiled slots from the bundle itself, so a missing
            // table is reported and left to that check rather than failing the case.
            if (slots == null)
            {
                Debug.LogWarning("Unity did not expose the compiled track table of the built clip for " + name
                    + "; completion checks its tracks from the bundle.");
                return;
            }
            var index = serialized.FindProperty("m_MuscleClip.m_IndexArray");
            bool indexed = index != null && index.isArray;
            var missing = CompiledTracks(variant, full, scoped).Where(track => !slots.TryGetValue(track.attribute, out int slot)
                || indexed && (track.attribute >= index.arraySize
                    || index.GetArrayElementAtIndex(track.attribute).intValue != slot)).Select(track => track.track).ToArray();
            Require(missing.Length == 0, "The built clip lost or moved its compiled " + string.Join(", ", missing)
                + " track(s) in " + name + (indexed ? " (m_IndexArray checked)." : " (m_IndexArray not exposed)."));
        }
    }

    // The compiled humanoid track table (m_ClipBindingConstant): each Animator
    // attribute bound to the Animator root (path 0, Animator class 95, humanoid
    // custom type 8) and its row; null where the Editor exposes no table.
    static Dictionary<int, int> CompiledSlots(SerializedObject serialized)
    {
        var bindings = serialized.FindProperty("m_ClipBindingConstant.genericBindings");
        if (bindings == null || !bindings.isArray) return null;
        int Field(SerializedProperty row, string field, string alias = null) =>
            (row.FindPropertyRelative(field) ?? (alias == null ? null : row.FindPropertyRelative(alias)))?.intValue ?? -1;
        var slots = new Dictionary<int, int>();
        for (int i = 0; i < bindings.arraySize; i++)
        {
            var row = bindings.GetArrayElementAtIndex(i);
            if (Field(row, "path") == 0 && Field(row, "typeID", "classID") == 95 && Field(row, "customType") == 8
                && !slots.ContainsKey(Field(row, "attribute")))
                slots.Add(Field(row, "attribute"), i);
        }
        return slots;
    }

    // Before a full-body clip records, every curved muscle binds. Where the Editor
    // lists the rig's animatable Animator properties, each muscle's property is
    // among them; where it exposes the authored clip's compiled track table, each
    // muscle's curve compiled to its Animator attribute (MuscleAttribute + muscle).
    // A muscle that does not bind fails the run: a curve under a name Unity does not
    // know would record nothing for that bone. The built clip's table and offline
    // completion check the same slots again.
    static void VerifyMuscleBindings(GameObject rig, AnimationClip clip, int[] muscles, string name)
    {
        var names = HumanTrait.MuscleName;
        var animatable = new HashSet<string>(AnimationUtility.GetAnimatableBindings(rig, rig)
            .Where(binding => binding.path.Length == 0 && binding.type == typeof(Animator))
            .Select(binding => binding.propertyName), StringComparer.Ordinal);
        if (animatable.Contains(SpineMuscles[0]))
        {
            var unknown = muscles.Select(i => MuscleProperty(names[i])).Where(property => !animatable.Contains(property)).ToArray();
            Require(unknown.Length == 0, "Unity lists no animatable Animator property " + string.Join(", ", unknown)
                + " on the rig of " + name + ".");
        }
        else
            Debug.LogWarning("Unity lists no humanoid muscle among the animatable bindings of " + name
                + "; the compiled track tables are checked instead.");
        using (var serialized = new SerializedObject(clip))
        {
            // An authored asset may hold no compiled rows yet; the built clip's table is then the check.
            var slots = CompiledSlots(serialized);
            if (slots == null || slots.Count == 0)
            {
                Debug.LogWarning("Unity did not expose the compiled track table of the authored " + name
                    + "; the built clip's table and completion check its muscle tracks.");
                return;
            }
            var unbound = muscles.Where(i => !slots.ContainsKey(MuscleAttribute + i)).Select(i => MuscleProperty(names[i])).ToArray();
            Require(unbound.Length == 0, "The authored " + name + " compiled no muscle track for " + string.Join(", ", unbound) + ".");
        }
    }
}
#endif
