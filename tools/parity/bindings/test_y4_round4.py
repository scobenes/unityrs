"""Round-four parity points and bounded allocation at every output boundary."""
import unittest

import UnityPy
import unityrs
import binding_y4_round4 as review


class RoundFour(unittest.TestCase):
    def compare(self, name, **kwargs):
        self.assertEqual(UnityPy.__version__, "1.25.2")
        observe = getattr(review, name)
        self.assertEqual(observe(unityrs), observe(UnityPy, **kwargs))

    def test_pairs_index_exactly_two_values(self):
        self.compare("observe_pairs")

    def test_managed_payload_uses_referenced_schema(self):
        self.compare("observe_managed")

    def test_cab_creation_follows_current_mapping(self):
        self.compare("observe_cab_mapping")

    def test_export_byte_fields_accept_buffer_protocol(self):
        self.compare("observe_export_buffers")

    def test_writers_refuse_strides_before_mutation(self):
        self.compare("observe_strided_writes")

    def test_output_names_validate_and_charge_before_copying(self):
        self.compare("observe_output_names", corrected=True)

    def test_bundle_adapters_use_payload_views(self):
        self.compare("observe_bundle_views")

    def test_vorbis_paths_fail_fallibly_under_address_limit(self):
        self.compare("observe_path_budgets", corrected=True)
        import sys
        import child
        script = r'''
import resource, sys
import unityrs
resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
path = b"x" * (8 << 20) if sys.argv[1] == "bytes" else "x" * (8 << 20)
# Imports and input construction precede the address-space constraint.
with open("/proc/self/statm") as stream:
    virtual = int(stream.read().split()[0]) * resource.getpagesize()
resource.setrlimit(resource.RLIMIT_AS, (virtual + (4 << 20), virtual + (4 << 20)))
try:
    unityrs._native.export_audio(b"RIFF", b"clip", None, None, path)
except MemoryError:
    pass
except Exception as error:
    raise AssertionError(type(error).__name__) from None
else:
    raise AssertionError("path allocation was not refused")
'''
        for kind in ("bytes", "str"):
            with self.subTest(kind=kind):
                result = child.run([sys.executable, "-c", script, kind], capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stderr)

    def test_python_output_entrypoints_refuse_before_allocation(self):
        import sys
        from pathlib import Path
        import child
        from binding_y4_budget import PYTHON_CASES
        self.assertEqual(len(PYTHON_CASES), 25)
        script = r'''
import resource, sys, tracemalloc
sys.path.insert(0, sys.argv[1])
import unityrs
from binding_y4_budget import python_case
call = python_case(unityrs, sys.argv[2])
before = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
tracemalloc.start()
try:
    call()
except MemoryError:
    pass
except Exception as error:
    raise AssertionError(type(error).__name__) from None
else:
    raise AssertionError("oversized input accepted")
peak = tracemalloc.get_traced_memory()[1]
tracemalloc.stop()
growth = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss - before
assert peak < 1_000_000, f"uncharged Python allocation: {peak}"
assert growth < 8192, f"uncharged native allocation: {growth} KiB"
'''
        for operation in PYTHON_CASES:
            with self.subTest(operation=operation):
                result = child.run([sys.executable, "-c", script,
                    str(Path(__file__).resolve().parents[1]), operation], capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stderr)
