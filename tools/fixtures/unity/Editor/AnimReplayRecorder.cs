#if UNITY_EDITOR
using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Linq;
using System.Security.Cryptography;
using UnityEditor;
using UnityEditor.SceneManagement;
using UnityEngine;
using UnityEngine.Animations;
using UnityEngine.Playables;
using UnityEngine.SceneManagement;
using Object = UnityEngine.Object;

public static partial class AnimReplayRecorder
{
    // JsonUtility reads only the fields used to drive capture. Preserve the complete
    // validated specification verbatim, including the owner-inspected serialized evidence.
    [Serializable] class Id { public int bundle, file_index; public string file, path_id; }
    [Serializable] class BundleName { public string name; }
    [Serializable] class Bundle : BundleName { public string sha256; }
    [Serializable] class Node { public string address, name; }
    [Serializable] class Channel { public int index; public string name; public float[] full_weights; }
    [Serializable] class RendererAddress { public string address; public int component; public Channel[] channels; }
    [Serializable] class RendererSpec : RendererAddress { public Id mesh; public int mesh_group; }
    [Serializable] class Timepoint { public double local; }
    [Serializable] class Point : Timepoint { public float[] muscles; }
    [Serializable] class EndpointEvidence { public double source; public string field; }
    [Serializable] class ClockEvidence { public EndpointEvidence start, stop; }
    [Serializable] class Evidence { public ClockEvidence clock; }
    enum SourceClock { Widened, F32Add }
    [Serializable] class Spec
    {
        public Bundle[] bundles;
        public Id prefab, clip, avatar;
        public string prefab_asset, clip_asset, avatar_asset, clip_name, binding_address, build_target, mode, source_clock;
        public int binding_component;
        public double start, stop, sample_rate;
        public double[] held_boundaries;
        public Point[] schedule;
        public Node[] nodes;
        public RendererSpec[] renderers;
        public string[] editor_axis_properties;
        public Evidence evidence;
    }

    [Serializable] class Trs { public string[] translation, rotation, scale; }
    [Serializable] class Weights { public string[] channels; }
    [Serializable] class Pose { public Trs[] transforms; public Weights[] weights; }
    [Serializable] class Sample
    {
        public double local, source, effective_source;
        public string local_f32;
        public string[] sides, muscles_f32;
        public Pose pose;
    }
    [Serializable] class Playback
    {
        public string api, wrap_mode, clip_length_f32, frame_rate_f32, legacy_state;
        public string reset = "fresh_prefab";
        public int speed = 1, delta_time = 0;
        public bool clip_looping, clip_human_motion, apply_root_motion = false, foot_ik = false, playable_ik = false,
            animator_move = false, controller_blending = false, events = false;
    }
    [Serializable] class ClipSettings
    {
        public string startTime, stopTime, orientationOffsetY, level, cycleOffset, additiveReferencePoseTime,
            additiveReferencePoseClipName;
        public bool loopTime, loopBlend, loopBlendOrientation, loopBlendPositionY, loopBlendPositionXZ,
            keepOriginalOrientation, keepOriginalPositionY, keepOriginalPositionXZ, heightFromFeet,
            mirror, hasAdditiveReferencePose;
    }
    [Serializable] class Bone { public string name; public int node; public bool required; }
    [Serializable] class Muscle { public string name, min, max; public int bone, axis; }
    [Serializable] class Limit { public string bone, human, axis_length; public string[] min, max, center; public bool defaults; }
    [Serializable] class AxisProperty { public string path, kind; public string[] value; }
    [Serializable] class Humanoid
    {
        public Bone[] bones;
        public Muscle[] muscles;
        public Limit[] limits;
        public AxisProperty[] axis_properties;
        public string[] rest_body_position, rest_body_rotation, rest_muscles;
        public string human_scale, upper_arm_twist, lower_arm_twist, upper_leg_twist, lower_leg_twist,
            arm_stretch, leg_stretch, feet_spacing;
        public bool translation_dof;
    }
    [Serializable] class Capture
    {
        public string editor_version, build_target;
        public string[] bundle_sha256;
        public Playback playback;
        public ClipSettings clip_settings;
        public Pose rest;
        public Sample[] samples;
    }
    // Separate DTO so JsonUtility never substitutes an empty object for an absent humanoid.
    [Serializable] class HumanCapture : Capture { public Humanoid humanoid; }

    [MenuItem("Tools/unityrs/Record animation replay")]
    public static void Record()
    {
        try
        {
            CheckEditMode();
            string input = EditorUtility.OpenFilePanel("Select validated recording specification", "", "json");
            if (input.Length == 0) return;
            string output = EditorUtility.SaveFilePanel("Save animation replay", Path.GetDirectoryName(input), "replay", "json");
            if (output.Length == 0) return;
            Need(!File.Exists(output), "Choose a new replay filename.");
            string json = File.ReadAllText(input);
            var spec = JsonUtility.FromJson<Spec>(json);
            var capture = RecordCapture(spec, Path.GetDirectoryName(input), pending: false);
            WriteReplay(output, "unityrs.anim_replay", json, capture);
            Debug.Log("Animation replay captured. Validate it before freezing tolerances.");
        }
        catch (Exception error)
        {
            Debug.LogException(error);
            EditorUtility.DisplayDialog("Animation replay failed", error.Message, "OK");
        }
    }

    static void CheckEditMode()
    {
        Need(!EditorApplication.isPlayingOrWillChangePlaymode && !AnimationMode.InAnimationMode(),
            "Exit Play Mode and animation preview before recording.");
    }

    static void WriteReplay(string output, string format, string spec, Capture capture)
    {
        string staging = Path.Combine(Path.GetDirectoryName(output), "." + Guid.NewGuid().ToString("N").Substring(0, 8) + ".tmp");
        var stream = new FileStream(staging, FileMode.CreateNew);
        try
        {
            using (stream)
            using (var writer = new StreamWriter(stream))
                writer.Write("{\"format\":\"" + format + "\",\"version\":1,\"spec\":" + spec
                    + ",\"capture\":" + JsonUtility.ToJson(capture, true) + "}\n");
            File.Move(staging, output);
        }
        finally
        {
            if (File.Exists(staging)) File.Delete(staging);
        }
    }

    static Capture RecordCapture(Spec spec, string bundleDir, bool pending,
        Action<AnimationClip, AssetBundle> inspectClip = null, Action<Transform, float> inspectPose = null,
        BodyHooks body = null)
    {
        Need(spec != null && spec.bundles != null && spec.bundles.Length > 0, "Expected a replay specification with bundles.");
        Need(pending || (spec.prefab != null && spec.clip != null
            && (string.IsNullOrEmpty(spec.avatar_asset) || spec.avatar != null)), "Missing built asset identity.");
        var clock = ReadSourceClock(spec);
        var bundles = new List<AssetBundle>();
        var digests = new List<string>();
        try
        {
            Need(spec.build_target == EditorUserBuildSettings.activeBuildTarget.ToString(), "Build target mismatch.");
            Need(!pending || spec.bundles.Length == 1, "Pending fixtures must use one self-contained bundle.");
            foreach (var row in spec.bundles)
            {
                Need(!string.IsNullOrEmpty(row.name) && row.name != "." && row.name != ".."
                    && row.name.IndexOfAny(new[] { '/', '\\', ':' }) < 0, "Expected a bundle basename.");
                // Hash the very bytes handed to Unity, including dependency bundles.
                byte[] bytes = File.ReadAllBytes(Path.Combine(bundleDir, row.name));
                using (var sha = SHA256.Create())
                    digests.Add(BitConverter.ToString(sha.ComputeHash(bytes)).Replace("-", "").ToLowerInvariant());
                Need(pending || digests[digests.Count - 1] == row.sha256, "Bundle digest mismatch: " + row.name);
                var bundle = AssetBundle.LoadFromMemory(bytes);
                Need(bundle != null, "Cannot load bundle: " + row.name);
                bundles.Add(bundle);
            }

            if (pending)
            {
                var keys = bundles[0].GetAllAssetNames();
                spec.prefab_asset = BuiltAssetKey(keys, spec.prefab_asset);
                spec.clip_asset = BuiltAssetKey(keys, spec.clip_asset);
                if (!string.IsNullOrEmpty(spec.avatar_asset))
                    spec.avatar_asset = BuiltAssetKey(keys, spec.avatar_asset);
            }
            var prefab = LoadPrefab(FindAssetBundle(bundles, pending ? 0 : spec.prefab.bundle, spec.prefab_asset),
                spec.prefab_asset);
            var clip = Load<AnimationClip>(bundles, pending ? 0 : spec.clip.bundle, spec.clip_asset);
            Need(clip.name == spec.clip_name, "Built clip name disagrees with the inspected clip.");
            if (inspectClip != null) inspectClip(clip, bundles[0]);
            foreach (var node in spec.nodes)
                Need(At(prefab.transform, node.address).name == node.name, "Built node name disagrees with the inspected node.");
            var avatar = string.IsNullOrEmpty(spec.avatar_asset) ? null
                : Load<Avatar>(bundles, pending ? 0 : spec.avatar.bundle, spec.avatar_asset);
            bool legacy = spec.mode == "legacy", knownPose = spec.mode == "human_pose";
            Need(legacy || knownPose || spec.mode == "animator", "Unknown playback mode.");
            Need(legacy == clip.legacy, "Clip legacy flag disagrees with playback mode.");
            Need(spec.mode != "animator" || !clip.isLooping, "Build a nonlooping Animator fixture for endpoint replay.");
            Need(clip.length == ClipLength(spec.start, spec.stop, legacy) && clip.frameRate == spec.sample_rate,
                "Built clip length " + clip.length.ToString("R", CultureInfo.InvariantCulture)
                + " or rate disagrees with the inspected source clock.");
            Need(prefab.GetComponentsInChildren<MonoBehaviour>(true).Length == 0,
                "Use a fixture prefab without scripts, including root-motion and IK callbacks.");
            bool humanClip = avatar != null && avatar.isHuman && avatar.isValid;
            Need(!knownPose || humanClip, "Known muscle vectors require a human Avatar.");
            Need(!clip.humanMotion || humanClip, "Human motion requires the bound human Avatar.");

            var capture = humanClip ? new HumanCapture() : new Capture();
            capture.editor_version = Application.unityVersion;
            capture.build_target = EditorUserBuildSettings.activeBuildTarget.ToString();
            capture.bundle_sha256 = digests.ToArray();
            capture.playback = new Playback {
                api = legacy ? "Animation.Sample" : knownPose ? "HumanPoseHandler.SetHumanPose" : "AnimationClipPlayable",
                wrap_mode = (legacy ? WrapMode.ClampForever : clip.wrapMode).ToString(),
                legacy_state = legacy ? "enabled_weight_one_clamp_forever" : "unused",
                clip_looping = clip.isLooping, clip_human_motion = clip.humanMotion,
                clip_length_f32 = Bits(clip.length), frame_rate_f32 = Bits(clip.frameRate)
            };
            capture.clip_settings = ReadSettings(clip);
            capture.samples = new Sample[spec.schedule.Length];

            Scene scene = EditorSceneManager.NewPreviewScene();
            try
            {
                var anchor = EditorUtility.CreateGameObjectWithHideFlags("Recorder", HideFlags.HideAndDontSave);
                SceneManager.MoveGameObjectToScene(anchor, scene);
                for (int i = 0; i < spec.schedule.Length; ++i)
                {
                    var rig = Object.Instantiate(prefab, anchor.transform, false);
                    try
                    {
                        var root = rig.transform;
                        var nodes = spec.nodes.Select(n => At(root, n.address)).ToArray();
                        var renderers = spec.renderers.Select(r => At<SkinnedMeshRenderer>(root, r.address, r.component)).ToArray();
                        Need(nodes.Distinct().Count() == rig.GetComponentsInChildren<Transform>(true).Length
                            && nodes.Distinct().Count() == nodes.Length, "Node table differs from built hierarchy.");
                        Need(renderers.Distinct().Count() == rig.GetComponentsInChildren<SkinnedMeshRenderer>(true).Length
                            && renderers.Distinct().Count() == renderers.Length, "Renderer table differs from built hierarchy.");

                        var binding = legacy ? (Behaviour)At<Animation>(root, spec.binding_address, spec.binding_component)
                            : At<Animator>(root, spec.binding_address, spec.binding_component);
                        Need(binding.gameObject.activeInHierarchy, "Playback component must be on an active node.");
                        foreach (var animation in rig.GetComponentsInChildren<Animation>(true))
                        {
                            animation.playAutomatically = false;
                            animation.Stop();
                            animation.enabled = animation == binding;
                        }
                        DisableAnimators(rig, binding);

                        var target = binding as Animator;
                        if (target != null)
                            Need(target.avatar == avatar && target.hasTransformHierarchy, "Avatar or materialized hierarchy mismatch.");
                        var rest = ReadPose(nodes, renderers, spec.renderers, pending);
                        if (i == 0)
                        {
                            capture.rest = rest;
                            if (capture is HumanCapture h)
                                h.humanoid = ReadHumanoid(target, nodes, spec.editor_axis_properties);
                        }
                        else
                            Need(JsonUtility.ToJson(rest) == JsonUtility.ToJson(capture.rest), "Fresh instances have different rest poses.");

                        var point = spec.schedule[i];
                        float time = (float)point.local;
                        var sample = ReadClock(spec, clock, point.local, time);
                        sample.muscles_f32 = Array.Empty<string>();
                        if (legacy)
                        {
                            SampleLegacy((Animation)binding, clip, time);
                            sample.pose = ReadPose(nodes, renderers, spec.renderers, pending);
                        }
                        else if (knownPose)
                        {
                            if (body != null) body.Apply(i, target, point.muscles);
                            else SampleMuscles(target, point.muscles);
                            sample.muscles_f32 = point.muscles.Select(Bits).ToArray();
                            sample.pose = ReadPose(nodes, renderers, spec.renderers, pending);
                            body?.Read(target, nodes, playback: false);
                        }
                        else
                        {
                            sample.pose = SampleAnimator(target, clip, time, () => {
                                if (inspectPose != null) inspectPose(root, time);
                                // Read back while the graph still owns the pose.
                                body?.Read(target, nodes, playback: true);
                                return ReadPose(nodes, renderers, spec.renderers, pending);
                            });
                        }
                        capture.samples[i] = sample;
                    }
                    finally { Object.DestroyImmediate(rig); }
                    if (body != null && body.RootMotion)
                    {
                        // The second, root-motion playback, from a fresh instance.
                        var moving = Object.Instantiate(prefab, anchor.transform, false);
                        try { body.PlayRootMotion(PrepareAnimators(moving, spec), clip, (float)spec.schedule[i].local); }
                        finally { Object.DestroyImmediate(moving); }
                    }
                }
            }
            finally { EditorSceneManager.ClosePreviewScene(scene); }

            return capture;
        }
        finally
        {
            for (int i = bundles.Count - 1; i >= 0; --i) bundles[i].Unload(true);
        }
    }

    static void DisableAnimators(GameObject rig, Behaviour binding)
    {
        foreach (var animator in rig.GetComponentsInChildren<Animator>(true))
        {
            animator.enabled = animator == binding;
            animator.applyRootMotion = false;
            animator.fireEvents = false;
            animator.runtimeAnimatorController = null;
            animator.cullingMode = AnimatorCullingMode.AlwaysAnimate;
            animator.speed = 1;
        }
    }

    // A fresh instance's bound Animator, set up as the main playback's.
    static Animator PrepareAnimators(GameObject rig, Spec spec)
    {
        var binding = At<Animator>(rig.transform, spec.binding_address, spec.binding_component);
        foreach (var animation in rig.GetComponentsInChildren<Animation>(true))
        {
            animation.playAutomatically = false;
            animation.Stop();
            animation.enabled = false;
        }
        DisableAnimators(rig, binding);
        return binding;
    }

    static string BuiltAssetKey(string[] keys, string assetPath)
    {
        // Find the generated path regardless of how Unity cases its key,
        // then preserve the exact built key for serialized-ID completion.
        var matches = keys.Where(key => string.Equals(key, assetPath, StringComparison.OrdinalIgnoreCase)).ToArray();
        Need(matches.Length == 1, "Expected one built asset key for " + assetPath + ".");
        return matches[0];
    }

    static AssetBundle FindAssetBundle(List<AssetBundle> bundles, int index, string key)
    {
        Need(index >= 0 && index < bundles.Count, "Invalid bundle index.");
        var bundle = bundles[index];
        Need(bundle.GetAllAssetNames().Contains(key, StringComparer.Ordinal), "Unknown exact asset key: " + key);
        return bundle;
    }

    static GameObject LoadPrefab(AssetBundle bundle, string key)
    {
        // Confirm this root lookup in the Editor with the multi-node fixture smoke test.
        var roots = bundle.LoadAssetWithSubAssets<GameObject>(key).Where(obj => obj.transform.parent == null).ToArray();
        Need(roots.Length == 1, "Use an asset key with exactly one prefab root: " + key);
        return roots[0];
    }

    static T Load<T>(List<AssetBundle> bundles, int index, string key) where T : Object
    {
        var matches = FindAssetBundle(bundles, index, key).LoadAssetWithSubAssets<T>(key);
        Need(matches.Length == 1, "Use an unambiguous typed asset key: " + key);
        return matches[0];
    }

    static Transform At(Transform root, string address)
    {
        if (address.Length == 0) return root;
        foreach (var part in address.Split('/'))
            root = root.GetChild(int.Parse(part, CultureInfo.InvariantCulture));
        return root;
    }

    static T At<T>(Transform root, string address, int component) where T : Component
    {
        return At(root, address).GetComponents<T>()[component];
    }

    static SourceClock ReadSourceClock(Spec spec)
    {
        var evidence = spec.evidence?.clock;
        Need(evidence?.start != null && evidence.stop != null
            && !string.IsNullOrEmpty(evidence.start.field) && !string.IsNullOrEmpty(evidence.stop.field)
            && evidence.start.source == spec.start && evidence.stop.source == spec.stop,
            "Source origin disagrees with independently inspected endpoints.");
        switch (spec.source_clock)
        {
            case "widened": return SourceClock.Widened;
            case "f32_add":
                Need((float)spec.start == spec.start, "The f32 source clock needs an exact f32 start.");
                return SourceClock.F32Add;
            default: throw new InvalidOperationException("Unknown source clock.");
        }
    }

    // Unity 2022.3 reports the length of a Mecanim clip whose interval starts
    // late as its stop time, measured from zero. Playback still starts at the
    // interval start: an offset fixture's held probes put the step at
    // playable time key - start, not at the key's source time.
    static float ClipLength(double start, double stop, bool legacy) =>
        !legacy && start > 0 ? (float)stop : (float)(stop - start);

    static Sample ReadClock(Spec spec, SourceClock clock, double local, float time)
    {
        Need(0 <= local && local <= spec.stop - spec.start, "Sample outside source interval.");
        double source = local == spec.stop - spec.start ? spec.stop : spec.start + local;
        double effective = spec.start + time;
        if (clock == SourceClock.F32Add) effective = (float)effective;
        // The playable endpoint is the interval length, not clip.length.
        effective = time == (float)(spec.stop - spec.start) ? spec.stop
            : Math.Max(spec.start, Math.Min(spec.stop, effective));
        return new Sample {
            local = local, source = source, local_f32 = Bits(time), effective_source = effective,
            sides = spec.held_boundaries.Select(t => effective < t ? "left" : "right").ToArray()
        };
    }

    static Pose SampleAnimator(Animator animator, AnimationClip clip, float time, Func<Pose> readPose)
    {
        var graph = PlayableGraph.Create("unityrs single clip");
        try
        {
            graph.SetTimeUpdateMode(DirectorUpdateMode.Manual);
            var playable = AnimationClipPlayable.Create(graph, clip);
            playable.SetApplyFootIK(false);
            playable.SetApplyPlayableIK(false);
            playable.SetSpeed(1);
            var output = AnimationPlayableOutput.Create(graph, "Fixture", animator);
            output.SetSourcePlayable(playable);
            graph.Play();
            playable.SetTime(time);
            // Graph destruction may restore animated properties. Capture while the graph owns the pose.
            graph.Evaluate(0);
            Need(playable.GetTime() == time, "Playable did not retain the requested time.");
            return readPose();
        }
        finally { graph.Destroy(); }
    }

    static void SampleLegacy(Animation animation, AnimationClip clip, float time)
    {
        const string key = "unityrs-direct-clip";
        Need(animation.GetClip(key) == null, "Reserved legacy state name is already in use.");
        animation.AddClip(clip, key);
        foreach (AnimationState other in animation)
        {
            other.enabled = false;
            other.weight = 0;
        }
        var state = animation[key];
        state.wrapMode = WrapMode.ClampForever;
        state.blendMode = AnimationBlendMode.Blend;
        state.layer = 0;
        state.speed = 1;
        state.weight = 1;
        state.time = time;
        state.enabled = true;
        Need(state.time == time, "Legacy state did not retain the requested time.");
        animation.Sample();
    }

    // A tilt moves the rest body by the offset and turns it by the tilt; the
    // applied pose is returned.
    static HumanPose SampleMuscles(Animator animator, float[] values, Vector3 offset = default, Quaternion? tilt = null)
    {
        Need(values != null && values.Length == HumanTrait.MuscleCount
            && values.All(v => -1 <= v && v <= 1), "Invalid known muscle vector.");
        using (var handler = new HumanPoseHandler(animator.avatar, animator.transform))
        {
            var pose = ReadRestBody(animator, handler);
            pose.muscles = (float[])values.Clone();
            if (tilt.HasValue)
            {
                pose.bodyPosition += offset;
                pose.bodyRotation = tilt.Value * pose.bodyRotation;
            }
            handler.SetHumanPose(ref pose);
            return pose;
        }
    }

    static HumanPose ReadRestBody(Animator animator, HumanPoseHandler handler)
    {
        var pose = new HumanPose();
        handler.GetHumanPose(ref pose);
        return InAnimatorSpace(animator, pose);
    }

    static Pose ReadPose(Transform[] nodes, SkinnedMeshRenderer[] renderers, RendererSpec[] specs, bool pending)
    {
        var meshIds = new Dictionary<Mesh, string>();
        var meshes = new Dictionary<string, Mesh>();
        var weights = new Weights[renderers.Length];
        for (int i = 0; i < renderers.Length; ++i)
        {
            var renderer = renderers[i];
            var mesh = renderer.sharedMesh;
            var spec = specs[i];
            Need(mesh != null && mesh.blendShapeCount == spec.channels.Length, "Built mesh channel count mismatch.");
            string id = pending ? spec.mesh_group.ToString(CultureInfo.InvariantCulture)
                : spec.mesh.bundle + ":" + spec.mesh.file_index + ":" + spec.mesh.path_id;
            Need(!meshIds.TryGetValue(mesh, out var prior) || prior == id, "Shared mesh identity mismatch.");
            Need(!meshes.TryGetValue(id, out var previous) || previous == mesh, "Mesh ID aliases two objects.");
            meshIds[mesh] = id;
            meshes[id] = mesh;
            var values = new string[spec.channels.Length];
            for (int j = 0; j < values.Length; ++j)
            {
                var channel = spec.channels[j];
                Need(channel.index == j && channel.name == mesh.GetBlendShapeName(j)
                    && channel.full_weights.Length == mesh.GetBlendShapeFrameCount(j), "Blend-shape layout mismatch.");
                for (int k = 0; k < channel.full_weights.Length; ++k)
                    Need(channel.full_weights[k] == mesh.GetBlendShapeFrameWeight(j, k), "Blend-shape full weight mismatch.");
                values[j] = Bits(renderer.GetBlendShapeWeight(j));
            }
            weights[i] = new Weights { channels = values };
        }
        return new Pose { transforms = nodes.Select(n => new Trs {
            translation = Bits(n.localPosition), rotation = Bits(n.localRotation), scale = Bits(n.localScale)
        }).ToArray(), weights = weights };
    }

    static ClipSettings ReadSettings(AnimationClip clip)
    {
        var s = AnimationUtility.GetAnimationClipSettings(clip);
        return new ClipSettings {
            startTime = Bits(s.startTime), stopTime = Bits(s.stopTime), orientationOffsetY = Bits(s.orientationOffsetY),
            level = Bits(s.level), cycleOffset = Bits(s.cycleOffset), additiveReferencePoseTime = Bits(s.additiveReferencePoseTime),
            additiveReferencePoseClipName = s.additiveReferencePoseClip == null ? "" : s.additiveReferencePoseClip.name,
            loopTime = s.loopTime, loopBlend = s.loopBlend, loopBlendOrientation = s.loopBlendOrientation,
            loopBlendPositionY = s.loopBlendPositionY, loopBlendPositionXZ = s.loopBlendPositionXZ,
            keepOriginalOrientation = s.keepOriginalOrientation, keepOriginalPositionY = s.keepOriginalPositionY,
            keepOriginalPositionXZ = s.keepOriginalPositionXZ, heightFromFeet = s.heightFromFeet,
            mirror = s.mirror, hasAdditiveReferencePose = s.hasAdditiveReferencePose
        };
    }

    static Humanoid ReadHumanoid(Animator animator, Transform[] nodes, string[] axisProperties)
    {
        var result = new Humanoid();
        var boneNames = HumanTrait.BoneName;
        result.bones = Enumerable.Range(0, (int)HumanBodyBones.LastBone).Select(i => {
            var bone = animator.GetBoneTransform((HumanBodyBones)i);
            int node = bone == null ? -1 : Array.IndexOf(nodes, bone);
            Need(!HumanTrait.RequiredBone(i) || node >= 0, "Missing required human bone.");
            return new Bone { name = boneNames[i], node = node, required = HumanTrait.RequiredBone(i) };
        }).ToArray();
        var muscleNames = HumanTrait.MuscleName;
        result.muscles = Enumerable.Range(0, HumanTrait.MuscleCount).Select(i => {
            int bone = HumanTrait.BoneFromMuscle(i);
            int axis = Enumerable.Range(0, 3).Single(a => HumanTrait.MuscleFromBone(bone, a) == i);
            return new Muscle { name = muscleNames[i], bone = bone, axis = axis,
                min = Bits(HumanTrait.GetMuscleDefaultMin(i)), max = Bits(HumanTrait.GetMuscleDefaultMax(i)) };
        }).ToArray();
        var description = animator.avatar.humanDescription;
        result.limits = (description.human ?? Array.Empty<HumanBone>()).Select(b => new Limit {
            bone = b.boneName, human = b.humanName, axis_length = Bits(b.limit.axisLength),
            min = Bits(b.limit.min), max = Bits(b.limit.max), center = Bits(b.limit.center), defaults = b.limit.useDefaultValues
        }).ToArray();
        result.human_scale = Bits(animator.humanScale);
        result.upper_arm_twist = Bits(description.upperArmTwist);
        result.lower_arm_twist = Bits(description.lowerArmTwist);
        result.upper_leg_twist = Bits(description.upperLegTwist);
        result.lower_leg_twist = Bits(description.lowerLegTwist);
        result.arm_stretch = Bits(description.armStretch);
        result.leg_stretch = Bits(description.legStretch);
        result.feet_spacing = Bits(description.feetSpacing);
        result.translation_dof = description.hasTranslationDoF;
        result.axis_properties = ReadAxisProperties(animator.avatar, axisProperties ?? Array.Empty<string>());
        using (var handler = new HumanPoseHandler(animator.avatar, animator.transform))
        {
            var pose = ReadRestBody(animator, handler);
            result.rest_body_position = Bits(pose.bodyPosition);
            result.rest_body_rotation = Bits(pose.bodyRotation);
            result.rest_muscles = pose.muscles.Select(Bits).ToArray();
        }
        return result;
    }

    static AxisProperty[] ReadAxisProperties(Avatar avatar, string[] paths)
    {
        using (var serialized = new SerializedObject(avatar))
            return paths.Select(path => {
                var p = serialized.FindProperty(path);
                Need(p != null, "No Editor property for Avatar field: " + path);
                var result = new AxisProperty { path = path, kind = p.propertyType.ToString() };
                switch (p.propertyType)
                {
                    case SerializedPropertyType.Float: result.value = new[] { Bits(p.floatValue) }; break;
                    case SerializedPropertyType.Vector3: result.value = Bits(p.vector3Value); break;
                    case SerializedPropertyType.Quaternion: result.value = Bits(p.quaternionValue); break;
                    case SerializedPropertyType.Integer:
                        result.value = new[] { p.longValue.ToString(CultureInfo.InvariantCulture) };
                        break;
                    default: throw new InvalidOperationException("Unsupported Editor axis property type: " + path);
                }
                return result;
            }).ToArray();
    }

    static string[] Bits(Vector3 v) => new[] { Bits(v.x), Bits(v.y), Bits(v.z) };
    static string[] Bits(Quaternion q) => new[] { Bits(q.x), Bits(q.y), Bits(q.z), Bits(q.w) };
    static string Bits(float f)
    {
        Need(!float.IsNaN(f) && !float.IsInfinity(f), "Nonfinite f32 in fixture capture.");
        return BitConverter.ToUInt32(BitConverter.GetBytes(f), 0).ToString("x8", CultureInfo.InvariantCulture);
    }
    static void Need(bool condition, string message)
    {
        if (!condition) throw new InvalidOperationException(message);
    }
}

#endif
