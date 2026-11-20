"""workloads.json, the table both sides read: its fixtures are the manifest's at both scales, its PARITY.md rows
exist with the tier it declares (decision 4 pairs only Tier A), and its operations are py_workloads.py's; and
py_workloads.py itself, on a fake implementation: the scope's result, its digest and a side that raises.
Standard library only."""

import contextlib
import io
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


class Entry:
    """A non-serialized entry whose `bytes` copies, as both sides' readers do; `reads` counts the copies."""

    def __init__(self, data):
        self.data, self.reads = data, 0

    @property
    def bytes(self):
        self.reads += 1
        return bytes(self.data)


def fake(objects, loads=None, entries=()):
    """`sys.modules` entries for a UnityPy whose `load` gives one bundle holding one SerializedFile of `objects`
    and the `entries` (each call counted in `loads`; data b"fail" raises), with the modules `warm` imports."""
    bundle = types.SimpleNamespace(files={"CAB-x": SerializedFile(objects),
                                          **{f"CAB-{i}.resS": e for i, e in enumerate(entries)}})

    def load(data):
        if loads is not None:
            loads.append(data)
        if data == b"fail":
            raise OSError("setup failed")
        return types.SimpleNamespace(files={"x": bundle})

    module = types.SimpleNamespace(load=load, __name__="UnityPy")
    return {"UnityPy": module, "UnityPy.classes": types.ModuleType("UnityPy.classes"),
            "UnityPy.export": types.ModuleType("UnityPy.export"), "PIL.Image": types.ModuleType("PIL.Image")}


class ScriptTests(unittest.TestCase):
    def setUp(self):
        import scratch

        self.fixture = scratch.for_case(self, "unityrs-bench-") / "input.bundle"
        self.fixture.write_bytes(b"synthetic")

    def run_with(self, objects, op):
        with mock.patch.dict(sys.modules, fake(objects)):
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

    def test_setup_warms_only_what_the_op_needs_before_the_timer(self):
        """The load ops load the fixture once more, untimed; only the texture op imports, what that side's first
        read would (UnityPy's `export` package too); the typetree op warms nothing."""
        texture = list(py_workloads.TEXTURE_IMPORTS["UnityPy"])
        self.assertNotIn("unityrs.export", py_workloads.TEXTURE_IMPORTS["unityrs"])
        for op, want_loads, want_imports in (("load", 2, []), ("load_entries", 2, []), ("typetree", 1, []),
                                             ("texture", 1, texture)):
            loads, imported = [], []
            with mock.patch.dict(sys.modules, fake([Reader({}, Image())], loads)), \
                    mock.patch.object(py_workloads.importlib, "import_module", imported.append):
                self.assertEqual(py_workloads.run("unitypy", op, self.fixture)["status"], "ok", op)
            self.assertEqual((len(loads), imported), (want_loads, want_imports), op)

    def test_the_entry_op_reads_each_entry_once(self):
        """`bytes` copies on both sides: the scope reads each entry's exactly once."""
        entries = [Entry(b"abc"), Entry(b"de")]
        with mock.patch.dict(sys.modules, fake([], entries=entries)):
            got = py_workloads.run("unitypy", "load_entries", self.fixture)
        self.assertEqual((got["counts"]["entries"], got["counts"]["entry_bytes"]), (2, 5))
        self.assertEqual([e.reads for e in entries], [1, 1])

    def test_a_side_that_raises_is_a_result_with_its_class_name_only(self):
        """The message, which can carry paths or data, goes to stderr only; an odd class name is not published."""
        err = io.StringIO()
        with contextlib.redirect_stderr(err):
            got = self.run_with([Reader(ValueError("bad image mode /private/x"))], "typetree")
            odd = self.run_with([Reader(type("odd name", (Exception,), {})("x"))], "typetree")
        self.assertEqual((got["status"], got["digest"]), ("raised:ValueError", None))
        self.assertEqual(odd["status"], "raised:Exception")
        self.assertIn("bad image mode", err.getvalue())

    def test_a_failing_setup_is_not_a_result(self):
        self.fixture.write_bytes(b"fail")
        for op in ("load", "typetree"):
            with mock.patch.dict(sys.modules, fake([])), self.assertRaises(OSError):
                py_workloads.run("unitypy", op, self.fixture)

    def test_the_script_prints_one_json_line(self):
        out = []
        with mock.patch.dict(sys.modules, fake([Reader({"x": 1})])), \
                mock.patch("builtins.print", lambda text: out.append(text)):
            self.assertEqual(py_workloads.main(["--impl", "unitypy", "typetree-objects-lz4", str(self.fixture)]), 0)
        self.assertEqual(json.loads(out[0])["status"], "ok")


if __name__ == "__main__":
    unittest.main()
