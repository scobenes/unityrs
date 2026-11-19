#if UNITY_EDITOR
using System;
using System.Collections.Generic;
using System.Linq;
using UnityEngine;

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
    // Read-back in the Animator's space (position over human scale), as the
    // rest body; the raw world read-back; and, during clip playback, the
    // Animator's world root and body rows, bit for bit even if nonfinite.
    [Serializable] sealed class BodySampleRow
    {
        public string[] readback_position, readback_rotation, world_position, world_rotation,
            root_position, root_rotation, body_position, body_rotation;
    }
    // The requested offset and tilt (Euler degrees), and the body pose SetHumanPose received.
    [Serializable] sealed class BodyTargetRow { public string[] offset, tilt, position, rotation; }
    [Serializable] sealed class HumanBoneRow { public string name, mass; public int parent; public bool required; }

    sealed class BodyHooks
    {
        public UnityrsFixtureBuilder.BodyProbe[] Probes;
        public readonly List<BodySampleRow> Samples = new List<BodySampleRow>();
        public readonly List<BodyTargetRow> Targets = new List<BodyTargetRow>();

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

        public void Read(Animator animator, bool playback)
        {
            using (var handler = new HumanPoseHandler(animator.avatar, animator.transform))
            {
                var world = new HumanPose();
                handler.GetHumanPose(ref world);
                var local = InAnimatorSpace(animator, world);
                var row = new BodySampleRow {
                    readback_position = Bits(local.bodyPosition), readback_rotation = Bits(local.bodyRotation),
                    world_position = Bits(world.bodyPosition), world_rotation = Bits(world.bodyRotation),
                    root_position = Array.Empty<string>(), root_rotation = Array.Empty<string>(),
                    body_position = Array.Empty<string>(), body_rotation = Array.Empty<string>()
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
        editor.body_case = new[] { BodyCaseOf(fixture) };
        editor.body_samples = body.Samples.ToArray();
        editor.body_targets = body.Targets.ToArray();
        editor.human_bones = HumanBones();
    }

    static string[] RawBits(Vector3 v) => new[] { RawBits(v.x), RawBits(v.y), RawBits(v.z) };
    static string[] RawBits(Quaternion q) => new[] { RawBits(q.x), RawBits(q.y), RawBits(q.z), RawBits(q.w) };
}
#endif
