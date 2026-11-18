#if UNITY_EDITOR
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
        try
        {
            CheckEditMode();
            TestTemporaryPrefab();
            EditorUtility.DisplayDialog("Animation replay prefab loading", "The prefab loading test passed.", "OK");
        }
        catch (Exception error)
        {
            Debug.LogException(error);
            EditorUtility.DisplayDialog("Animation replay prefab loading failed", error.Message, "OK");
        }
    }

    static void TestTemporaryPrefab()
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

            var loaded = LoadPrefab(bundle, BuiltAssetKey(bundle.GetAllAssetNames(), prefabPath));
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

static partial class UnityrsFixtureBuilder
{
    [MenuItem("Tools/unityrs/Test fixture scene close failure")]
    static void TestFixtureSceneCloseFailure()
    {
        var preparationError = new InvalidOperationException("Synthetic fixture preparation failure.");
        var closeError = new InvalidOperationException("Synthetic fixture scene close failure.");
        foreach (bool preparationFailed in new[] { false, true })
        {
            foreach (bool closeThrew in new[] { false, true })
            {
                bool preserveStaging = false;
                Exception observed = null;
                try
                {
                    PrepareAndCloseScene(() => preparationFailed ? throw preparationError : new Build(),
                        () => closeThrew ? throw closeError : false, ref preserveStaging);
                }
                catch (Exception error) { observed = error; }
                Require(preserveStaging, "A fixture scene close failure must retain staging.");
                if (preparationFailed || closeThrew)
                    Require(ReferenceEquals(observed, preparationFailed ? preparationError : closeError),
                        "A fixture scene close failure replaced the original exception.");
                else
                    Require(observed is InvalidOperationException && observed.Message == "Could not close the fixture scene before building.",
                        "A fixture scene close refusal must fail the run.");
            }
        }
        Debug.Log("Fixture scene close failure test passed.");
    }
}

#endif
