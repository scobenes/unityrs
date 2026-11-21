"""Round 12's seal (anim_twist_seal): the freeze record with its pinned hooks and reader, the one claim per intake
digest and its recovery, and the verdicts behind the allowlist: B5a computed and recorded on its own before
twist, the `none` path keeps B5a, any sealed twist failure refuses all twist, and a second claim, a wrong intake
or bytes changed after the claim fail closed."""

import hashlib
import inspect
import io
import itertools
import json
import sys
import tarfile
import unittest
from pathlib import Path
from unittest import mock

import test_anim_replay_coverage  # noqa: F401  (puts tools/parity on the path)

import anim_body_seal  # noqa: E402
import anim_fixture_round12 as round12  # noqa: E402
import anim_twist_b5a as b5a  # noqa: E402
import anim_twist_model as model  # noqa: E402
import anim_twist_seal as seal  # noqa: E402
import scratch  # noqa: E402
from test_anim_fixture_round12 import (EXPLORATION, TWIN, TWIST_CLIP, VERSIONS, Runs, claimed, readers, rows,  # noqa: E402
                                      write_json)
from test_anim_twist_b5a import observation  # noqa: E402

CANDIDATE = dict(limb=list(model.LIMBS[0]), foot=list(model.FEET[0]))
HOOK_MODULE = "round12_test_hooks"
HOOKS = dict(observe_b5a=f"{HOOK_MODULE}:observe", twist_rule=f"{HOOK_MODULE}:rule")
# The hook module the tests freeze: it forwards to the running test (CURRENT), as a frozen module would compute.
HOOK_SOURCE = f"""import sys


def observe(views, unityrs):
    return sys.modules[{__name__!r}].CURRENT.observer(views, unityrs)


def rule(candidate, views, unityrs):
    return sys.modules[{__name__!r}].CURRENT.rule(candidate, views, unityrs)
"""
CURRENT = None


def ledger(path):
    return [json.loads(line) for line in (path / "ledger.jsonl").read_text(encoding="utf-8").splitlines()]


FREEZES = itertools.count()


def bound(top, code, digest, candidate="none", hooks=None):
    """A `Bound` for a freeze of `digest` pinning `code` in checkout `top`, through `bind` itself, its git checks
    stubbed (anim_twist_b5a.b5a_expected is the caller's to patch)."""
    unityrs = top / "unityrs"
    unityrs.write_bytes(b"built")
    none = candidate == "none"
    record = dict(format=seal.FORMAT, version=1, intake_digest=digest, candidate=candidate,
                  reason=seal.REASONS[0] if none else None,
                  hooks=hooks or (dict(HOOKS, twist_rule=None) if none else HOOKS), code=code,
                  reader=dict(sha256=hashlib.sha256(b"built").hexdigest(), commit=code["commit"],
                              command=list(seal.READER_BUILD), remap="r"),
                  editors=sorted(VERSIONS), verdict=seal.VERDICT, b5a=seal.b5a_pins())
    path = top / f"freeze-{next(FREEZES)}.json"
    path.write_text(json.dumps(record), encoding="utf-8")
    with mock.patch.object(seal, "canonical"), mock.patch.object(anim_body_seal, "bind_artifact"), \
            mock.patch.object(anim_body_seal, "code_history"), \
            mock.patch.object(seal, "pinned_files", return_value=code["files"]):
        return seal.bind(path, str(unityrs), top)


def hook_top(case):
    """(a checkout top holding the hook module, the freeze's `code` pinning it)."""
    top = scratch.for_case(case)
    path = top / "tools" / "parity" / f"{HOOK_MODULE}.py"
    path.parent.mkdir(parents=True)
    path.write_text(HOOK_SOURCE, encoding="utf-8")
    name = f"tools/parity/{HOOK_MODULE}.py"
    return top, dict(commit="c" * 40, files=[name], sha256={name: hashlib.sha256(path.read_bytes()).hexdigest()})


class ClaimTests(unittest.TestCase):
    def setUp(self):
        patcher = mock.patch.object(b5a, "b5a_expected", return_value=(set(), set()))
        patcher.start()
        self.addCleanup(patcher.stop)
        self.top, self.code = hook_top(self)

    def test_a_second_claim_is_refused(self):
        folder = scratch.for_case(self) / "ledger"
        record, other = dict(intake="a"), "b" * 64
        digest = round12.sha256(round12.canonical_json(record))
        bound_ = bound(self.top, self.code, digest)
        with self.assertRaisesRegex(ValueError, "opens only after"):
            seal.require_claim(digest, folder)
        with self.assertRaisesRegex(ValueError, "not its intake's"):
            seal.claim(bound(self.top, self.code, other), record, folder)
        seal.claim(bound_, record, folder)
        seal.require_claim(digest, folder)
        self.assertEqual(json.loads((folder / f"{digest}.claim").read_text())["state"], "claimed")
        with self.assertRaisesRegex(ValueError, "already evaluated"):
            seal.claim(bound_, record, folder)
        seal.mark(digest, "b5a", folder)
        self.assertEqual(seal.claimed_record(digest, folder), record)
        # The claim is the intake's own: another intake's is separate.
        with self.assertRaisesRegex(ValueError, "opens only after"):
            seal.require_claim(other, folder)
        for bad in ("a" * 63, None, "../" + "a" * 61):
            with self.subTest(bad=bad), self.assertRaises(ValueError):
                seal.require_claim(bad, folder)

    def test_only_a_bound_freeze_claims_or_evaluates(self):
        folder = scratch.for_case(self) / "ledger"
        record = dict(intake="t")
        digest = round12.sha256(round12.canonical_json(record))
        real = bound(self.top, self.code, digest)
        forged = object.__new__(seal.Bound)
        forged.__dict__.update(real.__dict__)
        for token in (digest, real.freeze, None, forged):
            with self.subTest(token=type(token).__name__):
                with self.assertRaisesRegex(ValueError, "only through a freeze anim_twist_seal.bind verified"):
                    seal.claim(token, record, folder)
                with self.assertRaisesRegex(ValueError, "only through a freeze anim_twist_seal.bind verified"):
                    seal._evaluate(token, [], [], folder)
        with self.assertRaisesRegex(ValueError, "comes only from anim_twist_seal.bind"):
            seal.Bound(object(), real.freeze, real.sha256, real.unityrs, real.top)
        self.assertFalse(folder.exists() and list(folder.glob("*.claim")))
        # No rows, executable or freeze besides the token and the runs.
        self.assertEqual(list(inspect.signature(seal._evaluate).parameters),
                         ["bound", "round12_folders", "round11_folders", "ledger_dir"])

    def test_a_claim_half_written_by_a_crash_refuses_and_is_never_reclaimed(self):
        folder = scratch.for_case(self) / "ledger"
        record = dict(intake="c")
        digest = round12.sha256(round12.canonical_json(record))
        bound_ = bound(self.top, self.code, digest)

        def dies(handle, data):
            raise KeyboardInterrupt("the process dies mid-claim")
        with mock.patch.object(seal.os, "write", dies), self.assertRaises(KeyboardInterrupt):
            seal.claim(bound_, record, folder)
        self.assertTrue((folder / f"{digest}.claim").is_file())
        with self.assertRaisesRegex(ValueError, "half-written"):
            seal.require_claim(digest, folder)
        with self.assertRaisesRegex(ValueError, "half-written"):
            seal.claim(bound_, record, folder)
        with self.assertRaisesRegex(ValueError, "half-written"):
            seal.mark(digest, "failed", folder)
        # A marker holding another intake's state, or a truncated one, is half-written too.
        (folder / f"{digest}.claim").write_text('{"intake_digest": "' + digest + '", "sta', encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "half-written"):
            seal.require_claim(digest, folder)


class FreezeTests(unittest.TestCase):
    def test_the_pinned_files(self):
        files = seal.pinned_files(hooks=dict(observe_b5a="anim_twist_b5a:b5a_failures", twist_rule=None))
        for name in ("tools/parity/anim_twist_model.py", "tools/parity/anim_twist_bound.py",
                     "tools/parity/anim_muscle_pose.py", "tools/parity/anim_fixture_round12.py",
                     "tools/parity/anim_twist_seal.py", "tools/parity/anim_twist_b5a.py",
                     "tools/parity/anim_round12_tables.py", "crates/unityrs-anim/src/humanoid/muscle_pose.rs",
                     "crates/unityrs-anim/src/humanoid/mod.rs", *seal.DATA):
            self.assertIn(name, files)
        self.assertNotIn(f"tools/parity/{seal.FREEZE_PATH.name}", files)
        self.assertEqual(files, sorted(set(files)))
        self.assertTrue(all((seal.TOP / name).is_file() for name in files))
        for hooks in (dict(other="m:f"), dict(observe_b5a="../m:f"), dict(observe_b5a="m")):
            with self.subTest(hooks=hooks), self.assertRaises(ValueError):
                seal.pinned_files(hooks=hooks)

    def test_the_record(self):
        top, code = hook_top(self)
        reader = dict(sha256="d" * 64, commit=code["commit"], command=list(seal.READER_BUILD), remap="r")
        with mock.patch.object(seal, "code_state", return_value=code), \
                mock.patch.object(seal, "build_reader", return_value=reader) as built:
            record = seal.make_freeze("e" * 64, "none", dict(observe_b5a=HOOKS["observe_b5a"]), top / "u",
                                      seal.REASONS[0], top)
            self.assertEqual(built.call_args.args[:2], (code["commit"], top / "u"))
            self.assertEqual(seal.make_freeze("e" * 64, CANDIDATE, HOOKS, top / "u", top=top)["candidate"], CANDIDATE)
            for candidate, reason, hooks, pattern in (
                    ("none", None, HOOKS, "names one of"), (CANDIDATE, seal.REASONS[0], HOOKS, "only it"),
                    ("none", "tired", HOOKS, "names one of"),
                    (dict(limb=[9, 9, 9, "+", 0], foot=[1, 0]), None, HOOKS, "candidates"),
                    ("best", None, HOOKS, "no candidate"), (CANDIDATE, None, dict(observe_b5a=HOOKS["observe_b5a"]),
                                                            "twist rule unless"),
                    ("none", seal.REASONS[0], {}, "names B5a's observer"),
                    ("none", seal.REASONS[0], HOOKS, "and only then"),
                    ("none", seal.REASONS[0], dict(observe_b5a="anim_absent:f"), "not in a pinned module")):
                with self.subTest(candidate=candidate, reason=reason, pattern=pattern), \
                        self.assertRaisesRegex(ValueError, pattern):
                    seal.make_freeze("e" * 64, candidate, hooks, top / "u", reason, top)
            with self.assertRaisesRegex(ValueError, "intake digest"):
                seal.make_freeze("e", "none", HOOKS, top / "u", seal.REASONS[0], top)
            # The freeze never executes a hook: a module that fails on import is recorded by path and sha256.
            (top / code["files"][0]).write_text("raise ImportError('executed at the freeze')\n", encoding="utf-8")
            code["sha256"][code["files"][0]] = hashlib.sha256((top / code["files"][0]).read_bytes()).hexdigest()
            self.assertEqual(seal.make_freeze("e" * 64, CANDIDATE, HOOKS, top / "u", top=top)["hooks"], HOOKS)
        self.assertEqual(record["b5a"]["pinned"], b5a.pinned_refusals())
        self.assertEqual((record["verdict"], record["reader"], record["hooks"]),
                         (seal.VERDICT, reader, dict(observe_b5a=HOOKS["observe_b5a"], twist_rule=None)))
        path = top / "freeze.json"
        found = seal.write_freeze(record, path)
        self.assertEqual(seal.read_freeze(path.read_bytes()), record)
        self.assertEqual(len(found), 64)
        with self.assertRaisesRegex(ValueError, "written once"):
            seal.write_freeze(record, path)
        for change in (dict(format="other"), dict(reason=None), dict(candidate=CANDIDATE), dict(extra=1),
                       dict(hooks=dict(observe_b5a=None, twist_rule=None)), dict(hooks=HOOKS)):
            with self.subTest(change=change), self.assertRaises(ValueError):
                seal.read_freeze(json.dumps(record | change).encode())
        # Only the canonical freeze path is read.
        with self.assertRaisesRegex(ValueError, "canonical freeze"):
            seal.canonical(path)

    def test_hooks_load_only_from_the_pinned_bytes(self):
        top, code = hook_top(self)
        freeze = dict(hooks=HOOKS, code=code)
        self.assertEqual(seal.load_hook(freeze, "observe_b5a", top).__name__, "observe")
        self.assertIsNone(seal.load_hook(dict(hooks=dict(twist_rule=None), code=code), "twist_rule", top))
        # No callable is taken from a caller: only names, loaded from the pinned module.
        self.assertFalse({"observe_b5a", "twist_rule"} & set(inspect.signature(seal._evaluate).parameters))
        for hooks, pattern in ((dict(observe_b5a="anim_twist_b5a:b5a_failures"), "not in a pinned module"),
                               (dict(observe_b5a=print), "not a pinned module:function")):
            with self.subTest(pattern=pattern), self.assertRaisesRegex(ValueError, pattern):
                seal.load_hook(dict(hooks=hooks, code=code), "observe_b5a", top)
        path = top / code["files"][0]
        path.write_text(HOOK_SOURCE + "\n", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "changed"):
            seal.load_hook(freeze, "observe_b5a", top)

    def test_the_reader_is_built_from_the_pinned_commit(self):
        top = scratch.for_case(self)
        archive = io.BytesIO()
        with tarfile.open(fileobj=archive, mode="w") as tar:
            info = tarfile.TarInfo("Cargo.toml")
            tar.addfile(info, io.BytesIO(b""))
        runs = []

        def run(args, **kwargs):
            runs.append((args, kwargs))
            if args[0] == "git":
                return mock.Mock(returncode=0, stdout=archive.getvalue())
            target = Path(kwargs["env"]["CARGO_TARGET_DIR"]) / "release"
            target.mkdir(parents=True)
            (target / "unityrs").write_bytes(b"built")
            return mock.Mock(returncode=0, stderr="")
        with mock.patch("child.run", run):
            found = seal.build_reader("f" * 40, top / "unityrs", top)
        self.assertEqual(found["sha256"], hashlib.sha256(b"built").hexdigest())
        self.assertEqual((found["commit"], found["command"]), ("f" * 40, list(seal.READER_BUILD)))
        self.assertEqual(runs[0][0][-1], "f" * 40)
        self.assertEqual((runs[1][0], (Path(runs[1][1]["cwd"]) / "Cargo.toml").name), (list(seal.READER_BUILD),
                                                                                         "Cargo.toml"))
        self.assertIn("--remap-path-prefix=", runs[1][1]["env"]["RUSTFLAGS"])
        self.assertEqual((top / "unityrs").read_bytes(), b"built")

    def test_binding_refuses_an_executable_not_built_from_the_pinned_commit(self):
        top, code = hook_top(self)
        unityrs = top / "unityrs"
        unityrs.write_bytes(b"stale")
        record = dict(format=seal.FORMAT, version=1, intake_digest="e" * 64, candidate="none",
                      reason=seal.REASONS[0], hooks=dict(observe_b5a=HOOKS["observe_b5a"], twist_rule=None),
                      code=code, reader=dict(sha256=hashlib.sha256(b"built").hexdigest(), commit=code["commit"],
                                             command=list(seal.READER_BUILD), remap="r"),
                      editors=sorted(VERSIONS), verdict=seal.VERDICT, b5a=seal.b5a_pins())
        path = top / "freeze.json"
        path.write_text(json.dumps(record), encoding="utf-8")
        with mock.patch.object(seal, "canonical"), mock.patch.object(anim_body_seal, "bind_artifact"), \
                mock.patch.object(anim_body_seal, "code_history"), \
                mock.patch.object(seal, "pinned_files", return_value=code["files"]):
            with self.assertRaisesRegex(ValueError, "not the one built from the pinned commit"):
                seal.bind(path, str(unityrs), top)
            unityrs.write_bytes(b"built")
            self.assertEqual(seal.bind(path, str(unityrs), top).freeze, record)


class EvaluateTests(Runs, unittest.TestCase):
    def freeze(self, digest, candidate="none", hooks=None):
        """A freeze of `digest`, bound (`bind`) as the evaluation needs it."""
        return bound(self.top, self.code, digest, candidate, hooks)

    def setUp(self):
        global CURRENT
        super().setUp()
        self.r12, self.r11 = self.runs()
        self.digest, self.record = self.intake(self.r12, self.r11)
        self.ledger = scratch.for_case(self)
        self.top, self.code = hook_top(self)
        rows_ = {(e, TWIN, 7, sid) for e in VERSIONS for sid in rows()[TWIN]["times"][:1]}
        case = sorted(b5a.pinned_refusals())[0]
        self.refused = {(e, case, 7, "00000000"): b5a.pinned_refusals()[case] for e in VERSIONS}
        self.expected = (rows_, set(self.refused))
        patcher = mock.patch.object(b5a, "b5a_expected", return_value=self.expected)
        patcher.start()
        self.addCleanup(patcher.stop)
        self.seen = observation(rows_, self.refused)
        self.rule = None
        CURRENT = self
        self.addCleanup(setattr, sys.modules[__name__], "CURRENT", None)

    def observe(self, views, unityrs):
        """B5a's observation, reading a sealed twin through each Editor's round 12 view."""
        for editor, kind, view in views:
            if kind == "round12":
                round12.complete(view, readers(), [TWIN])
        return self.seen

    def evaluate(self, freeze, rule=None, observe=None):
        def never(*_):
            raise AssertionError("the twist rule ran")
        self.rule, self.observer = rule or never, observe or self.observe
        return seal._evaluate(freeze, list(self.r12.values()), list(self.r11.values()), self.ledger)

    def events(self):
        return [row["event"] for row in ledger(self.ledger)]

    def test_the_none_path_refuses_twist_and_keeps_b5a(self):
        report = self.evaluate(self.freeze(self.digest))
        self.assertEqual(report["b5a"], dict(passed=True, failures=[]))
        self.assertEqual(report["twist"]["admitted"], "none")
        self.assertIn("the freeze is none", report["twist"]["failures"][0])
        self.assertEqual(self.events(), ["claimed", "b5a", "evaluated"])
        # A failing B5a is reported on the none path too.
        self.setUp()
        self.seen.rows.popitem()
        self.assertFalse(self.evaluate(self.freeze(self.digest))["b5a"]["passed"])

    def test_b5a_is_recorded_before_twist_begins(self):
        def rule(candidate, views, unityrs):
            self.assertEqual(self.events(), ["claimed", "b5a"])
            self.assertEqual(json.loads((self.ledger / f"{self.digest}.claim").read_text())["state"], "b5a")
            raise ValueError("twist dies")
        report = self.evaluate(self.freeze(self.digest, CANDIDATE), rule)
        self.assertTrue(report["b5a"]["passed"])
        self.assertEqual(ledger(self.ledger)[1]["passed"], True)
        self.assertEqual(json.loads((self.ledger / f"{self.digest}.claim").read_text())["state"], "evaluated")

    def test_the_twist_rule_loads_only_after_b5a_is_written(self):
        # A twist module that fails to import: its failure, after B5a's verdict is written, refuses twist alone. A
        # `none` freeze names no twist rule at all, so never touches one.
        name = "tools/parity/round12_test_twist.py"
        broken = self.top / name
        broken.write_text("raise ImportError('the twist module is broken')\n", encoding="utf-8")
        self.code["files"].append(name)
        self.code["sha256"][name] = hashlib.sha256(broken.read_bytes()).hexdigest()
        hooks = dict(HOOKS, twist_rule="round12_test_twist:rule")
        with self.assertRaisesRegex(ValueError, "hooks are not"):
            self.freeze(self.digest, "none", hooks)
        report = self.evaluate(self.freeze(self.digest, CANDIDATE, hooks))
        self.assertEqual(report["b5a"], dict(passed=True, failures=[]))
        self.assertEqual(report["twist"]["admitted"], "none")
        self.assertIn("the twist module is broken", report["twist"]["failures"][0])
        self.assertEqual(self.events(), ["claimed", "b5a", "evaluated"])

    def test_no_public_path_spends_the_claim_unbound(self):
        # The evaluation's only public entry binds the committed freeze and executable first: given another
        # freeze, it refuses before any claim.
        self.assertFalse(hasattr(seal, "evaluate_bound"))
        path = self.top / "freeze.json"
        path.write_text(json.dumps(self.freeze(self.digest).freeze), encoding="utf-8")
        with mock.patch.object(seal, "LEDGER_DIR", self.ledger), self.assertRaisesRegex(ValueError, "canonical freeze"):
            seal.evaluate(list(self.r12.values()), list(self.r11.values()), "unityrs", path)
        self.assertFalse(list(Path(self.ledger).iterdir()))

    def test_any_sealed_twist_failure_refuses_all_twist(self):
        def raises(*_):
            raise ValueError("a sealed capture is short")
        for rule, pattern in ((lambda *_: {"2022": [], "6000": ["LeftLowerArm outside its bound"]}, "6000: Left"),
                              (lambda *_: {"2022": []}, "no twist verdict for Editor 6000"),
                              (raises, "the twist evaluation failed")):
            self.setUp()
            report = self.evaluate(self.freeze(self.digest, CANDIDATE), rule)
            with self.subTest(pattern=pattern):
                self.assertEqual(report["twist"]["admitted"], "none")
                self.assertIn(pattern, report["twist"]["failures"][0])
                self.assertTrue(report["b5a"]["passed"])
        self.setUp()
        seen = []

        def passes(candidate, views, unityrs):
            seen.append(sorted((e, kind) for e, kind, _ in views))
            return {"2022": [], "6000": []}
        self.assertEqual(self.evaluate(self.freeze(self.digest, CANDIDATE), passes)["twist"],
                         dict(admitted=CANDIDATE, failures=[]))
        self.assertEqual(seen, [[("2022", "round11"), ("2022", "round12"), ("6000", "round11"), ("6000", "round12")]])

    def test_a_twist_only_collection_failure_refuses_twist_and_keeps_b5a(self):
        path = self.r12["2022"] / "inventory.json"
        value = json.loads(path.read_text(encoding="utf-8"))
        next(row for row in value["rows"] if row["name"] == "body-twist-compact-t5")["status"] = "mismatch"
        write_json(path, value)
        digest, _ = self.intake(self.r12, self.r11)
        report = self.evaluate(self.freeze(digest, CANDIDATE))
        self.assertEqual(report["twist"]["admitted"], "none")
        self.assertIn("2022: a twist-only collection failure", report["twist"]["failures"][0])
        self.assertTrue(report["b5a"]["passed"])

    def test_a_wrong_intake_is_refused_before_the_claim(self):
        with self.assertRaisesRegex(ValueError, "not the frozen intake's"):
            self.evaluate(self.freeze("0" * 64))
        self.assertFalse(list(Path(self.ledger).iterdir()))
        # A run's file rewritten after the freeze changes the intake.
        (self.r12["6000"] / "authored.json").write_bytes((self.r12["6000"] / "authored.json").read_bytes() + b" ")
        with self.assertRaisesRegex(ValueError, "not the frozen intake's"):
            self.evaluate(self.freeze(self.digest))

    def test_a_second_evaluation_is_refused(self):
        self.evaluate(self.freeze(self.digest))
        with self.assertRaisesRegex(ValueError, "already evaluated"):
            self.evaluate(self.freeze(self.digest))
        self.assertEqual(self.events(), ["claimed", "b5a", "evaluated"])

    def test_bytes_changed_after_the_claim_fail_their_verdict_alone(self):
        twin = self.r12["6000"] / "replays" / "body" / f"{TWIN}.pending.json"
        data = twin.read_bytes()

        def tampers(views, unityrs):
            twin.write_bytes(data + b" ")
            return self.observe(views, unityrs)
        report = self.evaluate(self.freeze(self.digest), observe=tampers)
        self.assertFalse(report["b5a"]["passed"])
        self.assertIn("read bytes the intake never bound", report["b5a"]["failures"][0])
        self.assertEqual(self.events(), ["claimed", "b5a", "evaluated"])
        # Evidence only twist reads, changed after the claim: twist refused, B5a stands.
        self.setUp()
        twist = self.r12["6000"] / "replays" / "body" / f"{TWIST_CLIP}.pending.json"
        twist_data = twist.read_bytes()

        def rule(candidate, views, unityrs):
            twist.write_bytes(twist_data + b" ")
            for editor, kind, view in views:
                if kind == "round12":
                    round12.complete(view, readers(), [TWIST_CLIP])
            return {"2022": [], "6000": []}
        report = self.evaluate(self.freeze(self.digest, CANDIDATE), rule)
        self.assertTrue(report["b5a"]["passed"])
        self.assertEqual(report["twist"]["admitted"], "none")
        self.assertIn("never bound", report["twist"]["failures"][0])
        # The claim stays spent, the run restored.
        twist.write_bytes(twist_data)
        with self.assertRaisesRegex(ValueError, "already evaluated"):
            self.evaluate(self.freeze(self.digest))

    def test_the_sealed_views_open_only_after_the_claim(self):
        with self.assertRaisesRegex(ValueError, "opens only after the intake's claim"):
            seal.sealed_views(self.r12, {}, self.digest, self.ledger)
        seal.claim(self.freeze(self.digest), self.record, self.ledger)
        views = seal.sealed_views(self.r12, {}, self.digest, self.ledger)
        self.assertEqual(sorted(round12.complete(views[0][2], readers(), [TWIN, EXPLORATION])),
                         sorted([TWIN, EXPLORATION]))


if __name__ == "__main__":
    unittest.main()
