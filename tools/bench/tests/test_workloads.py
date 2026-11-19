"""workloads.json, the table both sides read: its fixtures are the manifest's at both scales, its PARITY.md rows
exist with the tier it declares (decision 4 pairs only Tier A), and its operations are py_workloads.py's; and
py_workloads.py itself, on a fake implementation: the scope's result, its digest and a side that raises.
Standard library only."""

import json
import re
import sys
import types
import unittest
from unittest import mock

from support import FIXTURES, HERE, ROOT, WORKLOADS

sys.path.insert(0, str(HERE))
import py_workloads  # noqa: E402

OPS = {"load", "load_entries", "typetree", "texture"}


def parity_tier(section: str, item: str) -> str | None:
    """The tier PARITY.md gives the row `item` (its first or second cell) of section `section`."""
    lines = (ROOT / "PARITY.md").read_text(encoding="utf-8").splitlines()
    head = re.compile(rf"^#{{2,3}} {re.escape(section)}\.? ")
    start = next(i for i, line in enumerate(lines) if head.match(line))
    header = None
    for line in lines[start + 1 :]:
        if line.startswith("#"):
            break
        if not line.startswith("|"):
            header = None
            continue
        cells = [c.strip() for c in line.strip().strip("|").split(" | ")]
        if header is None:
            header = cells
        elif item in cells[:2] and "tier" in header:
            return cells[header.index("tier")]
    return None


class TableTests(unittest.TestCase):
    def test_names_are_unique_and_every_category_and_op_is_known(self):
        names = [w["name"] for w in WORKLOADS["workloads"]]
        self.assertEqual(len(names), len(set(names)))
        cats = {c["name"] for c in WORKLOADS["categories"]}
        for w in WORKLOADS["workloads"]:
            self.assertIn(w["category"], cats, w["name"])
            self.assertIn(w["op"], OPS, w["name"])
            self.assertIn(w["tier"], ("A", "B", "C", "U"), w["name"])
        self.assertEqual((WORKLOADS["rounds"], WORKLOADS["warmup"], WORKLOADS["quick_rounds"]), (7, 1, 3))

    def test_every_fixture_is_the_manifests_at_both_scales_and_each_is_timed(self):
        for scale in ("full", "gate"):
            files = {f["name"] for f in FIXTURES["scales"][scale]["files"]}
            used = {w["fixture"] for w in WORKLOADS["workloads"]}
            self.assertEqual(used, files, scale)

    def test_every_texture_workload_names_its_format_and_fixture(self):
        inv = FIXTURES["scales"]["full"]["inventories"]
        for w in WORKLOADS["workloads"]:
            if w["op"] == "texture":
                fmt = inv[w["fixture"]]["format"]
                self.assertEqual((w["name"], w["parity_row"]), (f"texture-{fmt}", {"section": "17", "item": f"`{fmt}`"}))

    def test_every_parity_row_exists_with_the_declared_tier(self):
        for w in WORKLOADS["workloads"]:
            row = w["parity_row"]
            tier = parity_tier(row["section"], row["item"])
            self.assertIsNotNone(tier, w["name"])
            self.assertEqual(tier.split(";")[0].strip(), w["tier"], w["name"])

    def test_the_tier_reader_finds_a_known_row_and_misses_an_absent_one(self):
        self.assertEqual(parity_tier("17", "`BC6H`"), "B")
        self.assertIsNone(parity_tier("17", "`NoSuchFormat`"))


class Reader:
    def __init__(self, value, image=None):
        self.value, self.path_id, self.class_id, self.byte_size = value, 7, 28, 40
        self.type = types.SimpleNamespace(name="Texture2D" if image else "TextAsset")
        self.image = image

    def parse_as_dict(self):
        if isinstance(self.value, Exception):
            raise self.value
        return self.value

    def read(self):
        return self


class SerializedFile:
    def __init__(self, objects):
        self.objects = {o.path_id + i: o for i, o in enumerate(objects)}


class Image:
    mode, size = "RGBA", (2, 1)

    def tobytes(self):
        return b"\x01\x02\x03\x04" * 2


def fake(objects):
    """A module with UnityPy's `load` over one bundle holding one SerializedFile of `objects`."""
    bundle = types.SimpleNamespace(files={"CAB-x": SerializedFile(objects)})
    return types.SimpleNamespace(load=lambda data: types.SimpleNamespace(files={"x": bundle}))


class ScriptTests(unittest.TestCase):
    def setUp(self):
        import scratch

        self.fixture = scratch.for_case(self, "unityrs-bench-") / "input.bundle"
        self.fixture.write_bytes(b"synthetic")

    def run_with(self, objects, op):
        with mock.patch.dict(sys.modules, {"UnityPy": fake(objects)}):
            return py_workloads.run("unitypy", op, self.fixture)

    def test_each_op_digests_its_outputs_and_counts_them(self):
        value = {"m_Name": "a", "f": 0.1, "t": (1, 2), "l": [1, 2], "b": b"\x00", 3: None}
        got = self.run_with([Reader(value)], "typetree")
        self.assertEqual((got["status"], got["counts"]), ("ok", {"source_bytes": 9, "objects": 1}))
        again = self.run_with([Reader(dict(value))], "typetree")
        self.assertEqual(got["digest"], again["digest"])
        for other in ({**value, "t": [1, 2]}, {**value, "b": "\x00"}, {**value, "f": 0.1000000001}):
            self.assertNotEqual(self.run_with([Reader(other)], "typetree")["digest"], got["digest"], other)
        tex = self.run_with([Reader({}, Image()), Reader({})], "texture")
        self.assertEqual(tex["counts"], {"source_bytes": 9, "textures": 1, "pixels": 2})
        load = self.run_with([Reader({})], "load")
        self.assertEqual(load["counts"]["objects"], 1)

    def test_a_side_that_raises_is_a_result_with_no_digest(self):
        got = self.run_with([Reader(ValueError("bad image mode\nmore"))], "typetree")
        self.assertEqual((got["status"], got["digest"]), ("ValueError: bad image mode", None))

    def test_the_script_prints_one_json_line(self):
        out = []
        with mock.patch.dict(sys.modules, {"UnityPy": fake([Reader({"x": 1})])}), \
                mock.patch("builtins.print", lambda text: out.append(text)):
            self.assertEqual(py_workloads.main(["--impl", "unitypy", "typetree-objects-lz4", str(self.fixture)]), 0)
        self.assertEqual(json.loads(out[0])["status"], "ok")


if __name__ == "__main__":
    unittest.main()
