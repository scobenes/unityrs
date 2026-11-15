"""Wheel smoke and ownership checks; synthetic data uses existing test builders."""

import gc
import json
import sys
import unittest
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT / "tools/parity"))
sys.path.insert(0, str(ROOT / "tools/parity/tests"))
import unityrs


class API(unittest.TestCase):
    def data(self, name):
        rows = json.loads((ROOT / "crates/unityrs-py/tests/fixtures/api.json").read_text())
        return bytes.fromhex(next(row["hex"] for row in rows if row["name"] == name))

    def text(self):
        return self.data("text")

    def test_constructor_and_identity(self):
        env = unityrs.load(self.text())
        self.assertIs(env.file, next(iter(env.files.values())))
        obj = env.objects[0]
        self.assertIs(obj.assets_file, env.file)
        self.assertIs(obj, env.file.objects[7])
        self.assertIs(obj.assets_file.parent, env)
        self.assertIs(obj.assets_file.environment, env)
        self.assertEqual(obj.parse_as_dict(), {"m_Name": "name\udcff", "m_Script": "body"})
        del env
        gc.collect()
        self.assertEqual(obj.path_id, 7)
        self.assertEqual(obj.parse_as_dict()["m_Script"], "body")

    def test_nested_parent_and_registry(self):
        env = unityrs.load(self.data("nested"))
        obj = env.objects[0]
        self.assertIs(obj.assets_file.parent.parent, env.file)
        self.assertIs(env.get_cab("CAB-Y1"), obj.assets_file)
        self.assertIsNone(env.get_cab("missing"))

    def test_rust_budget_refuses_and_returns(self):
        budget = unityrs.Budget(16 * 1024 * 1024)
        env = unityrs.Environment(budget=budget)
        f = env.load_file(self.text(), name="f.assets")
        self.assertGreater(budget.used, 0)
        del f, env
        gc.collect()
        self.assertEqual(budget.used, 0)
        with self.assertRaises(MemoryError):
            unityrs.Environment(budget=unityrs.Budget(0)).load_file(self.text(), name="f")

    def test_limits(self):
        with self.assertRaises(unityrs.LimitError):
            unityrs.Environment(limits=unityrs.Limits(max_input_bytes=1)).load_file(self.text(), name="f")

    def test_threaded_reads(self):
        obj = unityrs.load(self.text()).objects[0]
        with ThreadPoolExecutor(4) as pool:
            values = list(pool.map(lambda _: obj.parse_as_dict(), range(32)))
        self.assertEqual(values, [values[0]] * 32)

    def test_explicit_name_and_hash_key(self):
        data = self.text()
        env = unityrs.Environment(data)
        self.assertEqual(list(env.files), [str(hash(data))])
        self.assertEqual(list(unityrs.Environment().files), [])
        with self.assertRaises(TypeError):
            unityrs.Environment(bytearray(data))


class EdgeCases(unittest.TestCase):
    def test_short_stream_reads_accumulate_and_restore_position(self):
        import io
        import zipfile
        class ShortStream(io.BytesIO):
            def read(self, size=-1):
                if size < 0:
                    raise AssertionError("unbounded read")
                return super().read(min(size, 5))
        data = API().text()
        buffer = io.BytesIO()
        with zipfile.ZipFile(buffer, "w") as archive:
            archive.writestr("text.assets", data)
        for method, payload in (("load_file", data), ("load_zip_file", buffer.getvalue())):
            for maximum in (len(payload), len(payload) - 1):
                with self.subTest(method=method, maximum=maximum):
                    stream = ShortStream(payload)
                    stream.seek(17)
                    env = unityrs.Environment(limits=unityrs.Limits(max_input_bytes=maximum))
                    if maximum < len(payload):
                        with self.assertRaises(unityrs.LimitError):
                            getattr(env, method)(stream)
                    else:
                        getattr(env, method)(stream)
                        self.assertEqual(env.objects[0].parse_as_dict()["m_Script"], "body")
                    self.assertEqual(stream.tell(), 17)

    def test_graph_failure_cleanup_on_small_stack(self):
        import child
        script = r"""
import gc, sys, threading
sys.path.insert(0, sys.argv[1])
import synth as S
import unityrs
depth = 16000
data = b"raw"
for _ in range(depth + 1):
    data = S.web_file([(b"n", data)])
limits = unityrs.Limits(max_container_depth=depth, max_tree_depth=0, max_value_depth=0)
budget = unityrs.Budget(1 << 30)
env = unityrs.Environment(limits=limits, budget=budget)
native = env._native.load_bytes(data, b"deep", False, False)
retained = budget.used
del native, env
gc.collect()
assert budget.used == 0
threading.stack_size(512 * 1024)
result = []
def load():
    budget = unityrs.Budget(retained + 512)
    env = unityrs.Environment(limits=limits, budget=budget)
    try:
        env.load_file(data, name="deep")
    except MemoryError:
        # Parsing succeeded; wrapper staging exhausted the remaining budget.
        assert budget.used >= retained
    else:
        raise AssertionError("wrapper budget was not refused")
    del env
    gc.collect()
    assert budget.used == 0, budget.used
    # A native file may be the final owner after its environment is gone.
    budget = unityrs.Budget(1 << 30)
    env = unityrs.Environment(limits=limits, budget=budget)
    native = env._native.load_bytes(data, b"deep", False, False)
    del env
    gc.collect()
    assert budget.used > 0
    del native
    gc.collect()
    assert budget.used == 0, budget.used
    result.append(True)
t = threading.Thread(target=load)
t.start()
t.join()
assert result == [True], result
"""
        proc = child.run([sys.executable, "-c", script, str(ROOT / "tools/parity/tests")],
                         stdout=child.PIPE, stderr=child.PIPE, text=True, timeout=180)
        self.assertEqual(proc.returncode, 0, proc.stderr)

    def test_deepest_container_at_limit_on_small_stack(self):
        import child
        script = r"""
import gc, sys, threading
sys.path.insert(0, sys.argv[1])
import synth as S
import unityrs
depth = int(sys.argv[2])
# Containers at depths 0..depth are permitted; raw leaves do not recurse.
data = b"raw"
for _ in range(depth + 1):
    data = S.web_file([(b"nested", data)])
threading.stack_size(512 * 1024)
result = []
def load():
    env = unityrs.Environment(data, limits=unityrs.Limits(
        max_container_depth=depth, max_tree_depth=0, max_value_depth=0))
    file = next(iter(env.files.values()))
    for _ in range(depth + 1):
        file = file.files["nested"]
    value = file.bytes
    del env, file
    gc.collect()
    result.append(value)
t = threading.Thread(target=load)
t.start()
t.join()
assert result == [b"raw"], result
# One more container is refused, rather than overflowing its worker stack.
try:
    unityrs.Environment(S.web_file([(b"nested", data)]), limits=unityrs.Limits(
        max_container_depth=depth, max_tree_depth=0, max_value_depth=0))
except unityrs.LimitError:
    pass
else:
    raise AssertionError("container depth limit was not enforced")
"""
        for depth in (0, 16, 1000, 16000):
            with self.subTest(depth=depth):
                proc = child.run([sys.executable, "-c", script, str(ROOT / "tools/parity/tests"), str(depth)],
                                 stdout=child.PIPE, stderr=child.PIPE, text=True, timeout=90)
                self.assertEqual(proc.returncode, 0, proc.stderr)

    def test_stream_read_is_bounded_and_position_restored(self):
        import io
        class Stream(io.BytesIO):
            def read(self, size=-1):
                self.requested = size
                if not 0 <= size <= 101:
                    raise AssertionError(f"unbounded read: {size}")
                return super().read(size)
        for method in ("load_file", "load_zip_file"):
            stream = Stream(b"x" * 1_000_000)
            stream.seek(17)
            env = unityrs.Environment(limits=unityrs.Limits(max_input_bytes=100))
            with self.subTest(method=method), self.assertRaises(unityrs.LimitError):
                getattr(env, method)(stream)
            self.assertEqual(stream.requested, 101)
            self.assertEqual(stream.tell(), 17)
            self.assertEqual(env.files, {})

    def test_zip_bytes_and_stream_use_rust_meter(self):
        import io
        import zipfile
        from unittest.mock import patch
        buffer = io.BytesIO()
        with zipfile.ZipFile(buffer, "w", compression=zipfile.ZIP_DEFLATED) as archive:
            archive.writestr("huge.resS", b"x" * 1_000_000)
        data = buffer.getvalue()
        for source in (data, io.BytesIO(data)):
            env = unityrs.Environment(limits=unityrs.Limits(max_input_memory=100_000))
            with self.subTest(source=type(source).__name__), patch.object(zipfile.ZipExtFile, "read", side_effect=AssertionError("Python decompressed a ZIP member")):
                with self.assertRaises(MemoryError):
                    env.load_zip_file(source)
                self.assertEqual(env.files, {})

    def test_concurrent_wrapper_identity_and_initialisation(self):
        import threading
        import time
        from unittest.mock import patch
        from unityrs.files import SerializedFile
        data = API().text()
        env = unityrs.Environment()
        native = env._native.load_bytes(data, b"shared", False, False)
        barrier = threading.Barrier(8)
        initialise = SerializedFile._init
        def slow_init(self, *args):
            time.sleep(0.01)  # Widen the old cache publication race.
            initialise(self, *args)
        def wrap(_):
            barrier.wait()
            file = env._wrap(native, env)
            self.assertEqual(list(file.objects), [7])
            return file
        with patch.object(SerializedFile, "_init", slow_init), ThreadPoolExecutor(8) as pool:
            files = list(pool.map(wrap, range(8)))
        self.assertTrue(all(file is files[0] for file in files))
        with ThreadPoolExecutor(8) as pool:
            loaded = list(pool.map(lambda _: env.load_file(data, name="shared"), range(16)))
        for file in loaded:
            self.assertIs(env._wrap(file._native, env), file)

    def test_packaged_licenses_match_originals(self):
        # The crate links its notices to the repository's, as every crate does.
        for name in ("LICENSE-MIT", "LICENSE-APACHE", "LICENSE-UNITYPY", "THIRD_PARTY_LICENSES"):
            packaged = ROOT / "crates/unityrs-py" / name
            original = ROOT / ("crates/unityrs-core" if name == "LICENSE-UNITYPY" else "") / name
            with self.subTest(name=name):
                self.assertEqual(packaged.read_bytes(), original.read_bytes())

    def test_deep_read_on_small_python_stack(self):
        import child
        script = r"""
import gc, sys, threading
from pathlib import Path
sys.path.insert(0, sys.argv[1])
import synth as S
import unityrs
sys.setrecursionlimit(20000)
depth = int(sys.argv[2])
node = S.Node("int", "leaf")
for _ in range(depth):
    node = S.Node("Thing", "x", [node])
data = S.serialized_file([S.Type(49, node)], [S.Obj(1, 49, b"\x01\x00\x00\x00")], version=11)
threading.stack_size(512 * 1024)
result = []
def read():
    budget = unityrs.Budget(1 << 30)
    obj = unityrs.Environment(data, limits=unityrs.Limits(max_tree_depth=depth+1, max_value_depth=depth+1), budget=budget).objects[0]
    value = obj.parse_as_dict()
    for _ in range(depth-1):
        value = value["x"]
    answer = value["leaf"]
    del obj, value
    gc.collect()
    assert budget.used == 0, budget.used
    result.append(answer)
t = threading.Thread(target=read)
t.start()
t.join()
assert result == [1], result
"""
        for depth in (250, 2000):
            with self.subTest(depth=depth):
                proc = child.run([sys.executable, "-c", script, str(ROOT / "tools/parity/tests"), str(depth)],
                                 stdout=child.PIPE, stderr=child.PIPE, text=True, timeout=45)
                self.assertEqual(proc.returncode, 0, proc.stderr)

    def test_core_call_releases_gil(self):
        import child
        script = r"""
import sys, threading, struct
sys.path.insert(0, sys.argv[1])
import synth as S
import unityrs
node = S.Node("TextAsset", "Base", [S.vec("items", S.Node("int", "data"))])
data = S.serialized_file([S.Type(49, node)], [S.Obj(1, 0, struct.pack("<i", 32768) + b"\0" * (32768*4))])
obj = unityrs.Environment(data).objects[0]
start = threading.Event()
progress = threading.Event()
def other():
    start.wait()
    progress.set()
t = threading.Thread(target=other)
t.start()
sys.setswitchinterval(30)
start.set()
obj.parse_as_dict()
assert progress.is_set(), "no other Python thread ran during the core call"
sys.setswitchinterval(0.005)
t.join()
"""
        proc = child.run([sys.executable, "-c", script, str(ROOT / "tools/parity/tests")],
                         stdout=child.PIPE, stderr=child.PIPE, text=True, timeout=45)
        self.assertEqual(proc.returncode, 0, proc.stderr)

    def test_wheel_contains_dependency_notices(self):
        import importlib.metadata
        files = importlib.metadata.files("unityrs")
        self.assertTrue(any(str(f).endswith("THIRD_PARTY_LICENSES") for f in files))

    def test_stream_origin_and_top_level_registry(self):
        import io
        env = unityrs.Environment()
        raw = env.load_file(io.BytesIO(b"A"*20), name="raw")
        self.assertIs(env.get_cab("RAW"), raw)
        self.assertEqual(raw.bytes, b"A"*20)

    def test_one_rust_input_meter(self):
        budget = unityrs.Budget(10_000_000)
        env = unityrs.Environment(limits=unityrs.Limits(max_input_memory=101000), budget=budget)
        env.load_file(b"x"*100000, name="f")
        # Existing immutable Python input is retained without a Rust copy.
        self.assertLess(budget.used, 101000)


if __name__ == "__main__":
    unittest.main()
