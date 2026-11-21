#if UNITY_EDITOR
using System;
using UnityEngine;

static partial class UnityrsFixtureBuilder
{
    static readonly string[] SpineMuscles = { "Spine Front-Back", "Spine Left-Right", "Spine Twist Left-Right" };
    static readonly float[] SingleAxis = { .5f, 0, 0 };
    static readonly float[] CombinedAxes = { -.25f, .375f, -.5f };

    static AnimationClip AuthorHumanoidClip(string dir, string name, bool bodyMotion, Action<AnimationClip> extra = null)
    {
        var clip = Author(dir, name, authored => {
            // On the first run, confirm HumanTrait names bind as Animator muscle
            // curves and humanMotion survives saving and bundle compilation.
            var names = HumanTrait.MuscleName;
            for (int i = 0; i < SpineMuscles.Length; i++)
            {
                string property = names[MuscleIndex(names, SpineMuscles[i])];
                float middle = SingleAxis[i], endValue = CombinedAxes[i];
                float firstSlope = 2 * middle, lastSlope = 2 * (endValue - middle);
                authored.SetCurve("", typeof(Animator), property, new AnimationCurve(
                    new Keyframe(0, 0, 0, firstSlope),
                    new Keyframe(.5f, middle, firstSlope, lastSlope),
                    new Keyframe(1, endValue, lastSlope, 0)));
            }

            extra?.Invoke(authored);
            if (!bodyMotion) return;
            // Keep RootT/RootQ on a separate probe. Even constant body
            // curves can make a clip ineligible for muscle-only playback.
            const string axes = "xyzw";
            var start = Vector3.up;
            var end = new Vector3(.25f, 1, 0);
            var turn = Quaternion.Euler(0, 30, 0);
            for (int i = 0; i < 3; i++)
                authored.SetCurve("", typeof(Animator), "RootT." + axes[i],
                    AnimationCurve.Linear(0, start[i], 1, end[i]));
            for (int i = 0; i < axes.Length; i++)
                authored.SetCurve("", typeof(Animator), "RootQ." + axes[i],
                    AnimationCurve.Linear(0, Quaternion.identity[i], 1, turn[i]));
            authored.EnsureQuaternionContinuity();
        });
        Require(clip.humanMotion, "Animator bindings on " + name + " did not produce a human-motion clip.");
        return clip;
    }

    internal static float[] KnownHumanoidMuscles(float time)
    {
        var names = HumanTrait.MuscleName;
        var vector = new float[names.Length];
        for (int i = 0; i < SpineMuscles.Length; i++)
        {
            // Compute the known input independently of the recorded clip.
            // The keys are neutral, single-axis, then noncommuting axes.
            float value = time <= .5f ? SingleAxis[i] * (2 * time)
                : Mathf.Lerp(SingleAxis[i], CombinedAxes[i], 2 * time - 1);
            vector[MuscleIndex(names, SpineMuscles[i])] = value;
        }
        return vector;
    }

    // A clip's own authored muscle vector at a time: each HumanTrait muscle's
    // Animator curve (MuscleProperty) evaluated there, 0 where it has none. The
    // recorder's Spine reference pose, so every clip is checked against the
    // values it authors (anim_fixture_full.authored_muscles mirrors it).
    internal static float[] AuthoredMuscles(AnimationClip clip, float time)
    {
        var names = HumanTrait.MuscleName;
        var vector = new float[names.Length];
        for (int i = 0; i < names.Length; i++)
        {
            var curve = UnityEditor.AnimationUtility.GetEditorCurve(clip,
                UnityEditor.EditorCurveBinding.FloatCurve("", typeof(Animator), MuscleProperty(names[i])));
            if (curve != null) vector[i] = curve.Evaluate(time);
        }
        return vector;
    }

    static int MuscleIndex(string[] names, string property)
    {
        int index = Array.IndexOf(names, property);
        Require(index >= 0, "HumanTrait does not list Animator muscle property " + property + ".");
        return index;
    }
}
#endif
