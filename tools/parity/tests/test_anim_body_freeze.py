"""B2's order of evidence: E3a on the training rigs, the freeze, then E3b on the hold-out rig against it.

The hold-out stays out of everything before E3b by construction, E3b refuses
to run without a whole freeze artifact made from training inputs alone, and
it never fits."""

import contextlib
import copy
import io
import json
import pathlib
import unittest
from unittest import mock

from anim_body_model_support import BITS, F32, avatar, recording, write_run

import anim_body_freeze as freeze  # noqa: E402  (test_anim_replay puts it on the path)
import anim_body_runs as runs  # noqa: E402
import anim_fixture_body as body  # noqa: E402
import scratch  # noqa: E402

TRAINING = ("body-probes-compact", "body-targets-tall", "body-clip-level", "body-chain-hips-scale-targets")
HOLDOUT = "body-probes-holdout"


def avatars(run, doc):
    return avatar(doc)


class FreezeTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.docs = {name: recording(name, samples=12) for name in (*TRAINING, HOLDOUT)}

    def setUp(self):
        self.folder = write_run(scratch.for_case(self) / "run", self.docs)
        self.artifact = self.folder.parent / "freeze.json"

    def main(self, *args):
        out, err = io.StringIO(), io.StringIO()
        with mock.patch.object(freeze, "avatar_reader", return_value=avatars), \
                contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = freeze.main([*args[:1], str(self.folder), *args[1:], "--unityrs", "cli"])
        return code, out.getvalue() + err.getvalue()

    def frozen(self):
        self.assertEqual(self.main("freeze", "--freeze", str(self.artifact))[0], 0)
        return json.loads(self.artifact.read_text(encoding="utf-8"))

    def rewrite(self, artifact):
        self.artifact.write_text(json.dumps(artifact), encoding="utf-8")

    def test_e3b_refuses_without_a_freeze_artifact(self):
        code, text = self.main("e3b")
        self.assertEqual(code, 2)
        self.assertIn("no freeze artifact", text)
        code, text = self.main("e3b", "--freeze", str(self.artifact))
        self.assertEqual(code, 2)
        self.assertFalse(self.artifact.exists())

    def test_the_freeze_then_the_hold_out(self):
        artifact = self.frozen()
        self.assertEqual(artifact["format"], freeze.FORMAT)
        self.assertEqual(set(artifact), {"format", "version", "model", "coefficients", "scope", "bounds", "training",
                                         "e3a"})
        self.assertEqual(artifact["model"], freeze.HYPOTHESES)
        cases = {row["case"] for row in artifact["training"]["inputs"]}
        self.assertEqual(cases, set(TRAINING))
        self.assertEqual(artifact["training"]["digest"], runs.digest(artifact["training"]["inputs"]))
        self.assertEqual(artifact["scope"]["holdout"], body.HOLDOUT)
        self.assertNotIn(body.HOLDOUT, artifact["scope"]["training_rigs"])
        self.assertEqual(artifact["bounds"]["condition_limit"], 1e6)
        code, text = self.main("e3b", "--freeze", str(self.artifact), "--json", str(self.folder.parent / "r.json"))
        self.assertEqual((code, text.strip()), (0, "e3b: pass"))
        report = json.loads((self.folder.parent / "r.json").read_text(encoding="utf-8"))
        # Every hold-out sample is checked or, twisted past 2.2's scope, refused.
        self.assertEqual(report["samples"] + report["refused"], len(self.docs[HOLDOUT]["capture"]["samples"]))
        self.assertGreater(report["samples"], report["refused"])
        self.assertEqual(report["digest"], artifact["training"]["digest"])
        # A freeze is made once.
        code, text = self.main("freeze", "--freeze", str(self.artifact))
        self.assertEqual(code, 2)
        self.assertIn("new artifact", text)

    def test_the_freeze_waits_for_e3a(self):
        docs = copy.deepcopy(self.docs)
        hips = docs["body-targets-tall"]["capture"]["samples"][1]["pose"]["transforms"][2]
        hips["translation"][1] = BITS(F32(hips["translation"][1]) + 1e-3)
        write_run(self.folder, docs)
        self.assertEqual(self.main("e3a")[0], 1)
        code, text = self.main("freeze", "--freeze", str(self.artifact))
        self.assertEqual(code, 2)
        self.assertIn("E3a must pass", text)
        self.assertFalse(self.artifact.exists())

    def test_e3b_refuses_an_artifact_made_with_hold_out_inputs(self):
        artifact = self.frozen()
        path = self.folder / "replays" / "body" / f"{HOLDOUT}.pending.json"
        sha = runs.hashlib.sha256(path.read_bytes()).hexdigest()
        poisoned = [*artifact["training"]["inputs"], dict(run=artifact["scope"]["editor_versions"][0], case=HOLDOUT,
                                                          rig=body.HOLDOUT, sha256=sha)]
        relabelled = [dict(row, rig=body.HOLDOUT) for row in artifact["training"]["inputs"]]
        for inputs, rows in ((poisoned, 0), (relabelled, 0), (artifact["training"]["inputs"], 3)):
            with self.subTest(rows=rows, inputs=len(inputs)):
                changed = copy.deepcopy(artifact)
                changed["training"].update(inputs=inputs, digest=runs.digest(inputs), holdout_rows=rows)
                self.rewrite(changed)
                code, text = self.main("e3b", "--freeze", str(self.artifact))
                self.assertEqual(code, 2)
                self.assertIn("produced from hold-out inputs", text)

    def test_e3b_refuses_a_changed_artifact_or_training_set(self):
        artifact = self.frozen()
        for edit, message in ((lambda a: a["training"].update(digest="0" * 64), "digest disagrees"),
                              (lambda a: a["model"].update(frame="another frame"), "model choices differ"),
                              (lambda a: a.update(version=2), "expected unityrs.anim_body_freeze"),
                              (lambda a: a["coefficients"].popitem(), "do not match the training design")):
            with self.subTest(message=message):
                changed = copy.deepcopy(artifact)
                edit(changed)
                self.rewrite(changed)
                code, text = self.main("e3b", "--freeze", str(self.artifact))
                self.assertEqual(code, 2)
                self.assertIn(message, text)
        # A training recording changed after the freeze.
        self.rewrite(artifact)
        docs = copy.deepcopy(self.docs)
        docs["body-targets-tall"]["capture"]["samples"].pop()
        docs["body-targets-tall"]["spec"]["evidence"]["editor"]["body_samples"].pop()
        write_run(self.folder, docs)
        code, text = self.main("e3b", "--freeze", str(self.artifact))
        self.assertEqual(code, 2)
        self.assertIn("training inputs differ from the frozen ones", text)

    def test_e3b_never_fits_and_fails_on_other_coefficients(self):
        artifact = self.frozen()
        with mock.patch.object(freeze, "solve", side_effect=AssertionError("fitted")), \
                mock.patch.object(runs, "solve", side_effect=AssertionError("fitted")):
            self.assertEqual(self.main("e3b", "--freeze", str(self.artifact))[0], 0)
        changed = copy.deepcopy(artifact)
        changed["coefficients"] = {key: value + .25 for key, value in artifact["coefficients"].items()}
        self.rewrite(changed)
        code, text = self.main("e3b", "--freeze", str(self.artifact))
        self.assertEqual(code, 1)
        self.assertIn(f"/{HOLDOUT}@", text)

    def test_e3a_never_reads_the_hold_out(self):
        opened = []
        read = pathlib.Path.read_bytes

        def spy(path):
            opened.append(path.name)
            return read(path)

        with mock.patch.object(pathlib.Path, "read_bytes", spy):
            code, _ = self.main("freeze", "--freeze", str(self.artifact))
        self.assertEqual(code, 0)
        self.assertTrue(opened and not any("holdout" in name for name in opened))
        before = self.main("e3a", "--json", str(self.folder.parent / "a.json"))
        report = (self.folder.parent / "a.json").read_text(encoding="utf-8")
        # A hold-out file that cannot even be parsed changes nothing before E3b.
        (self.folder / "replays" / "body" / f"{HOLDOUT}.pending.json").write_text("{", encoding="utf-8")
        self.assertEqual(self.main("e3a", "--json", str(self.folder.parent / "a.json")), before)
        self.assertEqual((self.folder.parent / "a.json").read_text(encoding="utf-8"), report)
        self.assertEqual(self.main("e3b", "--freeze", str(self.artifact))[0], 2)

    def test_the_roles_split_by_case_name(self):
        self.assertEqual(set(runs.recording_paths(self.folder, "training")), set(TRAINING))
        self.assertEqual(set(runs.recording_paths(self.folder, "holdout")), {HOLDOUT})
        # A completed clip is read over its pending recording.
        self.assertEqual(runs.recording_paths(self.folder, "training")["body-clip-level"].name, "body-clip-level.json")
        with self.assertRaisesRegex(ValueError, "unknown role"):
            runs.recording_paths(self.folder, "all")
        # The fit refuses a hold-out row however it arrives.
        held = runs.observe(runs.load([self.folder], "holdout"))
        trained = runs.observe(runs.load([self.folder], "training"))
        with self.assertRaisesRegex(ValueError, "refuses hold-out rows"):
            runs.Design(trained + held[:1])
        # A run whose role check sees a hold-out rig in a training load refuses.
        with mock.patch.object(runs, "role_of", return_value="training"), \
                self.assertRaisesRegex(ValueError, "read the wrong rig"):
            runs.load([self.folder], "training")

    def test_the_fitted_modes_need_the_avatars(self):
        for mode in freeze.FITTED:
            with self.subTest(mode=mode), self.assertRaisesRegex(ValueError, "give --unityrs"):
                freeze.run_mode(mode, [self.folder], self.artifact)


if __name__ == "__main__":
    unittest.main()
