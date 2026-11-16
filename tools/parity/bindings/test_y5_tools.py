"""Installed-wheel parity and confinement for the Y5 extractor surface."""
import importlib
import json
import os
import unittest
from pathlib import Path
from types import SimpleNamespace as NS
from unittest import mock

import UnityPy
import unityrs

import binding_y5_tools as B
from binding_cases import S
from binding_y4_save import _load
import scratch


class ToolsBindings(unittest.TestCase):
    def test_tools_signatures_dispatch_and_registry(self):
        self.assertEqual(UnityPy.__version__, "1.25.2")
        self.assertEqual(B.observe_tools_dispatch(unityrs), B.observe_tools_dispatch(UnityPy))

    def test_exporters_write_oracle_filenames_bytes_and_identities(self):
        self.assertEqual(B.observe_tools_files(unityrs), B.observe_tools_files(UnityPy))

    def test_export_obj_flags_filters_errors_and_dispatch_mutation(self):
        self.assertEqual(B.observe_tools_objects(unityrs), B.observe_tools_objects(UnityPy))

    def test_crawl_flatten_and_gameobject_preserve_oracle_quirks(self):
        self.assertEqual(B.observe_tools_crawl(unityrs), B.observe_tools_crawl(UnityPy))

    def test_existing_extractor_scenes_match_oracle_trees(self):
        actual = B.observe_tools_extract(unityrs)
        expected = B.observe_tools_extract(UnityPy)
        self.assertGreaterEqual(len(expected), 30)
        for name, observation in expected.items():
            with self.subTest(case=name):
                self.assertEqual(actual[name], observation)

    def test_asset_filter_is_called_before_and_after_read(self):
        def observe(module):
            calls = []
            file = B._reader_file(module)
            def accept(obj):
                calls.append(type(obj).__name__)
                return True
            with scratch.folder("unityrs-y5-filter-") as folder:
                with mock.patch.object(module, "load", return_value=NS(objects=[file.objects[1]])):
                    result = B.extractor(module).extract_assets(b"", folder, use_container=False, asset_filter=accept)
                return calls, [path_id for _, path_id in result]
        self.assertEqual(observe(unityrs), observe(UnityPy))

    def test_container_name_is_validated_before_directory_slicing(self):
        E = B.extractor(unityrs)
        file = B._reader_file(unityrs)
        with scratch.folder("unityrs-y5-paths-") as folder:
            for name in ("../text.txt", "/abs/text.txt", "bad\\text.txt", "c:/text.txt", "nul\0/text.txt"):
                with self.subTest(name=name), mock.patch.object(
                        unityrs, "load", return_value=NS(container={name: file.objects[1]})):
                    with self.assertRaises(ValueError):
                        E.extract_assets(b"", folder / "out", ignore_first_container_dirs=1)
            self.assertEqual(list(folder.rglob("*.txt")), [])

    def test_object_names_cannot_escape_destination(self):
        E = B.extractor(unityrs)
        with scratch.folder("unityrs-y5-object-paths-") as folder:
            dst = folder / "out"
            dst.mkdir()
            for name in ("../outside", "/absolute", "bad\\name", "drive:name", "nul\0"):
                text = S.Node("TextAsset", "Base", [S.Node("string", "m_Name"), S.Node("string", "m_Script")])
                file = _load(unityrs, S.serialized_file([S.Type(49, text)], [S.Obj(1, 0, S.s32(name) + S.s32("data"))]))
                with self.subTest(name=name), self.assertRaises(ValueError):
                    E.export_obj(file.objects[1], str(dst), append_name=True)
            class PathID(int):
                def __format__(self, spec):
                    return "segment/../../outside"
            file = _load(unityrs, S.serialized_file([S.Type(49, text)],
                         [S.Obj(1, 0, S.s32("item") + S.s32("data"))]))
            file.objects[1].m_PathID = PathID(1)
            (dst / "item_segment").mkdir()
            with self.assertRaises(ValueError):
                E.export_obj(file.objects[1], str(dst), append_name=True, append_path_id=True)
            self.assertEqual(list(folder.rglob("*.txt")), [])

    def test_multiple_audio_samples_validate_all_names_before_writing(self):
        E = B.extractor(unityrs)
        reader = NS(assets_file="file", path_id=1)
        with scratch.folder("unityrs-y5-audio-paths-") as folder:
            for name in ("../outside", "/absolute", "bad\\name", "drive:name", "nul\0"):
                obj = NS(samples={"safe": b"data", name: b"evil"}, object_reader=reader)
                with self.subTest(name=name), self.assertRaises(ValueError):
                    E.exportAudioClip(obj, str(folder / "audio"))
            self.assertEqual(list(folder.iterdir()), [])

    def test_sample_names_discard_subclass_formatting_before_join(self):
        class Name(str):
            def __format__(self, spec):
                return "../outside"
        E = B.extractor(unityrs)
        reader = NS(assets_file="file", path_id=1)
        obj = NS(samples={Name("safe"): b"data", "second": b"other"}, object_reader=reader)
        with scratch.folder("unityrs-y5-name-protocol-") as folder:
            E.exportAudioClip(obj, str(folder / "audio"))
            self.assertFalse((folder / "outside.wav").exists())
            self.assertEqual((folder / "audio" / "safe.wav").read_bytes(), b"data")

    def test_output_leaves_replace_links_and_preserve_outside_hardlink(self):
        E = B.extractor(unityrs)
        obj = NS(m_Script="new", assets_file="file", object_reader=NS(path_id=1))
        with scratch.folder("unityrs-y5-leaves-") as folder:
            outside = folder / "outside"
            outside.write_bytes(b"original")
            output = folder / "out"
            output.mkdir()
            (output / "symlink.txt").symlink_to(outside)
            os.link(outside, output / "hardlink.txt")
            for name in ("symlink", "hardlink"):
                E.exportTextAsset(obj, str(output / name))
                self.assertEqual((output / (name + ".txt")).read_bytes(), b"new")
            self.assertEqual(outside.read_bytes(), b"original")

    def test_intermediate_symlinks_are_refused(self):
        E = B.extractor(unityrs)
        file = B._reader_file(unityrs)
        with scratch.folder("unityrs-y5-parents-") as folder:
            dst, outside = folder / "out", folder / "outside"
            dst.mkdir()
            outside.mkdir()
            (dst / "nested").symlink_to(outside, target_is_directory=True)
            with mock.patch.object(unityrs, "load", return_value=NS(container={"nested/file.txt": file.objects[1]})):
                with self.assertRaises(OSError):
                    E.extract_assets(b"", dst)
            self.assertEqual(list(outside.iterdir()), [])

    def test_empty_extraction_does_not_open_destination(self):
        def observe(module):
            E = B.extractor(module)
            with scratch.folder("unityrs-y5-empty-") as folder:
                with mock.patch.object(module, "load", return_value=NS(container={}, objects=[])):
                    return [E.extract_assets(b"", folder / "absent", use_container=mode) for mode in (True, False)]
        self.assertEqual(observe(unityrs), observe(UnityPy))

    def test_json_and_names_refuse_small_remaining_input_budget(self):
        E = B.extractor(unityrs)
        tree = S.Node("TextAsset", "Base", [S.Node("string", "m_Name"), S.Node("string", "m_Script")])
        data = S.serialized_file([S.Type(49, tree)], [S.Obj(1, 0, S.s32("name") + S.s32("x" * 4096))])
        budget = unityrs.Budget(1_000_000)
        file = unityrs.Environment(budget=budget).load_file(data, name="budget.assets")
        with scratch.folder("unityrs-y5-budget-") as folder:
            # Retain the remaining budget so extraction's charged JSON staging
            # refuses before an unmetered encoded buffer or output is created.
            held = unityrs._native.TreeTraversal(budget=budget)
            held.step(0, budget.limit - budget.used - 256, 0)
            with self.assertRaises((unityrs.LimitError, MemoryError)):
                E.exportMonoBehaviour(file.objects[1], str(folder / "mono"))
            self.assertEqual(list(folder.iterdir()), [])


if __name__ == "__main__":
    unittest.main()
