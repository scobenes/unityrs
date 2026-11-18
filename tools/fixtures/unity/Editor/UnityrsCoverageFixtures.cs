#if UNITY_EDITOR
using System;
using System.Linq;
using UnityEditor;
using UnityEngine;
using Object = UnityEngine.Object;

// Optional coverage: an integer discrete binding and two humanoid refusal
// probes. A case Unity cannot reproduce is reported as not covered with
// Unity's reason, and the rest of the run continues.
static partial class UnityrsFixtureBuilder
{
    const string IntegerCase = "integer-discrete-binding";
    const string TwistCase = "humanoid-twist-refusal";
    const string SettingsCase = "humanoid-non-neutral-setting-refusal";
    const string MixedStorageReason = "Unity 2022.3 and 6000 keep the generic-scale binding in one storage range"
        + " (its constant z axis is streamed with x and y), and the Editor cannot read a bundle-loaded clip's"
        + " m_MuscleClip to confirm a split.";
    const string PackedSlopesReason = "Packed rotations come only from a legacy clip imported from a model file"
        + " with keyframe compression. A hand-written FBX with animation could not be import-tested off Unity, and"
        + " whether 2022.3 and 6000 keep the packed slopes is unconfirmed.";
    const string PrecedenceReason = "Unity's bundle build drops the authoring curves of a Mecanim clip (the 2022.3"
        + " built generic clips have empty m_PositionCurves and m_FloatCurves) and a legacy clip has no compiled playback,"
        + " so no Editor-built clip carries both sources to conflict.";
    internal const string DiscretePath = "Discrete";
    internal const string DiscreteProperty = "m_SortingOrder";
    internal static readonly float[] DiscreteTimes = { 0, .5f };
    internal static readonly int[] DiscreteValues = { 3, 7 };

    // Distinct values so a swapped field shows. Unity's default is .5 for each.
    const float UpperArmTwist = .5f, LowerArmTwist = .25f, UpperLegTwist = .75f, LowerLegTwist = .375f;
    static readonly string[] TwistMuscles = {
        "Left Arm Twist In-Out", "Left Forearm Twist In-Out", "Left Upper Leg Twist In-Out", "Left Lower Leg Twist In-Out"
    };
    static readonly float[] TwistValues = { .625f, -.5f, .5f, -.375f };
    const float HeightOffset = .125f, OrientationOffset = 20;

    internal sealed partial class ReplayCase
    {
        public bool Optional, ProbeDiscrete;
        public string ExpectedRefusal = "";
        public string[] TwistMuscles;
        public float[] TwistValues;
    }

    static void AddCoverageCases(RunReport report)
    {
        foreach (string name in new[] { IntegerCase, TwistCase, SettingsCase })
            report.AddDeferred(name, "The coverage fixture was not prepared.");
    }

    static void BuildCoverage(Build build, RunReport report)
    {
        report.Prepare(build, "Prepare " + IntegerCase, new[] { IntegerCase }, () => BuildIntegerDiscrete(build), "not_covered");
        report.Prepare(build, "Prepare " + TwistCase, new[] { TwistCase },
            () => BuildRefusalRig(build, "humanoid-twist", TwistCase, twist: true), "not_covered");
        report.Prepare(build, "Prepare " + SettingsCase, new[] { SettingsCase },
            () => BuildRefusalRig(build, "humanoid-settings", SettingsCase, twist: false), "not_covered");
    }

    static void BuildIntegerDiscrete(Build build)
    {
        const string bundle = "anim-discrete";
        string dir = Folder(Assets + "/Discrete");
        var rig = new GameObject("DiscreteRig");
        try
        {
            Child(rig.transform, MotionPath, Vector3.zero);
            var target = Child(rig.transform, DiscretePath, new Vector3(0, 0, 2)).gameObject;
            target.AddComponent<MeshRenderer>();
            var binding = EditorCurveBinding.DiscreteCurve(DiscretePath, typeof(MeshRenderer), DiscreteProperty);
            RequireDiscreteInteger(rig, target, binding);
            var clip = Author(dir, IntegerCase, authored => {
                SetMotion(authored, AnimationCurve.Linear(0, 3, 1, 9));
                AnimationUtility.SetEditorCurve(authored, binding, new AnimationCurve(
                    new Keyframe(DiscreteTimes[0], DiscreteValues[0]), new Keyframe(DiscreteTimes[1], DiscreteValues[1])));
            });
            var stored = AnimationUtility.GetCurveBindings(clip).Where(row => row.path == DiscretePath).ToArray();
            Require(stored.Length == 1 && stored[0].isDiscreteCurve && !stored[0].isPPtrCurve
                && stored[0].type == typeof(MeshRenderer) && stored[0].propertyName == DiscreteProperty,
                "Unity did not keep MeshRenderer." + DiscreteProperty + " as a discrete curve.");
            var keys = AnimationUtility.GetEditorCurve(clip, stored[0]).keys;
            Require(keys.Select(key => key.time).SequenceEqual(DiscreteTimes)
                && keys.Select(key => key.value).SequenceEqual(DiscreteValues.Select(value => (float)value)),
                "Unity did not retain both authored integer keys.");

            string controllerPath = dir + "/discrete.controller";
            var animator = rig.AddComponent<Animator>();
            animator.applyRootMotion = false;
            animator.runtimeAnimatorController = Controller(controllerPath, new[] { clip });
            string prefab = SavePrefab(rig, dir + "/discrete.prefab");
            string clipPath = AssetDatabase.GetAssetPath(clip);
            build.AddBundle(bundle, prefab, clipPath, controllerPath);
            var replay = Case(bundle, prefab, clipPath, 3, 9);
            replay.Optional = replay.ProbeDiscrete = true;
            build.Replays.Add(replay);
        }
        finally { Object.DestroyImmediate(rig); }
    }

    static void RequireDiscreteInteger(GameObject root, GameObject target, EditorCurveBinding binding)
    {
        var animatable = AnimationUtility.GetAnimatableBindings(target, root);
        bool listed = animatable.Any(row => row.path == binding.path && row.type == binding.type
            && row.propertyName == binding.propertyName && row.isDiscreteCurve && !row.isPPtrCurve);
        if (!listed)
        {
            var discrete = animatable.Where(row => row.isDiscreteCurve && !row.isPPtrCurve)
                .Select(row => row.type.Name + "." + row.propertyName).Distinct().Take(8).ToArray();
            throw new InvalidOperationException("Unity does not list MeshRenderer." + binding.propertyName
                + " as an animatable discrete binding (discrete candidates: "
                + (discrete.Length == 0 ? "none" : string.Join(", ", discrete)) + ").");
        }
        var type = AnimationUtility.GetEditorCurveValueType(root, binding);
        Require(type == typeof(int), "Unity reports MeshRenderer." + binding.propertyName + " as "
            + (type == null ? "untyped" : type.Name) + ", not an integer.");
    }

    // A compact rig with one muscle clip. The twist probe sets non-neutral
    // Avatar twist and animates the twist muscles; the settings probe keeps a
    // neutral Avatar and sets the clip's height and orientation offsets.
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
            Require(!twist || (built.upperArmTwist == UpperArmTwist && built.lowerArmTwist == LowerArmTwist
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
            if (!twist) SetClipOffsets(clip);
            animator.runtimeAnimatorController = Controller(controllerPath, new[] { clip });

            root = new GameObject("HumanoidFixture");
            rig.transform.SetParent(root.transform, false);
            string prefab = SavePrefab(root, dir + "/identity.prefab");
            string clipPath = AssetDatabase.GetAssetPath(clip);
            build.AddBundle(name, clipPath, prefab, avatarPath, meshPath, materialPath, controllerPath);
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

    static void SetClipOffsets(AnimationClip clip)
    {
        var settings = AnimationUtility.GetAnimationClipSettings(clip);
        settings.level = HeightOffset;
        settings.orientationOffsetY = OrientationOffset;
        AnimationUtility.SetAnimationClipSettings(clip, settings);
        EditorUtility.SetDirty(clip);
        var stored = AnimationUtility.GetAnimationClipSettings(clip);
        Require(stored.level == HeightOffset && stored.orientationOffsetY == OrientationOffset
            && stored.startTime == 0 && stored.stopTime == 1 && !stored.loopTime && !stored.mirror,
            "Unity did not keep the authored height and orientation offsets.");
    }
}
#endif
