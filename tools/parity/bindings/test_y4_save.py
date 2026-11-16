"""Installed-wheel Y4 file/object save parity on synthetic files."""

import unittest

import UnityPy
import unityrs
import binding_y4_save


class Save(unittest.TestCase):
    def compare(self, name):
        observe = getattr(binding_y4_save, name)
        self.assertEqual(observe(unityrs), observe(UnityPy))

    def test_objects(self):
        self.compare("observe_objects")

    def test_files(self):
        self.compare("observe_files")

    def test_environment(self):
        self.compare("observe_environment")

    def test_cabs(self):
        self.compare("observe_cabs")

    def test_writers(self):
        self.compare("observe_writers")

    def test_legacy(self):
        self.compare("observe_legacy")

    def test_refused_replacement_retains_previous_saved_data(self):
        import struct
        from binding_cases import S
        budget = unityrs.Budget(1_000_000)
        env = unityrs.Environment(budget=budget)
        file = env.load_file(binding_y4_save._asset(), name="bounded.assets")
        obj = file.objects[1]
        replacement = struct.pack("<i", 37) + S.s32("retained")
        obj.set_raw_data(replacement)
        before = file.save()
        with self.assertRaises(MemoryError):
            obj.set_raw_data(b"x" * 2_000_000)
        self.assertEqual(obj.data, replacement)
        self.assertEqual(file.save(), before)

    def test_named_hc_packer_round_trips(self):
        from binding_cases import S
        bundle = binding_y4_save._load(
            unityrs, S.fs_bundle([(b"save.assets", binding_y4_save._asset(), 4)]))
        saved = bundle.save(packer="lz4hc")
        loaded = binding_y4_save._load(unityrs, saved)
        self.assertEqual(loaded.files["save.assets"].objects[1].parse_as_dict(),
                         {"value": 7, "name": "before"})

    def test_serialized_cab_registers_reloadable_external(self):
        from binding_cases import S
        bundle = binding_y4_save._load(
            unityrs, S.fs_bundle([(b"save.assets", binding_y4_save._asset(), 4)]))
        asset = bundle.files["save.assets"]
        writer = asset.get_writeable_cab("extra.resS")
        writer.write_int(17)
        self.assertIs(writer, asset.get_writeable_cab("extra.resS"))
        self.assertEqual(writer.path, "archive:/save.assets/extra.resS")
        self.assertEqual(len(asset.externals), 1)
        self.assertRegex(asset.externals[0].guid, rb"^[0-9a-f]{3}-[0-9a-f]{12}$")
        self.assertFalse(asset.is_changed)
        self.assertFalse(bundle.is_changed)
        loaded = binding_y4_save._load(unityrs, bundle.save())
        self.assertEqual(loaded.files["extra.resS"].bytes, b"\0\0\0\x11")
        self.assertEqual([entry.path for entry in loaded.files["save.assets"].externals],
                         [writer.path])


if __name__ == "__main__":
    unittest.main()
