#if UNITY_EDITOR
using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Linq;
using UnityEngine;

// Round 12's recorder diagnostics (round 12 design v4 section 3). A round 12 case (ReplayCase.Diagnose) records
// the own-bone motion check, the Spine 1 degree and endpoint checks, the scoped lateral and shoulder-hip checks and
// a per-bone motion table as rows of <case>.diagnostics.json beside its capture: each a value, or "" with a reason
// where it is undefined (never NaN), and whether it passed. They never stop a capture from being written. Every
// other case keeps them fatal, as before (Check is Need there). Structural checks stay fatal for every case.
public static partial class AnimReplayRecorder
{
    [Serializable] sealed class Diagnostic
    {
        public string check;
        public int sample = -1;
        public string value, limit, reason;
        public bool passed;
    }
    [Serializable] sealed class MotionRow
    {
        public int sample;
        public string bone, turn_degrees;
    }
    [Serializable] sealed class DiagnosticFile
    {
        public string format = "unityrs.round12_diagnostics", fixture_case, run_id;
        public bool completed;
        public Diagnostic[] diagnostics;
        public MotionRow[] motion;
    }

    static List<Diagnostic> diagnostics;
    static List<MotionRow> motion;
    static bool completed;

    static string DecimalText(double value) => value.ToString("R", CultureInfo.InvariantCulture);

    // A motion check: Need for every case but a round 12 one, a diagnostic row there.
    static void Check(bool passed, string check, int sample, double value, double limit, string message)
    {
        if (diagnostics == null)
        {
            Need(passed, message);
            return;
        }
        bool defined = !double.IsNaN(value) && !double.IsInfinity(value);
        diagnostics.Add(new Diagnostic {
            check = check, sample = sample, value = defined ? DecimalText(value) : "", limit = DecimalText(limit),
            passed = passed && defined, reason = !defined ? "undefined: " + message : passed ? "" : message
        });
    }

    static void BeginDiagnostics(UnityrsFixtureBuilder.ReplayCase fixture)
    {
        diagnostics = fixture.Diagnose ? new List<Diagnostic>() : null;
        motion = fixture.Diagnose ? new List<MotionRow>() : null;
        completed = false;
    }

    // Written however the case ends, a structural failure included (`completed` false).
    static void EndDiagnostics(UnityrsFixtureBuilder.ReplayCase fixture, string output, string runId)
    {
        if (diagnostics == null) return;
        try
        {
            string folder = Path.Combine(output, "body");
            Directory.CreateDirectory(folder);
            var file = new DiagnosticFile {
                fixture_case = fixture.OutputName, run_id = runId, completed = completed,
                diagnostics = diagnostics.ToArray(), motion = motion.ToArray()
            };
            File.WriteAllText(Path.Combine(folder, fixture.OutputName + ".diagnostics.json"), JsonUtility.ToJson(file, true));
        }
        finally
        {
            diagnostics = null;
            motion = null;
        }
    }

    // Every mapped bone's turn at every sample from the first (the zero vector of a round 12 schedule or clip).
    static void AddMotionTable(HumanCapture capture)
    {
        if (motion == null) return;
        float Value(string bits) => BitConverter.ToSingle(BitConverter.GetBytes(
            uint.Parse(bits, NumberStyles.HexNumber, CultureInfo.InvariantCulture)), 0);
        double[] Values(string[] bits) => bits.Select(b => (double)Value(b)).ToArray();
        var zero = capture.samples[0];
        for (int k = 0; k < capture.samples.Length; k++)
            foreach (var bone in capture.humanoid.bones.Where(bone => bone.node >= 0))
                motion.Add(new MotionRow {
                    sample = k, bone = bone.name,
                    turn_degrees = DecimalText(Angle(Values(zero.pose.transforms[bone.node].rotation),
                        Values(capture.samples[k].pose.transforms[bone.node].rotation)) * 180 / Math.PI)
                });
    }
}
#endif
