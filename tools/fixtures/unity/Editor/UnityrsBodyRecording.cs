#if UNITY_EDITOR
using System;
using System.Collections.Generic;
using System.Globalization;
using System.Linq;
using UnityEngine;
using UnityEngine.Animations;
using UnityEngine.Playables;

// Recorder rows for the body fixtures (UnityrsBodyFixtures.cs), kept in the
// editor evidence beside the capture: the case, F2's GetHumanPose read-back
// and the Animator's root rows for each sample, each known vector's
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
    // during clip playback, the Animator's rootPosition and rootRotation
    // getter values with applyRootMotion off (they may then be the
    // GameObject's own transform, not the clip's projection);
    // Unity's own world transform of every node and, about the hips, its
    // position and rotation; and, for a clip, a second playback with root
    // motion applied (motion_*). Every value is raw bits, nonfinite included.
    [Serializable] sealed class BodySampleRow
    {
        public string[] readback_position, readback_rotation, readback_muscles, world_position, world_rotation,
            root_position, root_rotation,
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

        // Every body float is kept as raw bits, nonfinite included, and
        // counted by field: values seen, nonfinite values and the first.
        readonly Dictionary<string, (int total, int bad, float first)> fields =
            new Dictionary<string, (int, int, float)>(StringComparer.Ordinal);

        public string[] Raw(string field, params float[] values)
        {
            fields.TryGetValue(field, out var seen);
            foreach (float value in values)
            {
                bool bad = float.IsNaN(value) || float.IsInfinity(value);
                seen = (seen.total + 1, seen.bad + (bad ? 1 : 0), seen.bad == 0 && bad ? value : seen.first);
            }
            fields[field] = seen;
            return values.Select(RawBits).ToArray();
        }

        public string[] Raw(string field, Vector3 v) => Raw(field, v.x, v.y, v.z);
        public string[] Raw(string field, Quaternion q) => Raw(field, q.x, q.y, q.z, q.w);

        // One warning per field that held nonfinite values since the last
        // report; the case still records, and completion decides.
        public void ReportNonfinite(string name)
        {
            foreach (var pair in fields.Where(pair => pair.Value.bad > 0).OrderBy(pair => pair.Key, StringComparer.Ordinal))
                Debug.LogWarning("Nonfinite " + pair.Key + " in " + name + ": " + pair.Value.bad + " of " + pair.Value.total
                    + " values (" + pair.Value.first.ToString(CultureInfo.InvariantCulture) + ")");
            fields.Clear();
        }

        public void Apply(int index, Animator animator, float[] muscles)
        {
            var probe = Probes[index];
            var tilt = Quaternion.Euler(probe.Tilt.x, probe.Tilt.y, probe.Tilt.z);
            var applied = SampleMuscles(animator, muscles, probe.Offset, probe.Targeted ? tilt : (Quaternion?)null);
            Targets.Add(new BodyTargetRow {
                offset = Raw("body_targets.offset", probe.Offset), tilt = Raw("body_targets.tilt", probe.Tilt),
                position = Raw("body_targets.position", applied.bodyPosition),
                rotation = Raw("body_targets.rotation", applied.bodyRotation)
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
                    readback_position = Raw("readback_position", local.bodyPosition),
                    readback_rotation = Raw("readback_rotation", local.bodyRotation),
                    readback_muscles = Raw("readback_muscles", world.muscles ?? Array.Empty<float>()),
                    world_position = Raw("world_position", world.bodyPosition),
                    world_rotation = Raw("world_rotation", world.bodyRotation),
                    root_position = Array.Empty<string>(), root_rotation = Array.Empty<string>(),
                    motion_position = Array.Empty<string>(), motion_rotation = Array.Empty<string>(),
                    motion_delta_position = Array.Empty<string>(), motion_delta_rotation = Array.Empty<string>(),
                    nodes = nodes.Select(node => new BodyNodeRow {
                        position = Raw("nodes.position", node.position), rotation = Raw("nodes.rotation", node.rotation),
                        scale = Raw("nodes.scale", node.lossyScale),
                        hips_position = Raw("nodes.hips_position", hips.InverseTransformPoint(node.position)),
                        hips_rotation = Raw("nodes.hips_rotation", Quaternion.Inverse(hips.rotation) * node.rotation)
                    }).ToArray()
                };
                // Animator.bodyPosition and bodyRotation are not read: outside
                // OnAnimatorIK Unity warns on them, and GetHumanPose gives the body.
                if (playback)
                {
                    row.root_position = Raw("root_position", animator.rootPosition);
                    row.root_rotation = Raw("root_rotation", animator.rootRotation);
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
                row.motion_position = Raw("motion_position", node.localPosition);
                row.motion_rotation = Raw("motion_rotation", node.localRotation);
                row.motion_delta_position = Raw("motion_delta_position", animator.deltaPosition);
                row.motion_delta_rotation = Raw("motion_delta_rotation", animator.deltaRotation);
            }
            finally { graph.Destroy(); }
        }
    }

    // Completion's response and scope checks, run here too so a capture that
    // would fail them is never marked recorded (anim_fixture_round11.py
    // check_response, anim_fixture_scope.py check_scope):
    // - a muscle sweep's every single-muscle sample at one of MuscleValues turns
    //   that muscle's bone, locally, LeastTurn degrees or more from the recorded
    //   zero-vector sample;
    // - an in-scope clip's every sample keeps the shoulder-hip twist, about the
    //   up from the upper legs to the upper arms, within freeze v8's twist limit,
    //   and each spine Left-Right value Unity played (GetHumanPose's read-back
    //   of the sample) within its lateral limit. Completion checks the compiled
    //   curve's value at each sample time; the Editor cannot read the compiled
    //   track, so it checks what that track played.
    const double LeastTurn = 0.25, TwistLimit = 0.07417649320975901, LateralLimit = 0.193359375;
    static readonly string[] FrameBones = { "LeftUpperArm", "RightUpperArm", "LeftUpperLeg", "RightUpperLeg" };
    static readonly string[] LateralMuscles = { "Spine Left-Right", "Chest Left-Right", "UpperChest Left-Right" };

    static void CheckBodyResponse(HumanCapture capture, UnityrsFixtureBuilder.ReplayCase fixture, BodyHooks body)
    {
        string name = fixture.OutputName;
        float Value(string bits) => BitConverter.ToSingle(BitConverter.GetBytes(
            uint.Parse(bits, NumberStyles.HexNumber, CultureInfo.InvariantCulture)), 0);
        double[] Values(string[] bits) => bits.Select(b => (double)Value(b)).ToArray();
        Need(capture != null, "Body capture is not humanoid for " + name + ".");
        var nodes = capture.humanoid.bones.ToDictionary(bone => bone.name, bone => bone.node, StringComparer.Ordinal);
        // A round 12 table sweep or probe records it as a diagnostic (UnityrsRound12Recording.cs).
        if (fixture.Body.Variant == "muscle" || fixture.Body.Variant == "table")
        {
            var zero = capture.samples.FirstOrDefault(sample => sample.muscles_f32.All(bits => bits == "00000000" || bits == "80000000"));
            Need(zero != null, name + " records no zero-vector sample.");
            foreach (var sample in capture.samples)
            {
                var values = sample.muscles_f32.Select(Value).ToArray();
                var moved = Enumerable.Range(0, values.Length).Where(i => values[i] != 0).ToArray();
                if (moved.Length != 1 || !UnityrsFixtureBuilder.MuscleValues.Contains(Math.Abs(values[moved[0]]))) continue;
                var muscle = capture.humanoid.muscles[moved[0]];
                int node = capture.humanoid.bones[muscle.bone].node;
                if (node < 0)
                {
                    // A round 12 toe-absent probe sweeps a toe muscle on a rig without toes: a diagnostic there.
                    Check(false, "own_bone_mapped", Array.IndexOf(capture.samples, sample), double.NaN, 0,
                        name + " sweeps " + muscle.name + " but maps no bone for it.");
                    continue;
                }
                double turn = Angle(Values(zero.pose.transforms[node].rotation), Values(sample.pose.transforms[node].rotation));
                Check(turn * 180 / Math.PI >= LeastTurn, "own_bone_turn", Array.IndexOf(capture.samples, sample),
                    turn * 180 / Math.PI, LeastTurn, muscle.name + " at " + values[moved[0]].ToString("R", CultureInfo.InvariantCulture)
                    + " turned no bone in " + name + ".");
            }
        }
        if (fixture.Body.Kind == "clip" && fixture.Body.Spec.Scoped)
        {
            Need(FrameBones.All(bone => nodes.TryGetValue(bone, out int node) && node >= 0), name + " lacks a frame bone.");
            var lateral = LateralMuscles.Select(muscle => Array.FindIndex(capture.humanoid.muscles, row => row.name == muscle)).ToArray();
            Need(lateral.All(index => index >= 0), name + " lacks a spine Left-Right muscle.");
            for (int k = 0; k < body.Samples.Count; k++)
            {
                var played = Values(body.Samples[k].readback_muscles);
                foreach (int index in lateral)
                    Check(index < played.Length && Math.Abs(played[index]) <= LateralLimit, "lateral", k,
                        index < played.Length ? Math.Abs(played[index]) : double.NaN, LateralLimit, name + " is in scope but "
                        + capture.humanoid.muscles[index].name + " played " + (index < played.Length
                        ? played[index].ToString("R", CultureInfo.InvariantCulture) : "nothing") + " at sample " + k
                        + ", over the lateral limit " + LateralLimit.ToString("R", CultureInfo.InvariantCulture) + ".");
                var points = FrameBones.Select(bone => Values(body.Samples[k].nodes[nodes[bone]].position)).ToArray();
                double twist = ShoulderHipTwist(points);
                Check(twist <= TwistLimit, "shoulder_hip_twist", k, twist, TwistLimit, name + " is in scope but its shoulder-hip twist at sample " + k + " is "
                    + twist.ToString("R", CultureInfo.InvariantCulture) + " rad, over " + TwistLimit.ToString("R", CultureInfo.InvariantCulture) + ".");
            }
        }
        if (fixture.Diagnose) AddMotionTable(capture);
    }

    // anim_reference.py angle: radians between two rotations, either sign.
    static double Angle(double[] q, double[] r)
    {
        double Norm(double[] v) => Math.Sqrt(v.Sum(a => a * a));
        double nq = Norm(q), nr = Norm(r);
        q = q.Select(a => a / nq).ToArray();
        r = r.Select(a => a / nr).ToArray();
        if (q.Zip(r, (a, b) => a * b).Sum() < 0) r = r.Select(b => -b).ToArray();
        return 4 * Math.Atan2(Norm(q.Zip(r, (a, b) => a - b).ToArray()), Norm(q.Zip(r, (a, b) => a + b).ToArray()));
    }

    // anim_body_model.py shoulder_hip_twist on world positions (upper arms, then
    // upper legs); NaN, so over the limit, where it is undefined.
    static double ShoulderHipTwist(double[][] p)
    {
        double[] Sub(double[] a, double[] b) => a.Zip(b, (x, y) => x - y).ToArray();
        double Dot(double[] a, double[] b) => a.Zip(b, (x, y) => x * y).Sum();
        double Norm(double[] v) => Math.Sqrt(Dot(v, v));
        var up = Sub(p[0].Zip(p[1], (a, b) => a + b).ToArray(), p[2].Zip(p[3], (a, b) => a + b).ToArray());
        if (!(Norm(up) > 0)) return double.NaN;
        double length = Norm(up);
        up = up.Select(v => v / length).ToArray();
        double[] Flat(double[] v)
        {
            double along = Dot(v, up);
            var moved = v.Zip(up, (a, b) => a - along * b).ToArray();
            return Norm(moved) > 64 * Math.Pow(2, -53) * Norm(v) ? moved : null;
        }
        var top = Flat(Sub(p[1], p[0]));
        var bottom = Flat(Sub(p[3], p[2]));
        if (top == null || bottom == null) return double.NaN;
        var cross = new[] { bottom[1] * top[2] - bottom[2] * top[1], bottom[2] * top[0] - bottom[0] * top[2],
            bottom[0] * top[1] - bottom[1] * top[0] };
        return Math.Abs(Math.Atan2(Dot(cross, up), Dot(bottom, top)));
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

    static HumanBoneRow[] HumanBones(BodyHooks hooks) => Enumerable.Range(0, HumanTrait.BoneCount).Select(i => new HumanBoneRow {
        name = HumanTrait.BoneName[i], parent = HumanTrait.GetParentBone(i),
        mass = hooks.Raw("human_bones.mass", HumanTrait.GetBoneDefaultHierarchyMass(i))[0], required = HumanTrait.RequiredBone(i)
    }).ToArray();

    static BodyCaseRow BodyCaseOf(UnityrsFixtureBuilder.ReplayCase fixture, BodyHooks hooks)
    {
        var body = fixture.Body;
        var clip = fixture.BodyClip;
        return new BodyCaseRow {
            kind = body.Kind, rig = body.Rig, variant = body.Variant, root = body.Root,
            bake = clip?.Bake ?? new bool[3], keep = clip?.Keep ?? new bool[3],
            height_from_feet = clip?.HeightFromFeet ?? false, level = hooks.Raw("body_case.level", clip?.Level ?? 0)[0],
            orientation_offset = hooks.Raw("body_case.orientation_offset", clip?.Orientation ?? 0)[0], start_offset = clip?.Start ?? false,
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
        editor.body_case = new[] { BodyCaseOf(fixture, body) };
        editor.body_samples = body.Samples.ToArray();
        editor.body_targets = body.Targets.ToArray();
        editor.human_bones = HumanBones(body);
        body.ReportNonfinite(fixture.OutputName);
    }

}
#endif
