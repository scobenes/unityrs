#if UNITY_EDITOR
using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Linq;
using UnityEditor;
using UnityEngine;

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
        public string animation_source;
        public string expected_discovery;
        public string length_f32, frame_rate_f32;
        public ClipSettings clip_settings;
        public CurveBinding[] bindings;
        public string[] model_import_settings;
    }
    [Serializable] sealed class PendingEvidence
    {
        public string layout = "editor_authored";
        public Clock clock;
        public EditorEvidence editor;
    }
    [Serializable] sealed class PendingSpec
    {
        public BundleName[] bundles;
        public string prefab_asset, clip_asset, avatar_asset, clip_name, binding_address, build_target, mode;
        public string source_clock = "widened";
        public int binding_component = 0;
        public double start, stop, sample_rate;
        public double[] held_boundaries;
        public Timepoint[] schedule;
        public Node[] nodes;
        public PendingRenderer[] renderers;
        public PendingEvidence evidence;
    }

    internal static void RecordFixture(UnityrsFixtureBuilder.ReplayCase fixture, string bundles, string output)
    {
        var prefab = AssetDatabase.LoadAssetAtPath<GameObject>(fixture.Prefab);
        var clip = AssetDatabase.LoadAssetAtPath<AnimationClip>(fixture.Clip);
        Need(prefab != null && clip != null, "Could not read generated rig or animation clip.");
        var settings = AnimationUtility.GetAnimationClipSettings(clip);
        double start = fixture.Start ?? (clip.legacy ? 0 : settings.startTime);
        double stop = fixture.Stop ?? (clip.legacy ? clip.length : settings.stopTime);
        Need(0 <= start && start <= stop && clip.length == (float)(stop - start),
            "Editor clip interval differs from the authored interval for " + clip.name);
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
        var pending = new PendingSpec {
            bundles = new[] { new BundleName { name = fixture.Bundle } },
            prefab_asset = fixture.Prefab.ToLowerInvariant(), clip_asset = fixture.Clip.ToLowerInvariant(),
            avatar_asset = fixture.Avatar.ToLowerInvariant(), clip_name = clip.name,
            build_target = EditorUserBuildSettings.activeBuildTarget.ToString(),
            binding_address = fixture.BindingAddress, mode = clip.legacy ? "legacy" : "animator",
            start = start, stop = stop, sample_rate = clip.frameRate,
            held_boundaries = fixture.HeldTime.HasValue ? new[] { (double)fixture.HeldTime.Value } : Array.Empty<double>(),
            schedule = points, nodes = nodes, renderers = renderers,
            evidence = new PendingEvidence { clock = clock, editor = new EditorEvidence {
                animation_source = fixture.Avatar.Length == 0 ? "procedural_clip" : "humanoid_model_import",
                expected_discovery = fixture.ExpectedDiscovery,
                bindings = bindings, clip_settings = ReadSettings(clip), length_f32 = Bits(clip.length),
                frame_rate_f32 = Bits(clip.frameRate), model_import_settings = fixture.ImportSettings
            } }
        };

        var spec = JsonUtility.FromJson<Spec>(JsonUtility.ToJson(pending));
        var capture = RecordCapture(spec, bundles, pending: true);
        Need(ProbeMatches(clock.start.probe, capture.samples) && ProbeMatches(clock.stop.probe, capture.samples),
            "Built playback missed the authored endpoint probes for " + clip.name + ".");
        if (fixture.HeldTime.HasValue)
            SetHeldEvidence(pending, spec, capture, fixture, target, clip.length);

        string path = Path.Combine(output, fixture.OutputName + ".pending.json");
        WriteReplay(path, "unityrs.anim_replay_pending", JsonUtility.ToJson(pending, true), capture);
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
                // Sample within each authored segment, away from key rounding.
                double offKey = (keys[i - 1] - start + local) * .5;
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
        UnityrsFixtureBuilder.ReplayCase fixture, int target, float length)
    {
        double boundary = fixture.HeldTime.Value;
        Need(pending.evidence.clock.key_times.Contains(boundary), "Held boundary is missing from the authored curves.");
        foreach (SourceClock clock in new[] { SourceClock.Widened, SourceClock.F32Add })
        {
            var clocks = pending.schedule.Select(point => ReadClock(spec, clock, length, point.local, (float)point.local)).ToArray();
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
        throw new InvalidOperationException("Built playback did not match either held-boundary source clock for " + spec.clip_name);
    }

    static Probe ProbeAt(UnityrsFixtureBuilder.ReplayCase fixture, int target, int sample, float value) => new Probe {
        property = fixture.ProbeProperty, target = target, channel = fixture.ProbeChannel,
        component = 0, sample = sample, value = Bits(value)
    };

    static bool ProbeMatches(Probe probe, Sample[] samples)
    {
        var pose = samples[probe.sample].pose;
        return probe.value == (probe.property == "weight" ? pose.weights[probe.target].channels[probe.channel]
            : pose.transforms[probe.target].translation[probe.component]);
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
