"""OS worker refusal cannot retain a native graph or its budget charges."""
import os
from pathlib import Path
import sys
import unittest

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT / "tools/parity"))
import child
import scratch


@unittest.skipUnless(sys.platform == "linux", "Linux pthread interposition hook")
class Teardown(unittest.TestCase):
    def test_repeated_drop_with_workers_refused(self):
        folder = scratch.for_case(self)
        hook = folder / "refuse_threads.so"
        built = child.run(["cc", "-shared", "-fPIC", "-std=c11", "-Wall", "-Wextra", "-Werror",
                           "-o", str(hook), str(Path(__file__).with_name("refuse_threads.c")), "-ldl"],
                          stdout=child.PIPE, stderr=child.PIPE, text=True)
        self.assertEqual(built.returncode, 0, built.stderr)
        script = r"""
import ctypes, gc, json, sys, threading
import unityrs
hook = ctypes.CDLL(sys.argv[1])
rows = json.loads(open(sys.argv[2]).read())
data = bytes.fromhex(next(row['hex'] for row in rows if row['name'] == 'nested'))
budget = unityrs.Budget(1 << 30)
threading.stack_size(512 * 1024)
result = []
def exercise():
    # Prove that the hook really refuses worker creation.
    hook.unityrs_refuse_threads(1)
    try:
        threading.Thread(target=lambda: None).start()
    except RuntimeError:
        pass
    else:
        raise AssertionError('pthread refusal hook did not fire')
    finally:
        hook.unityrs_refuse_threads(0)
    refused = hook.unityrs_thread_attempts()
    assert refused == 1, refused
    for owner in ('environment', 'file', 'object'):
        for _ in range(5):
            env = unityrs.Environment(limits=unityrs.Limits(), budget=budget)
            native = env._native.load_bytes(data, b'nested', False, False)
            if owner == 'object':
                nested = native.entries()[0][1]
                asset = nested.entries()[0][1]
                obj = asset.objects()[0]
                del nested, asset
            assert budget.used > 0
            hook.unityrs_refuse_threads(1)
            try:
                if owner == 'environment':
                    del native, env
                elif owner == 'file':
                    del env
                    gc.collect()
                    assert budget.used > 0
                    del native
                else:
                    del native, env
                    gc.collect()
                    assert budget.used > 0
                    del obj
                gc.collect()
                assert budget.used == 0, (owner, budget.used)
            finally:
                hook.unityrs_refuse_threads(0)
    assert hook.unityrs_thread_attempts() == refused, 'teardown tried to create a worker'
    result.append(True)
t = threading.Thread(target=exercise)
t.start()
t.join()
assert result == [True], result
"""
        env = dict(os.environ, LD_PRELOAD=str(hook))
        proc = child.run([sys.executable, "-c", script, str(hook),
                          str(ROOT / "crates/unityrs-py/tests/fixtures/api.json")], env=env,
                         stdout=child.PIPE, stderr=child.PIPE, text=True, timeout=180)
        self.assertEqual(proc.returncode, 0, proc.stderr)
