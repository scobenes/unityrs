#if UNITY_EDITOR
using System;
using System.Collections.Generic;
using System.Linq;
using UnityEngine;
using UnityEngine.Animations;
using UnityEngine.Playables;

// Recorder rows for the body fixtures (UnityrsBodyFixtures.cs), kept in the
// editor evidence beside the capture: the case, F2's GetHumanPose read-back
// and the Animator's root and body rows for each sample, each known vector's
// requested and applied body target, and HumanTrait's bone table with the
// default hierarchy masses.
public static partial class AnimReplayRecorder
{
    [Serializable] sealed class BodyCaseRow
    {
        public string kind, rig, variant, root, level, orientation_offset;
        public bool[] bake, keep;
        public bool height_from_feet, start_offset, motion_curves, knee_bend, foot_goals;
    }
    // Per sample: GetHumanPose's read-back in the Animator's space (position
    // over human scale), as the rest body, its raw world pose and its muscles;
    // during clip playback, the Animator's rootPosition, rootRotation,
    // bodyPosition and bodyRotation getter values with applyRootMotion off
    // (rootPosition and rootRotation may then be the GameObject's own
    // transform, not the clip's projection), bit for bit even if nonfinite;
    // Unity's own world transform of every node and, about the hips, its
    // position and rotation; and, for a clip, a second playback with root
    // motion applied (motion_*).
    [Serializable] sealed class BodySampleRow
    {
        public string[] readback_position, readback_rotation, readback_muscles, world_position, world_rotation,
            root_position, root_rotation, body_position, body_rotation,
            motion_position, motion_rotation, motion_delta_position, motion_delta_rotation;
        public BodyNodeRow[] nodes;
    }
    // One node in Unity's own evaluation: world position, rotation and lossy
    // scale, and InverseTransformPoint and the rotation relative to the hips.
    [Serializable] sealed class BodyNodeRow { public string[] position, rotation, scale, hips_position, hips_rotation; }
    // The requested offset and tilt (Euler degrees), and the body pose SetHumanPose received.
    [Serializable] sealed class BodyTargetRow { public string[] offset, tilt, position, rotation; }
    [Serializable] sealed class HumanBoneRow { public string name, mass; public int parent; public bool required; }

    sealed class BodyHooks
    {
        public UnityrsFixtureBuilder.BodyProbe[] Probes;
        public UnityrsFixtureBuilder.BodyCase Case;
        public readonly List<BodySampleRow> Samples = new List<BodySampleRow>();
        public readonly List<BodyTargetRow> Targets = new List<BodyTargetRow>();

        // A clip case also plays with root motion applied.
        public bool RootMotion => Case.Kind == "clip";

        public void Apply(int index, Animator animator, float[] muscles)
        {
            var probe = Probes[index];
            var tilt = Quaternion.Euler(probe.Tilt.x, probe.Tilt.y, probe.Tilt.z);
            var applied = SampleMuscles(animator, muscles, probe.Offset, probe.Targeted ? tilt : (Quaternion?)null);
            Targets.Add(new BodyTargetRow {
                offset = Bits(probe.Offset), tilt = Bits(probe.Tilt),
                position = Bits(applied.bodyPosition), rotation = Bits(applied.bodyRotation)
            });
        }

        public void Read(Animator animator, Transform[] nodes, bool playback)
        {
            CheckRig(animator, Case);
            using (var handler = new HumanPoseHandler(animator.avatar, animator.transform))
            {
                var world = new HumanPose();
                handler.GetHumanPose(ref world);
                var local = InAnimatorSpace(animator, world);
                var hips = animator.GetBoneTransform(HumanBodyBones.Hips);
                var row = new BodySampleRow {
                    readback_position = Bits(local.bodyPosition), readback_rotation = Bits(local.bodyRotation),
                    readback_muscles = (world.muscles ?? Array.Empty<float>()).Select(RawBits).ToArray(),
                    world_position = Bits(world.bodyPosition), world_rotation = Bits(world.bodyRotation),
                    root_position = Array.Empty<string>(), root_rotation = Array.Empty<string>(),
                    body_position = Array.Empty<string>(), body_rotation = Array.Empty<string>(),
                    motion_position = Array.Empty<string>(), motion_rotation = Array.Empty<string>(),
                    motion_delta_position = Array.Empty<string>(), motion_delta_rotation = Array.Empty<string>(),
                    nodes = nodes.Select(node => new BodyNodeRow {
                        position = Bits(node.position), rotation = Bits(node.rotation), scale = Bits(node.lossyScale),
                        hips_position = Bits(hips.InverseTransformPoint(node.position)),
                        hips_rotation = Bits(Quaternion.Inverse(hips.rotation) * node.rotation)
                    }).ToArray()
                };
                if (playback)
                {
                    row.root_position = RawBits(animator.rootPosition);
                    row.root_rotation = RawBits(animator.rootRotation);
                    row.body_position = RawBits(animator.bodyPosition);
                    row.body_rotation = RawBits(animator.bodyRotation);
                }
                Samples.Add(row);
            }
        }

        // The clip again on a fresh instance with applyRootMotion on: the graph
        // starts at 0 and advances to the sample time in one evaluation, so the
        // Animator node's resulting transform is the clip's root motion from 0.
        public void PlayRootMotion(Animator animator, AnimationClip clip, float time)
        {
            animator.applyRootMotion = true;
            var graph = PlayableGraph.Create("unityrs root motion");
            try
            {
                graph.SetTimeUpdateMode(DirectorUpdateMode.Manual);
                var playable = AnimationClipPlayable.Create(graph, clip);
                playable.SetApplyFootIK(false);
                playable.SetApplyPlayableIK(false);
                playable.SetSpeed(1);
                var output = AnimationPlayableOutput.Create(graph, "Root motion", animator);
                output.SetSourcePlayable(playable);
                graph.Play();
                playable.SetTime(0);
                graph.Evaluate(0);
                graph.Evaluate(time);
                Need(playable.GetTime() == time, "Root-motion playback did not reach the sample time.");
                var row = Samples[Samples.Count - 1];
                var node = animator.transform;
                row.motion_position = RawBits(node.localPosition);
                row.motion_rotation = RawBits(node.localRotation);
                row.motion_delta_position = RawBits(animator.deltaPosition);
                row.motion_delta_rotation = RawBits(animator.deltaRotation);
            }
            finally { graph.Destroy(); }
        }
    }

    // The built Avatar maps exactly the body bones the case's schedule assumes,
    // and the F5 chain holds at this sample: the Animator node's scale, the
    // armature, the hips scale and every other mapped bone's authored offset.
    static void CheckRig(Animator animator, UnityrsFixtureBuilder.BodyCase body)
    {
        string name = body.Kind + " " + body.Rig;
        var mapped = Enumerable.Range(0, (int)HumanBodyBones.LastBone)
            .Select(i => (name: HumanTrait.BoneName[i], bone: animator.GetBoneTransform((HumanBodyBones)i)))
            .Where(row => row.bone != null).ToArray();
        Need(mapped.All(row => row.bone.name == row.name)
            && new HashSet<string>(mapped.Select(row => row.name)).SetEquals(body.Mapped),
            "The built Avatar does not map the bones the " + name + " schedule assumes.");
        var rig = body.Spec;
        bool Near(Vector3 value, Vector3 want) => (value - want).magnitude <= 1e-4f * Math.Max(1, want.magnitude);
        var root = animator.transform;
        var armature = root.Find("Armature");
        var hips = animator.GetBoneTransform(HumanBodyBones.Hips);
        Need(Near(root.localScale, Vector3.one * (rig.ScaledAnimator ? UnityrsFixtureBuilder.BodyAnimatorScale : 1))
            && (armature != null) == rig.Armature && hips.parent == (rig.Armature ? armature : root)
            && (!rig.Armature || Near(armature.localScale, Vector3.one * UnityrsFixtureBuilder.BodyArmatureScale))
            && Near(hips.localScale, Vector3.one * rig.HipsScale),
            "The " + name + " chain (Animator, armature or hips scale) differs from its rig.");
        var bones = mapped.ToDictionary(row => row.name, row => row.bone);
        Need(UnityrsFixtureBuilder.RigNodes(rig).Where(node => node.Parent.Length > 0)
            .All(node => bones[node.Name].parent == bones[node.Parent] && Near(bones[node.Name].localPosition, node.Offset)),
            "The " + name + " bone offsets differ from its rig.");
    }

    // GetHumanPose returns a world pose, with position divided by human scale.
    // SetHumanPose takes the corresponding normalized pose in Animator root space.
    static HumanPose InAnimatorSpace(Animator animator, HumanPose pose)
    {
        var root = animator.transform;
        float scale = animator.humanScale;
        float determinant = root.localToWorldMatrix.determinant;
        Need(scale > 0 && !float.IsInfinity(scale) && determinant != 0
            && !float.IsNaN(determinant) && !float.IsInfinity(determinant),
            "Cannot express the body pose in the Animator root.");
        pose.bodyPosition = root.InverseTransformPoint(pose.bodyPosition * scale) / scale;
        pose.bodyRotation = Quaternion.Inverse(root.rotation) * pose.bodyRotation;
        return pose;
    }

    static HumanBoneRow[] HumanBones() => Enumerable.Range(0, HumanTrait.BoneCount).Select(i => new HumanBoneRow {
        name = HumanTrait.BoneName[i], parent = HumanTrait.GetParentBone(i),
        mass = Bits(HumanTrait.GetBoneDefaultHierarchyMass(i)), required = HumanTrait.RequiredBone(i)
    }).ToArray();

    static BodyCaseRow BodyCaseOf(UnityrsFixtureBuilder.ReplayCase fixture)
    {
        var body = fixture.Body;
        var clip = fixture.BodyClip;
        return new BodyCaseRow {
            kind = body.Kind, rig = body.Rig, variant = body.Variant, root = body.Root,
            bake = clip?.Bake ?? new bool[3], keep = clip?.Keep ?? new bool[3],
            height_from_feet = clip?.HeightFromFeet ?? false, level = Bits(clip?.Level ?? 0),
            orientation_offset = Bits(clip?.Orientation ?? 0), start_offset = clip?.Start ?? false,
            motion_curves = clip?.Motion ?? false, knee_bend = clip?.Knee ?? false, foot_goals = clip?.Goals ?? false
        };
    }

    // The known-vector schedule of a body probe or target case.
    static Timepoint[] BodyPoints(UnityrsFixtureBuilder.ReplayCase fixture, double[] keys)
    {
        var probes = fixture.BodyProbes;
        Need(keys.Contains(0) && keys.Contains(.5) && keys.Contains(1)
            && probes.Select(probe => (float)probe.Time).Distinct().Count() == probes.Length
            && probes[0].Time == 0 && probes[probes.Length - 1].Time == 1 && probes.Any(probe => probe.Time == .5),
            "Body probe schedule must hold its spine anchors at 0, .5 and 1 for " + fixture.OutputName + ".");
        return probes.Select(probe => new Timepoint { local = probe.Time }).ToArray();
    }

    static void SetBodyEvidence(EditorEvidence editor, UnityrsFixtureBuilder.ReplayCase fixture, BodyHooks body,
        Capture capture, bool knownPose)
    {
        Need(body.Samples.Count == capture.samples.Length && body.Targets.Count == (knownPose ? capture.samples.Length : 0),
            "Body read-back rows do not cover every sample of " + fixture.OutputName + ".");
        Need(body.Samples.All(row => row.motion_position.Length == (body.RootMotion ? 3 : 0)),
            "Root-motion rows do not cover every sample of " + fixture.OutputName + ".");
        editor.body_case = new[] { BodyCaseOf(fixture) };
        editor.body_samples = body.Samples.ToArray();
        editor.body_targets = body.Targets.ToArray();
        editor.human_bones = HumanBones();
    }

    static string[] RawBits(Vector3 v) => new[] { RawBits(v.x), RawBits(v.y), RawBits(v.z) };
    static string[] RawBits(Quaternion q) => new[] { RawBits(q.x), RawBits(q.y), RawBits(q.z), RawBits(q.w) };
}
#endif
