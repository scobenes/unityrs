"""Complete an Editor handoff using synthetic unityrs inspect and show documents."""

import hashlib
import json
import struct
import sys
import unittest
from pathlib import Path

from test_anim_replay import identity, synthetic

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))

import anim_replay as replay  # noqa: E402
import child  # noqa: E402
import scratch  # noqa: E402

ROOT_ID = -9223372036854775808


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

        self.pending_path = self.replays / "synthetic.pending.json"
        self.complete_path = self.replays / "synthetic.json"
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
        spec["evidence"]["editor"] = {
            "bindings": [{"path": "", "property": "blendShape.Lift"}], "expected_discovery": "null_override"
        }

        headers = []
        for path_id, class_id, name, value in (
            (1, 1, "Synthetic", {"m_Component": [{"component": pointer(i)} for i in (ROOT_ID, 3, 7, 8)]}),
            (ROOT_ID, 4, None, {"m_GameObject": pointer(1), "m_Father": pointer(0), "m_Children": []}),
            (2, 74, "Synthetic", {"m_SampleRate": 60.0, "m_Legacy": False, "m_MuscleClip": {
                "m_StartTime": 2.0, "m_StopTime": 3.0, "m_LoopTime": False, "m_CycleOffset": .1, "m_Clip": {"data": {
                    "m_StreamedClip": {"curveCount": 1}, "m_DenseClip": {"m_CurveCount": 0},
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
            "format": "unityrs.inspect", "version": 1, "next_cursor": None, "total_objects": len(headers),
            "inputs": [{"path": "synthetic", "files": [
                {"file_index": 0, "file": "synthetic", "kind": "BundleFile"},
                {"file_index": 1, "file": "CAB-synthetic", "kind": "AssetsFile"},
            ]}], "objects": headers,
        }), encoding="utf-8")

    def write_show(self, header, value):
        doc = {"format": "unityrs.show", "version": 1, "object": header, "value": canonical(value),
               "truncated": dict.fromkeys(("items", "depth", "bytes", "binary", "cuts"), 0)}
        (self.inspect / f"synthetic.1.{header['path_id']}.show.json").write_text(
            json.dumps(doc), encoding="utf-8")

    def run_tool(self, *args):
        return child.run([sys.executable, "-I", str(HERE / "anim_replay.py"), *map(str, args)],
                         capture_output=True, text=True)

    def complete(self):
        self.pending_path.write_text(json.dumps(self.pending), encoding="utf-8")
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
        self.assertEqual(spec["evidence"]["editor"], self.pending["spec"]["evidence"]["editor"])
        self.assertEqual(spec["evidence"]["layout"], "streamed")
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

    def test_validate_refuses_pending_replay(self):
        self.pending_path.write_text(json.dumps(self.pending), encoding="utf-8")
        result = self.run_tool("validate", self.pending_path)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("pending replay", result.stderr)
