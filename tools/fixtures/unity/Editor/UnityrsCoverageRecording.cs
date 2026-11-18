#if UNITY_EDITOR
using System;
using System.Collections.Generic;
using System.Globalization;
using System.Linq;
using UnityEditor;
using UnityEditor.SceneManagement;
using UnityEngine;
using UnityEngine.SceneManagement;

// Recording evidence for the optional coverage fixtures.
public static partial class AnimReplayRecorder
{
    [Serializable] sealed class DiscreteSample { public string time_f32; public int value; }
    [Serializable] sealed class TwistEffect
    {
        public string bone;
        public int node;
        public string[] authored_f32, neutral_f32;
    }

    static readonly HumanBodyBones[] TwistBones = {
        HumanBodyBones.LeftUpperArm, HumanBodyBones.LeftLowerArm, HumanBodyBones.LeftHand,
        HumanBodyBones.LeftUpperLeg, HumanBodyBones.LeftLowerLeg, HumanBodyBones.LeftFoot
    };

    // Read the animated sorting order while the graph owns the pose.
    static DiscreteSample ReadDiscrete(Transform root, float time)
    {
        var node = root.Find(UnityrsFixtureBuilder.DiscretePath);
        Need(node != null, "Built playback is missing the discrete binding target.");
        var renderer = node.GetComponent<MeshRenderer>();
        Need(renderer != null, "Built playback is missing the discrete MeshRenderer.");
        var times = UnityrsFixtureBuilder.DiscreteTimes;
        var values = UnityrsFixtureBuilder.DiscreteValues;
        int expected = time < times[1] ? values[0] : values[1];
        UnityrsFixtureBuilder.Unavailable(renderer.sortingOrder == expected, "Built playback set MeshRenderer." + UnityrsFixtureBuilder.DiscreteProperty
            + " to " + renderer.sortingOrder.ToString(CultureInfo.InvariantCulture) + " at "
            + time.ToString("R", CultureInfo.InvariantCulture) + " instead of the authored "
            + expected.ToString(CultureInfo.InvariantCulture) + ".");
        return new DiscreteSample { time_f32 = Bits(time), value = renderer.sortingOrder };
    }

    // The authored integer keys, decoded from the bits Unity keeps in each key's value.
    static DiscreteSample[] ReadDiscreteKeys(AnimationClip clip)
    {
        var bindings = AnimationUtility.GetCurveBindings(clip).Where(row => row.isDiscreteCurve && !row.isPPtrCurve
            && row.path == UnityrsFixtureBuilder.DiscretePath).ToArray();
        Need(bindings.Length == 1, "Expected one authored discrete integer binding.");
        return AnimationUtility.GetEditorCurve(clip, bindings[0]).keys.Select(key => new DiscreteSample {
            time_f32 = Bits(key.time), value = UnityrsFixtureBuilder.IntegerValue(key.value)
        }).ToArray();
    }

    // Show that the authored Avatar twist redistributes the end-pose twist
    // muscles: compare the left limb bones against the same pose on a twin
    // Avatar with neutral twist, and the built playback against the authored.
    static void CheckTwistEffects(HumanCapture capture, UnityrsFixtureBuilder.ReplayCase fixture, GameObject prefab,
        EditorEvidence editor)
    {
        string name = fixture.OutputName;
        Need(capture != null && fixture.TwistValues != null && fixture.TwistMuscles.Length == fixture.TwistValues.Length,
            "Twist evidence needs a humanoid capture and its twist muscles for " + name + ".");
        var names = HumanTrait.MuscleName;
        var vector = UnityrsFixtureBuilder.KnownHumanoidMuscles(1);
        for (int i = 0; i < fixture.TwistMuscles.Length; i++)
        {
            int index = Array.IndexOf(names, fixture.TwistMuscles[i]);
            Need(index >= 0, "HumanTrait does not list the twist muscle " + fixture.TwistMuscles[i] + ".");
            vector[index] = fixture.TwistValues[i];
        }
        var authored = LimbRotations(prefab, fixture, vector, neutralTwist: false);
        var neutral = LimbRotations(prefab, fixture, vector, neutralTwist: true);
        var last = capture.samples[capture.samples.Length - 1];
        var effects = new List<TwistEffect>();
        float arm = 0, leg = 0;
        for (int i = 0; i < TwistBones.Length; i++)
        {
            string bone = HumanTrait.BoneName[(int)TwistBones[i]];
            int node = capture.humanoid.bones.Single(row => row.name == bone).node;
            Need(node >= 0, "Built Avatar is missing the mapped " + bone + " for " + name + ".");
            var bits = last.pose.transforms[node].rotation;
            float Component(int c) => BitConverter.ToSingle(BitConverter.GetBytes(
                uint.Parse(bits[c], NumberStyles.HexNumber, CultureInfo.InvariantCulture)), 0);
            var built = new Quaternion(Component(0), Component(1), Component(2), Component(3));
            Need(Quaternion.Angle(built, authored[i]) <= .25f,
                "Built " + bone + " rotation differs from the authored twist pose in " + name + ".");
            float moved = Quaternion.Angle(authored[i], neutral[i]);
            if (i < 3) arm = Math.Max(arm, moved);
            else leg = Math.Max(leg, moved);
            effects.Add(new TwistEffect { bone = bone, node = node, authored_f32 = Bits(authored[i]), neutral_f32 = Bits(neutral[i]) });
        }
        var invariant = CultureInfo.InvariantCulture;
        UnityrsFixtureBuilder.Unavailable(arm > 1 && leg > 1, "Unity posed the left limbs alike with neutral and authored Avatar twist (arm "
            + arm.ToString("R", invariant) + ", leg " + leg.ToString("R", invariant) + " degrees).");
        editor.twist_effects = effects.ToArray();
    }

    static Quaternion[] LimbRotations(GameObject prefab, UnityrsFixtureBuilder.ReplayCase fixture, float[] muscles,
        bool neutralTwist)
    {
        Scene scene = EditorSceneManager.NewPreviewScene();
        Avatar twin = null;
        try
        {
            var anchor = EditorUtility.CreateGameObjectWithHideFlags("Twist probe", HideFlags.HideAndDontSave);
            SceneManager.MoveGameObjectToScene(anchor, scene);
            var instance = UnityEngine.Object.Instantiate(prefab, anchor.transform, false);
            var animator = At<Animator>(instance.transform, fixture.BindingAddress, 0);
            // BuildHumanAvatar needs the rig as a topmost object. Unparent it
            // for both evaluations; the compared rotations are local.
            var rig = animator.gameObject;
            rig.transform.SetParent(null, false);
            SceneManager.MoveGameObjectToScene(rig, scene);
            animator.runtimeAnimatorController = null;
            animator.applyRootMotion = false;
            if (neutralTwist)
            {
                var description = animator.avatar.humanDescription;
                description.upperArmTwist = description.lowerArmTwist = 0;
                description.upperLegTwist = description.lowerLegTwist = 0;
                twin = AvatarBuilder.BuildHumanAvatar(rig, description);
                Need(twin != null && twin.isValid && twin.isHuman, "Could not build the neutral-twist twin Avatar.");
                animator.avatar = twin;
                animator.Rebind();
            }
            SampleMuscles(animator, muscles);
            return TwistBones.Select(id => {
                var bone = animator.GetBoneTransform(id);
                Need(bone != null, "Twist probe rig is missing " + id + ".");
                return bone.localRotation;
            }).ToArray();
        }
        finally
        {
            EditorSceneManager.ClosePreviewScene(scene);
            if (twin != null) UnityEngine.Object.DestroyImmediate(twin);
        }
    }
}
#endif
