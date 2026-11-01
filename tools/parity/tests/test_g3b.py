"""g3b.py: G3b's verdict, each admit checked on its input, against real UnityPy and a stand-in unityrs.

Needs the oracle stack (the re-runs load UnityPy); skipped elsewhere. The oracle's
dump is real: unitypy_dump.py run on the inputs. The unityrs side is a script that
answers each run from a table, so a test states what unityrs writes, bounded and
with each set of limits lifted. What it should write comes from UnityPy too: the
oracle's own records, or, for an input UnityPyBoost dies on, the oracle's records
of the same file with a valid placeholder in place of the bytes that are not UTF-8.
"""

from __future__ import annotations

import argparse
import contextlib
import gzip
import hashlib
import io
import json
import os
import shutil
import struct
import sys
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE / "tests"))

try:
    import unitypy_dump
except ImportError:  # pragma: no cover - outside the oracle image
    unitypy_dump = None

import canonical as C  # noqa: E402
import g3b  # noqa: E402
import scratch  # noqa: E402
import synth as S  # noqa: E402

VERSION = "0.0.0-test"


def assets(tree, *values):
    return S.serialized_file([S.Type(114, tree)], [S.Obj(i + 1, 0, v) for i, v in enumerate(values)])


def tree(child_type=b"int", child_name=b"i"):
    return S.Node("MyThing", "Base", [S.Node(child_type, child_name)])


def deep(levels):
    node = S.Node("int", "x")
    for i in range(levels):
        node = S.Node(f"C{i}", "c", [node])
    return S.Node("MyThing", "Base", [node])


def lzma_bundle(payload, dict_size):
    """A UnityFS bundle (format 8, blocks info stored after the header) of one LZMA block holding the
    SerializedFile `payload` as entry CAB-x, its LZMA header naming `dict_size`."""
    from UnityPy.helpers import CompressionHelper

    block = bytearray(CompressionHelper.compress_lzma(payload))
    struct.pack_into("<I", block, 1, dict_size)
    info = bytes(16) + struct.pack(">i", 1) + struct.pack(">IIH", len(payload), len(block), 1)
    info += struct.pack(">i", 1) + struct.pack(">qqI", 0, len(payload), 4) + b"CAB-x\0"
    head = b"UnityFS\0" + struct.pack(">I", 8) + b"5.x.x\0" + b"2021.3.2f1\0"
    out = bytearray(head + struct.pack(">qIII", 0, len(info), len(info), 0x40))
    out += bytes(-len(out) % 16)
    out += info + block
    struct.pack_into(">q", out, len(head), len(out))
    return bytes(out)


GOOD = assets(tree("int", "i"), struct.pack("<i", 7))
TWO = assets(tree("int", "i"), struct.pack("<i", 7), struct.pack("<i", 9))
BAD_NAME, BAD_NAME_OK = (assets(tree("int", n), bytes(4)) for n in (b"a\xffb", b"aqb"))
BAD_TYPE, BAD_TYPE_OK = (assets(tree(t, "i"), bytes(4)) for t in (b"in\xfft", b"inqt"))
LZMA = lzma_bundle(GOOD, 0xFFFF_FFFF)


def died(sig):
    return f"the process dumping this input died of {sig}"


def lines(text):
    return [line for line in text.split("\n") if line]


def rec_text(rec):
    return C.dumps(rec) + "\n"


def write_dump(path, head, texts):
    """A dump of `texts` ({input: its lines}) in input order, after `head`, with a matching end record."""
    body = "".join(texts[name] for name in sorted(texts))
    kinds = [json.loads(line)["rec"] for line in lines(body)]
    end = {"rec": "end", "files": kinds.count("file"), "objects": kinds.count("object")}
    path.write_text(head + body + rec_text(end))


FAKE = """\
#!{python}
import fnmatch, json, sys
table = json.load(open({table!r}))
a = sys.argv[1:]
glob, out = a[a.index("--name") + 1], a[a.index("-o") + 1]
lifted = a[a.index("--unbounded") + 1] if "--unbounded" in a else ""
producer = {{"name": "unityrs", "unityrs": {version!r}}}
if lifted:
    producer["unbounded"] = lifted.split(",")
head = {{"rec": "dump", "format": "unityrs-canonical-dump", "version": 1,
        "levels": ["L0", "L1", "L2", "L3"], "producer": producer}}
lines = [json.dumps(head, separators=(",", ":"))]
counts = {{"file": 0, "object": 0}}
for name in sorted(table):
    if fnmatch.fnmatchcase(name, glob):
        for line in table[name].get(lifted, table[name][""]).split("\\n"):
            if line:
                counts[json.loads(line)["rec"]] += 1
                lines.append(line)
lines.append(json.dumps({{"rec": "end", "files": counts["file"], "objects": counts["object"]}}, separators=(",", ":")))
open(out, "w").write("\\n".join(lines) + "\\n")
"""


def oracle_producer():
    """The `producer` member unitypy_dump.py writes on the pinned stack (oracle_stack.py's pins)."""
    return {
        "name": C.ORACLE_PRODUCER,
        "unitypy": C.ORACLE_UNITYPY,
        "boost": True,
        "boost_sha256": C.ORACLE_BOOST_SHA256,
        "packages": C.oracle_pins(),
        "records": C.oracle_records(),
        "python": "3.11",
    }


class OracleReferenceTests(unittest.TestCase):
    """Every gate's comparison refuses a reference the oracle did not write on the pinned stack: compare.py with
    `--oracle` (G1, G3) and G3b, which always asks for it. Without the flag, compare.py takes another producer's
    dump as it is (a comparison of two unityrs dumps). Standard library only: the dumps agree, so G3b re-runs
    nothing."""

    def setUp(self):
        self.run = scratch.for_case(self, "unityrs-g3b-oracle-")
        (self.run / "in").mkdir()
        self.head = {"rec": "dump", "format": C.FORMAT, "version": 1, "levels": list(C.LEVELS)}
        (self.run / "in" / "x").write_bytes(b"abcd")
        l0 = {"type": "ResourceFile", "parsed_as": "raw", "size": 4, "sha256": hashlib.sha256(b"abcd").hexdigest()}
        self.records = {"x": rec_text({"rec": "file", "file": ["x"], "l0": l0, "l1": None, "error": None})}
        unityrs = {**self.head, "producer": {"name": "unityrs", "unityrs": VERSION}}
        write_dump(self.run / "b.jsonl", rec_text(unityrs), self.records)

    def statuses(self, producer):
        """(compare.py, compare.py --oracle, g3b.py) on a reference whose header holds `producer` (none for None)."""
        head = dict(self.head) if producer is None else {**self.head, "producer": producer}
        a, b = self.run / "a.jsonl", self.run / "b.jsonl"
        write_dump(a, rec_text(head), self.records)
        pair = [str(a), str(b)]
        verdict = [*pair, "--inputs", str(self.run / "in"), "--unityrs", "/bin/false"]
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()) as err:
            got = (g3b.compare.main(pair), g3b.compare.main([*pair, "--oracle"]), g3b.main(verdict))
        return got, err.getvalue()

    def test_the_oracles_dump_on_the_pinned_stack_is_the_only_reference(self):
        statuses, err = self.statuses(oracle_producer())
        self.assertEqual(statuses, (0, 0, 0), err)
        refused = "the reference's producer is"
        for what, producer in {
            "another producer": {"name": "not-unitypy", "unitypy": "0.0", "boost": False},
            "unityrs": {"name": "unityrs", "unityrs": VERSION},
            "no producer": None,
            "not an object": C.ORACLE_PRODUCER,
        }.items():
            with self.subTest(what):
                (plain, gate, verdict), err = self.statuses(producer)
                self.assertEqual((gate, verdict), (2, 2), err)
                self.assertEqual(err.count(refused), 2, err)
                self.assertIn(plain, (0, 1), "without --oracle, compare.py compares")
        # The oracle's name on another stack was refused before, and still is.
        (plain, gate, verdict), err = self.statuses({**oracle_producer(), "unitypy": "1.24.0"})
        self.assertEqual((plain, gate, verdict), (2, 2, 2), err)


class InputCoverageTests(unittest.TestCase):
    """Both dumps hold every file under `--inputs` and nothing else: an input both left out fails G3b, as does
    one only one of them holds, one whose own file record is missing, and one that is no file there. Standard
    library only: no input differs, so G3b re-runs nothing."""

    def setUp(self):
        self.run = scratch.for_case(self, "unityrs-g3b-inputs-")
        (self.run / "in").mkdir()
        self.head = {"rec": "dump", "format": C.FORMAT, "version": 1, "levels": list(C.LEVELS)}

    def record(self, name, data):
        l0 = {"type": "ResourceFile", "parsed_as": "raw", "size": len(data), "sha256": hashlib.sha256(data).hexdigest()}
        return rec_text({"rec": "file", "file": [name], "l0": l0, "l1": None, "error": None})

    def verdict(self, oracle, unityrs):
        """(exit status, report, listed rows) of g3b.py on dumps of `oracle` and `unityrs` ({input: lines})."""
        a, b, listed = self.run / "a.jsonl", self.run / "b.jsonl", self.run / "list.json"
        write_dump(a, rec_text({**self.head, "producer": oracle_producer()}), oracle)
        write_dump(b, rec_text({**self.head, "producer": {"name": "unityrs", "unityrs": VERSION}}), unityrs)
        argv = [str(a), str(b), "--inputs", str(self.run / "in"), "--unityrs", "/bin/false", "--list", str(listed)]
        out = io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(io.StringIO()):
            status = g3b.main(argv)
        rows = {r["input"]: (r["class"], r["detail"]) for r in json.loads(listed.read_text())}
        return status, json.loads(out.getvalue()), rows

    def test_an_input_both_dumps_omit_fails(self):
        (self.run / "in" / "omitted").write_bytes(b"abcd")
        status, report, rows = self.verdict({}, {})
        self.assertEqual((status, report["inputs"]), (1, 1))
        self.assertEqual(rows, {"omitted": ("other", "missing from the unityrs and the oracle dump")})

    def test_every_input_under_the_folder_must_be_in_both_dumps_and_nothing_else(self):
        names = {"a": b"abcd", "b": b"efgh", "c": b"ijkl"}
        for name, data in names.items():
            (self.run / "in" / name).write_bytes(data)
        # A subfolder's files are inputs too, named by their path below the folder.
        (self.run / "in" / "sub").mkdir()
        (self.run / "in" / "sub" / "d").write_bytes(b"mnop")
        names["sub/d"] = b"mnop"
        full = {n: self.record(n, d) for n, d in names.items()}
        status, report, rows = self.verdict(full, full)
        self.assertEqual((status, report["classes"], rows), (0, {"agree": 4}, {}))
        status, _, rows = self.verdict(full, {n: t for n, t in full.items() if n != "b"})
        self.assertEqual((status, rows), (1, {"b": ("other", "missing from the unityrs dump")}))
        status, _, rows = self.verdict({n: t for n, t in full.items() if n != "sub/d"}, full)
        self.assertEqual((status, rows), (1, {"sub/d": ("other", "missing from the oracle dump")}))
        stray = {**full, "stray": self.record("stray", b"qrst")}
        status, _, rows = self.verdict(stray, stray)
        self.assertEqual(
            (status, rows), (1, {"stray": ("other", "in the unityrs and the oracle dump, but no file under --inputs")})
        )
        # An input whose own file record is missing, though a record of a file inside it is there.
        inner = rec_text({**json.loads(full["c"]), "file": ["c", "CAB-x"]})
        status, _, rows = self.verdict(full, {**full, "c": inner})
        self.assertEqual(rows["c"], ("other", "missing from the unityrs dump"))
        self.assertEqual(status, 1)


@unittest.skipIf(unitypy_dump is None, "needs UnityPy (the oracle image)")
class G3bTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        # Removed by a class cleanup, which unittest runs when this method raises too (review round 9: the
        # tearDownClass that removed it never runs then, and a failure here left the folder).
        cls.tmp = scratch.for_class(cls, "unityrs-g3b-")
        cls.inputs = cls.tmp / "in"
        cls.inputs.mkdir()
        files = {
            "agree": GOOD,
            "bad_name": BAD_NAME,
            "bad_name_wrong": BAD_NAME,
            "bad_type": BAD_TYPE,
            # 240 levels: 726 levels of JSON, past Python's recursion limit for a recursive walk.
            "deep": assets(deep(240), struct.pack("<i", 7)),
            "good_crashed": GOOD,
            "limit_and_a_missing_object": TWO,
            "limit_beside_a_wrong_object": TWO,
            "limit_chain": GOOD,
            "limit_in_an_object": TWO,
            "limit_not_reproduced": GOOD,
            "limit_ok": GOOD,
            "limit_still_differs": GOOD,
            "limit_that_does_not_trip": GOOD,
            "lzma": LZMA,
            "lzma_wrong": LZMA,
            "missing": GOOD,
            "wrong_signal": BAD_TYPE,
        }
        for name, data in files.items():
            (cls.inputs / name).write_bytes(data)
        # The oracle's dump, under a cap the LZMA dictionary (4 GiB) does not fit in.
        with open("/proc/self/statm") as f:
            cls.cap = int(f.read().split()[0]) * os.sysconf("SC_PAGE_SIZE") + (1 << 30)
        assert cls.cap < (4 << 30) - (256 << 20), "this process holds too much address space for the test's cap"
        dump = cls.tmp / "oracle.jsonl"
        with contextlib.redirect_stderr(io.StringIO()):
            argv = [str(cls.inputs), "-o", str(dump), "-q", "-j", "4", "--memory-cap", str(cls.cap)]
            status = unitypy_dump.main(argv)
        assert status == 0, status
        head, *recs = dump.read_text().split("\n")
        cls.oracle_head = head + "\n"
        cls.oracle = {}
        for line in recs:
            rec = json.loads(line) if line else {}
            if rec.get("rec") in ("file", "object"):
                cls.oracle[rec["file"][0]] = cls.oracle.get(rec["file"][0], "") + line + "\n"

    def uncapped(self, data, name):
        """The oracle's records of `data` named `name`, dumped in this process without a cap."""
        path = self.tmp / f"uncapped-{name}"
        path.write_bytes(data)
        return unitypy_dump.dump_input((str(path), name, C.LEVELS))[1]

    def placeholder(self, name, bad, ok, raw):
        """unityrs's records of `bad`: UnityPy's of `ok`, `raw`'s placeholder hex-tagged back, `bad`'s l0."""
        text = self.uncapped(ok, name)
        placeholder = raw.replace(b"\xff", b"q").decode()
        text = text.replace(json.dumps(placeholder), json.dumps({"t": "str", "hex": raw.hex()}, separators=(",", ":")))
        return text.replace(hashlib.sha256(ok).hexdigest(), hashlib.sha256(bad).hexdigest())

    def limit_rec(self, name, note):
        l0 = dict(json.loads(lines(self.oracle[name])[0])["l0"], parsed_as="error")
        return rec_text({"rec": "file", "file": [name], "l0": l0, "l1": None, "error": {"kind": "parse", "note": note}})

    def crash_rec(self, name, why):
        return unitypy_dump.crash_result((str(self.inputs / name), name, C.LEVELS), why)[1]

    def g3b(self, table, gate=None, oracle=None, names=None, extra=()):
        """Runs g3b.py on the oracle's dump (with `oracle`'s replacements) and a unityrs dump of `gate`
        (else each table entry's bounded answer), over `names` (default: the table's and the oracle's)."""
        run = scratch.make(within=self.tmp)
        names = set(names or set(table) | set(self.oracle))
        # The inputs folder holds the inputs the dumps are of, and no other: G3b requires each in both.
        (run / "in").mkdir()
        for name in names:
            shutil.copyfile(self.inputs / name, run / "in" / name)
        texts = {n: t for n, t in {**self.oracle, **(oracle or {})}.items() if n in names}
        write_dump(run / "a.jsonl", self.oracle_head, texts)
        gate = {n: t for n, t in {**{n: e[""] for n, e in table.items()}, **(gate or {})}.items() if n in names}
        head = {"rec": "dump", "format": C.FORMAT, "version": 1, "levels": list(C.LEVELS)}
        write_dump(run / "b.jsonl", rec_text({**head, "producer": {"name": "unityrs", "unityrs": VERSION}}), gate)
        (run / "table.json").write_text(json.dumps(table))
        fake = run / "unityrs"
        fake.write_text(FAKE.format(python=sys.executable, table=str(run / "table.json"), version=VERSION))
        fake.chmod(0o755)
        out = io.StringIO()
        argv = [run / "a.jsonl", run / "b.jsonl", "--inputs", run / "in", "--unityrs", fake, "-j", "4"]
        argv += ["--rerun-cap", str(16 << 30), "--list", run / "list.json", *extra]
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(io.StringIO()):
            status = g3b.main([str(a) for a in argv])
        listed = {}
        if (run / "list.json").exists():
            listed = {r["input"]: (r["class"], r["detail"]) for r in json.loads((run / "list.json").read_text())}
        return status, json.loads(out.getvalue()) if out.getvalue() else None, listed

    def table(self):
        """unityrs's answers: as UnityPy, but for the limits each input's name describes."""
        o = self.oracle

        def refused(name):
            return self.limit_rec(name, "limit TreeDepth: 9 > 8")

        def object_refused(name, note, wrong=None):
            """TWO's records with object 1 refused at a limit (its L2 an `other` error, its L3 skipped), and
            with `wrong` applied to object 2's value, or object 2 left out when `wrong` is "drop"."""
            recs = [json.loads(line) for line in lines(o[name])]
            recs[1]["l2"].update(error={"kind": "other", "bytes_read": None, "note": note}, value=None)
            recs[1]["l3"].update(resave="skipped")
            if wrong == "drop":
                del recs[2]
            elif wrong is not None:
                recs[2]["l2"]["value"]["v"][0][1] = wrong
            return "".join(rec_text(r) for r in recs)

        zero_size = "limit ZeroSizeElements: 1048577 > 1048576"
        return {
            "agree": {"": o["agree"]},
            "bad_name": {"": self.placeholder("bad_name", BAD_NAME, BAD_NAME_OK, b"a\xffb")},
            "bad_name_wrong": {"": self.limit_rec("bad_name_wrong", "Unexpected")},
            "bad_type": {"": self.placeholder("bad_type", BAD_TYPE, BAD_TYPE_OK, b"in\xfft")},
            "deep": {"": o["deep"]},
            "good_crashed": {"": o["good_crashed"]},
            # Lifted, each of these two gives UnityPy's records; bounded, a
            # record no refusal covers is wrong (object 2's value 10, not 9)
            # or missing, so the refusal is not the whole difference.
            "limit_and_a_missing_object": {
                "": object_refused("limit_and_a_missing_object", zero_size, "drop"),
                "ZeroSizeElements": o["limit_and_a_missing_object"],
            },
            "limit_beside_a_wrong_object": {
                "": object_refused("limit_beside_a_wrong_object", zero_size, 10),
                "ZeroSizeElements": o["limit_beside_a_wrong_object"],
            },
            "limit_chain": {
                "": refused("limit_chain"),
                "TreeDepth": self.limit_rec("limit_chain", "limit ValueDepth: 600 > 512"),
                "TreeDepth,ValueDepth": o["limit_chain"],
            },
            "limit_in_an_object": {
                "": object_refused("limit_in_an_object", "limit ValueDepth: 513 > 512"),
                "ValueDepth": o["limit_in_an_object"],
            },
            "limit_not_reproduced": {"": o["limit_not_reproduced"]},
            "limit_ok": {"": refused("limit_ok"), "TreeDepth": o["limit_ok"]},
            "limit_still_differs": {
                "": refused("limit_still_differs"),
                "TreeDepth": self.limit_rec("limit_still_differs", "not a limit"),
            },
            "limit_that_does_not_trip": {"": self.limit_rec("limit_that_does_not_trip", "limit TreeDepth: 8 > 8")},
            "lzma": {"": self.uncapped(LZMA, "lzma")},
            "lzma_wrong": {"": self.limit_rec("lzma_wrong", "Unexpected")},
            "wrong_signal": {"": self.placeholder("wrong_signal", BAD_TYPE, BAD_TYPE_OK, b"in\xfft")},
        }

    def test_the_oracle_died_where_expected(self):
        crashes = {n: json.loads(t)["error"] for n, t in self.oracle.items() if '"kind":"crash"' in t}
        self.assertEqual(
            {n: e["note"] for n, e in crashes.items()},
            {
                "bad_name": died("SIGABRT"),
                "bad_name_wrong": died("SIGABRT"),
                "bad_type": died("SIGSEGV"),
                "lzma": unitypy_dump.OUT_OF_MEMORY,
                "lzma_wrong": unitypy_dump.OUT_OF_MEMORY,
                "wrong_signal": died("SIGSEGV"),
            },
        )

    def test_each_admit_is_checked_on_its_input(self):
        oracle = {
            "good_crashed": self.crash_rec("good_crashed", died("SIGSEGV")),
            "wrong_signal": self.crash_rec("wrong_signal", died("SIGABRT")),
        }
        gate = {"limit_not_reproduced": self.limit_rec("limit_not_reproduced", "limit TreeDepth: 9 > 8")}
        status, report, listed = self.g3b(self.table(), gate=gate, oracle=oracle)
        self.assertEqual(
            report["classes"],
            {"agree": 2, "crash_non_utf8": 2, "limit": 3, "memory_bound": 1, "other": 10},
        )
        self.assertEqual((status, report["form"]), (1, {}))
        admitted = {n: r for n, r in listed.items() if r[0] != "other"}
        self.assertEqual(
            admitted,
            {
                "bad_name": ("crash_non_utf8", "m_Name: SIGABRT"),
                "bad_type": ("crash_non_utf8", "m_Type: SIGSEGV"),
                "limit_chain": ("limit", "TreeDepth+ValueDepth"),
                "limit_in_an_object": ("limit", "ValueDepth"),
                "limit_ok": ("limit", "TreeDepth"),
                "lzma": ("memory_bound", "re-run under the larger cap"),
            },
        )
        other = {n: r[1] for n, r in listed.items() if r[0] == "other"}
        uncovered = "a record no limit refusal covers differs, object 2 of limit_"
        expect = {
            # One limit note never admits a difference it does not explain:
            # not a record beside it, though the lifted run is UnityPy's ...
            "limit_beside_a_wrong_object": f"{uncovered}beside_a_wrong_object",
            "limit_and_a_missing_object": f"{uncovered}and_a_missing_object, in the reference only",
            # ... nor a lifted run that is not.
            "limit_still_differs": "records differ with TreeDepth lifted",
            "limit_that_does_not_trip": "a limit note that does not trip",
            "limit_not_reproduced": "the unityrs binary does not reproduce the dump",
            # A crash or memory admit compares unityrs's records with UnityPy's re-run.
            "bad_name_wrong": "records differ (reference: UnityPy's re-run, crash_non_utf8)",
            "lzma_wrong": "records differ (reference: UnityPy's re-run, memory_bound)",
            "good_crashed": "oracle SIGSEGV; the re-run: ok, first member not UTF-8: None",
            "wrong_signal": "oracle SIGABRT; the re-run: ok, first member not UTF-8: m_Type",
            "missing": "missing from the unityrs dump",
        }
        self.assertEqual(set(other), set(expect))
        for name, start in expect.items():
            self.assertTrue(other[name].startswith(start), f"{name}: {other[name]}")
        # The admitted ones alone pass.
        status, report, _ = self.g3b(self.table(), names=[*admitted, "agree", "deep"])
        self.assertEqual((status, report["unadmitted"]), (0, []))

    def test_what_unitypy_cannot_answer_is_beyond_memory(self):
        """The re-run under a cap the dictionary does not fit in either: no reference, and only what checks remain."""
        table = self.table()
        extra = ["--rerun-cap", str(2 << 30)]
        status, report, listed = self.g3b(table, names=["lzma_wrong"], extra=extra)
        self.assertEqual((status, listed), (0, {"lzma_wrong": ("beyond_memory", "after memory_bound")}))
        # A limit refusal there must still trip, and be reproduced.
        gate = {"lzma_wrong": self.limit_rec("lzma_wrong", "limit BlockOutput: 99 > 98")}
        status, report, listed = self.g3b(table, gate=gate, names=["lzma_wrong"], extra=extra)
        self.assertEqual(status, 1)
        self.assertTrue(listed["lzma_wrong"][1].endswith("does not reproduce the dump: other limit refusals []"))
        table["lzma_wrong"] = {"": gate["lzma_wrong"]}
        status, report, listed = self.g3b(table, names=["lzma_wrong"], extra=extra)
        refuses = ("beyond_memory", "after memory_bound; unityrs refuses at BlockOutput")
        self.assertEqual((status, listed), (0, {"lzma_wrong": refuses}))

    def test_a_unityrs_crash_is_never_admitted(self):
        """A unityrs crash record agrees with no oracle crash, though the two records are equal with their notes
        left out (UnityPyBoost's signal, the oracle's MemoryError), and is no admitted difference either."""

        def unityrs_crash(name, note):
            l0 = json.loads(lines(self.oracle[name])[0])["l0"]
            error = {"kind": "crash"} if note is None else {"kind": "crash", "note": note}
            return rec_text({"rec": "file", "file": [name], "l0": l0, "l1": None, "error": error})

        memory = "unityrs ran out of memory dumping this input (memory budget refused 64 bytes for Input)"
        table = {
            "bad_type": {"": unityrs_crash("bad_type", memory)},
            "lzma": {"": unityrs_crash("lzma", memory)},
            "agree": {"": unityrs_crash("agree", None)},
        }
        # The first two are records equal to the oracle's, notes left out.
        for name in ("bad_type", "lzma"):
            self.assertEqual(g3b.digest(json.loads(table[name][""]))[0], g3b.digest(json.loads(self.oracle[name]))[0])
        status, report, listed = self.g3b(table, names=list(table))
        self.assertEqual((status, report["classes"]), (1, {"agree": 0, "unityrs_crash": 3}))
        self.assertEqual(
            listed,
            {
                "agree": ("unityrs_crash", f"unityrs: {g3b.CRASH}; the oracle: no crash"),
                "bad_type": ("unityrs_crash", f"unityrs: {memory}; the oracle: SIGSEGV"),
                "lzma": ("unityrs_crash", f"unityrs: {memory}; the oracle: memory"),
            },
        )

    def test_a_deep_value_is_compared_without_recursion(self):
        status, report, _ = self.g3b({"deep": {"": self.oracle["deep"]}}, names=["deep"])
        self.assertEqual((status, report["classes"]), (0, {"agree": 1}))

    def test_a_malformed_or_incomplete_unityrs_dump_fails(self):
        run = scratch.make(within=self.tmp)
        (run / "in").mkdir()
        shutil.copyfile(self.inputs / "agree", run / "in" / "agree")
        write_dump(run / "a.jsonl", self.oracle_head, {"agree": self.oracle["agree"]})
        head = {"rec": "dump", "format": C.FORMAT, "version": 1, "levels": list(C.LEVELS)}
        recs = [line + "\n" for line in lines(self.oracle["agree"])]
        good = [rec_text({**head, "producer": {"name": "unityrs", "unityrs": VERSION}}), *recs]
        good.append(rec_text({"rec": "end", "files": 1, "objects": len(recs) - 1}))
        cases = {
            "version": (2, [good[0].replace('"version":1', '"version":99'), *good[1:]]),
            "levels": (1, [good[0].replace('"L0","L1","L2","L3"', '"L0"'), *good[1:]]),
            "no end": (1, good[:-1]),
            "end count": (1, [*good[:-1], rec_text({"rec": "end", "files": 2, "objects": len(recs) - 1})]),
            "noncanonical": (1, [good[0], json.dumps(json.loads(good[1])) + "\n", *good[2:]]),
            "lifted": (1, [good[0].replace(f'"{VERSION}"', f'"{VERSION}","unbounded":["TreeDepth"]'), *good[1:]]),
        }
        for what, (expect, parts) in cases.items():
            with self.subTest(what):
                (run / "b.jsonl").write_text("".join(parts))
                argv = [run / "a.jsonl", run / "b.jsonl", "--inputs", run / "in", "--unityrs", "/bin/false"]
                with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                    self.assertEqual(g3b.main([str(a) for a in argv]), expect)
        # Well-formed, it passes.
        (run / "b.jsonl").write_text("".join(good))
        argv = [run / "a.jsonl", run / "b.jsonl", "--inputs", run / "in", "--unityrs", "/bin/false"]
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(g3b.main([str(a) for a in argv]), 0)

    def test_a_gzip_dump_reads_as_its_lines(self):
        run = scratch.make(within=self.tmp)
        write_dump(run / "a.jsonl", self.oracle_head, {"agree": self.oracle["agree"]})
        with gzip.open(run / "a.jsonl.gz", "wb") as f:
            f.write((run / "a.jsonl").read_bytes())
        self.assertEqual(g3b.load(run / "a.jsonl.gz").keys(), g3b.load(run / "a.jsonl").keys())

    def test_a_limit_refusal_is_read_wherever_the_dump_writes_it(self):
        """Each form of note the unityrs dump writes for a limit refusal (`crates/unityrs-dump`'s
        `every_limit_note_is_in_g3bs_grammar` pins the detection and read ones), and the old detection form,
        which named no value, read as no refusal."""
        cases = {
            "limit TreeDepth: 257 > 256": [("TreeDepth", 257, 256)],
            "detection: limit Inflate: 5120 > 1000": [("Inflate", 5120, 1000)],
            "entry CAB-x: detection: limit Inflate: 5120 > 1000": [("Inflate", 5120, 1000)],
            "limit InputBytes: 5000 > 1000": [("InputBytes", 5000, 1000)],
            "detection: limit DecodeWork: 5120 > 1000": [("DecodeWork", 5120, 1000)],
            "its sha256 was not taken: limit DecodeWork: 5000 > 4999": [("DecodeWork", 5000, 4999)],
            "limit InputBytes: 200000 > 1000; its sha256 was not taken: limit DecodeWork: 196608 > 163840": [
                ("InputBytes", 200000, 1000),
                ("DecodeWork", 196608, 163840),
            ],
            "limit Inflate: a gzip stream inflates past 1000 bytes": [],
        }
        for note, want in cases.items():
            rec = {"rec": "file", "file": ["x"], "l0": {}, "l1": None, "error": {"kind": "detect", "note": note}}
            self.assertEqual(g3b.digest(rec)[1], want, note)

    def test_the_inputs_are_the_files_the_oracle_dumps_from_the_folder(self):
        """g3b.py's inputs are the names unitypy_dump.py's collect_inputs gives a folder: subfolders, a link to a
        file (named after itself), a link to a folder (not entered), names that are not UTF-8, glob characters."""
        root = scratch.make(within=self.tmp)
        for rel in (b"a/b", b"a/c/d", b"\xff\xfe", b"x[1]*?", b"z"):
            path = root / os.fsdecode(rel)
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(rel)
        (root / "empty").mkdir()
        os.symlink("a/b", root / "link_file")
        os.symlink("a", root / "link_dir")
        args = argparse.Namespace(
            input=str(root), index=False, kind=None, build=None, name=None, sample=None, limit=None
        )
        names = [json.dumps(C.enc_name(rel)) for _, rel, _ in unitypy_dump.collect_inputs(args)]
        self.assertEqual(g3b.inputs_under(root), sorted(names))
        self.assertEqual(len(names), 6)

    def test_a_name_is_matched_exactly(self):
        import fnmatch

        for raw in (b"a[b]*?c", b"[!x]", b"plain"):
            pattern = os.fsdecode(g3b.glob_exact(raw))
            self.assertTrue(fnmatch.fnmatchcase(os.fsdecode(raw), pattern), raw)
            self.assertFalse(fnmatch.fnmatchcase(os.fsdecode(raw) + "x", pattern), raw)


if __name__ == "__main__":
    unittest.main()
