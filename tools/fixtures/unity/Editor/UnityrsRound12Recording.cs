#if UNITY_EDITOR
using System;
using System.Collections.Generic;
using System.Globalization;
using System.IO;
using System.Linq;
using System.Text;

// Round 12's recorder diagnostics (round 12 design v4 section 3). A round 12 case (ReplayCase.Diagnose) records
// the own-bone motion check, the Spine 1 degree and endpoint checks, the scoped lateral and shoulder-hip checks and
// a per-bone motion table as rows of <case>.diagnostics.json beside its capture: each a number, or null with a reason
// where it is undefined (never NaN), and whether it passed. They never stop a capture from being written: the Spine
// endpoint evaluations run only after it is (RunDeferred), and their failure is a row too. Every other case keeps
// them fatal, as before (Check is Need there). Structural checks stay fatal for every case.
public static partial class AnimReplayRecorder
{
    sealed class Diagnostic
    {
        public string check, reason;
        public int sample = -1;
        public double? value;
        public double limit;
        public bool passed;
    }
    sealed class MotionRow
    {
        public int sample;
        public string bone, reason;
        public double? turn_degrees;
    }

    static List<Diagnostic> diagnostics;
    static List<MotionRow> motion;
    static bool completed;
    static Action deferred;

    static bool Finite(double value) => !double.IsNaN(value) && !double.IsInfinity(value);

    // A motion check: Need for every case but a round 12 one, a diagnostic row there.
    static void Check(bool passed, string check, int sample, double value, double limit, string message)
    {
        if (diagnostics == null)
        {
            Need(passed, message);
            return;
        }
        bool defined = Finite(value);
        diagnostics.Add(new Diagnostic {
            check = check, sample = sample, value = defined ? value : (double?)null, limit = limit,
            passed = passed && defined, reason = !defined ? "undefined: " + message : passed ? null : message
        });
    }

    static void BeginDiagnostics(UnityrsFixtureBuilder.ReplayCase fixture)
    {
        diagnostics = fixture.Diagnose ? new List<Diagnostic>() : null;
        motion = fixture.Diagnose ? new List<MotionRow>() : null;
        completed = false;
        deferred = null;
    }

    // After the capture is written: the deferred endpoint checks, any failure of theirs a row.
    static void RunDeferred()
    {
        var action = deferred;
        deferred = null;
        if (action == null || diagnostics == null) return;
        try { action(); }
        catch (Exception error)
        {
            diagnostics.Add(new Diagnostic { check = "spine_endpoints", reason = "not evaluated: " + error.Message });
        }
    }

    // Written however the case ends, a structural failure included (`completed` false).
    static void EndDiagnostics(UnityrsFixtureBuilder.ReplayCase fixture, string output, string runId)
    {
        if (diagnostics == null) return;
        try
        {
            string folder = Path.Combine(output, "body");
            Directory.CreateDirectory(folder);
            var json = new StringBuilder();
            json.Append("{\"format\":\"unityrs.round12_diagnostics\",\"fixture_case\":").Append(Text(fixture.OutputName))
                .Append(",\"run_id\":").Append(Text(runId)).Append(",\"completed\":").Append(completed ? "true" : "false")
                .Append(",\"diagnostics\":[");
            json.Append(string.Join(",", diagnostics.Select(row => "{\"check\":" + Text(row.check) + ",\"sample\":"
                + row.sample.ToString(CultureInfo.InvariantCulture) + ",\"value\":" + Number(row.value) + ",\"limit\":"
                + Number(row.limit) + ",\"passed\":" + (row.passed ? "true" : "false") + ",\"reason\":" + Text(row.reason) + "}")));
            json.Append("],\"motion\":[");
            json.Append(string.Join(",", motion.Select(row => "{\"sample\":" + row.sample.ToString(CultureInfo.InvariantCulture)
                + ",\"bone\":" + Text(row.bone) + ",\"turn_degrees\":" + Number(row.turn_degrees) + ",\"reason\":"
                + Text(row.reason) + "}")));
            json.Append("]}\n");
            File.WriteAllText(Path.Combine(folder, fixture.OutputName + ".diagnostics.json"), json.ToString());
        }
        finally
        {
            diagnostics = null;
            motion = null;
            deferred = null;
        }
    }

    // JSON text: null, or a string with its quotes, backslashes and control characters escaped.
    static string Text(string value)
    {
        if (value == null) return "null";
        var text = new StringBuilder("\"");
        foreach (char c in value)
            text.Append(c == '"' ? "\\\"" : c == '\\' ? "\\\\" : c < ' ' ? "\\u" + ((int)c).ToString("x4", CultureInfo.InvariantCulture)
                : c.ToString());
        return text.Append('"').ToString();
    }

    // A JSON number, or null where it is not finite.
    static string Number(double? value) =>
        value.HasValue && Finite(value.Value) ? value.Value.ToString("R", CultureInfo.InvariantCulture) : "null";

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
            {
                double turn = Angle(Values(zero.pose.transforms[bone.node].rotation),
                    Values(capture.samples[k].pose.transforms[bone.node].rotation)) * 180 / Math.PI;
                motion.Add(new MotionRow {
                    sample = k, bone = bone.name, turn_degrees = Finite(turn) ? turn : (double?)null,
                    reason = Finite(turn) ? null : "undefined: a nonfinite rotation at this sample or the first"
                });
            }
    }
}
#endif
