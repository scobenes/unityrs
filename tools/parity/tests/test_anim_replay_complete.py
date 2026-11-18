"""Complete an Editor handoff using synthetic unityrs inspect and show documents."""

import copy
import hashlib
import json
import struct
import sys
import unittest
from pathlib import Path

from test_anim_replay import bits, human_capture, identity, synthetic

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))

import anim_replay as replay  # noqa: E402
import child  # noqa: E402
import scratch  # noqa: E402

ROOT_ID = -9223372036854775808
STATIC = dict.fromkeys(("static-primitives", "static-materials", "static-lod", "static-skin"), "static-objects")
STATIC["static-batch-scene"] = "static-batch-scene"
REPLAYS = (
    "legacy-explicit", "generic-quaternion", "generic-euler", "generic-constant", "generic-offset-stepped",
    "generic-weighted", "generic-morph", "generic-object-reference", "generic-override-morph",
    "generic-null-override", "generic-scale", "humanoid-compact-body-motion",
    *(f"humanoid-{rig}-muscles{offset}" for rig in ("compact", "tall") for offset in ("", "-offset")),
)
DEFERRED = (
    "urp-materials", "packed-rotation-slopes", "authored-runtime-precedence", "mixed-transform-storage",
    "humanoid-twist-refusal", "humanoid-non-neutral-setting-refusal", "humanoid-slot-axis-inventory",
    "humanoid-root-hips-comparison", "integer-discrete-binding",
)
VECTORS = tuple(f"{name}-known-vectors" for name in REPLAYS if "-muscles" in name)


def pointer(path_id, file_id=0):
    wire_id = path_id if abs(path_id) <= 2**53 else {"t": "i64", "v": str(path_id)}
    return {"m_FileID": file_id, "m_PathID": wire_id}


def canonical(value):
    if isinstance(value, dict) and "t" not in value:
        return {"t": "dict", "v": [[key, canonical(item)] for key, item in value.items()]}
    if isinstance(value, list):
        return [canonical(item) for item in value]
    if type(value) is float:
        return {"t": "f32", "v": struct.pack(">f", value).hex()}
    return value


class CompleteReplayTests(unittest.TestCase):
    def setUp(self):
        self.folder = scratch.for_case(self)
        self.inspect = self.folder / "inspection"
        self.replays = self.folder / "replays"
        self.bundles = self.folder / "bundles"
        for folder in (self.inspect, self.replays, self.bundles):
            folder.mkdir()
        self.bundle = self.bundles / "synthetic"
        data = b"synthetic bundle bytes"
        self.bundle.write_bytes(data)
        self.digest = hashlib.sha256(data).hexdigest()

        self.pending_path = self.replays / "legacy-explicit.pending.json"
        self.complete_path = self.replays / "legacy-explicit.json"
        for bundle in set(STATIC.values()):
            (self.bundles / bundle).write_bytes(b"static bundle")
        self.run_path = self.folder / "run.json"
        self.run = dict(format="unityrs.fixture_run", version=1, cases=[
            dict(name=name, status="recorded", reason="") for name in (*STATIC, *REPLAYS, *VECTORS)
        ] + [dict(name=name, status="not_covered", reason="Separate fixture needed") for name in DEFERRED])
        self.pending = synthetic()
        self.pending["format"] = "unityrs.anim_replay_pending"
        spec, capture = self.pending["spec"], self.pending["capture"]
        capture["bundle_sha256"] = [self.digest]
        spec["bundles"] = [{"name": "synthetic"}]
        for field in ("prefab", "clip", "binding", "avatar"):
            del spec[field]
        del spec["nodes"][0]["id"]
        for renderer in spec["renderers"]:
            del renderer["id"], renderer["mesh"]
            renderer["mesh_group"] = 0
        del spec["evidence"]["discovery"], spec["evidence"]["import_settings"]
        spec["evidence"]["layout"] = "editor_authored"
        spec["held_boundaries"] = [2.125]
        spec["evidence"]["clock"]["key_times"] = [2, 2.125, 3]
        spec["evidence"]["clock"]["held"][0]["source"] = 2.125
        spec["evidence"]["editor"] = {
            "bindings": [{"path": "", "property": "blendShape.Lift"}], "expected_discovery": "null_override"
        }

        headers = []
        for path_id, class_id, name, value in (
            (1, 1, "Synthetic", {"m_Component": [{"component": pointer(i)} for i in (ROOT_ID, 3, 7, 8)]}),
            (ROOT_ID, 4, None, {"m_GameObject": pointer(1), "m_Father": pointer(0), "m_Children": []}),
            (2, 74, "Synthetic", {"m_SampleRate": 60.0, "m_Legacy": False, "m_MuscleClip": {
                "m_StartTime": 2.0, "m_StopTime": 3.0, "m_LoopTime": False, "m_CycleOffset": .1, "m_Clip": {"data": {
                    "m_StreamedClip": {"curveCount": 1, "data": [0x40080000, 1, 0, 0, 0, 0, 0,
                                                               0x40300000, 1, 0, 0, 0, 0, 0]},
                    "m_DenseClip": {"m_CurveCount": 1, "m_FrameCount": 3, "m_BeginTime": 2.125,
                                    "m_SampleRate": 8.0, "m_SampleArray": [0., 1., 2.]},
                    "m_ConstantClip": {"data": []}}}}}),
            # These components deliberately share names or have no names or asset keys.
            (3, 95, None, {"m_GameObject": pointer(1), "m_Controller": pointer(10), "m_Avatar": pointer(0)}),
            (7, 137, None, {"m_GameObject": pointer(1), "m_Mesh": pointer(9)}),
            (8, 137, None, {"m_GameObject": pointer(1), "m_Mesh": pointer(9)}),
            (9, 43, "Shared", {}),
            (10, 221, "Override", {"m_Controller": pointer(11), "m_Clips": [
                {"m_OriginalClip": pointer(2), "m_OverrideClip": pointer(0)}]}),
            (11, 91, "Controller", {"m_AnimationClips": [pointer(2)]}),
            (12, 142, "Bundle", {"m_Container": [
                {"t": "tuple", "v": [spec["prefab_asset"], canonical({"asset": pointer(1)})]},
                {"t": "tuple", "v": [spec["clip_asset"], canonical({"asset": pointer(2)})]}]}),
        ):
            header = dict(input=0, file_index=1, file="CAB-synthetic", path_id=path_id,
                          class_id=class_id, name=name, container_path=None)
            headers.append(header)
            self.write_show(header, value)
        (self.inspect / "synthetic.inspect.json").write_text(json.dumps({
            "format": "unityrs.inspect", "version": 1, "bundle_sha256": self.digest,
            "next_cursor": None, "total_objects": len(headers),
            "inputs": [{"path": "synthetic", "files": [
                {"file_index": 0, "file": "synthetic", "kind": "BundleFile"},
                {"file_index": 1, "file": "CAB-synthetic", "kind": "AssetsFile"},
            ]}], "objects": headers,
        }), encoding="utf-8")

    def write_show(self, header, value):
        doc = {"format": "unityrs.show", "version": 1, "bundle_sha256": self.digest,
               "object": header, "value": canonical(value),
               "truncated": dict.fromkeys(("items", "depth", "bytes", "binary", "cuts"), 0)}
        (self.inspect / f"synthetic.1.{header['path_id']}.show.json").write_text(
            json.dumps(doc), encoding="utf-8")

    def run_tool(self, *args):
        return child.run([sys.executable, "-I", str(HERE / "anim_replay.py"), *map(str, args)],
                         capture_output=True, text=True)

    def complete(self, *, missing=(), stale=None):
        self.run_path.write_text(json.dumps(self.run), encoding="utf-8")
        for name in REPLAYS:
            if name not in missing:
                doc = copy.deepcopy(self.pending)
                doc["spec"]["evidence"]["editor"]["fixture_case"] = "generic-euler" if name == stale else name
                (self.replays / f"{name}.pending.json").write_text(json.dumps(doc), encoding="utf-8")
        vectors = self.replays / "known-vectors"
        vectors.mkdir(exist_ok=True)
        for name in VECTORS:
            if name not in missing:
                doc = copy.deepcopy(self.pending)
                doc["spec"]["mode"] = "human_pose"
                doc["spec"]["evidence"]["editor"]["fixture_case"] = name
                doc["capture"]["humanoid"] = human_capture()
                doc["capture"]["playback"].update(api="HumanPoseHandler.SetHumanPose", clip_human_motion=True)
                for point, sample in zip(doc["spec"]["schedule"], doc["capture"]["samples"]):
                    point["muscles"] = [0]
                    sample["muscles_f32"] = [bits(0)]
                (vectors / f"{name}.pending.json").write_text(json.dumps(doc), encoding="utf-8")
        return self.run_tool("complete", self.folder, "--inspect-dir", self.inspect)

    def test_complete_resolves_exact_ids_from_container_and_pointers(self):
        result = self.complete()
        self.assertEqual(result.returncode, 0, result.stderr)
        finished = replay.read_json(self.complete_path.read_text(encoding="utf-8"))
        self.assertFalse(replay.validate(finished, self.bundles))
        spec = finished["spec"]
        self.assertEqual(finished["capture"], self.pending["capture"])
        self.assertEqual(spec["prefab"], identity(1))
        self.assertEqual(spec["clip"], identity(2))
        self.assertEqual(spec["binding"], identity(3))
        self.assertIsNone(spec["avatar"])
        self.assertEqual(spec["nodes"][0]["id"], identity(ROOT_ID))
        self.assertEqual([(r["id"], r["mesh"]) for r in spec["renderers"]],
                         [(identity(i), identity(9)) for i in (7, 8)])
        self.assertIn(dict(owner=identity(10), field="m_OverrideClip", ordinal=0,
                           original=identity(2), resolved=identity(2), outcome="null_override"),
                      spec["evidence"]["discovery"])
        self.assertEqual(spec["evidence"]["editor"], self.pending["spec"]["evidence"]["editor"] | {
            "fixture_case": "legacy-explicit"
        })
        self.assertEqual(spec["evidence"]["layout"], "streamed_dense")
        self.assertEqual(spec["evidence"]["clock"]["key_times"], [2, 2.125, 2.25, 2.375, 2.75, 3])
        self.assertEqual(spec["evidence"]["clock"]["authored_key_times"], [2, 2.125, 3])
        cycle = next(s for s in spec["evidence"]["import_settings"] if s["property"] == "cycleOffset")
        self.assertEqual(cycle["serialized"], struct.unpack(">f", struct.pack(">f", .1))[0])

    def test_complete_refuses_changed_bundle(self):
        self.bundle.write_bytes(b"different bytes")
        result = self.complete()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("bundle digest mismatch", result.stderr)
        self.assertFalse(self.complete_path.exists())

    def test_complete_refuses_wrong_component_address(self):
        self.pending["spec"]["renderers"][1]["component"] = 2
        result = self.complete()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("renderer component", result.stderr)
        self.assertFalse(self.complete_path.exists())

    def test_complete_refuses_a_pending_recording_from_another_case(self):
        result = self.complete(stale="generic-scale")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("pending recording does not identify manifest case generic-scale", result.stderr)
        self.assertFalse(self.complete_path.exists())

    def test_complete_confirms_reported_mixed_transform_storage(self):
        for row in self.run["cases"]:
            if row["name"] == "mixed-transform-storage":
                row.update(status="recorded", reason="Mixed scale vector")
        result = self.complete()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("mixed Transform storage reported by Unity is absent", result.stderr)
        self.assertFalse(self.complete_path.exists())

        path = self.inspect / "synthetic.1.2.show.json"
        doc = json.loads(path.read_text(encoding="utf-8"))
        clip = dict(doc["value"]["v"])
        playback = dict(dict(dict(clip["m_MuscleClip"]["v"])["m_Clip"]["v"])["data"]["v"])
        dict(playback["m_ConstantClip"]["v"])["data"].append(canonical(1.))
        doc["value"]["v"].append(["m_ClipBindingConstant", canonical({"genericBindings": [
            {"typeID": 4, "attribute": 3, "customType": 0}
        ]})])
        path.write_text(json.dumps(doc), encoding="utf-8")
        result = self.complete()
        self.assertEqual(result.returncode, 0, result.stderr)
        scale = json.loads((self.replays / "generic-scale.json").read_text(encoding="utf-8"))
        self.assertTrue(scale["spec"]["evidence"]["mixed_transform_storage"])

    def test_complete_requires_run_manifest(self):
        self.pending_path.write_text(json.dumps(self.pending), encoding="utf-8")
        result = self.run_tool("complete", self.folder, "--inspect-dir", self.inspect)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("run.json", result.stderr)
        self.assertFalse(self.complete_path.exists())

    def test_complete_reports_missing_and_failed_cases(self):
        self.run["cases"] = [row for row in self.run["cases"] if row["name"] != "generic-euler"]
        for row in self.run["cases"]:
            if row["name"] == "static-lod":
                row.update(status="failed", reason="LOD could not build")
        result = self.complete()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("generic-euler", result.stderr)
        self.assertIn("static-lod: failed (LOD could not build)", result.stderr)
        self.assertFalse(self.complete_path.exists())

    def test_complete_rejects_required_skips_and_missing_artifacts(self):
        for row in self.run["cases"]:
            if row["name"] == "generic-scale":
                row.update(status="skipped", reason="Build only")
        result = self.complete(missing=(VECTORS[0], "generic-scale"))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("generic-scale: skipped (Build only)", result.stderr)
        self.assertIn(f"{VECTORS[0]}.pending.json", result.stderr)
        self.assertFalse(self.complete_path.exists())

    def test_complete_refuses_unbound_or_stale_inspections(self):
        for filename, remove in (("synthetic.inspect.json", True), ("synthetic.1.2.show.json", False)):
            with self.subTest(filename=filename):
                path = self.inspect / filename
                original = path.read_text(encoding="utf-8")
                doc = json.loads(original)
                if remove:
                    del doc["bundle_sha256"]
                else:
                    doc["bundle_sha256"] = "0" * 64
                path.write_text(json.dumps(doc), encoding="utf-8")
                result = self.complete()
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(f"inspection bundle digest mismatch: {filename}", result.stderr)
                self.assertFalse(self.complete_path.exists())
                path.write_text(original, encoding="utf-8")

    def test_complete_refuses_truncated_and_cut_show_values(self):
        path = self.inspect / "synthetic.1.2.show.json"
        original = path.read_text(encoding="utf-8")
        for where in ("stats", "tagged", "nested"):
            with self.subTest(where=where):
                doc = json.loads(original)
                cut = {"$unityrs": "cut", "reason": "items", "omitted": 1}
                if where == "stats":
                    doc["truncated"].update(cuts=1, items=1)
                elif where == "tagged":
                    doc["value"]["cut"] = cut
                else:
                    doc["value"]["v"].append(["ignored", [cut]])
                path.write_text(json.dumps(doc), encoding="utf-8")
                result = self.complete()
                self.assertNotEqual(result.returncode, 0)
                self.assertRegex(result.stderr, "truncated show|cut show")
                self.assertFalse(self.complete_path.exists())

    def test_complete_refuses_samples_that_only_miss_authored_keys(self):
        path = self.inspect / "synthetic.1.2.show.json"
        doc = json.loads(path.read_text(encoding="utf-8"))
        clip = dict(doc["value"]["v"])
        playback = dict(dict(dict(clip["m_MuscleClip"]["v"])["m_Clip"]["v"])["data"]["v"])
        dict(playback["m_StreamedClip"]["v"])["data"][7] = 0x40200000
        dense = dict(playback["m_DenseClip"]["v"])
        dense["m_BeginTime"]["v"] = struct.pack(">f", 2.1).hex()
        # Use f32 addition: the first interior sample rounds to this dense key.
        self.pending["spec"]["source_clock"] = "f32_add"
        for sample in self.pending["capture"]["samples"]:
            sample["effective_source"], = struct.unpack(">f", struct.pack(">f", sample["effective_source"]))
        path.write_text(json.dumps(doc), encoding="utf-8")
        result = self.complete()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("off-key sample", result.stderr)
        self.assertFalse(self.complete_path.exists())

    def test_validate_refuses_pending_replay(self):
        self.pending_path.write_text(json.dumps(self.pending), encoding="utf-8")
        result = self.run_tool("validate", self.pending_path)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("pending replay", result.stderr)
