"""Installed-wheel tools exports, loader limits and caller hooks."""
import io
import struct
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT / "tools/parity"))
import binding_cases as B
import scratch
import unityrs


class Tools(unittest.TestCase):
    def text(self):
        return next(data for name, data in B.cases() if name == "text")

    def test_text_export_preserves_surrogate_bytes(self):
        from unityrs.tools import exportTextAsset
        env = unityrs.Environment(self.text())
        with scratch.folder("wheel-tools-") as directory:
            path = Path(directory) / "output"
            returned = exportTextAsset(env.objects[0], str(path))
            self.assertEqual(returned, [(env.file, 7)])
            self.assertEqual(path.with_suffix(".txt").read_bytes(), b"body")

    def test_object_patch_installs_bytes_and_marks_parent(self):
        env = unityrs.Environment(self.text())
        reader = env.objects[0]
        result = reader.patch({"m_Name": "edited", "m_Script": "replacement"})
        self.assertEqual(result, B.S.s32("edited") + B.S.s32("replacement"))
        self.assertEqual(reader.data, result)
        self.assertTrue(env.file.is_changed)

    def test_stream_reexports_read_written_values(self):
        from unityrs.streams import EndianBinaryReader, EndianBinaryWriter
        for endian in ("<", ">"):
            writer = EndianBinaryWriter(endian=endian)
            writer.write_int(-27)
            writer.write_aligned_string("text")
            reader = EndianBinaryReader(writer.bytes, endian=endian)
            self.assertEqual(reader.read_int(), -27)
            self.assertEqual(reader.read_aligned_string(), "text")

    def test_split_join_enforces_aggregate_input_limit(self):
        data = self.text()
        with scratch.folder("wheel-splits-") as directory:
            base = str(Path(directory) / "input.assets")
            Path(base + ".split0").write_bytes(data[:40])
            Path(base + ".split1").write_bytes(data[40:])
            env = unityrs.Environment(limits=unityrs.Limits(max_input_bytes=len(data) - 1))
            with self.assertRaises(unityrs.LimitError):
                env.load_file(base + ".split0")

    def test_split_join_releases_budget_after_refusal(self):
        budget = unityrs.Budget(4096)
        with scratch.folder("wheel-split-budget-") as directory:
            base = str(Path(directory) / "input.assets")
            Path(base + ".split0").write_bytes(b"x" * 8192)
            env = unityrs.Environment(budget=budget)
            with self.assertRaises(MemoryError):
                env._load_split_file(base)
            self.assertEqual(budget.used, 0)

    def test_custom_filesystem_split_parts_load_without_local_files(self):
        from fsspec.implementations.memory import MemoryFileSystem
        fs = MemoryFileSystem(skip_instance_cache=True)
        fs.pipe("/wheel-input.assets.split0", self.text()[:40])
        fs.pipe("/wheel-input.assets.split1", self.text()[40:])
        env = unityrs.Environment(fs=fs)
        env.load_file("/wheel-input.assets.split0")
        self.assertEqual(env.objects[0].parse_as_dict()["m_Script"], "body")

    def test_empty_environment_container(self):
        env = unityrs.Environment()
        self.assertEqual(len(env.container), 0)
        self.assertIsNone(env.typetree_generator)

    def test_direct_extension_escape_is_refused_before_creating(self):
        from unityrs.tools import exportTextAsset
        with scratch.folder("wheel-direct-path-") as folder:
            (folder / "out").mkdir()
            (folder / "out/name").mkdir()
            obj = unityrs.Environment(self.text()).objects[0]
            with self.assertRaises(ValueError):
                exportTextAsset(obj, str(folder / "out/name"), "/../../outside")
            self.assertEqual(sorted(p.relative_to(folder).as_posix() for p in folder.rglob("*")),
                             ["out", "out/name"])

    def test_direct_destination_format_override_is_not_called(self):
        from unityrs.tools import exportTextAsset
        class Destination(str):
            def __format__(self, spec):
                raise AssertionError("caller formatting must not run")
        with scratch.folder("wheel-direct-format-") as folder:
            obj = unityrs.Environment(self.text()).objects[0]
            exportTextAsset(obj, Destination(str(folder / "name")))
            self.assertEqual((folder / "name.txt").read_bytes(), b"body")

    def test_missing_destination_is_ignored_after_filters_and_dispatch(self):
        from binding_y5_review import observe_destination_order
        observation = observe_destination_order(unityrs)
        for key, value in observation.items():
            self.assertEqual(value, False if key.endswith("_exists") else {"value": ["list", []]})

    def test_failed_image_keeps_existing_file_and_removes_temporary(self):
        from binding_y5_review import observe_failed_image
        observation = observe_failed_image(unityrs)
        self.assertTrue(observation["preserved"])
        self.assertEqual(observation["leaves"], ["image.png"])

    def test_concurrent_container_access_shares_mutations(self):
        from binding_y5_round4 import observe_container_identity
        self.assertEqual(observe_container_identity(unityrs), [1, ['shared'] * 4, True])

    def test_container_extraction_root_spellings_write_root_and_nested_assets(self):
        from binding_y5_round4 import observe_root_spellings
        for name, (outcome, tree) in observe_root_spellings(unityrs).items():
            with self.subTest(root=name):
                self.assertIn('value', outcome)
                self.assertEqual({bytes.fromhex(key).decode(): bytes.fromhex(value)
                                  for key, value in tree.items()},
                                 {'root.txt': b'hello\xff', 'nested/child.txt': b'hello\xff'})

    def test_none_extension_defaults_and_ignored_arguments(self):
        from binding_y5_round4 import observe_none_extensions
        suffixes = {'TextAsset': '.txt', 'Font': '.otf', 'Mesh': '.obj', 'Shader': '.txt',
                    'MonoBehaviour': '.json', 'AudioClip': '.wav', 'Texture2D': '.png'}
        for name, rows in observe_none_extensions(unityrs).items():
            for outcome, tree in rows:
                with self.subTest(exporter=name):
                    if name == 'Sprite':
                        self.assertEqual(outcome, {'error': 'ValueError', 'message': 'unknown file extension: '})
                    else:
                        self.assertIn('value', outcome)
                    self.assertEqual([bytes.fromhex(key).decode() for key in tree],
                                     ['output' + suffixes[name]] if name in suffixes else [])

    def test_object_reader_cache_publication_holds_environment_lock(self):
        from binding_y5_round4 import check_object_reader_publication
        check_object_reader_publication(unityrs)

    def test_sprite_cache_decodes_once_on_concurrent_access(self):
        from binding_y5_round4 import check_sprite_cache_publication
        check_sprite_cache_publication(unityrs)
