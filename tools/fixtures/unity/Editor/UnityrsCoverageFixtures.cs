#if UNITY_EDITOR
using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using UnityEditor;
using UnityEngine;
using Object = UnityEngine.Object;

// Optional coverage: two humanoid refusal probes, built in their own bundle
// build, and a probe showing why the integer discrete binding is not covered. A case is not covered only when an
// explicit check shows Unity cannot produce it (CoverageUnavailable, with the
// reason); any other error fails the case and leaves the main fixtures intact.
static partial class UnityrsFixtureBuilder
{
    const string IntegerCase = "integer-discrete-binding";
    const string TwistCase = "humanoid-twist-refusal";
    const string SettingsCase = "humanoid-non-neutral-setting-refusal";
    static readonly string[] CoverageCases = { IntegerCase, TwistCase, SettingsCase };
    const string MixedStorageReason = "Unity 2022.3 and 6000 keep the generic-scale binding in one storage range"
        + " (its constant z axis is streamed with x and y), and the Editor cannot read a bundle-loaded clip's"
        + " m_MuscleClip to confirm a split.";
    const string PackedSlopesReason = "Packed rotations come only from a legacy clip imported from a model file"
        + " with keyframe compression. A hand-written FBX with animation could not be import-tested off Unity, and"
        + " whether 2022.3 and 6000 keep the packed slopes is unconfirmed.";
    const string PrecedenceReason = "Unity's bundle build drops the authoring curves of a Mecanim clip (the 2022.3"
        + " built generic clips have empty m_PositionCurves and m_FloatCurves) and a legacy clip has no compiled playback,"
        + " so no Editor-built clip carries both sources to conflict.";
    // UnityCsReference: EditorCurveBinding.DiscreteCurve accepts only "[System.Enum] and [System.Int32 with the
    // `DiscreteEvaluation` attribute]", AnimationUtility.IsDiscreteIntBinding checks a script's field
    // (InvalidScript, MissingField, IncompatibleFieldType, MissingDiscreteAttribute), and
    // DiscreteEvaluationAttribute is AttributeTargets.Field. A built-in component is native, so its
    // properties are float curves; only PPtrCurve sets isDiscreteCurve for one. The prefabs carry no scripts.
    const string IntegerReason = "Unity animates an integer as a discrete curve only on a script field (an enum, or"
        + " an int with [DiscreteEvaluation]); built-in component properties are float curves, and the fixture"
        + " prefabs carry no scripts.";
    // Built-in components on the probe, one renderer per object.
    static readonly Type[][] DiscreteProbeComponents = {
        new[] { typeof(MeshFilter), typeof(MeshRenderer), typeof(Light), typeof(AudioSource), typeof(Camera) },
        new[] { typeof(SpriteRenderer) },
        new[] { typeof(SkinnedMeshRenderer) },
    };

    // Distinct values so a swapped field shows. Unity's default is .5 for each.
    const float UpperArmTwist = .5f, LowerArmTwist = .25f, UpperLegTwist = .75f, LowerLegTwist = .375f;
    static readonly string[] TwistMuscles = {
        "Left Arm Twist In-Out", "Left Forearm Twist In-Out", "Left Upper Leg Twist In-Out", "Left Lower Leg Twist In-Out"
    };
    static readonly float[] TwistValues = { .625f, -.5f, .5f, -.375f };
    // The settings probe's single non-neutral clip setting.
    const float HeightOffset = .125f;

    internal sealed class CoverageUnavailable : Exception
    {
        public CoverageUnavailable(string message) : base(message) {}
    }

    internal static void Unavailable(bool condition, string message)
    {
        if (!condition) throw new CoverageUnavailable(message);
    }

    internal sealed partial class ReplayCase
    {
        public bool Optional;
        public string ExpectedRefusal = "";
        public string[] TwistMuscles;
        public float[] TwistValues;
    }

    sealed partial class Build
    {
        public readonly List<AssetBundleBuild> CoverageBundles = new List<AssetBundleBuild>();

        public void AddCoverageBundle(string name, params string[] assets)
        {
            CoverageBundles.Add(new AssetBundleBuild {
                assetBundleName = name, assetNames = assets.OrderBy(path => path, StringComparer.Ordinal).ToArray()
            });
        }
    }

    static void AddCoverageCases(RunReport report)
    {
        foreach (string name in CoverageCases)
            report.AddDeferred(name, "The coverage fixture was not prepared.");
    }

    static void BuildCoverage(Build build, RunReport report)
    {
        report.Set(IntegerCase, "not_covered", ProbeDiscreteIntegers());
        report.Prepare(build, "Prepare " + TwistCase, new[] { TwistCase },
            () => BuildRefusalRig(build, "humanoid-twist", TwistCase, twist: true), true);
        report.Prepare(build, "Prepare " + SettingsCase, new[] { SettingsCase },
            () => BuildRefusalRig(build, "humanoid-settings", SettingsCase, twist: false), true);
    }

    // Build the coverage bundles apart from the main fixtures, then move them
    // beside the others. Returns null, with the cases failed, if that fails.
    static Dictionary<string, string[]> BuildCoverageBundles(Build build, RunReport report, string staging, string bundles,
        BuildTarget target)
    {
        if (build.CoverageBundles.Count == 0) return null;
        var names = build.Replays.Where(replay => replay.Optional).Select(replay => replay.OutputName).ToArray();
        Dictionary<string, string[]> contents = null;
        string folder = Path.Combine(staging, "coverage-bundles");
        bool built = report.Try("Build coverage bundles", names, () => {
            Directory.CreateDirectory(folder);
            var manifest = BuildPipeline.BuildAssetBundles(folder, build.CoverageBundles.ToArray(),
                BuildAssetBundleOptions.ChunkBasedCompression | BuildAssetBundleOptions.StrictMode, target);
            Require(manifest != null, "Unity could not build the coverage bundles. See the log and Console.");
            contents = BundleContents(manifest);
            foreach (var bundle in build.CoverageBundles)
            {
                string name = bundle.assetBundleName;
                Require(contents.ContainsKey(name), "Unity did not emit " + name + ".");
                File.Move(Path.Combine(folder, name), Path.Combine(bundles, name));
            }
        });
        try { if (Directory.Exists(folder)) Directory.Delete(folder, true); }
        catch (IOException error) { Debug.LogException(error); }
        catch (UnauthorizedAccessException error) { Debug.LogException(error); }
        if (!built) return null;
        // A failed check leaves its case failed and unrecorded.
        build.Replays.RemoveAll(replay => replay.Optional && !report.Try("Check built " + replay.OutputName,
            new[] { replay.OutputName }, () => CheckBuiltCoverage(replay, Path.Combine(bundles, replay.Bundle))));
        return contents;
    }

    // Catch serialization loss in the built bundle before recording.
    static void CheckBuiltCoverage(ReplayCase replay, string path)
    {
        var bundle = AssetBundle.LoadFromFile(path);
        Require(bundle != null, "Cannot load the built coverage bundle " + replay.Bundle + ".");
        try
        {
            var keys = bundle.GetAllAssetNames();
            string Key(string asset) => keys.Single(key => string.Equals(key, asset, StringComparison.OrdinalIgnoreCase));
            var clip = bundle.LoadAsset<AnimationClip>(Key(replay.Clip));
            Require(clip != null && !clip.legacy && clip.humanMotion == replay.IsHumanoid,
                "The built coverage clip lost its playback mode in " + replay.Bundle + ".");
            if (!replay.IsHumanoid) return;
            var description = bundle.LoadAsset<Avatar>(Key(replay.Avatar)).humanDescription;
            bool twist = replay.TwistMuscles != null;
            Require(description.upperArmTwist == (twist ? UpperArmTwist : 0)
                && description.lowerArmTwist == (twist ? LowerArmTwist : 0)
                && description.upperLegTwist == (twist ? UpperLegTwist : 0)
                && description.lowerLegTwist == (twist ? LowerLegTwist : 0),
                "The built Avatar lost its authored twist in " + replay.Bundle + ".");
            var settings = AnimationUtility.GetAnimationClipSettings(clip);
            Require(settings.level == (twist ? 0 : HeightOffset) && !settings.keepOriginalPositionY,
                "The built clip lost its authored settings in " + replay.Bundle + ".");
        }
        finally { bundle.Unload(true); }
    }

    // Always not covered. The reason lists what Unity reports on every
    // component of the probe objects, so a discrete integer would show; a
    // probe error only changes that list.
    static string ProbeDiscreteIntegers()
    {
        var rig = new GameObject("DiscreteProbe");
        try
        {
            var found = new List<string>();
            int total = 0, pptr = 0;
            for (int i = 0; i < DiscreteProbeComponents.Length; i++)
            {
                var target = Child(rig.transform, "Probe" + i, Vector3.zero).gameObject;
                foreach (var type in DiscreteProbeComponents[i]) target.AddComponent(type);
                var animatable = AnimationUtility.GetAnimatableBindings(target, rig);
                total += animatable.Length;
                pptr += animatable.Count(row => row.isPPtrCurve);
                found.AddRange(animatable.Where(row => row.isDiscreteCurve && !row.isPPtrCurve).Select(row =>
                    row.type.Name + "." + row.propertyName + " (" + ValueTypeName(rig, row) + ")"));
            }
            string components = string.Join(", ", new[] { typeof(Transform) }
                .Concat(DiscreteProbeComponents.SelectMany(types => types)).Select(type => type.Name).Distinct());
            return IntegerReason + " Unity listed " + total + " animatable bindings on " + components + ": "
                + pptr + " PPtr, and non-PPtr discrete candidates: "
                + (found.Count == 0 ? "none" : string.Join(", ", found.Distinct())) + ".";
        }
        catch (Exception error)
        {
            Debug.LogException(error);
            return IntegerReason + " The binding probe failed: " + error.Message;
        }
        finally { Object.DestroyImmediate(rig); }
    }

    static string ValueTypeName(GameObject root, EditorCurveBinding binding)
    {
        var type = AnimationUtility.GetEditorCurveValueType(root, binding);
        return type == null ? "untyped" : type.Name;
    }

    // A compact rig with one muscle clip and exactly one refusal trigger: the
    // twist probe's non-neutral Avatar twist (animating the twist muscles), or
    // the settings probe's height offset on a neutral Avatar.
    static void BuildRefusalRig(Build build, string name, string clipName, bool twist)
    {
        var rig = new GameObject("HumanoidRig");
        GameObject root = null;
        Avatar avatar = null;
        try
        {
            var hips = BuildHumanoidSkeleton(rig.transform, false);
            var skeleton = rig.GetComponentsInChildren<Transform>();
            var description = DescribeHumanoid(skeleton, false);
            if (twist)
            {
                Unavailable(TwistMuscles.All(HumanTrait.MuscleName.Contains),
                    "HumanTrait does not list the four left-limb twist muscles.");
                description.upperArmTwist = UpperArmTwist;
                description.lowerArmTwist = LowerArmTwist;
                description.upperLegTwist = UpperLegTwist;
                description.lowerLegTwist = LowerLegTwist;
            }
            avatar = AvatarBuilder.BuildHumanAvatar(rig, description);
            Require(avatar != null && avatar.isValid && avatar.isHuman,
                "AvatarBuilder.BuildHumanAvatar returned an invalid Humanoid Avatar.");
            avatar.name = name;
            avatar.hideFlags = HideFlags.None;
            var built = avatar.humanDescription;
            Unavailable(!twist || (built.upperArmTwist == UpperArmTwist && built.lowerArmTwist == LowerArmTwist
                && built.upperLegTwist == UpperLegTwist && built.lowerLegTwist == LowerLegTwist),
                "The built Avatar did not keep the authored twist settings.");

            string dir = Folder(Assets + "/" + name);
            string avatarPath = dir + "/avatar.asset";
            string meshPath = dir + "/skin.asset";
            string materialPath = dir + "/skin.mat";
            string controllerPath = dir + "/humanoid.controller";
            var animator = rig.AddComponent<Animator>();
            animator.avatar = Save(avatar, avatarPath);
            animator.applyRootMotion = false;
            var skin = rig.AddComponent<SkinnedMeshRenderer>();
            skin.bones = skeleton.Skip(1).ToArray();
            skin.rootBone = hips;
            skin.sharedMesh = Save(NewHumanoidSkin(rig.transform, skin.bones), meshPath);
            var shader = Shader.Find("Standard");
            Require(shader != null, "The Built-in Standard shader is required for the humanoid skin.");
            skin.sharedMaterial = Save(new Material(shader) { name = "HumanoidBoxes" }, materialPath);

            var clip = AuthorHumanoidClip(dir, clipName, bodyMotion: false, extra: twist ? (Action<AnimationClip>)SetTwistMuscles : null);
            SetNeutralSettings(clip, twist ? 0 : HeightOffset);
            animator.runtimeAnimatorController = Controller(controllerPath, new[] { clip });

            root = new GameObject("HumanoidFixture");
            rig.transform.SetParent(root.transform, false);
            string prefab = SavePrefab(root, dir + "/identity.prefab");
            string clipPath = AssetDatabase.GetAssetPath(clip);
            build.AddCoverageBundle(name, clipPath, prefab, avatarPath, meshPath, materialPath, controllerPath);
            var replay = HumanoidReplay(name, prefab, avatarPath, clipPath, clipName);
            replay.Optional = true;
            replay.ExpectedRefusal = twist ? "unsupported_humanoid_twist" : "unsupported_humanoid_clip_settings";
            if (twist)
            {
                replay.TwistMuscles = TwistMuscles;
                replay.TwistValues = TwistValues;
            }
            build.Replays.Add(replay);
        }
        finally
        {
            Object.DestroyImmediate(root == null ? rig : root);
            if (avatar != null && !AssetDatabase.Contains(avatar)) Object.DestroyImmediate(avatar);
        }
    }

    static void SetTwistMuscles(AnimationClip clip)
    {
        var names = HumanTrait.MuscleName;
        for (int i = 0; i < TwistMuscles.Length; i++)
            clip.SetCurve("", typeof(Animator), names[MuscleIndex(names, TwistMuscles[i])],
                AnimationCurve.Linear(0, 0, 1, TwistValues[i]));
    }

    // Clear the code-made clip's default original-Y basis so the only
    // non-neutral clip setting is the given height offset (zero for the twist probe).
    static void SetNeutralSettings(AnimationClip clip, float level)
    {
        var settings = AnimationUtility.GetAnimationClipSettings(clip);
        settings.level = level;
        settings.keepOriginalPositionY = false;
        AnimationUtility.SetAnimationClipSettings(clip, settings);
        EditorUtility.SetDirty(clip);
        var s = AnimationUtility.GetAnimationClipSettings(clip);
        Unavailable(s.level == level && s.startTime == 0 && s.stopTime == 1 && s.orientationOffsetY == 0
            && s.cycleOffset == 0 && s.additiveReferencePoseTime == 0 && s.additiveReferencePoseClip == null
            && !s.loopTime && !s.loopBlend && !s.loopBlendOrientation && !s.loopBlendPositionY
            && !s.loopBlendPositionXZ && !s.keepOriginalOrientation && !s.keepOriginalPositionY
            && !s.keepOriginalPositionXZ && !s.heightFromFeet && !s.mirror && !s.hasAdditiveReferencePose,
            "Unity did not keep the clip's neutral settings and its height offset.");
    }
}
#endif
