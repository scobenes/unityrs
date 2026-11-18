#if UNITY_EDITOR
using System;
using System.IO;
using System.Linq;
using UnityEditor;
using UnityEngine;
using Object = UnityEngine.Object;

static partial class UnityrsFixtureBuilder
{
    static void BuildHumanoid(Build build, Action<string> log)
    {
        const string input = "Assets/UnityrsHumanoid.fbx";
        const string bundle = "anim-humanoid";
        if (!File.Exists(input))
        {
            log("Skipped humanoid. Optional original animated FBX expected at " + input + ".");
            return;
        }

        // Import a generated copy so the user's source and import settings
        // are untouched. Confirm Avatar extraction and prefab binding on the
        // first run with an optional FBX.
        string dir = Folder(Assets + "/Humanoid");
        string modelPath = dir + "/model.fbx";
        File.Copy(input, modelPath, true);
        AssetDatabase.ImportAsset(modelPath, ImportAssetOptions.ForceSynchronousImport | ImportAssetOptions.ForceUpdate);
        var importer = AssetImporter.GetAtPath(modelPath) as ModelImporter;
        if (importer == null)
        {
            log("Skipped humanoid. Unity could not import the supplied FBX.");
            return;
        }
        importer.animationType = ModelImporterAnimationType.Human;
        importer.avatarSetup = ModelImporterAvatarSetup.CreateFromThisModel;
        importer.optimizeGameObjects = false;
        importer.importAnimation = true;
        importer.materialImportMode = ModelImporterMaterialImportMode.None;
        importer.importCameras = false;
        importer.importLights = false;
        importer.SaveAndReimport();

        var model = AssetDatabase.LoadAssetAtPath<GameObject>(modelPath);
        var subassets = AssetDatabase.LoadAllAssetsAtPath(modelPath);
        var avatars = subassets.OfType<Avatar>().Where(avatar => avatar.isValid && avatar.isHuman).ToArray();
        var motion = subassets.OfType<AnimationClip>().Where(clip => clip.humanMotion && clip.length > 0
            && !clip.name.StartsWith("__preview__", StringComparison.Ordinal))
            .OrderBy(clip => clip.name, StringComparer.Ordinal).FirstOrDefault();
        if (model == null || avatars.Length != 1 || motion == null)
        {
            log("Skipped humanoid. The FBX must import with one valid Humanoid Avatar and an animation take.");
            return;
        }

        string avatarPath = dir + "/humanoid-avatar.asset";
        var avatar = Object.Instantiate(avatars[0]);
        avatar.name = "HumanoidAvatar";
        avatar.hideFlags = HideFlags.None;
        avatar = Save(avatar, avatarPath);
        string clipPath = dir + "/humanoid-imported.anim";
        var clip = Object.Instantiate(motion);
        clip.name = "HumanoidImported";
        clip.hideFlags = HideFlags.None;
        var settings = AnimationUtility.GetAnimationClipSettings(clip);
        settings.loopTime = false;
        settings.loopBlend = false;
        clip.wrapMode = WrapMode.Once;
        AnimationUtility.SetAnimationClipSettings(clip, settings);
        clip = Save(clip, clipPath);
        string controllerPath = dir + "/humanoid.controller";
        var controller = Controller(controllerPath, new[] { clip });

        var root = new GameObject("HumanoidFixture");
        try
        {
            var rig = Object.Instantiate(model, root.transform, false);
            rig.name = "HumanoidRig";
            var animator = rig.GetComponent<Animator>();
            if (animator == null) animator = rig.AddComponent<Animator>();
            animator.avatar = avatar;
            animator.applyRootMotion = false;
            animator.runtimeAnimatorController = controller;
            // The sentinel is outside the Animator's hierarchy and gives an
            // independently known endpoint value even on an arbitrary take.
            Child(root.transform, "Sentinel", new Vector3(2, 0, 0));
            string prefab = SavePrefab(root, dir + "/humanoid.prefab");
            build.AddBundle(bundle, prefab, avatarPath, clipPath, controllerPath);
            build.Replays.Add(new ReplayCase {
                Bundle = bundle, Prefab = prefab, Clip = clipPath, Avatar = avatarPath, BindingAddress = "0",
                ProbePath = "Sentinel", First = 2, Last = 2,
                ImportSettings = new[] {
                    "animationType=" + importer.animationType, "avatarSetup=" + importer.avatarSetup,
                    "optimizeGameObjects=" + importer.optimizeGameObjects, "importAnimation=" + importer.importAnimation,
                    "animationCompression=" + importer.animationCompression, "materialImportMode=" + importer.materialImportMode,
                    "importCameras=" + importer.importCameras, "importLights=" + importer.importLights,
                    "sourceTake=" + motion.name
                }
            });
            log("Added optional humanoid playback. Runtime muscle-only qualification is a separate repo step.");
        }
        finally { Object.DestroyImmediate(root); }
    }
}
#endif
