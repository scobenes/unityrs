#if UNITY_EDITOR
using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Linq;
using UnityEditor;
using UnityEditor.SceneManagement;
using UnityEngine;
using UnityEngine.SceneManagement;

public static partial class AnimReplayRecorder
{
    // The pending DTO carries only Editor-visible identity. JsonUtility
    // serializes nested objects inline, so the handoff uses explicit types
    // without the built-ID fields instead of serializing empty IDs.
    [Serializable] sealed class PendingRenderer : RendererAddress { public int mesh_group; }
    [Serializable] sealed class Probe
    {
        public int sample, target, channel, component;
        public string property, value;
    }
    [Serializable] sealed class Endpoint { public double source; public string field; public Probe probe; }
    [Serializable] sealed class Held { public double source; public Probe left, right; }
    [Serializable] sealed class Clock
    {
        public Endpoint start, stop;
        public double[] key_times;
        public Held[] held;
    }
    [Serializable] sealed class CurveKey
    {
        public string time_f32, value_f32, in_tangent_f32, out_tangent_f32, in_weight_f32, out_weight_f32;
        public string weighted_mode;
    }
    [Serializable] sealed class ObjectKey { public string time_f32, asset, name; }
    [Serializable] sealed class CurveBinding
    {
        public string path, property, type;
        public bool discrete, object_reference;
        public CurveKey[] keys;
        public ObjectKey[] object_keys;
    }
    [Serializable] sealed class EditorEvidence
    {
        public string fixture_case, run_id;
        public string animation_source;
        public string expected_discovery;
        public string length_f32, frame_rate_f32;
        public ClipSettings clip_settings;
        public CurveBinding[] bindings;
        public string[] model_import_settings;
        public ObjectKey[] object_reference_samples;
        public string[] spine_start_f32, spine_stop_f32;
        public float spine_endpoint_tolerance_degrees;
        public string expected_refusal;
        public DiscreteSample[] discrete_samples, discrete_keys;
        public TwistEffect[] twist_effects;
    }
    [Serializable] sealed class PendingEvidence
    {
        public string layout = "editor_authored";
        public Clock clock;
        public EditorEvidence editor;
    }
    [Serializable] class PendingSpec
    {
        public BundleName[] bundles;
        public string prefab_asset, clip_asset, avatar_asset, clip_name, binding_address, build_target, mode;
        public string source_clock = "widened";
        public int binding_component = 0;
        public double start, stop, sample_rate;
        public double[] held_boundaries;
        public Node[] nodes;
        public PendingRenderer[] renderers;
        public PendingEvidence evidence;
    }
    [Serializable] sealed class PendingClipSpec : PendingSpec { public Timepoint[] schedule; }
    [Serializable] sealed class PendingPoseSpec : PendingSpec { public Point[] schedule; }

    internal static bool RecordFixture(UnityrsFixtureBuilder.ReplayCase fixture, string bundles, string output,
        string runId, Action<string> log)
    {
        var prefab = AssetDatabase.LoadAssetAtPath<GameObject>(fixture.Prefab);
        var clip = AssetDatabase.LoadAssetAtPath<AnimationClip>(fixture.Clip);
        Need(prefab != null && clip != null, "Could not read generated rig or animation clip.");
        var settings = AnimationUtility.GetAnimationClipSettings(clip);
        double start = fixture.Start ?? (clip.legacy ? 0 : settings.startTime);
        double stop = fixture.Stop ?? (clip.legacy ? clip.length : settings.stopTime);
        Need(0 <= start && start <= stop, "Editor clip interval is not ordered for " + clip.name);
        var invariant = CultureInfo.InvariantCulture;
        Need(clip.length == ClipLength(start, stop, clip.legacy), "Clip length for " + clip.name + " is "
            + clip.length.ToString("R", invariant) + " for the interval " + start.ToString("R", invariant)
            + " to " + stop.ToString("R", invariant) + ".");
        if (fixture.Start.HasValue && !clip.legacy)
            Need(start == settings.startTime && stop == settings.stopTime, "Unity did not retain the authored clip interval.");

        var bindings = ReadBindings(clip, out double[] authoredTimes);
        var keys = authoredTimes.Where(time => start <= time && time <= stop).Concat(new[] { start, stop })
            .Distinct().OrderBy(time => time).ToArray();
        var points = Schedule(keys, fixture.HeldTime);
        string probeAddress = Address(prefab.transform, prefab.transform.Find(fixture.ProbePath));
        var nodes = prefab.GetComponentsInChildren<Transform>(true)
            .Select(node => new Node { name = node.name, address = Address(prefab.transform, node) }).ToArray();
        var meshGroups = new Dictionary<Mesh, int>();
        var renderers = prefab.GetComponentsInChildren<SkinnedMeshRenderer>(true).Select(renderer => {
            Mesh mesh = renderer.sharedMesh;
            Need(mesh != null, "Skinned renderer does not have a mesh: " + renderer.name);
            if (!meshGroups.TryGetValue(mesh, out int group))
            {
                group = meshGroups.Count;
                meshGroups.Add(mesh, group);
            }
            return new PendingRenderer {
                address = Address(prefab.transform, renderer.transform),
                component = Array.IndexOf(renderer.GetComponents<SkinnedMeshRenderer>(), renderer),
                mesh_group = group,
                channels = Enumerable.Range(0, mesh.blendShapeCount).Select(index => new Channel {
                    index = index, name = mesh.GetBlendShapeName(index),
                    full_weights = Enumerable.Range(0, mesh.GetBlendShapeFrameCount(index))
                        .Select(frame => mesh.GetBlendShapeFrameWeight(index, frame)).ToArray()
                }).ToArray()
            };
        }).ToArray();

        int target = fixture.ProbeProperty == "weight"
            ? Array.FindIndex(renderers, row => row.address == probeAddress)
            : Array.FindIndex(nodes, row => row.address == probeAddress);
        Need(target >= 0, "Missing clock probe target for " + clip.name);
        var clock = new Clock {
            start = new Endpoint { source = start, field = "editor_interval", probe = ProbeAt(fixture, target, 0, fixture.First) },
            stop = new Endpoint { source = stop, field = "editor_interval", probe = ProbeAt(fixture, target, points.Length - 1, fixture.Last) },
            key_times = keys,
            held = Array.Empty<Held>()
        };
        bool human = fixture.IsHumanoid;
        bool knownPose = fixture.KnownMuscles != null;
        if (human)
            Need(keys.Contains(.5), "Humanoid clip is missing the single-axis key for " + clip.name + ".");
        PendingSpec pending;
        if (knownPose)
        {
            Need(human && !clip.legacy && !fixture.HeldTime.HasValue,
                "Known-vector baselines require an unheld Humanoid clip schedule.");
            pending = new PendingPoseSpec { schedule = points.Select(point => new Point {
                local = point.local, muscles = fixture.KnownMuscles((float)point.local)
            }).ToArray() };
        }
        else
        {
            pending = new PendingClipSpec { schedule = points };
        }
        pending.bundles = new[] { new BundleName { name = fixture.Bundle } };
        pending.prefab_asset = fixture.Prefab;
        pending.clip_asset = fixture.Clip;
        pending.avatar_asset = fixture.Avatar;
        pending.clip_name = clip.name;
        pending.build_target = EditorUserBuildSettings.activeBuildTarget.ToString();
        pending.binding_address = fixture.BindingAddress;
        pending.mode = knownPose ? "human_pose" : clip.legacy ? "legacy" : "animator";
        pending.start = start;
        pending.stop = stop;
        pending.sample_rate = clip.frameRate;
        pending.held_boundaries = fixture.HeldTime.HasValue ? new[] { (double)fixture.HeldTime.Value } : Array.Empty<double>();
        pending.nodes = nodes;
        pending.renderers = renderers;
        var editor = new EditorEvidence {
            fixture_case = fixture.OutputName,
            run_id = runId,
            animation_source = human ? "procedural_humanoid" : "procedural_clip",
            expected_discovery = fixture.ExpectedDiscovery,
            bindings = bindings, clip_settings = ReadSettings(clip), length_f32 = Bits(clip.length),
            frame_rate_f32 = Bits(clip.frameRate), model_import_settings = Array.Empty<string>(),
            expected_refusal = fixture.ExpectedRefusal
        };
        pending.evidence = new PendingEvidence { clock = clock, editor = editor };

        var referenceAssets = fixture.ObjectReferenceAssets;
        if (referenceAssets != null) CheckAuthoredReferences(bindings, referenceAssets);
        Mesh[] builtReferences = null;
        var referenceSamples = new List<ObjectKey>();
        var discreteSamples = new List<DiscreteSample>();
        bool mixed = false;

        var spec = JsonUtility.FromJson<Spec>(JsonUtility.ToJson(pending));
        var capture = RecordCapture(spec, bundles, pending: true, inspectClip: (builtClip, bundle) => {
            if (fixture.ProbeMixedStorage)
            {
                mixed = HasMixedTransformStorage(builtClip, out string reason);
                if (!mixed) log("mixed-transform-storage not covered: " + reason);
            }
            if (referenceAssets == null) return;
            var keysInBundle = bundle.GetAllAssetNames();
            builtReferences = referenceAssets.Select(path => bundle.LoadAsset<Mesh>(BuiltAssetKey(keysInBundle, path))).ToArray();
            Need(builtReferences.All(mesh => mesh != null) && builtReferences[0] != builtReferences[1],
                "Built object-reference assets are missing or indistinguishable.");
        }, inspectPose: (root, time) => {
            if (fixture.ProbeDiscrete) discreteSamples.Add(ReadDiscrete(root, time));
            if (referenceAssets == null) return;
            int key = time < .5f ? 0 : 1;
            var mesh = root.Find(UnityrsFixtureBuilder.ReferencePath).GetComponent<MeshFilter>().sharedMesh;
            Need(mesh == builtReferences[key], "Built playback missed the authored MeshFilter object reference at " + time + ".");
            referenceSamples.Add(new ObjectKey { time_f32 = Bits(time), asset = referenceAssets[key], name = mesh.name });
        });
        editor.object_reference_samples = referenceSamples.ToArray();
        editor.discrete_samples = discreteSamples.ToArray();
        if (fixture.ProbeDiscrete) editor.discrete_keys = ReadDiscreteKeys(clip);
        pending.prefab_asset = spec.prefab_asset;
        pending.clip_asset = spec.clip_asset;
        pending.avatar_asset = spec.avatar_asset;
        if (human)
            CheckHumanoidPlayback(capture as HumanCapture, fixture, prefab, clock, editor);
        if (fixture.TwistMuscles != null)
            CheckTwistEffects(capture as HumanCapture, fixture, prefab, editor);
        Need(ProbeMatches(clock.start.probe, capture.samples) && ProbeMatches(clock.stop.probe, capture.samples),
            "Built playback missed the authored endpoint probes for " + clip.name + ".");
        if (fixture.HeldTime.HasValue)
            SetHeldEvidence(pending, spec, capture, fixture, target);

        // The final human_pose schema needs an inspected runtime slot
        // inventory. Keep these captures until the maintainer adds it.
        if (knownPose)
        {
            output = Path.Combine(output, "known-vectors");
            Directory.CreateDirectory(output);
        }
        string path = Path.Combine(output, fixture.OutputName + ".pending.json");
        WriteReplay(path, "unityrs.anim_replay_pending", JsonUtility.ToJson(pending, true), capture);
        return mixed;
    }

    static void CheckAuthoredReferences(CurveBinding[] bindings, string[] assets)
    {
        var binding = bindings.Single(row => row.path == UnityrsFixtureBuilder.ReferencePath
            && row.type == typeof(MeshFilter).FullName && row.property == UnityrsFixtureBuilder.ReferenceProperty);
        Need(binding.object_reference && binding.object_keys.Select(key => key.asset).SequenceEqual(assets)
            && binding.object_keys.Select(key => key.time_f32).SequenceEqual(new[] { Bits(0), Bits(.5f) }),
            "The authored MeshFilter binding must retain both object-reference keys.");
    }

    static bool HasMixedTransformStorage(AnimationClip clip, out string reason)
    {
        using (var serialized = new SerializedObject(clip))
        {
            var playback = serialized.FindProperty("m_MuscleClip.m_Clip.data")
                ?? serialized.FindProperty("m_MuscleClip.m_Clip");
            reason = "Unity did not expose m_MuscleClip playback on the bundle-loaded scale clip.";
            if (playback == null) return false;

            var bindings = serialized.FindProperty("m_ClipBindingConstant.genericBindings");
            reason = "Unity did not expose a single local-scale binding on the bundle-loaded clip.";
            if (bindings == null || !bindings.isArray || bindings.arraySize != 1) return false;
            var binding = bindings.GetArrayElementAtIndex(0);
            var type = binding.FindPropertyRelative("typeID") ?? binding.FindPropertyRelative("classID");
            if (type?.intValue != 4 || binding.FindPropertyRelative("attribute")?.intValue != 3
                || binding.FindPropertyRelative("customType")?.intValue != 0) return false;

            var stream = playback.FindPropertyRelative("m_StreamedClip.curveCount");
            var dense = playback.FindPropertyRelative("m_DenseClip.m_CurveCount");
            var constant = playback.FindPropertyRelative("m_ConstantClip.data");
            reason = "Unity did not expose valid runtime storage counts on the bundle-loaded scale clip.";
            if (stream == null || dense == null || (constant != null && !constant.isArray)) return false;
            int[] counts = { stream.intValue, dense.intValue, constant?.arraySize ?? 0 };
            reason = "The runtime counts do not show the three scale components split across storage ranges (streamed="
                + counts[0] + ", dense=" + counts[1] + ", constant=" + counts[2] + ").";
            // The candidate has only one three-component local scale binding.
            return counts.All(count => count >= 0) && counts.Sum() == 3 && counts.Count(count => count > 0) > 1;
        }
    }

    static void CheckHumanoidPlayback(HumanCapture capture, UnityrsFixtureBuilder.ReplayCase fixture,
        GameObject prefab, Clock clock, EditorEvidence editor)
    {
        string name = fixture.OutputName;
        Need(capture != null && capture.playback.clip_human_motion,
            "Built clip is missing its human-motion flag for " + name + ".");
        int spine = capture.humanoid.bones.Single(bone => bone.name == "Spine").node;
        Need(spine >= 0 && clock.start.probe.target == spine, "Built Avatar is missing the mapped Spine probe for " + name + ".");
        Quaternion Rotation(Sample sample)
        {
            var bits = sample.pose.transforms[spine].rotation;
            float Component(int i) => BitConverter.ToSingle(BitConverter.GetBytes(
                uint.Parse(bits[i], NumberStyles.HexNumber, CultureInfo.InvariantCulture)), 0);
            return new Quaternion(Component(0), Component(1), Component(2), Component(3));
        }

        // Evaluate the authored muscle vectors independently on the source rig.
        // Compare local Spine rotations so root motion and placement do not
        // change the endpoint being tested.
        Quaternion AuthoredRotation(float time)
        {
            Scene scene = EditorSceneManager.NewPreviewScene();
            try
            {
                var anchor = EditorUtility.CreateGameObjectWithHideFlags("Authored Spine probe", HideFlags.HideAndDontSave);
                SceneManager.MoveGameObjectToScene(anchor, scene);
                var instance = UnityEngine.Object.Instantiate(prefab, anchor.transform, false);
                var animator = At<Animator>(instance.transform, fixture.BindingAddress, 0);
                animator.runtimeAnimatorController = null;
                animator.applyRootMotion = false;
                SampleMuscles(animator, UnityrsFixtureBuilder.KnownHumanoidMuscles(time));
                var bone = animator.GetBoneTransform(HumanBodyBones.Spine);
                Need(bone != null && bone == instance.transform.Find(fixture.ProbePath), "Authored Avatar is missing the Spine probe.");
                return bone.localRotation;
            }
            finally { EditorSceneManager.ClosePreviewScene(scene); }
        }

        const float tolerance = .25f;
        var expectedStart = AuthoredRotation(0);
        var expectedStop = AuthoredRotation(1);
        var neutral = Rotation(capture.samples[0]);
        var single = Rotation(capture.samples.Single(sample => sample.local == .5));
        var last = capture.samples[capture.samples.Length - 1];
        var combined = Rotation(last);
        Need(Quaternion.Angle(neutral, expectedStart) <= tolerance && Quaternion.Angle(combined, expectedStop) <= tolerance,
            "Built Spine rotation differs from the authored endpoint pose in " + name + ".");
        Need(Quaternion.Angle(neutral, single) > 1 && Quaternion.Angle(single, combined) > 1,
            "Humanoid playback did not move the Spine at the authored probes for " + name + ".");
        editor.spine_start_f32 = Bits(expectedStart);
        editor.spine_stop_f32 = Bits(expectedStop);
        editor.spine_endpoint_tolerance_degrees = tolerance;
        clock.start.probe.value = capture.samples[0].pose.transforms[spine].rotation[clock.start.probe.component];
        clock.stop.probe.value = last.pose.transforms[spine].rotation[clock.stop.probe.component];
    }

    static Timepoint[] Schedule(double[] keys, float? heldTime)
    {
        var times = new SortedDictionary<float, double>();
        double start = keys[0], length = keys[keys.Length - 1] - start;
        for (int i = 0; i < keys.Length; i++)
        {
            double local = keys[i] - start;
            times[(float)local] = local;
            if (i > 0)
            {
                // Avoid simple frame-grid fractions. Completion checks this
                // sample against the independently inspected built key times.
                double offKey = keys[i - 1] - start + (keys[i] - keys[i - 1]) * (391.0 / 1024.0);
                if ((float)offKey > (float)(keys[i - 1] - start) && (float)offKey < (float)local)
                    times[(float)offKey] = offKey;
            }
        }
        if (heldTime.HasValue)
        {
            float boundary = (float)(heldTime.Value - start);
            Need(boundary > 0 && boundary < length, "Expected an interior held boundary.");
            int bits = BitConverter.ToInt32(BitConverter.GetBytes(boundary), 0);
            // At a nonzero origin the previous f32 local can already round
            // to the source boundary under f32 addition. Also add a sample
            // guaranteed to be distinctly to its left for the other clock.
            float prior = BitConverter.ToSingle(BitConverter.GetBytes(bits - 1), 0);
            times[prior] = prior;
            float left = boundary - 1f / 4096f;
            Need(left > 0 && start + left < heldTime.Value && (float)(start + left) < heldTime.Value,
                "Could not schedule a distinct left-side probe.");
            times[left] = left;
        }
        times[0] = 0;
        times[(float)length] = length;
        return times.Values.Select(time => new Timepoint { local = time }).ToArray();
    }

    static void SetHeldEvidence(PendingSpec pending, Spec spec, Capture capture,
        UnityrsFixtureBuilder.ReplayCase fixture, int target)
    {
        double boundary = fixture.HeldTime.Value;
        Need(pending.evidence.clock.key_times.Contains(boundary), "Held boundary is missing from the authored curves.");
        foreach (SourceClock clock in new[] { SourceClock.Widened, SourceClock.F32Add })
        {
            var clocks = spec.schedule.Select(point => ReadClock(spec, clock, point.local, (float)point.local)).ToArray();
            int right = Array.FindIndex(clocks, point => point.effective_source >= boundary);
            Need(right > 0, "Held boundary must have probes on both sides.");
            var leftProbe = ProbeAt(fixture, target, right - 1, fixture.HeldLeft.Value);
            var rightProbe = ProbeAt(fixture, target, right, fixture.HeldRight.Value);
            if (!ProbeMatches(leftProbe, capture.samples) || !ProbeMatches(rightProbe, capture.samples)) continue;
            pending.source_clock = clock == SourceClock.Widened ? "widened" : "f32_add";
            pending.evidence.clock.held = new[] { new Held { source = boundary, left = leftProbe, right = rightProbe } };
            for (int i = 0; i < clocks.Length; i++)
            {
                capture.samples[i].effective_source = clocks[i].effective_source;
                capture.samples[i].sides = clocks[i].sides;
            }
            return;
        }
        // Report the probed component at every playable time, so the step
        // shows where Unity's playable clock reaches the boundary.
        var probe = ProbeAt(fixture, target, 0, 0);
        var invariant = CultureInfo.InvariantCulture;
        string trace = string.Join(", ", spec.schedule.Select((point, i) => {
            probe.sample = i;
            uint bits = uint.Parse(ProbeValue(probe, capture.samples), NumberStyles.HexNumber, invariant);
            float value = BitConverter.ToSingle(BitConverter.GetBytes(bits), 0);
            return point.local.ToString("R", invariant) + "->" + value.ToString("R", invariant);
        }));
        throw new InvalidOperationException("Built playback did not match either held-boundary source clock for "
            + spec.clip_name + " (playable time->probe: " + trace + ").");
    }

    static Probe ProbeAt(UnityrsFixtureBuilder.ReplayCase fixture, int target, int sample, float value) => new Probe {
        property = fixture.ProbeProperty, target = target, channel = fixture.ProbeChannel,
        component = 0, sample = sample, value = Bits(value)
    };

    static bool ProbeMatches(Probe probe, Sample[] samples) => probe.value == ProbeValue(probe, samples);

    static string ProbeValue(Probe probe, Sample[] samples)
    {
        var pose = samples[probe.sample].pose;
        if (probe.property == "weight") return pose.weights[probe.target].channels[probe.channel];
        var transform = pose.transforms[probe.target];
        switch (probe.property)
        {
            case "rotation": return transform.rotation[probe.component];
            case "scale": return transform.scale[probe.component];
            case "translation": return transform.translation[probe.component];
            default: throw new InvalidOperationException("Unknown clock probe property " + probe.property + ".");
        }
    }

    static string Address(Transform root, Transform target)
    {
        Need(target != null, "Could not locate generated probe Transform.");
        var parts = new Stack<string>();
        for (Transform node = target; node != root; node = node.parent)
        {
            Need(node.parent != null, "Transform is outside of the fixture prefab.");
            parts.Push(node.GetSiblingIndex().ToString(CultureInfo.InvariantCulture));
        }
        return string.Join("/", parts);
    }

    static CurveBinding[] ReadBindings(AnimationClip clip, out double[] authoredTimes)
    {
        var times = new List<double>();
        var rows = new List<CurveBinding>();
        foreach (var binding in AnimationUtility.GetCurveBindings(clip))
        {
            var keys = AnimationUtility.GetEditorCurve(clip, binding).keys;
            times.AddRange(keys.Select(key => (double)key.time));
            rows.Add(new CurveBinding {
                path = binding.path, property = binding.propertyName, type = binding.type.FullName,
                discrete = binding.isDiscreteCurve, keys = keys.Select(key => new CurveKey {
                    time_f32 = Bits(key.time), value_f32 = Bits(key.value),
                    in_tangent_f32 = RawBits(key.inTangent), out_tangent_f32 = RawBits(key.outTangent),
                    in_weight_f32 = Bits(key.inWeight), out_weight_f32 = Bits(key.outWeight),
                    weighted_mode = key.weightedMode.ToString()
                }).ToArray(), object_keys = Array.Empty<ObjectKey>()
            });
        }
        foreach (var binding in AnimationUtility.GetObjectReferenceCurveBindings(clip))
        {
            Need(binding.isPPtrCurve, "Expected an object-reference curve flag on " + binding.propertyName + ".");
            var keys = AnimationUtility.GetObjectReferenceCurve(clip, binding);
            times.AddRange(keys.Select(key => (double)key.time));
            rows.Add(new CurveBinding {
                path = binding.path, property = binding.propertyName, type = binding.type.FullName,
                object_reference = true, keys = Array.Empty<CurveKey>(),
                object_keys = keys.Select(key => new ObjectKey {
                    time_f32 = Bits(key.time), asset = key.value == null ? "" : AssetDatabase.GetAssetPath(key.value),
                    name = key.value == null ? "" : key.value.name
                }).ToArray()
            });
        }
        authoredTimes = times.ToArray();
        return rows.OrderBy(row => row.path, StringComparer.Ordinal).ThenBy(row => row.type, StringComparer.Ordinal)
            .ThenBy(row => row.property, StringComparer.Ordinal).ToArray();
    }

    static string RawBits(float value) => BitConverter.ToUInt32(BitConverter.GetBytes(value), 0)
        .ToString("x8", CultureInfo.InvariantCulture);
}
#endif
