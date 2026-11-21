"""Round 12's seal (anim_twist_seal): the freeze record, the one claim per intake digest, and the verdicts behind the
allowlist: the `none` path keeps B5a, any sealed twist failure refuses all twist, and a second claim, a wrong
intake or bytes changed after the claim fail closed."""

import json
import unittest
from pathlib import Path
from unittest import mock

import test_anim_replay_coverage  # noqa: F401  (puts tools/parity on the path)

import anim_fixture_round12 as round12  # noqa: E402
import anim_twist_model as model  # noqa: E402
import anim_twist_seal as seal  # noqa: E402
import scratch  # noqa: E402
from test_anim_fixture_round12 import EXPLORATION, TWIN, VERSIONS, Runs, observation, rows, write_json  # noqa: E402

CANDIDATE = dict(limb=list(model.LIMBS[0]), foot=list(model.FEET[0]))


def ledger(path):
    return [json.loads(line) for line in (path / "ledger.jsonl").read_text(encoding="utf-8").splitlines()]


class ClaimTests(unittest.TestCase):
    def test_a_second_claim_is_refused(self):
        folder = scratch.for_case(self)
        digest, other = "a" * 64, "b" * 64
        with self.assertRaisesRegex(ValueError, "opens only after"):
            seal.require_claim(digest, folder)
        seal.claim(digest, folder)
        seal.require_claim(digest, folder)
        with self.assertRaisesRegex(ValueError, "already evaluated"):
            seal.claim(digest, folder)
        # The claim is the intake's own: another intake's is separate.
        with self.assertRaisesRegex(ValueError, "opens only after"):
            seal.require_claim(other, folder)
        for bad in ("a" * 63, None, "../" + "a" * 61):
            with self.subTest(bad=bad), self.assertRaises(ValueError):
                seal.claim(bad, folder)


class FreezeTests(unittest.TestCase):
    def test_the_pinned_files(self):
        files = seal.pinned_files()
        for name in ("tools/parity/anim_twist_model.py", "tools/parity/anim_twist_bound.py",
                     "tools/parity/anim_muscle_pose.py", "tools/parity/anim_fixture_round12.py",
                     "tools/parity/anim_twist_seal.py", "tools/parity/anim_round12_tables.py",
                     "crates/unityrs-anim/src/humanoid/muscle_pose.rs", "crates/unityrs-anim/src/humanoid/mod.rs",
                     *seal.DATA):
            self.assertIn(name, files)
        self.assertNotIn(f"tools/parity/{seal.FREEZE_PATH.name}", files)
        self.assertEqual(files, sorted(set(files)))
        self.assertTrue(all((seal.TOP / name).is_file() for name in files))

    def test_the_record(self):
        folder = scratch.for_case(self)
        unityrs = folder / "unityrs"
        unityrs.write_bytes(b"reader")
        with mock.patch.object(seal, "code_state", return_value=("c" * 40, ["f"], "d" * 64)):
            record = seal.make_freeze("e" * 64, "none", str(unityrs), seal.REASONS[0])
            self.assertEqual(seal.make_freeze("e" * 64, CANDIDATE, str(unityrs))["candidate"], CANDIDATE)
            for candidate, reason, pattern in (("none", None, "names one of"), (CANDIDATE, seal.REASONS[0], "only it"),
                                               ("none", "tired", "names one of"),
                                               (dict(limb=[9, 9, 9, "+", 0], foot=[1, 0]), None, "candidates"),
                                               ("best", None, "no candidate")):
                with self.subTest(candidate=candidate, reason=reason), self.assertRaisesRegex(ValueError, pattern):
                    seal.make_freeze("e" * 64, candidate, str(unityrs), reason)
            with self.assertRaisesRegex(ValueError, "intake digest"):
                seal.make_freeze("e", "none", str(unityrs), seal.REASONS[0])
        self.assertEqual(record["b5a"]["pinned"], round12.pinned_refusals())
        self.assertEqual(record["verdict"], seal.VERDICT)
        path = folder / "freeze.json"
        found = seal.write_freeze(record, path)
        self.assertEqual(seal.read_freeze(path.read_bytes()), record)
        self.assertEqual(len(found), 64)
        with self.assertRaisesRegex(ValueError, "written once"):
            seal.write_freeze(record, path)
        for change in (dict(format="other"), dict(reason=None), dict(candidate=CANDIDATE), dict(extra=1)):
            with self.subTest(change=change), self.assertRaises(ValueError):
                seal.read_freeze(json.dumps(record | change).encode())
        # Only the canonical freeze path is read.
        with self.assertRaisesRegex(ValueError, "canonical freeze"):
            seal.canonical(path)


class EvaluateTests(Runs, unittest.TestCase):
    def freeze(self, digest, candidate="none"):
        return dict(intake_digest=digest, candidate=candidate, reason=seal.REASONS[0] if candidate == "none" else None)

    def setUp(self):
        super().setUp()
        self.r12, self.r11 = self.runs()
        self.digest, _ = self.intake(self.r12, self.r11)
        self.ledger = scratch.for_case(self)
        rows_ = {(e, TWIN, 7, sid) for e in VERSIONS for sid in rows()[TWIN]["times"][:1]}
        case = sorted(round12.pinned_refusals())[0]
        self.refused = {(e, case, 7, "00000000"): "unsupported_humanoid_limb_twist(23)" for e in VERSIONS}
        self.expected = (rows_, set(self.refused))
        self.seen = observation(rows_, self.refused)

    def observe(self, views, unityrs):
        """B5a's observation, reading a sealed twin through each Editor's round 12 view."""
        for editor, kind, view in views:
            if kind == "round12":
                round12.complete(view, [TWIN])
        return self.seen

    def evaluate(self, freeze, rule=None, observe=None):
        def never(*_):
            raise AssertionError("the twist rule ran")
        return seal.evaluate_bound(freeze, list(self.r12.values()), list(self.r11.values()), "unityrs",
                                   observe or self.observe, rule or never, self.ledger, self.expected)

    def test_the_none_path_refuses_twist_and_keeps_b5a(self):
        report = self.evaluate(self.freeze(self.digest))
        self.assertEqual(report["b5a"], dict(passed=True, failures=[]))
        self.assertEqual(report["twist"]["admitted"], "none")
        self.assertIn("the freeze is none", report["twist"]["failures"][0])
        self.assertEqual([row["event"] for row in ledger(self.ledger)], ["claimed", "evaluated"])
        # A failing B5a is reported on the none path too.
        self.setUp()
        self.seen.rows.popitem()
        self.assertFalse(self.evaluate(self.freeze(self.digest))["b5a"]["passed"])

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
        # A run swapped for another after the freeze changes the intake.
        (self.r12["6000"] / "build.log").write_text("rebuilt\n", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "not the frozen intake's"):
            self.evaluate(self.freeze(self.digest))

    def test_a_second_evaluation_is_refused(self):
        self.evaluate(self.freeze(self.digest))
        with self.assertRaisesRegex(ValueError, "already evaluated"):
            self.evaluate(self.freeze(self.digest))
        self.assertEqual([row["event"] for row in ledger(self.ledger)], ["claimed", "evaluated"])

    def test_bytes_changed_after_the_claim_fail_closed(self):
        path = self.r12["6000"] / "replays" / "body" / f"{TWIN}.pending.json"
        data = path.read_bytes()

        def tampers(views, unityrs):
            path.write_bytes(data + b" ")
            return self.observe(views, unityrs)
        with self.assertRaisesRegex(ValueError, "read bytes the intake never bound"):
            self.evaluate(self.freeze(self.digest), observe=tampers)
        self.assertEqual([row["event"] for row in ledger(self.ledger)], ["claimed", "failed"])
        # The claim stays spent, the run restored.
        path.write_bytes(data)
        with self.assertRaisesRegex(ValueError, "already evaluated"):
            self.evaluate(self.freeze(self.digest))

    def test_the_sealed_views_open_only_after_the_claim(self):
        runs = {e: (folder, None) for e, folder in self.r12.items()}
        with self.assertRaisesRegex(ValueError, "opens only after the intake's claim"):
            seal.sealed_views(runs, {}, self.digest, self.ledger)
        seal.claim(self.digest, self.ledger)
        views = seal.sealed_views(runs, {}, self.digest, self.ledger)
        self.assertEqual(sorted(round12.complete(views[0][2], [TWIN, EXPLORATION])), sorted([TWIN, EXPLORATION]))


if __name__ == "__main__":
    unittest.main()
