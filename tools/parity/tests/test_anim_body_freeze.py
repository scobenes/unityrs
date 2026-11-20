"""B2's order of evidence: E3a on the training rigs, the freeze, then E3b on the hold-out rig against it.

The hold-out stays out of everything before E3b by construction; the freeze
needs both Editor versions, every required case, E4, full rank and |a| <= 1;
E3b refuses an artifact git does not bind, or whose E3a, E4, model, bounds,
sources, fit or lateral limit are not the code's and the training runs',
takes only the canonical artifact and the frozen unityrs CLI, checks the
hold-out by name before claiming its one shot (an exclusive marker, then
ledger rows; any later failure keeps it consumed), reads the hold-out only
through sealed completion, and evaluates each Editor version alone with
the stored coefficients. The git binding itself is test_anim_body_seal's."""

import contextlib
import copy
import io
import json
import os
import pathlib
import unittest
from unittest import mock

from anim_body_model_support import BITS, CLIPS, F32, avatar, planned_margins, recording, write_run

import anim_body_freeze as freeze  # noqa: E402  (test_anim_replay puts it on the path)
import anim_body_holdout as holdout  # noqa: E402
import anim_body_runs as runs  # noqa: E402
import anim_body_seal as seal  # noqa: E402
import anim_replay as replay  # noqa: E402
import anim_fixture_body as body  # noqa: E402
import child  # noqa: E402
import scratch  # noqa: E402

# A training set whose design has full rank, as the freeze needs.
TRAINING = ("body-probes-compact", "body-targets-tall", "body-clip-level", "body-chain-hips-scale-targets",
            "body-probes-noncoplanar", "body-probes-no-upper-chest")
HOLDOUT = body.primary_probes(body.HOLDOUT)
VERSIONS = ("2022.3.62f3", "6000.6.9f1")
GIT_VARIABLES = ("GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_COMMON_DIR")


def avatars(run, doc):
    return avatar(doc)


def clips(run, doc):
    return CLIPS[doc["spec"]["evidence"]["editor"]["fixture_case"]]


def sealed_completion(folder, unityrs, inspect_dir, replay_module, unseal):
    """Completion's unseal (anim_replay_complete.complete_folder, test_anim_body_fixtures checks it): the claim
    checked, then each recorded hold-out recording with the sha256 of its bytes."""
    seal.require_claim(unseal)
    unsealed = []
    if runs.statuses(json.loads((folder / "run.json").read_text(encoding="utf-8"))).get(HOLDOUT) == "recorded":
        data = (folder / "replays" / "body" / f"{HOLDOUT}.pending.json").read_bytes()
        unsealed.append((HOLDOUT, runs.hashlib.sha256(data).hexdigest(), replay.read_json(data.decode("utf-8"))))
    return 0, [], unsealed


def git(folder, *args):
    """git in a scratch repository, free of any inherited repository."""
    env = {key: value for key, value in os.environ.items() if key not in GIT_VARIABLES}
    return child.run(["git", "-C", str(folder), "-c", "user.name=t", "-c", "user.email=t@example.invalid",
                      "-c", "commit.gpgsign=false", *args], env=env, capture_output=True, text=True, check=True)


class FreezeTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.docs = {name: recording(name, samples=None if name == "body-probes-compact" else 12) for name in (*TRAINING, HOLDOUT)}

    def setUp(self):
        root = scratch.for_case(self)
        self.folders = [write_run(root / version, self.docs, version) for version in VERSIONS]
        # The hold-out's bundle by name (E3b's preflight lists it; the fake completion never reads it).
        for folder in self.folders:
            (folder / "bundles").mkdir(exist_ok=True)
            (folder / "bundles" / body.BODY_CASES[HOLDOUT].bundle).write_bytes(b"")
        self.repo = root / "repo"
        self.repo.mkdir()
        git(self.repo, "init", "-q")
        # The code commit precedes the freeze commit, in the one checkout.
        (self.repo / "code.py").write_text("", encoding="utf-8")
        git(self.repo, "add", "code.py")
        git(self.repo, "commit", "-q", "-m", "code")
        self.code_commit = git(self.repo, "rev-parse", "HEAD").stdout.strip()
        self.artifact = self.repo / "freeze.json"
        self.ledger_dir = root / "ledger"
        self.ledger = self.ledger_dir / "ledger.jsonl"
        # The canonical artifact is the scratch repository's here; the checkout's own one, if committed, is then
        # just another file the sources name.
        self.real_path = seal.FREEZE_PATH
        canonical = mock.patch.object(seal, "FREEZE_PATH", self.artifact)
        canonical.start()
        self.addCleanup(canonical.stop)
        # The code's own git binding (test_anim_body_seal) is the checkout's: here, a commit that holds it.
        code = (self.code_commit, seal.sources(), seal.source_digest())
        patches = (mock.patch.object(runs, "REQUIRED", frozenset(TRAINING)),
                   mock.patch.dict(os.environ, {key: "" for key in GIT_VARIABLES}),
                   mock.patch.object(seal, "code_state", return_value=code),
                   mock.patch.object(seal, "code_history"),
                   mock.patch.object(seal, "MIN_COMPARISONS", 1),
                   mock.patch.object(seal, "LEDGER_DIR", self.ledger_dir),
                   mock.patch.object(seal, "reader_sha256", return_value="5" * 64),
                   mock.patch.object(seal, "executable", side_effect=lambda unityrs: unityrs),
                   mock.patch.object(seal, "code_repo", return_value=self.repo.resolve()),
                   mock.patch.object(holdout, "predicted", planned_margins()),
                   mock.patch.object(freeze, "completion", return_value=sealed_completion))
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

    def commit(self, *, amend=False):
        git(self.repo, "add", "freeze.json")
        git(self.repo, "commit", "-q", "-m", "freeze", *["--amend"] * amend)

    def frozen(self):
        self.assertEqual(self.main("freeze", "--freeze", str(self.artifact))[0], 0)
        self.commit()
        return json.loads(self.artifact.read_text(encoding="utf-8"))

    def rewrite(self, artifact, *, commit=True):
        """The artifact changed and, with `commit`, in its one freeze commit."""
        self.artifact.write_text(json.dumps(artifact), encoding="utf-8")
        if commit:
            self.commit(amend=True)

    def rows(self):
        """The ledger's rows."""
        return [json.loads(line) for line in self.ledger.read_text(encoding="utf-8").splitlines()]

    def claimed(self):
        return self.ledger_dir.is_dir() and any(self.ledger_dir.glob("*.claim"))

    def unclaim(self):
        """A test's fresh shot: the ledger directory emptied (a scratch one here)."""
        for path in self.ledger_dir.iterdir():
            path.unlink()

    def e3b(self, message=None, code=2, *args):
        found, text = self.main("e3b", "--freeze", str(self.artifact), *args)
        self.assertEqual(found, code, text)
        if message:
            self.assertIn(message, text)
        return text

    def test_the_freeze_and_e3b_need_the_predicted_hold_out_margin(self):
        short = "2022.3.62f3/body-probes-holdout-2@0.25: predicted u_s 2e-05 against E_unity,s 2e-05, margin 1 under 1.25"
        with mock.patch.object(holdout, "predicted", planned_margins([short])):
            code, text = self.main("e3a")
            self.assertEqual(code, 1, text)
            self.assertIn(short, text)
            self.assertEqual(self.main("freeze", "--freeze", str(self.artifact))[0], 2)
        self.assertFalse(self.artifact.exists())
        calls = []
        passing = planned_margins()

        def spy(runs, *args, **kwargs):
            calls.append(kwargs)
            return passing(runs)
        with mock.patch.object(holdout, "predicted", spy):
            artifact = self.frozen()
        # With E3b's filters: the lateral limit E3a sets, and E3b's minimum of in-scope comparisons.
        self.assertEqual(calls, [dict(limit=artifact["scope"]["lateral_limit"], minimum=seal.MIN_COMPARISONS)])
        self.assertEqual(artifact["e3a"]["holdout_plan"], {version: dict(worst_margin=2., in_frame=1, refused=0,
                                                                         minimum=holdout.HOLDOUT_MARGIN)
                                                           for version in VERSIONS})
        # E3b predicts again before it claims anything: a short prediction leaves the shot unspent.
        with mock.patch.object(holdout, "predicted", planned_margins([short])), \
                mock.patch.object(freeze, "completion", return_value=mock.Mock(side_effect=AssertionError("unsealed"))):
            self.e3b("the hold-out is predicted short of its margin")
        self.assertFalse(self.claimed() or self.ledger.exists())
        # An artifact that records no margin of at least the code's is refused.
        self.rewrite(dict(artifact, e3a=dict(artifact["e3a"], holdout_plan={})))
        self.e3b("no predicted hold-out margin")
        self.assertFalse(self.claimed())

    def test_the_next_freeze_writes_the_canonical_path_and_v7_is_spent(self):
        # The checkout's canonical path is the next version's, absent until that freeze writes it; the spent v7
        # stays tracked beside it.
        self.assertEqual(self.real_path.name, "anim_body_freeze_v8.json")
        self.assertTrue((seal.HERE / "anim_body_freeze_v7.json").is_file())
        self.assertFalse(self.artifact.exists())
        self.assertEqual(self.main("freeze", "--freeze", str(self.artifact))[0], 0)
        self.assertTrue(self.artifact.is_file())
        self.assertEqual(self.main("freeze", "--freeze", str(self.artifact))[0], 2)
        # E3b refuses v7, before any claim: it is not the canonical artifact.
        with mock.patch.object(seal, "FREEZE_PATH", self.real_path):
            self.e3b("E3b reads only the canonical freeze artifact anim_body_freeze_v8.json", 2)
            code, text = self.main("e3b", "--freeze", str(seal.HERE / "anim_body_freeze_v7.json"))
        self.assertEqual(code, 2)
        self.assertIn("E3b reads only the canonical freeze artifact anim_body_freeze_v8.json", text)
        self.assertFalse(self.claimed())

    def test_e3b_refuses_without_the_canonical_artifact(self):
        code, text = self.main("e3b")
        self.assertEqual(code, 2)
        self.assertIn("only the canonical freeze artifact", text)
        self.e3b("no freeze artifact")
        self.assertFalse(self.artifact.exists() or self.claimed())
        # A copy committed under another path, however valid, is not the freeze.
        self.frozen()
        other = self.repo / "other.json"
        other.write_bytes(self.artifact.read_bytes())
        git(self.repo, "add", "other.json")
        git(self.repo, "commit", "-q", "-m", "copy")
        code, text = self.main("e3b", "--freeze", str(other))
        self.assertIn("only the canonical freeze artifact", text)
        # A symlink at the canonical path, to that valid copy.
        self.artifact.unlink()
        self.artifact.symlink_to(other)
        self.e3b("the freeze path holds a symlink")
        self.artifact.unlink()
        git(self.repo, "checkout", "-q", "--", "freeze.json")
        # Another checkout's artifact.
        with mock.patch.object(seal, "code_repo", return_value=self.repo.parent):
            self.e3b("not in this code's checkout")
        # The frozen unityrs CLI, and the code's Editor versions, hold-out cases and minimum.
        with mock.patch.object(seal, "reader_sha256", return_value="6" * 64):
            self.e3b("the unityrs CLI is not the frozen one")
        with mock.patch.object(seal, "MIN_COMPARISONS", 2):
            self.e3b("hold-out cases or minimum are not the code's")
        with mock.patch.object(seal, "HOLDOUT_CASES", ()):
            self.e3b("hold-out cases or minimum are not the code's")
        with mock.patch.object(freeze, "EDITORS", ("2022", "6000", "2023")):
            self.e3b("frozen Editor versions are not the code's")
        self.assertFalse(self.claimed())

    def test_e3b_claims_its_one_shot_before_opening_the_hold_out(self):
        artifact = self.frozen()
        complete, seen = sealed_completion, []

        def spy(folder, *args):
            seen.append((self.claimed(), [row["event"] for row in self.rows()]))
            return complete(folder, *args)

        with mock.patch.object(freeze, "completion", return_value=spy):
            self.e3b(code=0)
        # Claimed, with its row, before completion opened either hold-out.
        self.assertEqual(seen, [(True, ["claimed"])] * 2)
        [claimed, evaluated] = self.rows()
        sha = runs.hashlib.sha256(self.artifact.read_bytes()).hexdigest()
        self.assertEqual([path.name for path in self.ledger_dir.glob("*.claim")], [f"{sha}.claim"])
        self.assertEqual({row["freeze_sha256"] for row in self.rows()}, {sha})
        self.assertEqual((claimed["freeze_commit"], claimed["code_commit"], claimed["training"], claimed["reader"]),
                         (git(self.repo, "rev-parse", "HEAD").stdout.strip(), artifact["code"]["commit"],
                          artifact["training"]["digest"], "5" * 64))
        self.assertEqual(claimed["files"], {version: {name: runs.hashlib.sha256((folder / path).read_bytes()).hexdigest()
                                                      for name, path in (("run.json", "run.json"), (HOLDOUT, (
                                                          f"replays/body/{HOLDOUT}.pending.json")))}
                                            for version, folder in zip(VERSIONS, self.folders)})
        self.assertEqual((evaluated["event"], evaluated["passed"]), ("evaluated", True))
        self.assertRegex(claimed["time"], r"^\d{4}-\d\d-\d\dT")
        # Once: the same freeze again refuses before any hold-out file opens.
        with mock.patch.object(freeze, "completion", return_value=mock.Mock(side_effect=AssertionError("unsealed"))):
            self.e3b("E3b already ran on this freeze")
        self.assertEqual(len(self.rows()), 2)
        # A run changed between the claim and completion fails closed, recorded.
        self.unclaim()

        def swapped(folder, *args):
            recording = folder / "replays" / "body" / f"{HOLDOUT}.pending.json"
            recording.write_bytes(recording.read_bytes() + b" ")
            return complete(folder, *args)

        with mock.patch.object(freeze, "completion", return_value=swapped):
            self.e3b("the run changed after the claim")
        self.assertEqual([row["event"] for row in self.rows()], ["claimed", "failed"])
        # A failure after the claim is recorded and keeps the shot consumed.
        self.unclaim()
        for folder in self.folders:
            (folder / "replays" / "body" / f"{HOLDOUT}.pending.json").write_text("{", encoding="utf-8")
        self.e3b("Expecting")
        self.assertEqual([row["event"] for row in self.rows()], ["claimed", "failed"])
        self.e3b("E3b already ran on this freeze")

    def test_two_claims_on_one_freeze_cannot_both_succeed(self):
        seal.claim("a" * 64, self.ledger_dir)
        seal.claim("b" * 64, self.ledger_dir)
        with self.assertRaisesRegex(ValueError, "already ran on this freeze"):
            seal.claim("a" * 64, self.ledger_dir)

    def test_e3b_needs_each_version_with_every_hold_out_case_and_its_own_comparisons(self):
        limit = self.frozen()["scope"]["lateral_limit"]
        # A missing Editor version.
        found, text = self.main("e3b", "--freeze", str(self.artifact), folders=self.folders[1:])
        self.assertEqual(found, 2)
        self.assertIn("training inputs differ from the frozen ones", text)
        # A version without the hold-out case, or without its file, refuses by names alone, before the claim.
        write_run(self.folders[1], {name: doc for name, doc in self.docs.items() if name != HOLDOUT}, VERSIONS[1])
        self.e3b(f"{VERSIONS[1]}: hold-out case(s) ['{HOLDOUT}'] not recorded")
        write_run(self.folders[1], self.docs, VERSIONS[1])
        recording = self.folders[0] / "replays" / "body" / f"{HOLDOUT}.pending.json"
        recording.rename(recording.with_name("moved"))
        self.e3b(f"{VERSIONS[0]}: hold-out case(s) ['{HOLDOUT}'] not recorded")
        recording.with_name("moved").rename(recording)
        self.assertFalse(self.claimed())
        # A version whose every hold-out sample its scope refuses (here past the lateral limit) fails alone,
        # however the other version passes.
        docs = copy.deepcopy(self.docs)
        capture = docs[HOLDOUT]["capture"]
        lateral = [row["name"] for row in capture["humanoid"]["muscles"]].index("Spine Left-Right")
        for sample in capture["samples"]:
            sample["muscles_f32"][lateral] = BITS(limit + 1 / 32)
        write_run(self.folders[0], docs, VERSIONS[0])
        text = self.e3b(code=1)
        self.assertIn(f"{VERSIONS[0]}: 0 in-scope comparison(s), under the minimum 1", text)
        self.assertIn(f"{VERSIONS[0]}: E3: no comparison", text)
        self.assertNotIn(VERSIONS[1], text)

    def test_e3b_fails_each_version_under_the_design_minimum(self):
        minimum = seal.IN_SCOPE_BY_DESIGN - 3
        with mock.patch.object(seal, "MIN_COMPARISONS", minimum):
            self.assertEqual(self.frozen()["scope"]["holdout_minimum"], minimum)
            text = self.e3b(code=1)
        self.assertIn("2 failure(s)", text)
        for version in VERSIONS:
            self.assertRegex(text, f"{version}: [0-9]+ in-scope comparison\\(s\\), under the minimum {minimum}")

    def test_the_freeze_then_the_hold_out(self):
        artifact = self.frozen()
        self.assertEqual(artifact["format"], freeze.FORMAT)
        self.assertEqual(set(artifact), {"format", "version", "model", "coefficients", "scope", "bounds", "training",
                                         "e3a", "e4", "code", "reader", "evidence"})
        self.assertEqual(artifact["reader"]["sha256"], "5" * 64)
        self.assertEqual((artifact["scope"]["holdout_cases"], artifact["scope"]["holdout_minimum"]), ([HOLDOUT], 1))
        self.assertEqual(artifact["scope"]["refusals"], freeze.REFUSALS)
        self.assertEqual(set(freeze.REFUSALS), {"topology", "frame", "spine_lateral_range"})
        self.assertEqual(artifact["evidence"]["runs"], [])
        self.assertEqual(artifact["model"], freeze.HYPOTHESES)
        self.assertTrue(artifact["e3a"]["passed"])
        self.assertEqual(artifact["code"], dict(commit=self.code_commit, digest=seal.source_digest(),
                                                files=seal.sources()))
        # T and the observations that bracket it: the passing sample at T, no failing one here.
        lateral = artifact["scope"]["lateral"]
        self.assertEqual((lateral["limit"], lateral["failing"]), (artifact["scope"]["lateral_limit"], None))
        self.assertEqual(lateral["passing"]["value"], lateral["limit"])
        # E4 per configuration, naming the passing ones.
        self.assertEqual(artifact["e4"]["passing"], [f"level/compact/{version}/identity" for version in VERSIONS])
        self.assertEqual(artifact["e4"]["excluded"], [])
        self.assertEqual(artifact["scope"]["editor_versions"], list(VERSIONS))
        cases = {row["case"] for row in artifact["training"]["inputs"]}
        self.assertEqual(cases, set(TRAINING))
        self.assertEqual(artifact["training"]["digest"], runs.digest(artifact["training"]["inputs"]))
        self.assertEqual(artifact["scope"]["holdout"], body.HOLDOUT)
        self.assertNotIn(body.HOLDOUT, artifact["scope"]["training_rigs"])
        self.assertEqual(artifact["bounds"]["condition_limit"], 1e6)
        report_path = self.repo.parent / "r.json"
        self.assertEqual(self.e3b(None, 0, "--json", str(report_path)).strip(), "e3b: pass")
        report = json.loads(report_path.read_text(encoding="utf-8"))
        # Every hold-out sample of each version is checked or, twisted past 2.2's scope, refused; and its
        # Avatar passes E0 against the training table.
        self.assertEqual(sorted(report["versions"]), list(VERSIONS))
        for entry in report["versions"].values():
            self.assertEqual(entry["samples"] + entry["refused"], len(self.docs[HOLDOUT]["capture"]["samples"]))
            self.assertGreater(entry["samples"], entry["refused"])
            self.assertEqual(len(entry["avatars"]), 1)
        self.assertEqual(report["digest"], artifact["training"]["digest"])
        # A freeze is made once, and never over its report.
        code, text = self.main("freeze", "--freeze", str(self.artifact))
        self.assertEqual(code, 2)
        self.assertIn("new artifact", text)
        self.e3b("--json and --freeze name one file", 2, "--json", str(self.artifact))

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
        # A replacement committed after the freeze commit.
        self.commit()
        self.e3b("changed after its freeze commit")
        # Or removed and added again: no longer the one commit that added it.
        git(self.repo, "rm", "-q", "freeze.json")
        git(self.repo, "commit", "-q", "-m", "gone")
        self.rewrite(artifact, commit=False)
        self.commit()
        self.e3b("not added by exactly one commit")
        git(self.repo, "rm", "-q", "--cached", "freeze.json")
        self.e3b("not tracked by git")
        self.assertFalse(self.claimed())

    def test_e3b_refuses_a_changed_artifact_or_training_set(self):
        artifact = self.frozen()
        key = sorted(artifact["coefficients"])[0]
        for edit, message in ((lambda a: a["training"].update(digest="0" * 64), "digest disagrees"),
                              (lambda a: a["model"].update(frame="another frame"), "model choices differ"),
                              (lambda a: a.update(version=1), "expected unityrs.anim_body_freeze"),
                              (lambda a: a["e3a"].update(passed=False), "no passed E3a"),
                              (lambda a: a["bounds"].update(fold_depth=49), "bounds differ"),
                              (lambda a: a["code"].update(digest="0" * 64), "sources differ from the frozen"),
                              (lambda a: a["code"]["files"].pop(), "source files are not the code's"),
                              (lambda a: a["e4"].update(passing=[]), "no passed E4"),
                              (lambda a: a["bounds"].update(rank=1), "rank or condition differs"),
                              (lambda a: a["scope"].update(lateral_limit="0.1"), "malformed frozen lateral limit"),
                              (lambda a: a["scope"].update(lateral_limit=.0625), "malformed frozen lateral limit"),
                              (lambda a: (a["scope"].update(lateral_limit=.0625),
                                          a["scope"]["lateral"].update(limit=.0625)), "lateral limit differs"),
                              (lambda a: a["scope"]["lateral"].update(passing=None), "lateral limit differs"),
                              (lambda a: a["coefficients"].popitem(), "do not match the training design"),
                              # One coefficient tampered and committed: not the training fit.
                              (lambda a: a["coefficients"].update({key: a["coefficients"][key] + 1e-9}),
                               "not the training fit")):
            with self.subTest(message=message):
                changed = copy.deepcopy(artifact)
                edit(changed)
                self.rewrite(changed)
                self.e3b(message)
        # The code's sources changed after the freeze: another import, or another file's bytes.
        self.rewrite(artifact)
        with mock.patch.object(seal, "sources", return_value=seal.sources()[1:]):
            self.e3b("source files are not the code's")
        with mock.patch.object(seal, "source_digest", return_value="0" * 64):
            self.e3b("sources differ from the frozen ones")
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
        # The frozen training rows give the frozen limit once more, then each version's hold-out compares within it.
        self.assertEqual(seen[-2:], [(artifact["coefficients"], artifact["scope"]["lateral_limit"])] * 2)
        # One hold-out observation moved: e3b fails on that sample alone.
        docs = copy.deepcopy(self.docs)
        doc = docs[HOLDOUT]
        hips = next(row["node"] for row in doc["capture"]["humanoid"]["bones"] if row["name"] == "Hips")
        pose = doc["capture"]["samples"][0]["pose"]["transforms"][hips]
        pose["translation"][1] = BITS(F32(pose["translation"][1]) + 1e-3)
        write_run(self.folders[0], docs, VERSIONS[0])
        self.unclaim()
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
        # E3b reads it only through sealed completion, after its claim.
        self.e3b("Expecting")
        self.assertEqual([row["event"] for row in self.rows()], ["claimed", "failed"])

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
        # No load reads a hold-out file: only sealed completion hands one over.
        with self.assertRaisesRegex(ValueError, "only through sealed completion"):
            runs.load([folder], "holdout")
        # The fit refuses a hold-out row however it arrives.
        data = (body_dir / f"{HOLDOUT}.pending.json").read_bytes()
        held = runs.Run.sealed(folder, VERSIONS[0], {HOLDOUT: ("0" * 64, replay.read_json(data.decode("utf-8")))})
        held = runs.observe([held])
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
