#if UNITY_EDITOR
using System;
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

    static void BuildGeneric(Build build)
    {
        const string bundle = "anim-generic";
        string dir = Folder(Assets + "/Generic");
        string prefab = dir + "/generic.prefab";
        string overridePrefab = dir + "/override.prefab";
        string meshPath = dir + "/animated-skin.asset";
        var skin = Save(NewSkin(), meshPath);
        string panelPath = dir + "/reference-a.asset";
        string otherPath = dir + "/reference-b.asset";
        var panel = Save(NewPanel("ReferenceA"), panelPath);
        var other = Save(NewPanel("ReferenceB"), otherPath);
        var clips = new[] {
            Author(dir, "generic-quaternion", clip => {
                SetMotion(clip, AnimationCurve.Linear(0, 1, 1, 3));
                SetQuaternion(clip, MotionPath);
            }),
            Author(dir, "generic-euler", clip => {
                SetMotion(clip, AnimationCurve.Linear(0, 2, 1, 5));
                SetEuler(clip, MotionPath, "x", 15, 150);
                SetEuler(clip, MotionPath, "y", 30, -80);
                SetEuler(clip, MotionPath, "z", 10, 430);
            }),
            Author(dir, "generic-constant", clip => SetMotion(clip, AnimationCurve.Constant(0, 1, 8))),
            Author(dir, "generic-offset-stepped", clip => {
                const float hold = float.PositiveInfinity;
                SetMotion(clip, new AnimationCurve(new Keyframe(.5f, 4, hold, hold),
                    new Keyframe(.75f, 12, hold, hold), new Keyframe(1.5f, 12, hold, hold)));
            }, start: .5f, stop: 1.5f),
            Author(dir, "generic-weighted", clip => {
                var left = new Keyframe(0, 1, 0, 5, .25f, .25f) { weightedMode = WeightedMode.Out };
                var right = new Keyframe(1, 6, -2, 0, .375f, .25f) { weightedMode = WeightedMode.In };
                SetMotion(clip, new AnimationCurve(left, right));
            }),
            Author(dir, "generic-morph", clip => SetMorph(clip, 12.5f, 80, 70, 25)),
            Author(dir, "generic-object-reference", clip => {
                SetMotion(clip, AnimationCurve.Linear(0, 3, 1, 9));
                var binding = EditorCurveBinding.PPtrCurve(ReferencePath, typeof(MeshFilter), ReferenceProperty);
                AnimationUtility.SetObjectReferenceCurve(clip, binding, new[] {
                    new ObjectReferenceKeyframe { time = 0, value = panel },
                    new ObjectReferenceKeyframe { time = .5f, value = other }
                });
            }),
            Author(dir, "generic-scale", clip => {
                // Give the three axes different curve shapes so we can check
                // whether the compiler stores the vector in separate ranges.
                clip.SetCurve(MotionPath, typeof(Transform), "m_LocalScale.x", new AnimationCurve(
                    new Keyframe(0, 1), new Keyframe(.375f, .75f), new Keyframe(1, 2.5f)));
                var left = new Keyframe(0, 1, 0, 3, .25f, .25f) { weightedMode = WeightedMode.Out };
                var right = new Keyframe(1, .5f, -2, 0, .375f, .25f) { weightedMode = WeightedMode.In };
                clip.SetCurve(MotionPath, typeof(Transform), "m_LocalScale.y", new AnimationCurve(left, right));
                clip.SetCurve(MotionPath, typeof(Transform), "m_LocalScale.z", AnimationCurve.Constant(0, 1, 1));
            })
        };
        var replacement = Author(dir, "generic-override-morph", clip => SetMorph(clip, 25, 50, 80, 10));
        string controllerPath = dir + "/generic.controller";
        var controller = Controller(controllerPath, clips);
        string overridePath = dir + "/generic.overrideController";
        var overridden = new AnimatorOverrideController(controller) { name = "GenericOverrides" };
        // ApplyOverrides retains null as the authored request to use the
        // original slot. Repo completion checks what Unity actually stored.
        var slots = clips.Select(clip => new System.Collections.Generic.KeyValuePair<AnimationClip, AnimationClip>(
            clip, clip.name == "generic-morph" ? replacement : null)).ToList();
        overridden.ApplyOverrides(slots);
        EnsureNullOverride(overridden, clips[2]);
        overridden = Save(overridden, overridePath);

        var root = new GameObject("GenericRig");
        try
        {
            Child(root.transform, MotionPath, Vector3.zero);
            AddSkins(root.transform, skin);
            var reference = Child(root.transform, ReferencePath, new Vector3(0, 0, 2)).gameObject;
            reference.AddComponent<MeshFilter>().sharedMesh = panel;
            var animator = root.AddComponent<Animator>();
            animator.applyRootMotion = false;
            animator.runtimeAnimatorController = controller;
            SavePrefab(root, prefab);
            animator.runtimeAnimatorController = overridden;
            SavePrefab(root, overridePrefab);
        }
        finally { Object.DestroyImmediate(root); }

        var paths = clips.Select(AssetDatabase.GetAssetPath).ToArray();
        string replacementPath = AssetDatabase.GetAssetPath(replacement);
        build.AddBundle(bundle, paths.Concat(new[] { prefab, overridePrefab, meshPath, panelPath, otherPath,
            replacementPath, controllerPath, overridePath }).ToArray());
        build.Replays.Add(Case(bundle, prefab, paths[0], 1, 3));
        build.Replays.Add(Case(bundle, prefab, paths[1], 2, 5));
        build.Replays.Add(Case(bundle, prefab, paths[2], 8, 8));
        var offset = Case(bundle, prefab, paths[3], 4, 12);
        offset.Start = .5f;
        offset.Stop = 1.5f;
        offset.HeldTime = .75f;
        offset.HeldLeft = 4;
        offset.HeldRight = 12;
        build.Replays.Add(offset);
        build.Replays.Add(Case(bundle, prefab, paths[4], 1, 6));
        build.Replays.Add(MorphCase(bundle, prefab, paths[5], 12.5f, 80));
        var referenceReplay = Case(bundle, prefab, paths[6], 3, 9);
        referenceReplay.ObjectReferenceAssets = new[] { panelPath, otherPath };
        build.Replays.Add(referenceReplay);
        var scale = Case(bundle, prefab, paths[7], 1, 2.5f);
        scale.ProbeProperty = "scale";
        scale.ProbeMixedStorage = true;
        build.Replays.Add(scale);
        var replaced = MorphCase(bundle, overridePrefab, replacementPath, 25, 50);
        replaced.ExpectedDiscovery = "override";
        build.Replays.Add(replaced);
        var nullOverride = Case(bundle, overridePrefab, paths[2], 8, 8);
        nullOverride.ReplayName = "generic-null-override";
        nullOverride.ExpectedDiscovery = "null_override";
        build.Replays.Add(nullOverride);
    }

    static void EnsureNullOverride(AnimatorOverrideController controller, AnimationClip original)
    {
        // ApplyOverrides may leave an unchanged slot implicit. Keep one
        // serialized null row for the fixture that exercises it. Confirm
        // these serialized field names and the built row on the first run.
        using (var serialized = new SerializedObject(controller))
        {
            var rows = serialized.FindProperty("m_Clips");
            if (rows == null || !rows.isArray)
            {
                Debug.LogWarning("Could not check the authored null Animator override row.");
                return;
            }
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

    static ReplayCase MorphCase(string bundle, string prefab, string clip, float first, float last)
    {
        var fixture = Case(bundle, prefab, clip, first, last);
        fixture.ProbePath = MorphA;
        fixture.ProbeProperty = "weight";
        fixture.ProbeChannel = 0;
        return fixture;
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
        var controller = AssetDatabase.LoadAssetAtPath<AnimatorController>(path);
        if (controller == null) controller = AnimatorController.CreateAnimatorControllerAtPath(path);
        Require(controller != null, "Could not create " + path);
        var machine = controller.layers[0].stateMachine;
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
