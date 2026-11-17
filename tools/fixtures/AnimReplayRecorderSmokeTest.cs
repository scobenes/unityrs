// Copy beside AnimReplayRecorder.cs in Assets/Editor.
using System;
using System.IO;
using UnityEditor;
using UnityEngine;
using Object = UnityEngine.Object;

public static partial class AnimReplayRecorder
{
    [MenuItem("Tools/unityrs/Test animation replay prefab loading")]
    public static void TestPrefabLoading()
    {
        string name = "unityrs-replay-" + Guid.NewGuid().ToString("N");
        string assetDir = "Assets/" + name;
        string prefabPath = assetDir + "/rig.prefab";
        string output = Path.Combine(Path.GetTempPath(), name);
        const string bundleName = "replay-rig";
        AssetBundle bundle = null;
        var rig = new GameObject("Rig");
        try
        {
            var hips = new GameObject("Hips");
            hips.transform.SetParent(rig.transform);
            var spine = new GameObject("Spine");
            spine.transform.SetParent(hips.transform);
            AssetDatabase.CreateFolder("Assets", name);
            Need(PrefabUtility.SaveAsPrefabAsset(rig, prefabPath) != null, "Could not save the smoke-test rig.");
            Directory.CreateDirectory(output);
            var build = new AssetBundleBuild { assetBundleName = bundleName, assetNames = new[] { prefabPath } };
            Need(BuildPipeline.BuildAssetBundles(output, new[] { build }, BuildAssetBundleOptions.None,
                EditorUserBuildSettings.activeBuildTarget) != null, "Could not build the smoke-test rig.");
            bundle = AssetBundle.LoadFromFile(Path.Combine(output, bundleName));
            Need(bundle != null, "Could not load the smoke-test bundle.");

            var loaded = LoadPrefab(bundle, prefabPath.ToLowerInvariant());
            Need(loaded.name == "Rig" && loaded.transform.parent == null
                && loaded.GetComponentsInChildren<Transform>(true).Length == 3
                && At(loaded.transform, "0/0").name == "Spine",
                "Prefab lookup did not return the root and full hierarchy.");
            Debug.Log("Animation replay prefab lookup passed.");
        }
        finally
        {
            if (bundle != null) bundle.Unload(true);
            Object.DestroyImmediate(rig);
            AssetDatabase.DeleteAsset(assetDir);
            if (Directory.Exists(output)) Directory.Delete(output, true);
        }
    }
}
