"""B2's order of evidence: E3a on the training rigs, the freeze, then E3b on the hold-out rig against it.

The hold-out stays out of everything before E3b by construction; the freeze
needs both Editor versions, every required case, full rank and |a| <= 1;
E3b refuses an artifact git does not hold unmodified, or whose E3a, model,
bounds, source files or fit are not the code's, and evaluates the hold-out
with the stored coefficients alone."""

import contextlib
import copy
import io
import json
import os
import pathlib
import unittest
from unittest import mock

from anim_body_model_support import BITS, CLIPS, F32, avatar, recording, write_run

import anim_body_freeze as freeze  # noqa: E402  (test_anim_replay puts it on the path)
import anim_body_runs as runs  # noqa: E402
import anim_fixture_body as body  # noqa: E402
import child  # noqa: E402
import scratch  # noqa: E402

# A training set whose design has full rank, as the freeze needs.
TRAINING = ("body-probes-compact", "body-targets-tall", "body-clip-level", "body-chain-hips-scale-targets",
            "body-probes-noncoplanar", "body-probes-no-upper-chest")
HOLDOUT = "body-probes-holdout"
VERSIONS = ("2022.3.62f3", "6000.6.9f1")
GIT_VARIABLES = ("GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_COMMON_DIR")


def avatars(run, doc):
    return avatar(doc)


def clips(run, doc):
    return CLIPS[doc["spec"]["evidence"]["editor"]["fixture_case"]]


def git(folder, *args):
    """git in a scratch repository, free of any inherited repository."""
    env = {key: value for key, value in os.environ.items() if key not in GIT_VARIABLES}
    return child.run(["git", "-C", str(folder), "-c", "user.name=t", "-c", "user.email=t@example.invalid",
                      "-c", "commit.gpgsign=false", *args], env=env, capture_output=True, text=True, check=True)


class FreezeTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.docs = {name: recording(name, samples=12) for name in (*TRAINING, HOLDOUT)}

    def setUp(self):
        root = scratch.for_case(self)
        self.folders = [write_run(root / version, self.docs, version) for version in VERSIONS]
        self.repo = root / "repo"
        self.repo.mkdir()
        git(self.repo, "init", "-q")
        self.artifact = self.repo / "freeze.json"
        patches = (mock.patch.object(runs, "REQUIRED", frozenset(TRAINING)),
                   mock.patch.dict(os.environ, {key: "" for key in GIT_VARIABLES}))
        for patch in patches:
            patch.start()
            self.addCleanup(patch.stop)
        for key in GIT_VARIABLES:
            del os.environ[key]

    def main(self, *args, folders=None):
        out, err = io.StringIO(), io.StringIO()
        with mock.patch.object(freeze, "readers", return_value=(avatars, clips)), \
                contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            code = freeze.main([args[0], *map(str, folders or self.folders), *args[1:], "--unityrs", "cli"])
        return code, out.getvalue() + err.getvalue()

    def commit(self):
        git(self.repo, "add", "freeze.json")
        git(self.repo, "commit", "-q", "-m", "freeze")

    def frozen(self):
        self.assertEqual(self.main("freeze", "--freeze", str(self.artifact))[0], 0)
        self.commit()
        return json.loads(self.artifact.read_text(encoding="utf-8"))

    def rewrite(self, artifact, *, commit=True):
        self.artifact.write_text(json.dumps(artifact), encoding="utf-8")
        if commit:
            self.commit()

    def e3b(self, message=None, code=2):
        found, text = self.main("e3b", "--freeze", str(self.artifact))
        self.assertEqual(found, code, text)
        if message:
            self.assertIn(message, text)
        return text

    def test_e3b_refuses_without_a_freeze_artifact(self):
        code, text = self.main("e3b")
        self.assertEqual(code, 2)
        self.assertIn("no freeze artifact", text)
        self.e3b("no freeze artifact")
        self.assertFalse(self.artifact.exists())

    def test_the_freeze_then_the_hold_out(self):
        artifact = self.frozen()
        self.assertEqual(artifact["format"], freeze.FORMAT)
        self.assertEqual(set(artifact), {"format", "version", "model", "coefficients", "scope", "bounds", "training",
                                         "e3a", "code", "evidence"})
        self.assertEqual(artifact["scope"]["refusals"], freeze.REFUSALS)
        self.assertEqual(set(freeze.REFUSALS), {"topology", "frame", "spine_lateral_range"})
        self.assertEqual(artifact["evidence"]["runs"], [])
        self.assertEqual(artifact["model"], freeze.HYPOTHESES)
        self.assertTrue(artifact["e3a"]["passed"])
        self.assertEqual(artifact["code"]["model_digest"], freeze.model_digest())
        self.assertRegex(artifact["code"]["commit"], "^[0-9a-f]{40}$")
        self.assertEqual(artifact["scope"]["editor_versions"], list(VERSIONS))
        cases = {row["case"] for row in artifact["training"]["inputs"]}
        self.assertEqual(cases, set(TRAINING))
        self.assertEqual(artifact["training"]["digest"], runs.digest(artifact["training"]["inputs"]))
        self.assertEqual(artifact["scope"]["holdout"], body.HOLDOUT)
        self.assertNotIn(body.HOLDOUT, artifact["scope"]["training_rigs"])
        self.assertEqual(artifact["bounds"]["condition_limit"], 1e6)
        report_path = self.repo.parent / "r.json"
        code, text = self.main("e3b", "--freeze", str(self.artifact), "--json", str(report_path))
        self.assertEqual((code, text.strip()), (0, "e3b: pass"))
        report = json.loads(report_path.read_text(encoding="utf-8"))
        # Every hold-out sample of both versions is checked or, twisted past 2.2's scope, refused; and its
        # Avatars pass E0 against the training table.
        self.assertEqual(report["samples"] + report["refused"], 2 * len(self.docs[HOLDOUT]["capture"]["samples"]))
        self.assertGreater(report["samples"], report["refused"])
        self.assertEqual(len(report["avatars"]), 2)
        self.assertEqual(report["digest"], artifact["training"]["digest"])
        # A freeze is made once, and never over its report.
        code, text = self.main("freeze", "--freeze", str(self.artifact))
        self.assertEqual(code, 2)
        self.assertIn("new artifact", text)
        code, text = self.main("e3b", "--freeze", str(self.artifact), "--json", str(self.artifact))
        self.assertEqual(code, 2)
        self.assertIn("--json and --freeze name one file", text)

    def test_supporting_runs_are_checked_within_the_limit_and_never_inputs(self):
        root = self.repo.parent / "earlier"
        earlier = [write_run(root / version, self.docs, version) for version in VERSIONS]
        code, text = self.main("e3a", "--evidence", *map(str, earlier))
        self.assertEqual(code, 2)
        self.assertIn("--evidence is for freeze", text)
        code, text = self.main("freeze", "--freeze", str(self.artifact), "--evidence", *map(str, self.folders))
        self.assertEqual(code, 2)
        self.assertIn("one of the freeze's own", text)
        # A supporting run whose hips moved refuses the freeze.
        docs = copy.deepcopy(self.docs)
        doc = docs["body-targets-tall"]
        hips = next(row["node"] for row in doc["capture"]["humanoid"]["bones"] if row["name"] == "Hips")
        pose = doc["capture"]["samples"][0]["pose"]["transforms"][hips]
        pose["translation"][1] = BITS(F32(pose["translation"][1]) + 1e-3)
        write_run(earlier[0], docs, VERSIONS[0])
        code, text = self.main("freeze", "--freeze", str(self.artifact), "--evidence", *map(str, earlier))
        self.assertEqual(code, 2)
        self.assertIn("the supporting runs fail within the lateral limit", text)
        self.assertFalse(self.artifact.exists())
        write_run(earlier[0], self.docs, VERSIONS[0])
        code, text = self.main("freeze", "--freeze", str(self.artifact), "--evidence", *map(str, earlier))
        self.assertEqual(code, 0, text)
        self.commit()
        artifact = json.loads(self.artifact.read_text(encoding="utf-8"))
        support = artifact["evidence"]["runs"]
        self.assertEqual([entry["run"] for entry in support], list(VERSIONS))
        self.assertTrue(all(entry["e2"]["passed"] and entry["e3"]["passed"] and entry["e3"]["samples"]
                            for entry in support))
        self.assertEqual([entry["digest"] for entry in support],
                         [runs.digest(run.inputs) for run in runs.load(earlier, "training")])
        # Never an input: the training digest is the freeze's own runs', and e3b passes on them.
        self.assertEqual(artifact["training"]["digest"], runs.digest([row for run in runs.load(self.folders, "training")
                                                                      for row in run.inputs]))
        self.e3b(code=0)

    def test_the_freeze_waits_for_e3a_both_versions_and_every_case(self):
        docs = copy.deepcopy(self.docs)
        hips = docs["body-targets-tall"]["capture"]["samples"][1]["pose"]["transforms"][2]
        hips["translation"][1] = BITS(F32(hips["translation"][1]) + 1e-3)
        write_run(self.folders[0], docs, VERSIONS[0])
        self.assertEqual(self.main("e3a")[0], 1)
        code, text = self.main("freeze", "--freeze", str(self.artifact))
        self.assertEqual(code, 2)
        self.assertIn("E3a must pass", text)
        write_run(self.folders[0], self.docs, VERSIONS[0])
        code, text = self.main("freeze", "--freeze", str(self.artifact), folders=self.folders[:1])
        self.assertEqual(code, 2)
        self.assertIn("one run of each Editor version", text)
        with mock.patch.object(runs, "REQUIRED", frozenset({*TRAINING, "body-probes-tall"})):
            code, text = self.main("freeze", "--freeze", str(self.artifact))
        self.assertEqual(code, 2)
        self.assertIn("E3a must pass", text)
        self.assertFalse(self.artifact.exists())

    def test_the_freeze_needs_full_rank_and_bounded_coefficients(self):
        passed = dict(passed=True, failures=[], samples=1, worst_ratio=0., refused=0)
        with mock.patch.object(freeze.check, "e3", return_value=passed):
            with mock.patch.object(freeze, "solve", side_effect=lambda design: dict.fromkeys(design.keys, 1.5)):
                code, text = self.main("freeze", "--freeze", str(self.artifact))
            self.assertEqual(code, 2)
            self.assertIn("every coefficient within |a| <= 1.0", text)
            short = runs.Design

            def deficient(rows):
                design = short(rows)
                design.rank -= 1
                return design

            with mock.patch.object(freeze, "Design", deficient):
                code, text = self.main("freeze", "--freeze", str(self.artifact))
            self.assertEqual(code, 2)
            self.assertIn("full rank", text)
        self.assertFalse(self.artifact.exists())

    def test_e3b_refuses_an_artifact_made_with_hold_out_inputs(self):
        artifact = self.frozen()
        path = self.folders[0] / "replays" / "body" / f"{HOLDOUT}.pending.json"
        sha = runs.hashlib.sha256(path.read_bytes()).hexdigest()
        poisoned = [*artifact["training"]["inputs"], dict(run=VERSIONS[0], case=HOLDOUT, rig=body.HOLDOUT, sha256=sha)]
        relabelled = [dict(row, rig=body.HOLDOUT) for row in artifact["training"]["inputs"]]
        for inputs, rows in ((poisoned, 0), (relabelled, 0), (artifact["training"]["inputs"], 3)):
            with self.subTest(rows=rows, inputs=len(inputs)):
                changed = copy.deepcopy(artifact)
                changed["training"].update(inputs=inputs, digest=runs.digest(inputs), holdout_rows=rows)
                self.rewrite(changed)
                self.e3b("produced from hold-out inputs")

    def test_e3b_refuses_an_artifact_git_does_not_hold(self):
        artifact = self.frozen()
        # One coefficient tampered after the commit, or the artifact never committed.
        changed = copy.deepcopy(artifact)
        key = sorted(changed["coefficients"])[0]
        changed["coefficients"][key] += 1e-9
        self.rewrite(changed, commit=False)
        self.e3b("differs from its committed version")
        git(self.repo, "rm", "-q", "--cached", "freeze.json")
        self.e3b("not tracked by git")

    def test_e3b_refuses_a_changed_artifact_or_training_set(self):
        artifact = self.frozen()
        key = sorted(artifact["coefficients"])[0]
        for edit, message in ((lambda a: a["training"].update(digest="0" * 64), "digest disagrees"),
                              (lambda a: a["model"].update(frame="another frame"), "model choices differ"),
                              (lambda a: a.update(version=1), "expected unityrs.anim_body_freeze"),
                              (lambda a: a["e3a"].update(passed=False), "no passed E3a"),
                              (lambda a: a["bounds"].update(fold_depth=49), "bounds differ"),
                              (lambda a: a["code"].update(model_digest="0" * 64), "source files differ"),
                              (lambda a: a["bounds"].update(rank=1), "rank or condition differs"),
                              (lambda a: a["scope"].update(lateral_limit="0.1"), "malformed frozen lateral limit"),
                              (lambda a: a["scope"].update(lateral_limit=.0625), "lateral limit differs"),
                              (lambda a: a["coefficients"].popitem(), "do not match the training design"),
                              # One coefficient tampered and committed: not the training fit.
                              (lambda a: a["coefficients"].update({key: a["coefficients"][key] + 1e-9}),
                               "not the training fit")):
            with self.subTest(message=message):
                changed = copy.deepcopy(artifact)
                edit(changed)
                self.rewrite(changed)
                self.e3b(message)
        # The code's model files changed after the freeze.
        self.rewrite(artifact)
        with mock.patch.object(freeze, "MODEL_FILES", freeze.MODEL_FILES[1:]):
            self.e3b("source files differ")
        # A training recording changed after the freeze.
        docs = copy.deepcopy(self.docs)
        docs["body-targets-tall"]["capture"]["samples"].pop()
        docs["body-targets-tall"]["spec"]["evidence"]["editor"]["body_samples"].pop()
        write_run(self.folders[0], docs, VERSIONS[0])
        self.e3b("training inputs differ from the frozen ones")

    def test_e3b_evaluates_the_stored_coefficients_and_fails_one_moved_observation(self):
        artifact = self.frozen()
        seen = []
        e3 = freeze.check.e3

        def spy(rows, design, coefficients, limit=None, compared=None):
            seen.append((coefficients, limit))
            return e3(rows, design, coefficients, limit, compared)

        with mock.patch.object(freeze.check, "e3", spy):
            self.e3b(code=0)
        # The frozen training rows give the frozen limit once more, then the hold-out compares within it.
        self.assertEqual(seen[-1], (artifact["coefficients"], artifact["scope"]["lateral_limit"]))
        # One hold-out observation moved: e3b fails on that sample alone.
        docs = copy.deepcopy(self.docs)
        doc = docs[HOLDOUT]
        hips = next(row["node"] for row in doc["capture"]["humanoid"]["bones"] if row["name"] == "Hips")
        pose = doc["capture"]["samples"][0]["pose"]["transforms"][hips]
        pose["translation"][1] = BITS(F32(pose["translation"][1]) + 1e-3)
        write_run(self.folders[0], docs, VERSIONS[0])
        text = self.e3b(code=1)
        self.assertIn(f"{VERSIONS[0]}/{HOLDOUT}@{doc['capture']['samples'][0]['local']}: hips translation off", text)
        self.assertIn("1 failure(s)", text)

    def test_e3a_never_reads_the_hold_out(self):
        opened = []
        read = pathlib.Path.read_bytes

        def spy(path):
            opened.append(path.name)
            return read(path)

        with mock.patch.object(pathlib.Path, "read_bytes", spy):
            code, _ = self.main("freeze", "--freeze", str(self.artifact))
        self.assertEqual(code, 0)
        self.commit()
        self.assertTrue(opened and not any("holdout" in name for name in opened))
        report_path = self.repo.parent / "a.json"
        before = self.main("e3a", "--json", str(report_path))
        report = report_path.read_text(encoding="utf-8")
        # A hold-out file that cannot even be parsed changes nothing before E3b.
        for folder in self.folders:
            (folder / "replays" / "body" / f"{HOLDOUT}.pending.json").write_text("{", encoding="utf-8")
        self.assertEqual(self.main("e3a", "--json", str(report_path)), before)
        self.assertEqual(report_path.read_text(encoding="utf-8"), report)
        self.e3b()

    def test_the_roles_come_from_run_json_and_the_case_table(self):
        folder = self.folders[0]
        self.assertEqual(set(runs.recording_paths(folder, "training")), set(TRAINING))
        self.assertEqual(set(runs.recording_paths(folder, "holdout")), {HOLDOUT})
        # A completed clip is read over its pending recording.
        self.assertEqual(runs.recording_paths(folder, "training")["body-clip-level"].name, "body-clip-level.json")
        with self.assertRaisesRegex(ValueError, "unknown role"):
            runs.recording_paths(folder, "all")
        # A file run.json does not record is not read, whatever its name.
        stray = folder / "replays" / "body" / "body-probes-tall.pending.json"
        stray.write_text("{", encoding="utf-8")
        self.assertEqual(set(runs.recording_paths(folder, "training")), set(TRAINING))
        # The hold-out's recording under a training case's name refuses.
        body_dir = folder / "replays" / "body"
        (body_dir / "body-probes-compact.pending.json").write_bytes((body_dir / f"{HOLDOUT}.pending.json").read_bytes())
        with self.assertRaisesRegex(ValueError, "records another case than body-probes-compact"):
            runs.load([folder], "training")
        write_run(folder, self.docs, VERSIONS[0])
        # The fit refuses a hold-out row however it arrives.
        held = runs.observe(runs.load([folder], "holdout"))
        trained = runs.observe(runs.load([folder], "training"))
        with self.assertRaisesRegex(ValueError, "refuses hold-out rows"):
            runs.Design(trained + held[:1])
        with mock.patch.object(runs, "role_of", return_value="training"), \
                self.assertRaisesRegex(ValueError, "read the wrong rig"):
            runs.load([folder], "training")

    def test_every_mode_needs_the_bundles(self):
        for mode in freeze.MODES:
            with self.subTest(mode=mode), self.assertRaisesRegex(ValueError, "give --unityrs"):
                freeze.run_mode(mode, self.folders, self.artifact)
        with self.assertRaisesRegex(ValueError, "give --unityrs"):
            freeze.run_mode("e4", self.folders, avatar_of=avatars)


class ReaderTests(unittest.TestCase):
    """The bundle reader: one view per run folder and digest, each bundle against the recording's pinned sha256."""

    def test_a_view_per_run_and_a_pinned_digest(self):
        root = scratch.for_case(self)
        made = []

        class View:
            def __init__(self, path, digest, *args):
                made.append((path, digest))

            def asset(self, name, class_id):
                return name, class_id

            def value(self, key):
                return key

        docs = {}
        for version in VERSIONS:
            bundles = root / version / "bundles"
            bundles.mkdir(parents=True)
            (bundles / "body-compact").write_bytes(version.encode())
            docs[version] = dict(spec=dict(bundles=[dict(name="body-compact")], avatar_asset="a", clip_asset="c"),
                                 capture=dict(bundle_sha256=[runs.hashlib.sha256(version.encode()).hexdigest()]))
        import anim_replay_complete as complete

        with mock.patch.object(complete, "BundleView", View), \
                mock.patch.object(complete, "read_manifest", return_value=(None, {})):
            avatar_of, clip_of = freeze.readers("cli")
            for version in VERSIONS:
                run = mock.Mock(folder=root / version)
                self.assertEqual(avatar_of(run, docs[version]), ("a", 90))
                self.assertEqual(clip_of(run, docs[version]), ("c", 74))
            # Two runs share the bundle's name, never its view.
            self.assertEqual(len(made), 2)
            self.assertNotEqual(made[0][0], made[1][0])
            with self.assertRaisesRegex(ValueError, "differs from the recording's pinned sha256"):
                avatar_of(mock.Mock(folder=root / VERSIONS[0]), docs[VERSIONS[1]])


if __name__ == "__main__":
    unittest.main()
