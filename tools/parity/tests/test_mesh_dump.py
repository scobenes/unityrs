"""The mesh corpus protocol is exact in bits, tags, topology, keys and masks."""

import hashlib
import importlib.util
import json
import struct
import sys
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import mesh_dump as M

HAVE_ORACLE = importlib.util.find_spec("UnityPy") is not None


class MeshDumpTests(unittest.TestCase):
    def test_array_digest_has_framing_bits_and_type_tags(self):
        data = b"array-v1\0" + struct.pack(">QQ", 1, 3)
        data += b"i" + struct.pack(">q", 0)
        data += b"f" + struct.pack(">d", -0.0)
        data += b"f" + struct.pack(">d", float("inf"))
        self.assertEqual(M.array_digest([[0, -0.0, float("inf")]]), hashlib.sha256(data).hexdigest())
        self.assertNotEqual(M.array_digest([[0]]), M.array_digest([[0.0]]))
        self.assertNotEqual(M.array_digest([[0.0]]), M.array_digest([[-0.0]]))
        self.assertNotEqual(M.array_digest([[1, 2]]), M.array_digest([[1], [2]]))

    def test_nan_payload_is_preserved(self):
        a, b = (struct.unpack(">d", bytes.fromhex(x))[0] for x in ("7ff8000000000001", "7ff8000000000002"))
        self.assertNotEqual(M.array_digest([[a]]), M.array_digest([[b]]))

    def test_triangles_preserve_cut_empty_counts_and_strip_counts(self):
        a = M.triangles_digest([[(0, 1, 2), (3,), ()]], [7])
        self.assertNotEqual(a, M.triangles_digest([[(0, 1, 2), (3,)]], [7]))
        self.assertNotEqual(a, M.triangles_digest([[(0, 1, 2), (3,), ()]], [8]))
        self.assertNotEqual(a, M.triangles_digest([[(0, 2, 1), (3,), ()]], [7]))

    def test_arrays_cover_all_optional_attributes_and_flat_bind_pose(self):
        attrs = {name: None for name in M.ATTRS}
        attrs.update(
            m_Vertices=[[1.0, 2.0, 3.0]],
            m_UV7=[[0, 1]],
            m_BindPose=[[[1.0, 0.0], [], [], []]],
            m_VertexCount=1,
            m_Use16BitIndices=True,
            m_IndexBuffer=[0, 1, 2],
        )
        result = M.arrays(SimpleNamespace(**attrs))
        self.assertEqual(set(result["attrs"]), {"m_Vertices", "m_UV7", "m_BindPose"})
        self.assertEqual(result["attrs"]["m_BindPose"], M.array_digest([[1.0, 0.0]]))
        self.assertNotEqual(M.index_digest([]), M.index_digest([0]))

    def test_json_keeps_unicode_literal_and_invalid_names_as_hex(self):
        self.assertEqual(M.name(b"caf\xc3\xa9"), "café")
        self.assertEqual(M.name(b"\xff"), {"t": "str", "hex": "ff"})
        self.assertIn("café", M.dumps({"input": "café"}))
        self.assertNotIn("\\u00e9", M.dumps({"input": "café"}))

    @unittest.skipUnless(HAVE_ORACLE, "requires the pinned UnityPy oracle")
    def test_committed_synthetic_meshes_replay_the_pinned_obj_answers(self):
        import scratch

        fixture = Path(__file__).resolve().parents[3] / "crates/unityrs-mesh/tests/fixtures/obj.json"
        checked = 0
        with scratch.folder("mesh-dump-") as root:
            path = root / "mesh.assets"
            for case in json.loads(fixture.read_text())["cases"]:
                if case["materials"] is not None:
                    continue
                path.write_bytes(bytes.fromhex(case["input"]))
                rows = M.records(path, "mesh.assets")
                self.assertEqual(len(rows), 1)
                expected = case["oracle"]
                if "error" in expected:
                    self.assertEqual(rows[0]["result"], expected)
                else:
                    value = expected["value"]
                    self.assertEqual(
                        rows[0]["result"]["obj"],
                        hashlib.sha256(bytes.fromhex(value)).hexdigest() if value is not False else None,
                    )
                checked += 1
        self.assertGreater(checked, 0)

    @unittest.skipUnless(HAVE_ORACLE, "requires the pinned UnityPy oracle")
    def test_resource_and_memory_refusals_propagate_instead_of_becoming_mesh_rows(self):
        import UnityPy

        for error in (FileNotFoundError(), PermissionError(), MemoryError()):
            obj = SimpleNamespace(type=SimpleNamespace(value=43), parse_as_object=mock.Mock(side_effect=error))
            with (
                self.subTest(error=type(error).__name__),
                mock.patch.object(
                    UnityPy, "Environment", return_value=SimpleNamespace(objects=[obj], load_file=mock.Mock())
                ),
                self.assertRaises(type(error)),
            ):
                M.records("mesh.assets", "mesh.assets", b"original")

    @unittest.skipUnless(HAVE_ORACLE, "requires the pinned UnityPy oracle")
    def test_records_parse_the_bytes_that_were_hashed_without_reopening_the_source(self):
        import UnityPy

        env = SimpleNamespace(objects=[], load_file=mock.Mock())
        with (
            mock.patch.object(UnityPy, "Environment", return_value=env),
            mock.patch.object(Path, "read_bytes", side_effect=AssertionError("source reopened")),
        ):
            self.assertEqual(M.records("/source/mesh.assets", "mesh.assets", b"original"), [])
        env.load_file.assert_called_once_with(b"original", name="mesh.assets")
        self.assertEqual(env.path, "/source")

    def test_comparison_masks_only_exact_named_field(self):
        a = {"input": "in", "file": "ab", "path_id": 1, "result": {"obj": "a", "triangles": "t"}}
        b = json.loads(json.dumps(a))
        b["result"]["obj"] = "b"
        mask = [{"key": ["in", "ab", 1], "path": ["result", "obj"]}]
        self.assertEqual(M.compare([a], [b], mask), {"meshes": 1, "masked": 1, "mismatches": 0})
        b["result"]["triangles"] = "wrong"
        self.assertEqual(M.compare([a], [b], mask)["mismatches"], 1)

    def test_comparison_keeps_json_numeric_and_boolean_types_distinct(self):
        row = {"input": "in", "file": "ab", "path_id": 1, "result": {"vertex_count": 1}}
        for value in (True, 1.0):
            other = json.loads(json.dumps(row))
            other["result"]["vertex_count"] = value
            self.assertEqual(M.compare([row], [other])["mismatches"], 1)

    def test_compare_rejects_missing_duplicate_and_stale_masks(self):
        a = {"input": "in", "file": "ab", "path_id": 1, "result": {"obj": None}}
        with self.assertRaises(ValueError):
            M.compare([a, a], [a])
        self.assertEqual(M.compare([a], [])["mismatches"], 1)
        with self.assertRaises(ValueError):
            M.compare([a], [a], [{"key": ["in", "ab", 1], "path": ["result", "obj"]}])


if __name__ == "__main__":
    unittest.main()
