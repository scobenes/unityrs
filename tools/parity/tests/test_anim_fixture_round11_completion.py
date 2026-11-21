"""Round 11's completion (anim_fixture_round11): the Avatar read as a value, never its asset key; each bundle
object's identity with its full container path, as the bundle's own container table lists it, where `inspect`
lists a preview (by UTF-8 bytes, as the CLI counts); and the two body sweeps'
expected failure (URS-42), accepted, as not covered only, with the recorder's exact message and the sweep's
bundle."""

import json
import unittest

import test_anim_replay_coverage as coverage  # noqa: F401  (puts tools/parity on the path)
from anim_fixture_support import RUN
from test_anim_fixture_round11 import Built

import anim_fixture_body as body  # noqa: E402
import anim_fixture_round11 as r11  # noqa: E402
import anim_replay as replay  # noqa: E402
import anim_replay_complete as complete  # noqa: E402
import scratch  # noqa: E402

FULL = "assets/unityrsfixtures/" + "x" * 120 + ".anim"
# The same first 128 bytes and length, another suffix.
TWIN = FULL[:-6] + "y.anim"
# Non-ASCII: under 128 code points but over 128 UTF-8 bytes, its 128th byte inside a code point.
WIDE = "assets/unityrsfixtures/" + "é" * 60 + ".anim"
HEADER = dict(input=0, file_index=1, file="CAB-0", path_id=-7, class_id=74, class_name="AnimationClip", name="x")
SWEEPS = ("body-probes-muscles-compact", "body-probes-muscles-tall")


def preview(text, **edits):
    return dict(r11.preview_of(text), **edits)


class AvatarTests(unittest.TestCase):
    def test_the_avatar_is_its_value_never_its_key(self):
        # BundleView.asset returns a key, as the fake now does: a key in place of the value refuses.
        self.assertEqual(Built.AVATAR, (0, -90))
        with self.assertRaisesRegex(ValueError, "no serialized Avatar m_Scale in x"):
            r11.human_scale("x", Built.AVATAR, replay)
        self.assertEqual(r11.human_scale("x", {"m_Avatar": {"m_Human": {"m_Scale": 1.25}}}, replay), 1.25)


class PreviewTests(unittest.TestCase):
    def resolved(self, listed, shown, table=(FULL,)):
        headers = {(1, -7): dict(HEADER, container_path=listed)}
        r11.resolve_preview(headers, {"object": dict(HEADER, **shown)},
                            lambda key: set(table) if key == (1, -7) else set())
        return headers[(1, -7)]["container_path"]

    def test_the_preview_is_the_clis_by_utf8_bytes(self):
        self.assertEqual(r11.preview_of("a" * 128), "a" * 128)
        self.assertEqual(r11.preview_of(FULL), {"$unityrs": "preview", "length": 148, "prefix": FULL[:128],
                                                "omitted": 20})
        self.assertEqual(len(WIDE), 88)
        wide = r11.preview_of(WIDE)
        # 23 ASCII bytes, then 52 whole two-byte code points: byte 128 would split the 53rd.
        self.assertEqual(wide, {"$unityrs": "preview", "length": 148, "prefix": WIDE[:75], "omitted": 21})

    def test_a_shortened_path_resolves_to_the_bundles_own_path(self):
        self.assertEqual(self.resolved(preview(FULL), dict(container_path=FULL)), FULL)
        self.assertEqual(self.resolved(preview(WIDE), dict(container_path=WIDE), table=(WIDE,)), WIDE)

    def test_anything_else_stays_and_refuses(self):
        for label, listed, shown, table in (
                # The same prefix and length but another suffix than the bundle's container table lists.
                ("another suffix", preview(FULL), dict(container_path=TWIN), (FULL,)),
                ("a second listed path", preview(FULL), dict(container_path=FULL), (FULL, TWIN)),
                ("no listed path", preview(FULL), dict(container_path=FULL), ()),
                ("another length", preview(FULL), dict(container_path=FULL + "x"), (FULL + "x",)),
                ("another prefix", preview(TWIN.replace("assets", "assetz")), dict(container_path=TWIN), (TWIN,)),
                ("a wrong omitted count", preview(FULL, omitted=1), dict(container_path=FULL), (FULL,)),
                ("a code point count", preview(WIDE, length=len(WIDE)), dict(container_path=WIDE), (WIDE,)),
                ("another object", preview(FULL), dict(container_path=FULL, name="y"), (FULL,)),
                ("a listed full path", "a.anim", dict(container_path="b.anim"), ("b.anim",))):
            with self.subTest(label):
                self.assertEqual(self.resolved(listed, shown, table), listed)

    def view(self, folder, shown):
        """The round 11 view of one saved bundle: an AnimationClip listed by its preview, and the AssetBundle whose
        container table lists FULL for it; `shown` is the clip's show path."""
        clip, bundle = (1, -7), (1, 1)
        headers = {clip: dict(HEADER, container_path=preview(FULL)),
                   bundle: dict(HEADER, path_id=1, class_id=142, class_name="AssetBundle", name="b",
                                container_path=None)}
        values = {clip: {"m_Name": "x"}, bundle: {"m_Container": [[FULL, {"asset": {"m_FileID": 0, "m_PathID": -7}}]]}}
        objects = {clip: dict(headers[clip], container_path=shown), bundle: headers[bundle]}
        for (file, path), value in values.items():
            (folder / f"b.{file}.{path}.show.json").write_text(json.dumps(dict(
                format="unityrs.show", version=1, bundle_sha256="d", object=objects[(file, path)],
                truncated=dict(cuts=0, items=0, depth=0, bytes=0, binary=0), value=value)), encoding="utf-8")
        view = complete.BundleView.__new__(complete.BundleView)
        view.path, view.inspect_dir, view.digest, view.index = folder / "b", folder, "d", 0
        view.require, view.read_json, view.replay = replay.require, replay.read_json, replay
        view.headers, view.values, view.container = headers, {}, None
        return view

    def test_completion_reads_the_full_identity_through_the_view(self):
        base = complete.BundleView
        with r11.round11_tables(), scratch.folder() as folder:
            self.assertTrue(issubclass(complete.BundleView, base) and complete.BundleView is not base)
            self.assertEqual(self.view(folder, FULL).value((1, -7)), {"m_Name": "x"})
            with self.assertRaisesRegex(ValueError, r"show identity disagrees with inspect: \(1, -7\)"):
                self.view(folder, TWIN).value((1, -7))
        self.assertIs(complete.BundleView, base)


class ExpectedFailureTests(unittest.TestCase):
    def row(self, name, status="failed", reason=None):
        return dict(name=name, status=status,
                    reason=reason if reason is not None else r11.EXPECTED_FAILURES[name][0])

    def test_only_the_body_sweeps_failing_with_the_recorders_message(self):
        self.assertEqual(set(r11.EXPECTED_FAILURES), set(SWEEPS))
        for name in SWEEPS:
            self.assertEqual(r11.accepted_failure(self.row(name)), dict(
                name=name, status="not_covered", reason="expected failure: limb-twist redistributes at twist 0 "
                                                        "(URS-42)"))
            self.assertIsNone(r11.accepted_failure(self.row(name, reason=f"Record {name}: Spine Front-Back at 1 "
                                                                          f"turned no bone in {name}.")))
            self.assertIsNone(r11.accepted_failure(self.row(name, status="skipped")))
        self.assertIsNone(r11.accepted_failure(self.row("body-probes-fingers", reason=r11.EXPECTED_FAILURES[
            SWEEPS[0]][0])))

    def inventory(self, rows, bundles=SWEEPS):
        with scratch.folder() as folder:
            (folder / "bundles").mkdir()
            for name in bundles:
                (folder / "bundles" / r11.ROUND11_CASES[name].bundle).write_bytes(b"UnityFS\0")
            names = [*body.BODY_CASES, *r11.ROUND11_CASES]
            cases = {name: dict(name=name, status="recorded", reason="") for name in names}
            cases |= {row["name"]: row for row in rows}
            (folder / "run.json").write_text(json.dumps(dict(format="unityrs.fixture_run", version=1, run=RUN,
                                                             cases=list(cases.values()))), encoding="utf-8")
            (folder / "round.json").write_text(json.dumps(dict(format="unityrs.fixture_round", round=11,
                                                               run_id=RUN["run_id"])), encoding="utf-8")
            r11.check_inventory(folder, replay)

    def test_the_inventory_accepts_them_and_nothing_else(self):
        self.inventory([self.row(name) for name in SWEEPS])
        self.inventory([])
        with self.assertRaisesRegex(ValueError, r"not recorded \(1\): body-probes-muscles-tall \(failed\)"):
            self.inventory([self.row(SWEEPS[0]), self.row(SWEEPS[1], reason="Record: another failure.")])
        with self.assertRaisesRegex(ValueError, r"not recorded \(1\): body-probes-fingers \(failed\)"):
            self.inventory([self.row("body-probes-fingers", reason="x")])
        # The sweep was built before it was recorded: without its bundle the failure is not the expected one.
        self.assertEqual(r11.ROUND11_CASES[SWEEPS[1]].bundle, "body-muscles-tall")
        with self.assertRaisesRegex(ValueError, "body-probes-muscles-tall: expected failure without its bundle "
                                                "body-muscles-tall"):
            self.inventory([self.row(name) for name in SWEEPS], bundles=SWEEPS[:1])

    def test_completion_lists_them_as_not_covered(self):
        with scratch.folder() as folder:
            cases = [self.row(SWEEPS[0]), self.row("body-probes-fingers", reason="x")]
            (folder / "run.json").write_text(json.dumps(dict(format="unityrs.fixture_run", version=1, run=RUN,
                                                             cases=cases)), encoding="utf-8")
            with r11.round11_tables():
                found, _ = complete.read_manifest(folder, replay)
            self.assertEqual([(row["status"], row["reason"]) for row in found["cases"]], [
                ("not_covered", "expected failure: limb-twist redistributes at twist 0 (URS-42)"), ("failed", "x")])
            self.assertEqual(complete.read_manifest(folder, replay)[0]["cases"], cases)


if __name__ == "__main__":
    unittest.main()
