"""Complete an Editor handoff using synthetic unityrs inspect and show documents."""

import copy
import hashlib
import json
import struct
import sys
import unittest
from pathlib import Path

from anim_fixture_support import (
    DEFERRED, REPLAYS, ROOT_ID, STATIC, VECTORS, bits, canonical, fixture_archives, static_bundles, write,
)
from test_anim_replay import identity

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))

import anim_replay as replay  # noqa: E402
import child  # noqa: E402
import scratch  # noqa: E402
from anim_replay_complete import packed_times  # noqa: E402


class CompleteReplayTests(unittest.TestCase):
    def setUp(self):
        self.folder = scratch.for_case(self)
        self.inspect = self.folder / "inspection"
        self.replays = self.folder / "replays"
        self.bundles = self.folder / "bundles"
        for folder in (self.inspect, self.replays, self.bundles, self.replays / "known-vectors"):
            folder.mkdir()
        self.run_path = self.folder / "run.json"
        self.run = dict(format="unityrs.fixture_run", version=1, cases=[
            dict(name=name, status="recorded", reason="") for name in (*STATIC, *REPLAYS, *VECTORS)
        ] + [dict(name=name, status="not_covered", reason="Separate fixture needed") for name in DEFERRED])
        self.recordings = fixture_archives(self.bundles, self.inspect)
        static_bundles(self.bundles, self.inspect)
        self.pending_path = self.replays / "legacy-explicit.pending.json"
        self.complete_path = self.replays / "legacy-explicit.json"

    def run_tool(self, *args):
        return child.run([sys.executable, "-I", str(HERE / "anim_replay.py"), *map(str, args)],
                         capture_output=True, text=True)

    def complete(self, *, missing=()):
        write(self.run_path, self.run)
        for name, doc in self.recordings.items():
            if name not in missing:
                folder = self.replays / "known-vectors" if name in VECTORS else self.replays
                write(folder / f"{name}.pending.json", doc)
        return self.run_tool("complete", self.folder, "--inspect-dir", self.inspect)

    def assert_refused(self, message, **kwargs):
        result = self.complete(**kwargs)
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn(message, result.stderr)
        self.assertFalse(self.complete_path.exists())

    def clip_show(self, case):
        bundle = self.recordings[case]["spec"]["bundles"][0]["name"]
        objects = json.loads((self.inspect / f"{bundle}.inspect.json").read_text())["objects"]
        header = next(row for row in objects if row["container_path"] == self.recordings[case]["spec"]["clip_asset"])
        path = self.inspect / f"{bundle}.1.{header['path_id']}.show.json"
        return path, json.loads(path.read_text())

    def test_complete_resolves_exact_ids_from_container_and_pointers(self):
        result = self.complete()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Completed and validated 16 replay(s)", result.stdout)
        for name in REPLAYS:
            finished = replay.read_json((self.replays / f"{name}.json").read_text(encoding="utf-8"))
            self.assertEqual(finished["capture"], self.recordings[name]["capture"])
            replay.validate(finished, self.bundles)
        finished = replay.read_json(self.complete_path.read_text(encoding="utf-8"))
        spec = finished["spec"]
        self.assertEqual((spec["prefab"], spec["clip"], spec["binding"], spec["avatar"]),
                         (identity(1), identity(2), identity(3), None))
        self.assertEqual(spec["nodes"][0]["id"], identity(ROOT_ID))
        finished = replay.read_json((self.replays / "generic-null-override.json").read_text(encoding="utf-8"))
        spec = finished["spec"]
        self.assertEqual([r["mesh"] for r in spec["renderers"]], [identity(8009), identity(8009)])
        self.assertIn(dict(owner=identity(221), field="m_OverrideClip", ordinal=0,
                           original=identity(2002), resolved=identity(2002), outcome="null_override"),
                      spec["evidence"]["discovery"])
        self.assertEqual(spec["evidence"]["editor"], self.recordings["generic-null-override"]["spec"]["evidence"]["editor"])
        self.assertEqual(spec["evidence"]["layout"], "streamed_dense")
        self.assertEqual(spec["evidence"]["clock"]["key_times"], [0, .25, .5, .75, 1])
        self.assertEqual(spec["evidence"]["clock"]["authored_key_times"], [0, .5, 1])
        cycle = next(s for s in spec["evidence"]["import_settings"] if s["property"] == "cycleOffset")
        self.assertEqual(cycle["serialized"], struct.unpack(">f", struct.pack(">f", .1))[0])
        self.assertEqual(sorted((self.replays / "known-vectors").glob("*.json")),
                         sorted((self.replays / "known-vectors").glob("*.pending.json")))

    def test_complete_refuses_changed_bundle(self):
        (self.bundles / "anim-legacy").write_bytes(b"different bytes")
        self.assert_refused("bundle digest mismatch")

    def test_complete_refuses_wrong_component_address(self):
        self.recordings["generic-morph"]["spec"]["renderers"][1]["component"] = 2
        self.assert_refused("renderer component")

    def test_complete_refuses_a_pending_recording_from_another_case(self):
        self.recordings["generic-scale"]["spec"]["evidence"]["editor"]["fixture_case"] = "generic-euler"
        self.assert_refused("pending recording does not identify manifest case generic-scale")

    def test_complete_refuses_relabelled_clip_and_prefab(self):
        for source, target in (("generic-quaternion", "generic-euler"), ("generic-constant", "generic-null-override"),
                               ("humanoid-compact-muscles", "humanoid-compact-muscles-offset")):
            with self.subTest(source=source, target=target):
                original = self.recordings[target]
                self.recordings[target] = copy.deepcopy(self.recordings[source])
                self.recordings[target]["spec"]["evidence"]["editor"]["fixture_case"] = target
                self.assert_refused("fixture identity mismatch")
                self.recordings[target] = original

    def test_complete_refuses_wrong_playback_mode(self):
        self.recordings["legacy-explicit"]["spec"]["mode"] = "animator"
        self.assert_refused("fixture playback mode mismatch")

    def test_complete_requires_case_specific_evidence(self):
        for case, field in (("generic-object-reference", "object_reference_samples"),
                            ("humanoid-tall-muscles", "spine_stop_f32")):
            with self.subTest(case=case, field=field):
                editor = self.recordings[case]["spec"]["evidence"]["editor"]
                original = editor.pop(field)
                self.assert_refused("missing fixture evidence")
                editor[field] = original

    def test_complete_refuses_wrong_discrete_flag(self):
        bindings = self.recordings["generic-object-reference"]["spec"]["evidence"]["editor"]["bindings"]
        next(row for row in bindings if row["property"] == "m_Mesh")["object_reference"] = False
        self.assert_refused("fixture binding flags mismatch")

    def test_complete_confirms_reported_mixed_transform_storage(self):
        for row in self.run["cases"]:
            if row["name"] == "mixed-transform-storage":
                row.update(status="recorded", reason="Mixed scale vector")
        self.assert_refused("mixed Transform storage reported by Unity is absent")
        path, doc = self.clip_show("generic-scale")
        clip = dict(doc["value"]["v"])
        playback = dict(dict(dict(clip["m_MuscleClip"]["v"])["m_Clip"]["v"])["data"]["v"])
        dict(playback["m_ConstantClip"]["v"])["data"].append(canonical(1.))
        doc["value"]["v"].append(["m_ClipBindingConstant", canonical({"genericBindings": [
            {"typeID": 4, "attribute": 3, "customType": 0}
        ]})])
        write(path, doc)
        result = self.complete()
        self.assertEqual(result.returncode, 0, result.stderr)
        scale = json.loads((self.replays / "generic-scale.json").read_text(encoding="utf-8"))
        self.assertTrue(scale["spec"]["evidence"]["mixed_transform_storage"])

    def test_complete_requires_run_manifest(self):
        write(self.pending_path, self.recordings["legacy-explicit"])
        result = self.run_tool("complete", self.folder, "--inspect-dir", self.inspect)
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("run.json", result.stderr)
        self.assertFalse(self.complete_path.exists())

    def test_complete_reports_missing_and_failed_cases(self):
        self.run["cases"] = [row for row in self.run["cases"] if row["name"] != "generic-euler"]
        for row in self.run["cases"]:
            if row["name"] == "static-lod":
                row.update(status="failed", reason="LOD could not build")
        result = self.complete()
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("generic-euler", result.stderr)
        self.assertIn("static-lod: failed (LOD could not build)", result.stderr)
        self.assertFalse(self.complete_path.exists())

    def test_complete_rejects_required_skips_and_missing_artifacts(self):
        for row in self.run["cases"]:
            if row["name"] == "generic-scale":
                row.update(status="skipped", reason="Build only")
        result = self.complete(missing=(VECTORS[0], "generic-scale"))
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("generic-scale: skipped (Build only)", result.stderr)
        self.assertIn(f"{VECTORS[0]}.pending.json", result.stderr)
        self.assertFalse(self.complete_path.exists())

    def test_complete_refuses_text_as_a_static_bundle(self):
        data = b"this is a text file"
        (self.bundles / "static-objects").write_bytes(data)
        for path in self.inspect.glob("static-objects.*.json"):
            doc = json.loads(path.read_text())
            doc["bundle_sha256"] = hashlib.sha256(data).hexdigest()
            write(path, doc)
        self.assert_refused("invalid UnityFS signature: static-objects")

    def test_complete_refuses_static_bundle_missing_expected_asset(self):
        path = self.inspect / "static-objects.1.142.show.json"
        doc = json.loads(path.read_text())
        container = dict(doc["value"]["v"])["m_Container"]
        container.remove(next(row for row in container if row["v"][0].endswith("/lod.prefab")))
        write(path, doc)
        self.assert_refused("static/lod.prefab")

    def test_complete_refuses_static_inspection_with_wrong_digest(self):
        path = self.inspect / "static-batch-scene.inspect.json"
        doc = json.loads(path.read_text())
        doc["bundle_sha256"] = "0" * 64
        write(path, doc)
        self.assert_refused("inspection bundle digest mismatch: static-batch-scene.inspect.json")

    def test_complete_refuses_known_vectors_missing_a_schedule_point(self):
        doc = self.recordings[VECTORS[0]]
        del doc["spec"]["schedule"][1], doc["capture"]["samples"][1]
        doc["spec"]["evidence"]["clock"]["stop"]["probe"]["sample"] -= 1
        self.assert_refused("known-vector schedule mismatch")

    def test_complete_refuses_known_vectors_from_a_different_editor(self):
        self.recordings[VECTORS[0]]["capture"]["editor_version"] = "6000.0.1f1"
        self.assert_refused("known-vector recording disagrees with its clip replay")

    def test_complete_refuses_wrong_known_muscle_vector(self):
        doc = self.recordings[VECTORS[0]]
        doc["spec"]["schedule"][2]["muscles"][0] = .25
        doc["capture"]["samples"][2]["muscles_f32"][0] = bits(.25)
        self.assert_refused("known-vector inputs mismatch")

    def test_complete_refuses_unbound_or_stale_inspections(self):
        for filename, remove in (("anim-legacy.inspect.json", True), ("anim-legacy.1.2.show.json", False)):
            with self.subTest(filename=filename):
                path = self.inspect / filename
                original = path.read_text(encoding="utf-8")
                doc = json.loads(original)
                if remove:
                    del doc["bundle_sha256"]
                else:
                    doc["bundle_sha256"] = "0" * 64
                write(path, doc)
                self.assert_refused(f"inspection bundle digest mismatch: {filename}")
                path.write_text(original, encoding="utf-8")

    def test_complete_refuses_truncated_and_cut_show_values(self):
        path, original = self.clip_show("legacy-explicit")
        for where in ("stats", "tagged", "nested"):
            with self.subTest(where=where):
                doc = copy.deepcopy(original)
                cut = {"$unityrs": "cut", "reason": "items", "omitted": 1}
                if where == "stats":
                    doc["truncated"].update(cuts=1, items=1)
                elif where == "tagged":
                    doc["value"]["cut"] = cut
                else:
                    doc["value"]["v"].append(["ignored", [cut]])
                write(path, doc)
                result = self.complete()
                self.assertEqual(result.returncode, 1, result.stderr)
                self.assertRegex(result.stderr, "truncated show|cut show")
                self.assertFalse(self.complete_path.exists())

    def test_complete_refuses_samples_that_only_miss_authored_keys(self):
        path, doc = self.clip_show("generic-euler")
        clip = dict(doc["value"]["v"])
        playback = dict(dict(dict(clip["m_MuscleClip"]["v"])["m_Clip"]["v"])["data"]["v"])
        stream = dict(playback["m_StreamedClip"]["v"])["data"]
        stream[7:7] = [int(bits(391 / 2048), 16), 1, 0, 0, 0, 0, 0]
        stream[21:21] = [int(bits(.5 + 391 / 2048), 16), 1, 0, 0, 0, 0, 0]
        write(path, doc)
        self.assert_refused("off-key sample")

    def test_complete_refuses_positive_stream_count_with_no_frame_data(self):
        path, doc = self.clip_show("generic-euler")
        clip = dict(doc["value"]["v"])
        playback = dict(dict(dict(clip["m_MuscleClip"]["v"])["m_Clip"]["v"])["data"]["v"])
        dict(playback["m_StreamedClip"]["v"])["data"].clear()
        write(path, doc)
        self.assert_refused("missing built streamed clip frame data")

    def test_packed_time_array_over_64_bytes_decodes_all_deltas(self):
        deltas = [0] + [33] * 89
        packed = sum(delta << (7 * i) for i, delta in enumerate(deltas)).to_bytes(79, "little")
        payload = {"m_BitSize": 7, "m_NumItems": len(deltas), "m_Data": list(packed)}
        self.assertEqual(list(packed_times(payload, replay.require)), [i * 33 / 100 for i in range(90)])
        payload["m_Data"] = {"t": "u8s", "hex": packed.hex()}
        self.assertEqual(list(packed_times(payload, replay.require)), [i * 33 / 100 for i in range(90)])

    def test_complete_refuses_packed_time_digest_with_zero_cut_counters(self):
        path, doc = self.clip_show("legacy-explicit")
        packed = bytes(65)
        data = {"t": "u8s", "len": len(packed), "sha256": hashlib.sha256(packed).hexdigest()}
        doc["value"]["v"].append(["m_CompressedRotationCurves", canonical([{
            "m_Times": {"m_BitSize": 8, "m_NumItems": 65, "m_Data": data},
        }])])
        write(path, doc)
        self.assert_refused("incomplete built packed rotation times: canonical show elided the bytes")

    def test_validate_refuses_pending_replay(self):
        write(self.pending_path, self.recordings["legacy-explicit"])
        result = self.run_tool("validate", self.pending_path)
        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertIn("pending replay", result.stderr)
