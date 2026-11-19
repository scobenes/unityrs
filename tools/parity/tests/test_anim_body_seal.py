"""B2's seal (anim_body_seal): the freeze's sources, their git binding, E3b's ledger and the hold-out's minimum.

The sources are every local module the entry points import, transitively;
E3b refuses an artifact that is not the one its freeze commit added, a code
commit that is not an ancestor of HEAD and of the freeze commit, or a source
changed since. E3b's end-to-end refusals are test_anim_body_freeze's."""

import json
import os
import sys
import unittest
from unittest import mock

import anim_body_model_support  # noqa: F401  (puts tools/parity on the path)

import anim_body_seal as seal  # noqa: E402  (test_anim_replay puts it on the path)
import anim_fixture_body as body  # noqa: E402
import child  # noqa: E402
import scratch  # noqa: E402
from anim_body_model import LATERAL_MUSCLES  # noqa: E402

GIT_VARIABLES = ("GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_COMMON_DIR")


def git(folder, *args):
    """git in a scratch repository, free of any inherited repository; its stdout."""
    env = {key: value for key, value in os.environ.items() if key not in GIT_VARIABLES}
    return child.run(["git", "-C", str(folder), "-c", "user.name=t", "-c", "user.email=t@example.invalid",
                      "-c", "commit.gpgsign=false", *args], env=env, capture_output=True, text=True,
                     check=True).stdout.strip()


class SourceTests(unittest.TestCase):
    def test_the_sources_are_every_local_import_transitively(self):
        folder = scratch.for_case(self)
        (folder / "a.py").write_text("import json\nimport b\n", encoding="utf-8")
        (folder / "b.py").write_text("def f():\n    from c import g\n", encoding="utf-8")
        (folder / "c.py").write_text("import os.path\n", encoding="utf-8")
        (folder / "unused.py").write_text("", encoding="utf-8")
        self.assertEqual(seal.sources(folder, ("a.py",)), ["a.py", "b.py", "c.py"])
        # A new import, at any depth, is a source at once, and changes the digest.
        before = seal.source_digest(folder, seal.sources(folder, ("a.py",)))
        (folder / "c.py").write_text("import os.path\nfrom unused import x\n", encoding="utf-8")
        self.assertEqual(seal.sources(folder, ("a.py",)), ["a.py", "b.py", "c.py", "unused.py"])
        self.assertNotEqual(seal.source_digest(folder, seal.sources(folder, ("a.py",))), before)
        # So is a data file of the folder a source names, but never the freeze artifact.
        (folder / "table.json").write_text("{}", encoding="utf-8")
        (folder / seal.FREEZE_PATH.name).write_text("{}", encoding="utf-8")
        (folder / "b.py").write_text(f"T = 'table.json'\nF = '{seal.FREEZE_PATH.name}'\nG = 'missing.json'\n"
                                     "def f():\n    from c import g\n", encoding="utf-8")
        self.assertEqual(seal.sources(folder, ("a.py",)), ["a.py", "b.py", "c.py", "table.json", "unused.py"])

    def test_the_freeze_sources_hold_validation_cases_and_decoding(self):
        files = seal.sources()
        for name in ("anim_body_freeze.py", "anim_body_check.py", "anim_body_seal.py", "anim_replay.py",
                     "anim_fixture_contract.py", "anim_replay_complete.py", "anim_fixture_body.py",
                     "anim_body_runs.py", "anim_body_model.py", "anim_replay.schema.json", "anim_bounds.json"):
            self.assertIn(name, files)
        # Every module of this folder that the checker, E3b and completion load is one of them.
        code = ("import sys; sys.path.insert(0, sys.argv[1]); import anim_body_freeze as f, anim_body_seal as s; "
                "f.completion(); import os; print('\\n'.join(sorted({os.path.basename(m.__file__) for m in "
                "list(sys.modules.values()) if getattr(m, '__file__', None) and os.path.dirname(m.__file__) == "
                "sys.argv[1]})))")
        loaded = child.run([sys.executable, "-I", "-c", code, str(seal.HERE)], capture_output=True, text=True,
                           check=True).stdout.split()
        self.assertIn("anim_replay_complete.py", loaded)
        self.assertLessEqual(set(loaded), set(files))


class BindingTests(unittest.TestCase):
    def setUp(self):
        self.repo = scratch.for_case(self)
        git(self.repo, "init", "-q")
        for name, text in (("a.py", "import b\n"), ("b.py", "X = 1\n")):
            (self.repo / name).write_text(text, encoding="utf-8")
        patches = (mock.patch.object(seal, "ENTRY_POINTS", ("a.py",)),
                   mock.patch.dict(os.environ, {key: "" for key in GIT_VARIABLES}))
        for patch in patches:
            patch.start()
            self.addCleanup(patch.stop)
        for key in GIT_VARIABLES:
            del os.environ[key]
        self.artifact = self.repo / "freeze.json"

    def code(self):
        commit, files, digest = seal.code_state(self.repo)
        return dict(commit=commit, files=files, digest=digest)

    def test_the_freeze_records_committed_unchanged_sources(self):
        with self.assertRaisesRegex(ValueError, "run it in a checkout"):
            seal.code_state(self.repo)
        git(self.repo, "add", "a.py")
        git(self.repo, "commit", "-q", "-m", "a")
        with self.assertRaisesRegex(ValueError, "not tracked by git"):
            seal.code_state(self.repo)
        git(self.repo, "add", "b.py")
        git(self.repo, "commit", "-q", "-m", "b")
        (self.repo / "b.py").write_text("X = 2\n", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "commit the code first"):
            seal.code_state(self.repo)
        git(self.repo, "checkout", "-q", "--", "b.py")
        code = self.code()
        self.assertEqual(code["commit"], git(self.repo, "rev-parse", "HEAD"))
        self.assertEqual(code["files"], ["a.py", "b.py"])
        seal.bind_code(code, self.repo)

    def test_e3b_binds_the_code_commit_and_the_sources_since(self):
        git(self.repo, "add", ".")
        git(self.repo, "commit", "-q", "-m", "code")
        code = self.code()
        # A later commit that leaves the sources alone keeps the binding.
        (self.repo / "notes.txt").write_text("n\n", encoding="utf-8")
        git(self.repo, "add", "notes.txt")
        git(self.repo, "commit", "-q", "-m", "notes")
        seal.bind_code(code, self.repo)
        # A source changed in the work tree, or committed since.
        (self.repo / "b.py").write_text("X = 2\n", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "changed since the freeze's code commit"):
            seal.bind_code(code, self.repo)
        git(self.repo, "commit", "-q", "-am", "changed")
        with self.assertRaisesRegex(ValueError, "changed since the freeze's code commit"):
            seal.bind_code(code, self.repo)
        # A code commit HEAD does not descend from.
        git(self.repo, "checkout", "-q", "-b", "other", "HEAD~2")
        (self.repo / "c.txt").write_text("c\n", encoding="utf-8")
        git(self.repo, "add", "c.txt")
        git(self.repo, "commit", "-q", "-m", "other")
        elsewhere = dict(code, commit=git(self.repo, "rev-parse", "HEAD"))
        git(self.repo, "checkout", "-q", "-")
        with self.assertRaisesRegex(ValueError, "not an ancestor of HEAD"):
            seal.bind_code(elsewhere, self.repo)
        # Another import list, or another digest.
        with self.assertRaisesRegex(ValueError, "source files are not the code's"):
            seal.bind_code(dict(code, files=["a.py"]), self.repo)

    def test_e3b_binds_the_artifact_to_its_one_freeze_commit_after_the_code(self):
        git(self.repo, "add", ".")
        git(self.repo, "commit", "-q", "-m", "code")
        commit = self.code()["commit"]
        self.artifact.write_text("{}\n", encoding="utf-8")
        git(self.repo, "add", "freeze.json")
        git(self.repo, "commit", "-q", "-m", "freeze")
        frozen = git(self.repo, "rev-parse", "HEAD")
        self.assertEqual(seal.bind_artifact(self.artifact, commit), frozen)
        # The freeze commit is not after the code commit.
        with self.assertRaisesRegex(ValueError, "does not precede its freeze commit"):
            seal.bind_artifact(self.artifact, frozen)
        # A replacement committed later: the file is HEAD's, but not the freeze commit's.
        self.artifact.write_text('{"x": 1}\n', encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "differs from its committed version"):
            seal.bind_artifact(self.artifact, commit)
        git(self.repo, "commit", "-q", "-am", "replaced")
        with self.assertRaisesRegex(ValueError, "changed after its freeze commit"):
            seal.bind_artifact(self.artifact, commit)
        # HEAD on a branch without the freeze commit: untracked there.
        git(self.repo, "checkout", "-q", "-b", "before", commit)
        self.artifact.write_text("{}\n", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "not tracked by git"):
            seal.bind_artifact(self.artifact, commit)


class LedgerTests(unittest.TestCase):
    def test_one_claim_per_freeze_and_appended_rows(self):
        folder = scratch.for_case(self) / "ledger"
        seal.claim("a" * 64, folder)
        seal.claim("b" * 64, folder)
        with self.assertRaisesRegex(ValueError, "already ran on this freeze"):
            seal.claim("a" * 64, folder)
        self.assertEqual(sorted(path.name for path in folder.iterdir()), [f"{'a' * 64}.claim", f"{'b' * 64}.claim"])
        seal.record("a" * 64, dict(event="claimed"), folder)
        seal.record("a" * 64, dict(event="failed", error="x"), folder)
        rows = [json.loads(line) for line in (folder / "ledger.jsonl").read_text(encoding="utf-8").splitlines()]
        self.assertEqual([(row["freeze_sha256"], row["event"]) for row in rows], [("a" * 64, "claimed"),
                                                                                  ("a" * 64, "failed")])

    def test_the_ledger_and_the_artifact_are_fixed_paths(self):
        self.assertEqual(seal.FREEZE_PATH.parent, seal.HERE)
        # Under the account's home from its passwd entry, whatever $HOME says.
        import pwd

        self.assertTrue(seal.LEDGER_DIR.is_relative_to(pwd.getpwuid(os.getuid()).pw_dir))
        self.assertTrue(seal.LEDGER_DIR.is_absolute())
        # Outside the repository.
        self.assertFalse(seal.LEDGER_DIR.is_relative_to(seal.HERE.parents[1]))


class ReaderTests(unittest.TestCase):
    def test_the_cli_resolves_once_to_an_absolute_real_path(self):
        folder = scratch.for_case(self)
        (folder / "real").write_bytes(b"cli")
        (folder / "link").symlink_to(folder / "real")
        resolved = seal.executable(str(folder / "link"))
        self.assertEqual(resolved, str((folder / "real").resolve()))
        self.assertEqual(seal.reader_sha256(resolved), seal.hashlib.sha256(b"cli").hexdigest())
        with self.assertRaisesRegex(ValueError, "not resolved"):
            seal.reader_sha256("real")
        with self.assertRaisesRegex(ValueError, "no unityrs executable"):
            seal.executable(str(folder / "absent"))


class ReaderEntryTests(unittest.TestCase):
    """Every single-file reader refuses a pending or hold-out file by its name, before opening it."""

    def test_validate_and_budgets_never_open_a_sealed_or_pending_file(self):
        import anim_replay as replay
        import anim_replay_budgets as budgets

        folder = scratch.for_case(self)
        case = seal.HOLDOUT_CASES[0]
        names = {f"{case}.pending.json": "sealed hold-out", f"{case}.json": "sealed hold-out",
                 body.BODY_CASES[case].bundle: "sealed hold-out", "body-probes-compact.pending.json": "pending replay"}
        (folder / "link.json").symlink_to(folder / f"{case}.pending.json")
        names["link.json"] = "sealed hold-out"
        for name, message in names.items():
            path = folder / name
            if not path.is_symlink():
                path.write_text("{}", encoding="utf-8")
            argvs = [["anim_replay.py", mode, str(path)] for mode in ("validate", "validate-spec", "derive",
                                                                       "capture-id")]
            argvs.append(["anim_replay_budgets.py", str(path), "--show-dir", str(folder)])
            for argv in argvs:
                with self.subTest(name=name, argv=argv[:2]), mock.patch.object(sys, "argv", argv), \
                        mock.patch("pathlib.Path.read_text", side_effect=AssertionError("opened")), \
                        mock.patch("pathlib.Path.read_bytes", side_effect=AssertionError("opened")), \
                        mock.patch("builtins.open", side_effect=AssertionError("opened")), \
                        mock.patch("sys.stderr") as stderr:
                    with self.assertRaises(SystemExit) as exit:
                        (replay if argv[0] == "anim_replay.py" else budgets).main()
                    self.assertEqual(exit.exception.code, 1)
                    self.assertIn(message, "".join(call.args[0] for call in stderr.write.call_args_list))


class MinimumTests(unittest.TestCase):
    def test_the_minimum_comes_from_the_hold_out_design(self):
        self.assertEqual(seal.HOLDOUT_CASES, ("body-probes-holdout",))
        names = [muscle for _, group in body.PROBE_GROUPS for muscle in group]
        points = body.probe_schedule(body.HOLDOUT, names, False)
        lateral = [names.index(muscle) for muscle in LATERAL_MUSCLES]
        refused = [names.index(muscle) for muscle in seal.FRAME_REFUSED]
        wide = [vector for _, vector in points if max(abs(vector[i]) for i in lateral) >= .25]
        kept = [vector for _, vector in points if max(abs(vector[i]) for i in lateral) == 0
                and not any(vector[i] for i in refused)]
        self.assertEqual((len(points), len(wide), len(kept)), (45, 7, seal.IN_SCOPE_BY_DESIGN))
        self.assertEqual(seal.MIN_COMPARISONS, seal.IN_SCOPE_BY_DESIGN - 3)


if __name__ == "__main__":
    unittest.main()
