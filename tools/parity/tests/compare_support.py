"""What compare.py's test modules share, not a test module: hand-built records and dumps, and running
compare.py on them (CompareCase, the base of each module's CompareTests).
"""

import contextlib
import copy
import gzip
import io
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import canonical as C  # noqa: E402
import compare  # noqa: E402

LEVELS = ["L0", "L1", "L2", "L3"]
SIDES = ("reference", "candidate")


def header(levels=LEVELS, version=C.VERSION, producer=None):
    return {
        "rec": "dump",
        "format": C.FORMAT,
        "version": version,
        "levels": levels,
        "producer": producer or {"name": "test"},
    }


def type_rec():
    return {
        "class_id": 4,
        "is_stripped_type": False,
        "script_type_index": -1,
        "script_id": None,
        "old_type_hash": "00" * 16,
        "nodes": [[0, "Transform", "Base", -1, 1, 0, 0, 0, 0, None]],
        "type_dependencies": None,
        "class_name": None,
        "namespace": None,
        "assembly": None,
    }


def file_rec(name="a.bundle"):
    return {
        "rec": "file",
        "file": [name],
        "l0": {"type": "AssetsFile", "parsed_as": "SerializedFile", "size": 10, "sha256": "00" * 32},
        "l1": {
            "kind": "serialized",
            "header": {
                "metadata_size": 1,
                "file_size": 10,
                "version": 22,
                "data_offset": 2,
                "endian": "<",
                "reserved": "000000",
                "unknown": 0,
            },
            "unity_version": "2021.3.2f1",
            "version_packed": 0x7E5000300020301,
            "target_platform": 19,
            "enable_type_tree": True,
            "big_id_enabled": 0,
            "types": [type_rec()],
            "object_order": [-5, 7],
            "script_types": [],
            "externals": [],
            "ref_types": [],
            "user_information": "",
            "container": [],
        },
        "error": None,
    }


def bundle_rec(name="b.bundle"):
    return {
        "rec": "file",
        "file": [name],
        "l0": {"type": "BundleFile", "parsed_as": "BundleFile", "size": 10, "sha256": "11" * 32},
        "l1": {
            "kind": "bundle",
            "signature": "UnityFS",
            "format": 6,
            "version_player": "5.x.x",
            "version_engine": "2018.4.11f1",
            "engine_version_packed": 0x7E20004000B0301,
            "flags": 67,
            "flags_class": "ArchiveFlagsOld",
            "block_alignment": False,
            "encrypted": False,
            "hash": None,
            "crc": None,
            "blocks_info": [153, 88, 67],
            "blocks": [[93300, 76633, 3]],
            "directory": [[0, 10, 4, "CAB-x"]],
        },
        "error": None,
    }


def obj_rec(path_id, value, name="a.bundle", root="Transform"):
    return {
        "rec": "object",
        "file": [name],
        "path_id": path_id,
        "l1": {"type_id": 0, "class_id": 4, "byte_start": 0, "byte_size": 8, "is_destroyed": None, "is_stripped": None},
        "l2": {"root_type": root, "source": "embedded", "error": None, "value": value},
        "l3": {
            "raw_size": 8,
            "raw_sha256": "11" * 32,
            "resave": "equal",
            "resave_size": None,
            "resave_sha256": None,
            "first_diff": None,
        },
    }


def f32(bits):
    return {"t": "f32", "v": f"{bits:08x}"}


def S(*items):
    """A typetree struct: a dict value from (name, value) pairs."""
    return {"t": "dict", "v": [list(i) for i in items]}


def end(files, objects):
    return {"rec": "end", "files": files, "objects": objects}


def base_records():
    v = S(["x", f32(0x3F800000)], ["list", [1, 2]], ["fs", {"t": "f32s", "v": "3f80000040000000"}])
    return [file_rec(), obj_rec(-5, v), obj_rec(7, copy.deepcopy(v)), end(1, 2)]


def one_object(value):
    return [file_rec(), obj_rec(1, value), end(1, 1)]


class CompareCase(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)

    def write(self, name, records, head=None, lines=None, raw=None):
        path = Path(self.tmp.name) / name
        if raw is None:
            body = lines if lines is not None else [C.dumps(r) for r in records]
            raw = ("\n".join([C.dumps(head or header()), *body]) + "\n").encode()
        if name.endswith(".gz"):
            raw = gzip.compress(raw, mtime=0)
        path.write_bytes(raw)
        return str(path)

    def run_compare(self, a, b, **kw):
        report, *_ = compare.run(a, b, **kw)
        return report, report.as_dict()

    def paths(self, d):
        return {(r["level"], r["path"], r["kind"]) for r in d["by_path"]}

    def cli(self, *argv):
        """compare.main's exit status; its stdout and stderr land in self.output and self.errors."""
        with contextlib.redirect_stdout(io.StringIO()) as out, contextlib.redirect_stderr(io.StringIO()) as err:
            rc = compare.main([*map(str, argv)])
        self.output, self.errors = out.getvalue(), err.getvalue()
        return rc
