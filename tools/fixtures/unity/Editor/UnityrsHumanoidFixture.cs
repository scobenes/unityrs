#if UNITY_EDITOR
using System;
using System.Collections.Generic;
using System.Linq;
using UnityEditor;
using UnityEngine;
using Object = UnityEngine.Object;

static partial class UnityrsFixtureBuilder
{
    static void BuildHumanoid(Build build, string name, bool tall)
    {
        var rig = new GameObject("HumanoidRig");
        GameObject root = null;
        Avatar avatar = null;
        try
        {
            var hips = BuildHumanoidSkeleton(rig.transform, tall);
            var skeleton = rig.GetComponentsInChildren<Transform>();
            var description = DescribeHumanoid(skeleton, tall);

            // On the first run, confirm AvatarBuilder accepts the top-level T-pose,
            // the optional UpperChest omission, and the custom Spine limits.
            avatar = AvatarBuilder.BuildHumanAvatar(rig, description);
            Require(avatar != null && avatar.isValid && avatar.isHuman,
                "AvatarBuilder.BuildHumanAvatar returned an invalid Humanoid Avatar.");
            avatar.name = name;
            avatar.hideFlags = HideFlags.None;

            string dir = Folder(Assets + "/" + name);
            string avatarPath = dir + "/avatar.asset";
            string meshPath = dir + "/skin.asset";
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
            string materialPath = dir + "/skin.mat";
            skin.sharedMaterial = Save(new Material(shader) { name = "HumanoidBoxes" }, materialPath);

            var muscleClip = AuthorHumanoidClip(dir, name + "-muscles", bodyMotion: false);
            var clips = tall ? new[] { muscleClip } : new[] {
                muscleClip, AuthorHumanoidClip(dir, name + "-body-motion", bodyMotion: true)
            };
            animator.runtimeAnimatorController = Controller(controllerPath, clips);

            root = new GameObject("HumanoidFixture");
            rig.transform.SetParent(root.transform, false);
            string prefab = SavePrefab(root, dir + "/identity.prefab");
            rig.transform.localPosition = new Vector3(2, .25f, -1);
            rig.transform.localRotation = Quaternion.Euler(0, 35, 0);
            string offsetPrefab = SavePrefab(root, dir + "/offset.prefab");

            var clipPaths = clips.Select(AssetDatabase.GetAssetPath).ToArray();
            build.AddBundle(name, clipPaths.Concat(new[] { prefab, offsetPrefab, avatarPath, meshPath,
                materialPath, controllerPath }).ToArray());
            string muscleName = muscleClip.name;
            AddHumanoidReplays(build, name, prefab, avatarPath, clipPaths[0], muscleName);
            AddHumanoidReplays(build, name, offsetPrefab, avatarPath, clipPaths[0], muscleName + "-offset");
            if (!tall)
                build.Replays.Add(HumanoidReplay(name, prefab, avatarPath, clipPaths[1], clips[1].name));
        }
        finally
        {
            Object.DestroyImmediate(root == null ? rig : root);
            if (avatar != null && !AssetDatabase.Contains(avatar)) Object.DestroyImmediate(avatar);
        }
    }

    static ReplayCase HumanoidReplay(string bundle, string prefab, string avatar, string clip, string name) =>
        new ReplayCase {
            Bundle = bundle, Prefab = prefab, Avatar = avatar, Clip = clip, ReplayName = name,
            BindingAddress = "0", ProbePath = "HumanoidRig/Hips/Spine", ProbeProperty = "rotation"
        };

    static void AddHumanoidReplays(Build build, string bundle, string prefab, string avatar, string clip, string name)
    {
        build.Replays.Add(HumanoidReplay(bundle, prefab, avatar, clip, name));
        var baseline = HumanoidReplay(bundle, prefab, avatar, clip, name + "-known-vectors");
        baseline.KnownMuscles = KnownHumanoidMuscles;
        build.Replays.Add(baseline);
    }

    static Transform BuildHumanoidSkeleton(Transform root, bool tall)
    {
        // Upright T-pose facing +Z. Anatomical left is on negative X.
        var hips = Child(root, "Hips", Vector3.up * (tall ? 1.18f : .88f));
        var spine = Child(hips, "Spine", Vector3.up * (tall ? .21f : .16f));
        var chest = Child(spine, "Chest", Vector3.up * (tall ? .27f : .13f));
        var torso = tall ? chest : Child(chest, "UpperChest", Vector3.up * .08f);
        var neck = Child(torso, "Neck", Vector3.up * .14f);
        Child(neck, "Head", Vector3.up * .12f);
        BuildHumanoidSide(hips, torso, "Left", -1, tall);
        BuildHumanoidSide(hips, torso, "Right", 1, tall);
        return hips;
    }

    static void BuildHumanoidSide(Transform hips, Transform torso, string side, int sign, bool tall)
    {
        var thigh = Child(hips, side + "UpperLeg", new Vector3(sign * (tall ? .10f : .13f), -.05f, 0));
        var shin = Child(thigh, side + "LowerLeg", new Vector3(0, tall ? -.58f : -.36f, .04f));
        var foot = Child(shin, side + "Foot", new Vector3(0, tall ? -.48f : -.37f, -.04f));
        Child(foot, side + "Toes", new Vector3(0, -.04f, .14f));

        var shoulder = Child(torso, side + "Shoulder", new Vector3(sign * .08f, .08f, 0));
        var upper = Child(shoulder, side + "UpperArm", Vector3.right * (sign * (tall ? .10f : .14f)));
        var lower = Child(upper, side + "LowerArm", Vector3.right * (sign * (tall ? .34f : .26f)));
        Child(lower, side + "Hand", Vector3.right * (sign * (tall ? .26f : .23f)));
    }

    static HumanDescription DescribeHumanoid(Transform[] skeleton, bool tall)
    {
        var transforms = skeleton.ToDictionary(bone => bone.name, StringComparer.Ordinal);
        var names = HumanTrait.BoneName;
        var human = new List<HumanBone>();
        for (int i = 0; i < names.Length; i++)
        {
            string name = names[i];
            if (!transforms.TryGetValue(name, out var bone))
            {
                Require(!HumanTrait.RequiredBone(i), "Procedural skeleton is missing required bone " + name + ".");
                continue;
            }

            var limit = new HumanLimit { useDefaultValues = true };
            if (tall && name == "Spine")
            {
                limit = new HumanLimit {
                    useDefaultValues = false, center = Vector3.zero,
                    min = new Vector3(-18, -32, -12), max = new Vector3(42, 14, 26),
                    axisLength = bone.GetChild(0).localPosition.magnitude
                };
            }
            human.Add(new HumanBone { humanName = name, boneName = bone.name, limit = limit });
        }

        // On the first run, confirm the public HumanDescription fields suffice on
        // both Editors. Include the root and every joint in hierarchy order
        // with local TRS. AvatarBuilder gets the parent links from the rig.
        return new HumanDescription {
            human = human.ToArray(),
            skeleton = skeleton.Select(bone => new SkeletonBone {
                name = bone.name, position = bone.localPosition, rotation = bone.localRotation, scale = bone.localScale
            }).ToArray(),
            upperArmTwist = 0, lowerArmTwist = 0, upperLegTwist = 0, lowerLegTwist = 0,
            armStretch = 0, legStretch = 0, feetSpacing = 0, hasTranslationDoF = false
        };
    }
}
#endif
