"""Installed-wheel regression coverage for current Python save state."""

import unittest

import UnityPy
import unityrs
import binding_y4_review_save


class ReviewSave(unittest.TestCase):
    def test_live_buffer_payloads_match_oracle(self):
        self.compare("observe_buffer_payloads")

    def test_buffer_snapshots_charge_bytes_before_copy_and_recover(self):
        file = unityrs.Environment(budget=unityrs.Budget(1_000_000)).load_file(
            binding_y4_review_save._asset(), name="bounded.assets")
        obj = file.objects[1]
        original = file.save()
        for view in (memoryview(bytearray(2_000_000)).cast("B", (1, 2_000_000)),
                     memoryview(bytearray(4_000_000))[::2]):
            obj.data = view
            with self.assertRaises(MemoryError):
                file.save()
            obj.data = None
            with self.assertRaises(MemoryError):
                obj.set_raw_data(view)
            self.assertEqual(file.save(), original)

    def compare(self, name):
        observe = getattr(binding_y4_review_save, name)
        self.assertEqual(observe(unityrs), observe(UnityPy))

    def test_serialized_mapping(self):
        self.compare("observe_serialized_mapping")

    def test_serialized_payload(self):
        self.compare("observe_serialized_payload")

    def test_serialized_metadata(self):
        self.compare("observe_serialized_metadata")

    def test_serialized_snapshot_budget_refusal_is_recoverable(self):
        file = unityrs.Environment(budget=unityrs.Budget(1_000_000)).load_file(
            binding_y4_review_save._asset(), name="bounded.assets")
        obj = file.objects[1]
        original = file.save()
        obj.data = b"x" * 2_000_000
        with self.assertRaises(MemoryError):
            file.save()
        obj.data = None
        self.assertEqual(file.save(), original)
        self.assertEqual(list(file.objects), [1])
        reserved = file.header.reserved
        file.header.reserved = bytearray(2_000_000)
        with self.assertRaises(MemoryError):
            file.save()
        file.header.reserved = reserved
        self.assertEqual(file.save(), original)

    def test_older_serialized_formats_preserve_optional_metadata(self):
        import struct
        from binding_cases import S
        # The asset detector requires 128 bytes. Keep real metadata above
        # that threshold after save removes the fixture builder's padding.
        external_path = "directory/" * 16 + "other.assets"
        for version in (1, 4, 5, 6, 8, 10, 15, 16):
            with self.subTest(version=version):
                payload = struct.pack("<i", 73)
                source = S.serialized_file([S.Type(49, S.Node("int", "Base"))],
                    [S.Obj(7, 0 if version >= 16 else 49, payload)], version=version,
                    externals=[S.External(external_path)])
                file = binding_y4_review_save._load(unityrs, source)
                if version < 9:
                    file.header.reserved = None
                if version < 6:
                    file.externals[0].temp_empty = None
                if version < 5:
                    file.externals[0].guid = None
                    file.externals[0].type = None
                saved = file.save()
                reloaded = binding_y4_review_save._load(unityrs, saved)
                self.assertEqual(reloaded.objects[7].get_raw_data(), payload)
                self.assertEqual([entry.path for entry in reloaded.externals], [external_path])

    def test_bundle_headers(self):
        self.compare("observe_bundle_headers")

    def test_legacy_headers(self):
        self.compare("observe_legacy_headers")


if __name__ == "__main__":
    unittest.main()
