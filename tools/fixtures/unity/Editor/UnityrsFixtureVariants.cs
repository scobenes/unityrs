#if UNITY_EDITOR
using System;
using System.Linq;
using UnityEditor;
using UnityEngine;
using UnityEngine.Rendering;
using UnityEngine.SceneManagement;
using UnityrsFixtures;

static partial class UnityrsFixtureBuilder
{
    const string UrpCase = "urp-materials";
    const string UrpMenu = "Tools → unityrs → Build URP material fixtures";
    const string AndroidTextureCase = "android-textures";

    // One fixture run: its target, output folder, cases and preparation.
    // run.json records the name, so the repository checks the right contract.
    sealed class Variant
    {
        public string Name = "builtin", Folder;
        public BuildTarget Target;
        public bool Record;
        public string[] Cases;
        public Func<bool> PipelineReady = () => GraphicsSettings.currentRenderPipeline == null;
        public string PipelineMessage = "Open an empty 3D (Built-in) project to build these fixtures.";
        public Action<RunReport> AddDeferred = report => { };
        public Func<RunReport, Scene, string, Build> Prepare;
    }

    static Variant BuiltInVariant(bool record)
    {
        var target = EditorUserBuildSettings.activeBuildTarget;
        return new Variant {
            Folder = target.ToString(), Target = target, Record = record,
            AddDeferred = AddDeferredCases, Prepare = PrepareFixtures
        };
    }

    [MenuItem("Tools/unityrs/Build URP material fixtures")]
    static void BuildUrpFixtures()
    {
        if (!UnityrsUrpPipeline.Active)
        {
            EditorUtility.DisplayDialog("unityrs URP fixtures need a URP project", UrpMessage(), "OK");
            return;
        }
        var target = EditorUserBuildSettings.activeBuildTarget;
        Run(new Variant {
            Name = "urp", Folder = "URP-" + target, Target = target, Cases = new[] { UrpCase },
            PipelineReady = () => UnityrsUrpPipeline.Active, PipelineMessage = UrpMessage(), Prepare = PrepareUrp
        });
    }

    static string UrpMessage() => UnityrsUrpPipeline.PackageInstalled
        ? "This project's render pipeline is " + UnityrsUrpPipeline.Current + ", not URP. In Unity Hub, create a new "
            + "project from the Universal 3D template, copy the Editor folder into its Assets folder, and run this "
            + "menu item there."
        : "The Universal RP package is not installed in this project. In Unity Hub, create a new project from the "
            + "Universal 3D template, copy the Editor folder into its Assets folder, and run this menu item there.";

    [MenuItem("Tools/unityrs/Build Android fixtures")]
    static void BuildAndroidFixtures()
    {
        if (!BuildPipeline.IsBuildTargetSupported(BuildTargetGroup.Android, BuildTarget.Android))
        {
            EditorUtility.DisplayDialog("unityrs Android fixtures need Android Build Support",
                "Android Build Support is not installed for Unity " + Application.unityVersion + ".\n\n"
                + "Close Unity. In Unity Hub, open Installs, click the gear next to " + Application.unityVersion
                + ", choose Add modules, tick Android Build Support and click Install. Then reopen this project "
                + "and run the menu item again.", "OK");
            return;
        }
        // Its own folder: a Built-in run with Android as the active target
        // publishes to <version>/Android.
        Run(new Variant {
            Name = "android", Folder = "Android-variant", Target = BuildTarget.Android,
            Cases = StaticCases.Concat(new[] { AndroidTextureCase }).ToArray(), Prepare = PrepareAndroid
        });
    }

    static Build PrepareUrp(RunReport report, Scene scene, string stamp)
    {
        var build = new Build();
        report.Prepare(build, "Prepare " + UrpCase, new[] { UrpCase }, () => {
            build.AddBundle(UrpCase, BuildUrpMaterials(Folder(Assets + "/Urp")));
            build.StaticBundles.Add(UrpCase, UrpCase);
        });
        return build;
    }

    static Build PrepareAndroid(RunReport report, Scene scene, string stamp)
    {
        var build = new Build();
        BuildStatic(build, report, scene, stamp);
        Progress("Preparing Android textures", .3f);
        report.Prepare(build, "Prepare " + AndroidTextureCase, new[] { AndroidTextureCase }, () => {
            build.AddBundle(AndroidTextureCase, BuildAndroidTextures(Folder(Assets + "/Android")));
            build.StaticBundles.Add(AndroidTextureCase, AndroidTextureCase);
        });
        return build;
    }
}
#endif
