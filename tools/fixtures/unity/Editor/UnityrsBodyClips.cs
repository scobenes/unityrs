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

    static IEnumerable<string> ClipCases() => ClipVariants.SelectMany(row =>
        new[] { "body-clip-" + row.Name, "body-clip-" + row.Name + "-offset" });

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

    static void AddBodyCurves(AnimationClip clip, BodyClipVariant variant)
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

    static AnimationClip AuthorBodyClip(string dir, string name, BodyClipVariant variant)
    {
        var clip = AuthorHumanoidClip(dir, name, bodyMotion: false, extra: authored => AddBodyCurves(authored, variant));
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

    static void BuildBodyClips(Build build, RunReport report)
    {
        var rig = new GameObject("HumanoidRig");
        GameObject root = null;
        Avatar avatar = null;
        try
        {
            var hips = BuildBodySkeleton(rig.transform, ProbeRigs[0]);
            var skeleton = rig.GetComponentsInChildren<Transform>();
            avatar = AvatarBuilder.BuildHumanAvatar(rig, DescribeHumanoid(skeleton, false));
            Require(avatar != null && avatar.isValid && avatar.isHuman,
                "AvatarBuilder.BuildHumanAvatar returned an invalid Humanoid Avatar.");
            avatar.name = BodyClipBundle;
            avatar.hideFlags = HideFlags.None;
            string dir = Folder(Assets + "/" + BodyClipBundle);
            var animator = SaveCoverageRig(rig, hips, skeleton, avatar, dir, out var assets);
            var clips = new List<(AnimationClip clip, BodyClipVariant variant)>();
            foreach (var variant in ClipVariants)
            {
                // Unity may refuse a variant's settings, or motion or goal curves on a
                // human-motion clip: only that variant is then not covered.
                AnimationClip clip = null;
                string missing = null;
                try { clip = AuthorBodyClip(dir, "body-clip-" + variant.Name, variant); }
                catch (CoverageUnavailable error) { missing = error.Message; }
                if (missing == null && variant.Motion && !KeepsCurves(clip, "MotionT.x", "MotionQ.w"))
                    missing = "The Editor dropped the authored MotionT and MotionQ curves.";
                if (missing == null && variant.Goals && !KeepsCurves(clip, "LeftFootT.x", "RightFootQ.w"))
                    missing = "The Editor dropped the authored foot goal curves.";
                if (missing != null)
                {
                    foreach (string suffix in new[] { "", "-offset" })
                        report.Set("body-clip-" + variant.Name + suffix, "not_covered", missing);
                    continue;
                }
                clips.Add((clip, variant));
            }
            animator.runtimeAnimatorController = Controller(assets[assets.Length - 1], clips.Select(row => row.clip).ToArray());

            root = new GameObject("HumanoidFixture");
            rig.transform.SetParent(root.transform, false);
            string prefab = SavePrefab(root, dir + "/identity.prefab");
            rig.transform.localPosition = new Vector3(2, .25f, -1);
            rig.transform.localRotation = Quaternion.Euler(0, 35, 0);
            string offsetPrefab = SavePrefab(root, dir + "/offset.prefab");
            var clipPaths = clips.Select(row => AssetDatabase.GetAssetPath(row.clip)).ToArray();
            build.AddBodyBundle(BodyClipBundle, assets.Concat(clipPaths).Concat(new[] { prefab, offsetPrefab }).ToArray());
            for (int i = 0; i < clips.Count; i++)
            {
                var variant = clips[i].variant;
                foreach (var (path, suffix, rootKind) in new[] { (prefab, "", "identity"), (offsetPrefab, "-offset", "offset") })
                {
                    var body = new BodyCase { Kind = "clip", Rig = "compact", Variant = variant.Name, Root = rootKind };
                    var replay = BodyReplay(BodyClipBundle, path, assets[0], clipPaths[i],
                        "body-clip-" + variant.Name + suffix, body, "HumanoidRig/Hips/Spine");
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
                Unavailable(BodySettingsMatch(AnimationUtility.GetAnimationClipSettings(clip), replay.BodyClip),
                    "The built clip did not keep its authored root settings in " + replay.Bundle + ".");
        }
        finally { bundle.Unload(true); }
    }
}
#endif
