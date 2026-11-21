"""A failed allocation while the binding makes an error raises MemoryError.

PyO3's lazy `new_err` converts its argument as the error is raised, and its
string, integer and tuple conversions panic on a null: an allocation that
failed there raised a `PanicException` (whose own making then failed, and
the process aborted) where Python raises `MemoryError`. Each case below is
run with every allocation of its call made to fail in turn, from the first,
by `_testcapi`'s failing allocator (as `test_direct_object`'s failed list
allocation is): every outcome is the call's own error or `MemoryError`. The
sweep runs in a child, so an abort fails the test instead of the suite.
"""
import json
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT / "tools/parity"))

# Each allocation of a call, counted from its start, failed in turn; the
# cases make at most a few, so the last runs complete.
SWEEP = 64

SCRIPT = r"""
import json, struct, sys
sys.path.insert(0, sys.argv[1])
sys.path.insert(0, sys.argv[2])
import _testcapi
import synth as S
import unityrs
from unityrs import _native
from binding_y4_save import _legacy

sweep = int(sys.argv[3])
env = unityrs.Environment()
native = env._native
# A legacy bundle whose block offset is far before its buffer.
legacy = bytearray(_legacy(3))
struct.pack_into(">i", legacy, 39, -100)
node = S.Node("TextAsset", "Base", [S.Node("int", "a"), S.Node("int", "b")])
short = S.serialized_file([S.Type(49, node)], [S.Obj(1, 0, b"\1\0\0\0\2")])
obj = unityrs.Environment(short).objects[0]
cases = [
    # errors::core: struct.error, its class and message made fallibly.
    ("core", native.load_bytes, (bytes(legacy), b"f", False, 0, 0)),
    # errors::file_parse: a short serialized header (struct.error), a
    # rejected signature formatted into ValueError / NotImplementedError,
    # in the result tuple parse_bytes makes.
    ("file_parse", native.parse_bytes, (b"\0" * 10, b"s", 0, 0)),
    ("web_signature", native.parse_bytes, (b"NotAWebFile\0" + b"\0" * 20, b"w", 2, 0)),
    ("bundle_signature", native.parse_bytes, (b"NotABundle\0" + b"\0" * 20, b"b", 1, 0)),
    # errors::parse_reader: a short object read (EOFError), raised, and
    # kept in object_data's result list.
    ("parse_reader", obj._native.parse, (True,)),
    ("object_data", obj._native.object_data, ()),
    # errors::new outside errors.rs.
    ("array_format", _native._array_format, (-12345,)),
]


def run(f, args, at):
    def profile(frame, event, arg):
        if event == "c_call" and arg is f:
            sys.setprofile(None)
            _testcapi.set_nomemory(at, at + 1)
    sys.setprofile(profile)
    try:
        result = f(*args)
    except BaseException as error:
        result = error
    finally:
        sys.setprofile(None)
        _testcapi.remove_mem_hooks()
    if isinstance(result, tuple):
        result = result[3]
    elif isinstance(result, list):
        result = result[5]
    kind = type(result)
    return kind.__module__ + "." + kind.__qualname__


out = {}
for name, f, args in cases:
    out[name] = [run(f, args, at) for at in range(sweep)]
print(json.dumps(out))
"""

EXPECTED = {
    "core": "struct.error",
    "file_parse": "struct.error",
    "web_signature": "builtins.ValueError",
    "bundle_signature": "builtins.NotImplementedError",
    "parse_reader": "builtins.EOFError",
    "object_data": "builtins.EOFError",
    "array_format": "builtins.NotImplementedError",
}


class ErrorAllocation(unittest.TestCase):
    def test_failed_error_allocations_raise_memory_error(self):
        try:
            import _testcapi  # noqa: F401
        except ImportError:
            self.skipTest("no _testcapi")
        import child
        proc = child.run(
            [sys.executable, "-c", SCRIPT, str(ROOT / "tools/parity/tests"),
             str(ROOT / "tools/parity"), str(SWEEP)],
            stdout=child.PIPE, stderr=child.PIPE, text=True, timeout=120)
        self.assertEqual(proc.returncode, 0, proc.stderr[-4000:])
        outcomes = json.loads(proc.stdout.splitlines()[-1])
        self.assertEqual(set(outcomes), set(EXPECTED))
        for name, expected in EXPECTED.items():
            with self.subTest(name):
                seen = outcomes[name]
                self.assertLessEqual(set(seen), {expected, "builtins.MemoryError"}, seen)
                # The first allocation failed, and the sweep ran past the
                # call's last: both outcomes were met.
                self.assertEqual(seen[0], "builtins.MemoryError", seen)
                self.assertEqual(seen[-1], expected, seen)


if __name__ == "__main__":
    unittest.main()
