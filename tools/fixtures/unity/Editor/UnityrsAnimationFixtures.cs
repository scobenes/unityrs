#if UNITY_EDITOR
using System;
using System.Collections.Generic;
using System.Linq;
using UnityEditor;
using UnityEditor.Animations;
using UnityEngine;
using Object = UnityEngine.Object;

static partial class UnityrsFixtureBuilder
{
    const string MotionPath = "Mötión";
    const string MorphA = "Skin/RendererA";
    const string MorphB = "Skin/RendererB";
    const string PositionX = "m_LocalPosition.x";
    internal const string ReferencePath = "Reference";
    internal const string ReferenceProperty = "m_Mesh";

    static void BuildLegacy(Build build)
    {
        const string bundle = "anim-legacy";
        string dir = Folder(Assets + "/Legacy");
        string path = dir + "/legacy-explicit.anim";
        var clip = NewClip("LegacyExplicit", legacy: true);
        SetMotion(clip, AnimationCurve.Linear(0, 2, 1, 6));
        SetQuaternion(clip, MotionPath);
        clip = Save(clip, path);
        var rig = new GameObject("LegacyRig");
        try
        {
            Child(rig.transform, MotionPath, Vector3.zero);
            var animation = rig.AddComponent<Animation>();
            animation.clip = clip;
            animation.AddClip(clip, clip.name);
            animation.playAutomatically = false;
            animation.wrapMode = WrapMode.ClampForever;
            string prefab = SavePrefab(rig, dir + "/legacy.prefab");
            build.AddBundle(bundle, prefab, path);
            build.Replays.Add(Case(bundle, prefab, path, 2, 6));
        }
        finally { Object.DestroyImmediate(rig); }
    }

    static void BuildGeneric(Build build, RunReport report)
    {
        const string bundle = "anim-generic";
        string dir = null, meshPath = null;
        Mesh skin = null;
        if (!report.Try("Prepare shared generic rig assets", GenericCases, () => {
            dir = Folder(Assets + "/Generic");
            meshPath = dir + "/animated-skin.asset";
            skin = Save(NewSkin(), meshPath);
        })) return;

        string prefab = dir + "/generic.prefab";
        string overridePrefab = dir + "/override.prefab";
        string panelPath = dir + "/reference-a.asset";
        string otherPath = dir + "/reference-b.asset";
        Mesh panel = null, referencePanel = null;
        var clips = new List<AnimationClip>();
        var replays = new List<ReplayCase>();
        var assets = new List<string> { meshPath };

        AnimationClip Add(string name, float first, float last, Action<AnimationClip> curves, Action<ReplayCase> setup = null)
        {
            AnimationClip clip = null;
            report.Try("Prepare " + name, new[] { name }, () => {
                bool offset = name == "generic-offset-stepped";
                var authored = Author(dir, name, curves, offset ? .5f : 0, offset ? 1.5f : 1);
                string path = AssetDatabase.GetAssetPath(authored);
                var replay = Case(bundle, prefab, path, first, last);
                setup?.Invoke(replay);
                clips.Add(authored);
                assets.Add(path);
                replays.Add(replay);
                clip = authored;
            });
            return clip;
        }

        Add("generic-quaternion", 1, 3, clip => {
            SetMotion(clip, AnimationCurve.Linear(0, 1, 1, 3));
            SetQuaternion(clip, MotionPath);
        });
        Add("generic-euler", 2, 5, clip => {
            SetMotion(clip, AnimationCurve.Linear(0, 2, 1, 5));
            SetEuler(clip, MotionPath, "x", 15, 150);
            SetEuler(clip, MotionPath, "y", 30, -80);
            SetEuler(clip, MotionPath, "z", 10, 430);
        });
        var constant = Add("generic-constant", 8, 8, clip => SetMotion(clip, AnimationCurve.Constant(0, 1, 8)));
        Add("generic-offset-stepped", 4, 12, clip => {
            const float hold = float.PositiveInfinity;
            SetMotion(clip, new AnimationCurve(new Keyframe(.5f, 4, hold, hold),
                new Keyframe(.75f, 12, hold, hold), new Keyframe(1.5f, 12, hold, hold)));
        }, replay => {
            replay.Start = .5f;
            replay.Stop = 1.5f;
            replay.HeldTime = .75f;
            replay.HeldLeft = 4;
            replay.HeldRight = 12;
        });
        Add("generic-weighted", 1, 6, clip => {
            var left = new Keyframe(0, 1, 0, 5, .25f, .25f) { weightedMode = WeightedMode.Out };
            var right = new Keyframe(1, 6, -2, 0, .375f, .25f) { weightedMode = WeightedMode.In };
            SetMotion(clip, new AnimationCurve(left, right));
        });
        var morph = Add("generic-morph", 12.5f, 80, clip => SetMorph(clip, 12.5f, 80, 70, 25), SetMorphProbe);
        Add("generic-object-reference", 3, 9, clip => {
            var first = Save(NewPanel("ReferenceA"), panelPath);
            var second = Save(NewPanel("ReferenceB"), otherPath);
            SetMotion(clip, AnimationCurve.Linear(0, 3, 1, 9));
            SetMeshReferenceCurve(clip, first, second);
            referencePanel = first;
        }, replay => {
            panel = referencePanel;
            replay.ObjectReferenceAssets = new[] { panelPath, otherPath };
            assets.AddRange(replay.ObjectReferenceAssets);
        });
        Add("generic-scale", 1, 2.5f, clip => {
            // Vary the curve shapes so Unity can store the axes in different ranges.
            clip.SetCurve(MotionPath, typeof(Transform), "m_LocalScale.x", new AnimationCurve(
                new Keyframe(0, 1), new Keyframe(.375f, .75f), new Keyframe(1, 2.5f)));
            var left = new Keyframe(0, 1, 0, 3, .25f, .25f) { weightedMode = WeightedMode.Out };
            var right = new Keyframe(1, .5f, -2, 0, .375f, .25f) { weightedMode = WeightedMode.In };
            clip.SetCurve(MotionPath, typeof(Transform), "m_LocalScale.y", new AnimationCurve(left, right));
            clip.SetCurve(MotionPath, typeof(Transform), "m_LocalScale.z", AnimationCurve.Constant(0, 1, 1));
        }, replay => {
            replay.ProbeProperty = "scale";
            replay.ProbeMixedStorage = true;
        });

        const string morphCase = "generic-override-morph";
        const string nullCase = "generic-null-override";
        string controllerPath = dir + "/generic.controller";
        AnimatorController controller = null;
        var controllerCases = replays.Select(replay => replay.OutputName).Concat(new[] { morphCase, nullCase });
        if (!report.Try("Prepare shared generic Animator controller", controllerCases, () =>
            controller = Controller(controllerPath, clips.ToArray()))) return;
        assets.Add(controllerPath);
        if (report.Try("Prepare generic prefab", replays.Select(replay => replay.OutputName), () =>
            SaveGenericPrefab(prefab, controller, skin, panel)))
        {
            assets.Add(prefab);
            build.Replays.AddRange(replays);
        }

        ReplayCase replaced = null, fallback = null;
        AnimationClip replacement = null;
        report.Try("Prepare " + morphCase, new[] { morphCase }, () => {
            Require(morph != null, "The original generic-morph clip could not be prepared.");
            replacement = Author(dir, morphCase, clip => SetMorph(clip, 25, 50, 80, 10));
            replaced = Case(bundle, overridePrefab, AssetDatabase.GetAssetPath(replacement), 25, 50);
            SetMorphProbe(replaced);
            replaced.ExpectedDiscovery = "override";
        });
        report.Try("Prepare " + nullCase, new[] { nullCase }, () => {
            Require(constant != null, "The original generic-constant clip could not be prepared.");
            fallback = Case(bundle, overridePrefab, AssetDatabase.GetAssetPath(constant), 8, 8);
            fallback.ReplayName = nullCase;
            fallback.ExpectedDiscovery = "null_override";
        });
        if (replaced != null || fallback != null)
        {
            var candidates = new[] { replaced, fallback }.Where(replay => replay != null).Select(replay => replay.OutputName);
            AnimatorOverrideController overridden = null;
            if (report.Try("Create generic override controller", candidates, () =>
                overridden = new AnimatorOverrideController(controller) { name = "GenericOverrides" }))
            {
                if (replaced != null && !report.Try("Set morph override", new[] { morphCase }, () =>
                    overridden.ApplyOverrides(new List<KeyValuePair<AnimationClip, AnimationClip>> {
                        new KeyValuePair<AnimationClip, AnimationClip>(morph, replacement)
                    }))) replaced = null;
                if (fallback != null && !report.Try("Set null override", new[] { nullCase }, () =>
                    EnsureNullOverride(overridden, constant))) fallback = null;

                var overrides = new[] { replaced, fallback }.Where(replay => replay != null).ToArray();
                string overridePath = dir + "/generic.overrideController";
                if (overrides.Length > 0 && report.Try("Prepare override prefab", overrides.Select(replay => replay.OutputName), () => {
                    overridden = Save(overridden, overridePath);
                    SaveGenericPrefab(overridePrefab, overridden, skin, panel);
                }))
                {
                    assets.AddRange(new[] { overridePrefab, overridePath });
                    if (replaced != null) assets.Add(replaced.Clip);
                    build.Replays.AddRange(overrides);
                }
            }
            if (overridden != null && !AssetDatabase.Contains(overridden)) Object.DestroyImmediate(overridden);
        }
        if (build.Replays.Any(replay => replay.Bundle == bundle)) build.AddBundle(bundle, assets.ToArray());
    }

    static void SaveGenericPrefab(string path, RuntimeAnimatorController controller, Mesh skin, Mesh panel)
    {
        var root = new GameObject("GenericRig");
        try
        {
            Child(root.transform, MotionPath, Vector3.zero);
            AddSkins(root.transform, skin);
            if (panel != null)
            {
                var reference = Child(root.transform, ReferencePath, new Vector3(0, 0, 2)).gameObject;
                reference.AddComponent<MeshFilter>().sharedMesh = panel;
            }
            var animator = root.AddComponent<Animator>();
            animator.applyRootMotion = false;
            animator.runtimeAnimatorController = controller;
            SavePrefab(root, path);
        }
        finally { Object.DestroyImmediate(root); }
    }

    static void SetMeshReferenceCurve(AnimationClip clip, Mesh first, Mesh second)
    {
        try
        {
            var binding = EditorCurveBinding.PPtrCurve(ReferencePath, typeof(MeshFilter), ReferenceProperty);
            AnimationUtility.SetObjectReferenceCurve(clip, binding, new[] {
                new ObjectReferenceKeyframe { time = 0, value = first },
                new ObjectReferenceKeyframe { time = .5f, value = second }
            });
            var keys = AnimationUtility.GetObjectReferenceCurve(clip, binding);
            Require(keys != null && keys.Length == 2 && keys[0].time == 0 && keys[0].value == first
                && keys[1].time == .5f && keys[1].value == second, "Unity did not retain both authored mesh keys.");
        }
        catch (Exception error)
        {
            throw new InvalidOperationException("Unity rejected the MeshFilter.m_Mesh object-reference curve: "
                + error.Message, error);
        }
    }

    static void EnsureNullOverride(AnimatorOverrideController controller, AnimationClip original)
    {
        // ApplyOverrides may leave an unchanged slot implicit. Keep one
        // serialized null row for the fixture that exercises it. Confirm
        // these serialized field names and the built row on the first run.
        using (var serialized = new SerializedObject(controller))
        {
            var rows = serialized.FindProperty("m_Clips");
            Require(rows != null && rows.isArray, "Unity did not expose the authored Animator override rows.");
            for (int i = 0; i < rows.arraySize; i++)
            {
                var row = rows.GetArrayElementAtIndex(i);
                if (row.FindPropertyRelative("m_OriginalClip").objectReferenceValue != original) continue;
                row.FindPropertyRelative("m_OverrideClip").objectReferenceValue = null;
                serialized.ApplyModifiedPropertiesWithoutUndo();
                return;
            }
            int index = rows.arraySize;
            rows.InsertArrayElementAtIndex(index);
            var added = rows.GetArrayElementAtIndex(index);
            added.FindPropertyRelative("m_OriginalClip").objectReferenceValue = original;
            added.FindPropertyRelative("m_OverrideClip").objectReferenceValue = null;
            serialized.ApplyModifiedPropertiesWithoutUndo();
        }
    }

    static ReplayCase Case(string bundle, string prefab, string clip, float first, float last) => new ReplayCase {
        Bundle = bundle, Prefab = prefab, Clip = clip, ProbePath = MotionPath, First = first, Last = last
    };

    static void SetMorphProbe(ReplayCase fixture)
    {
        fixture.ProbePath = MorphA;
        fixture.ProbeProperty = "weight";
        fixture.ProbeChannel = 0;
    }

    static AnimationClip NewClip(string name, bool legacy = false) => new AnimationClip {
        name = name, frameRate = 16, legacy = legacy, wrapMode = legacy ? WrapMode.ClampForever : WrapMode.Once
    };

    static AnimationClip Author(string dir, string name, Action<AnimationClip> curves, float start = 0, float stop = 1)
    {
        var clip = NewClip(name);
        curves(clip);
        // Confirm the built interval survives SetAnimationClipSettings,
        // especially when the first authored key has a nonzero time.
        var settings = AnimationUtility.GetAnimationClipSettings(clip);
        settings.startTime = start;
        settings.stopTime = stop;
        settings.loopTime = false;
        AnimationUtility.SetAnimationClipSettings(clip, settings);
        return Save(clip, dir + "/" + name + ".anim");
    }

    static void SetMotion(AnimationClip clip, AnimationCurve x) => SetTranslation(clip, MotionPath, x);

    static void SetTranslation(AnimationClip clip, string path, AnimationCurve x)
    {
        // SetCurve supports non-legacy clips in the Editor. Provide the
        // complete local translation vector used in compiled bindings.
        clip.SetCurve(path, typeof(Transform), PositionX, x);
        var zero = AnimationCurve.Constant(x.keys[0].time, x.keys[x.length - 1].time, 0);
        clip.SetCurve(path, typeof(Transform), "m_LocalPosition.y", zero);
        clip.SetCurve(path, typeof(Transform), "m_LocalPosition.z", zero);
    }

    static void SetQuaternion(AnimationClip clip, string path)
    {
        var a = Quaternion.Euler(0, 15, 25);
        var b = Quaternion.Euler(35, -20, 55);
        var c = Quaternion.Euler(-15, 65, 10);
        const string axes = "xyzw";
        for (int i = 0; i < axes.Length; ++i)
        {
            clip.SetCurve(path, typeof(Transform), "m_LocalRotation." + axes[i], new AnimationCurve(
                new Keyframe(0, a[i], .125f, .25f),
                new Keyframe(.5f, b[i], -.375f, .125f),
                new Keyframe(1, c[i], .25f, .125f)));
        }
        clip.EnsureQuaternionContinuity();
    }

    static void SetEuler(AnimationClip clip, string path, string axis, float first, float last)
    {
        // localEulerAnglesRaw is Unity's authoring binding for unwrapped
        // Euler degrees. Check how this gets compiled on the first run.
        clip.SetCurve(path, typeof(Transform), "localEulerAnglesRaw." + axis,
            AnimationCurve.Linear(0, first, 1, last));
    }

    static void SetMorph(AnimationClip clip, float a, float b, float c, float d)
    {
        clip.SetCurve(MorphA, typeof(SkinnedMeshRenderer), "blendShape.Lift", AnimationCurve.Linear(0, a, 1, b));
        clip.SetCurve(MorphB, typeof(SkinnedMeshRenderer), "blendShape.Lift", AnimationCurve.Linear(0, c, 1, d));
        SetQuaternion(clip, "Skin/RootBone/Upper");
    }

    static AnimatorController Controller(string path, AnimationClip[] clips)
    {
        Require(clips.Length > 0, "No clips were prepared for " + path + ".");
        var controller = AssetDatabase.LoadAssetAtPath<AnimatorController>(path);
        if (controller == null) controller = AnimatorController.CreateAnimatorControllerAtPath(path);
        Require(controller != null, "Could not create " + path);
        var machine = controller.layers[0].stateMachine;
        var names = new HashSet<string>(clips.Select(clip => clip.name), StringComparer.Ordinal);
        foreach (var child in machine.states)
            if (!names.Contains(child.state.name)) machine.RemoveState(child.state);
        foreach (var clip in clips)
        {
            var state = machine.states.Select(child => child.state).FirstOrDefault(item => item.name == clip.name);
            if (state == null) state = machine.AddState(clip.name);
            state.motion = clip;
            EditorUtility.SetDirty(state);
        }
        machine.defaultState = machine.states.First(child => child.state.motion == clips[0]).state;
        EditorUtility.SetDirty(machine);
        EditorUtility.SetDirty(controller);
        return controller;
    }
}
#endif
