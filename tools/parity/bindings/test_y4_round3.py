"""Round-three installed binding regressions, also recorded for Rust replay."""
import unittest

import UnityPy
import unityrs
import binding_y4_round3 as review


class RoundThree(unittest.TestCase):
    def compare(self, name, **kwargs):
        self.assertEqual(UnityPy.__version__, "1.25.2")
        observe = getattr(review, name)
        self.assertEqual(observe(unityrs), observe(UnityPy, **kwargs))

    def test_cab_charges_mutable_copy_and_padding_before_allocation(self):
        self.compare("observe_cab_budget", corrected=True)

    def test_cab_length_does_not_materialize_payload(self):
        self.compare("observe_cab_length")

    def test_cab_padding_overwrites_retains_tail_and_zeroes_gaps(self):
        self.compare("observe_cab_padding")

    def test_typed_sequences_follow_vector_schema(self):
        self.compare("observe_typed_sequences")

    def test_packer_callbacks_reenter_children_on_caller(self):
        self.compare("observe_packer_threads")

    def test_mesh_getters_run_on_caller(self):
        self.compare("observe_mesh_thread")

    def test_shader_getters_run_on_caller(self):
        self.compare("observe_shader_thread")

    def test_tight_sprite_getters_run_on_caller(self):
        self.compare("observe_sprite_thread")

    def test_native_entry_row_protocols_run_on_caller(self):
        self.compare("observe_entry_threads")

    def test_vorbis_header_path_subclass_is_normalized_on_caller(self):
        import struct
        import threading
        from scratch import for_case
        caller = threading.get_ident()

        class Path(str):
            def __len__(self):
                self.assert_caller()
                return super().__len__()
            def assert_caller(self):
                assert threading.get_ident() == caller, "audio path callback left caller"

        payload = b"\x01\0\0\0\0"
        header = b"FSB5" + struct.pack("<7I", 1, 1, 16, 0, len(payload), 15, 0)
        word = (8 << 1) | (8 << 34) | 1
        bank = header.ljust(60, b"\0") + struct.pack("<QII", word, (11 << 25) | 8, 42) + payload
        path = Path(str(for_case(self) / "absent"))
        with self.assertRaises(ValueError):
            unityrs._native.export_audio(bank, b"clip", None, None, path)

    def test_mutable_native_inputs_are_refused_without_rust_copies(self):
        import sys
        from pathlib import Path
        import child
        script = r'''
import resource, sys
sys.path.insert(0, sys.argv[1])
import unityrs
from binding_y4_round3 import _container
from binding_y4_save import _asset
class Buffer(bytearray):
    def __len__(self):
        return 0
file = _container(unityrs, bounded=True)
owner = file.files["child.assets"].objects[1]
cab = file.get_writeable_cab("new.resS")
source = Buffer(128 << 20)
source[::4096] = b"x" * 32768
native = unityrs._native
operations = {
    "cab": lambda: cab.write(source),
    "decode": lambda: native._image_decode(source, 1, 1, 4, (0, 0), 0, None, owner._native),
    "encode": lambda: native._image_encode(source, 1, 1, "RGBA", 4, 0, None, owner._native),
    "sprite": lambda: native._sprite_render(source, "RGBA", 1, 1, (0,0,1,1), 2, 1, None, owner._native),
    "audio": lambda: native.export_audio(source, b"name", owner._native),
    "web_rows": lambda: _container(unityrs, web=True, bounded=True)._native.save_web_entries([(b"x", source)], "none", b"UnityWebData1.0"),
    "bundle_rows": lambda: file._native.save_bundle_entries("save", [(b"x", 0, source)], "none", (64,64), file._save_header(file._native.tree_traversal())),
    "load": lambda: file.environment.load_file(source, name="big"),
    "zip": lambda: file.environment.load_zip_file(source),
    "fallback": lambda: file.environment._native.configure(source, True),
    "tpk": lambda: native.class_tpk_lz4(source, 300 << 20),
    "metadata": lambda: setattr(file.files["child.assets"].header, "reserved", source) or file.files["child.assets"].save(),
}
before = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
try:
    operations[sys.argv[2]]()
except MemoryError:
    pass
except ValueError:
    if sys.argv[2] != "tpk":
        raise
else:
    raise AssertionError("oversized mutable input accepted")
growth = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss - before
assert growth < 8192, f"mutable input copied before refusal: {growth} KiB"
'''
        for operation in ("cab", "decode", "encode", "sprite", "audio", "web_rows", "bundle_rows", "load", "zip", "fallback", "tpk", "metadata"):
            with self.subTest(operation=operation):
                result = child.run([sys.executable, "-c", script,
                    str(Path(__file__).resolve().parents[1]), operation],
                    capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stderr)

    def test_invalid_switch_mip_counts_preserve_oracle_mutation_order(self):
        self.compare("observe_switch_invalid_counts")
