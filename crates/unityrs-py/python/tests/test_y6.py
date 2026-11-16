"""Input bounds and ownership for the remaining binding adapters."""
import io
import contextlib
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[4]
sys.path.insert(0, str(ROOT / 'tools/parity'))
import binding_cases as B
import scratch
import child
import binding_y6_review as review
import unityrs


class ReadOnly:
    def __init__(self, payload):
        self.stream = io.BytesIO(payload)
        self.requests = []

    def read(self, size=-1):
        self.requests.append(size)
        return self.stream.read(size)


class Residue(unittest.TestCase):
    def test_oversized_forced_member_retains_stream_short_read_cursor(self):
        self.assertEqual(review.observe_oversized_member_states(unityrs),
                         [[137, '<', '726177'], [40, '<', '726177']])

    def test_failed_forced_constructor_keeps_applied_endian(self):
        payloads = [(b'raw', unityrs.enums.FileType.WebFile),
                    (B.S.serialized_file([], [], version=17)[:20], unityrs.enums.FileType.AssetsFile)]
        for stream in (False, True):
            for payload, typ in payloads:
                reader = unityrs.streams.EndianBinaryReader(io.BytesIO(payload) if stream else payload)
                with contextlib.redirect_stdout(io.StringIO()), self.assertRaises(Exception):
                    unityrs.helpers.ImportHelper.parse_file(reader, unityrs.Environment(), 'forced', typ)
                self.assertEqual((reader.Position, reader.endian), (0, '<'))

    def test_failed_forced_container_exposes_successful_child(self):
        self.assertEqual(review.observe_partial_child_registration(unityrs),
                         [[[], ['first'], ['6e616d65ff', 'body']]] * 4)

    def test_nested_forced_bounds_report_child_buffer(self):
        expected = ['error', 'unpack_from requires a buffer of at least 24 bytes for unpacking '
                    '4 bytes at offset 20 (actual buffer size is 21)']
        self.assertEqual(review.observe_nested_bounds_failures(unityrs), [expected] * 4)

    def test_detached_files_keep_private_owner_for_mutations(self):
        for name in ("save", "mark", "raw", "patch"):
            for value in getattr(review, "observe_detached_" + name)(unityrs):
                self.assertNotIn("error", value)
                self.assertTrue(all(value["detached"]))

    def test_exception_lookup_reenters_on_calling_thread(self):
        code = "import unityrs; from binding_y6_review import exception_reentry; exception_reentry(unityrs)"
        child.run([sys.executable, "-c", code], check=True, capture_output=True,
                  text=True, timeout=45, env=dict(__import__('os').environ,
                    PYTHONPATH=str(ROOT / 'tools/parity')))

    def test_mutable_nameless_buffers_respect_input_limit(self):
        for source in (bytearray(b'12345'), memoryview(bytearray(b'12345'))):
            env = unityrs.Environment(limits=unityrs.Limits(max_input_bytes=4))
            with self.assertRaises(unityrs.LimitError):
                env.load_file(source)
            self.assertEqual(env.files, {})
            self.assertEqual(env.cabs, {})

    def test_mutable_hash_staging_refuses_before_publication(self):
        env = unityrs.Environment(budget=unityrs.Budget(0))
        with self.assertRaises(MemoryError):
            env.load_file(bytearray(b'raw'))
        self.assertEqual(env.files, {})
        self.assertEqual(env.cabs, {})

    def test_readonly_stream_obeys_bounded_requests(self):
        source = ReadOnly(b'123456')
        env = unityrs.Environment(limits=unityrs.Limits(max_input_bytes=4))
        with self.assertRaises(unityrs.LimitError):
            env.load_file(source)
        self.assertTrue(source.requests)
        self.assertTrue(all(0 < size <= 5 for size in source.requests))
        self.assertEqual(env.files, {})

    def test_readonly_stream_budget_refuses_before_read(self):
        source = ReadOnly(b'raw')
        env = unityrs.Environment(budget=unityrs.Budget(0))
        with self.assertRaises(MemoryError):
            env.load_file(source)
        self.assertEqual(source.requests, [])
        self.assertEqual(env.files, {})

    def test_readonly_stream_hash_is_its_bytes_reference(self):
        env = unityrs.Environment()
        loaded = env.load_file(ReadOnly(b'raw'))
        self.assertEqual(loaded.bytes, b'raw')
        self.assertIs(env.files[str(hash(b'raw'))], loaded)

    def test_explicit_raw_reader_stays_mutable_by_identity(self):
        reader = unityrs.streams.EndianBinaryReader(b'abcdef')
        env = unityrs.Environment()
        loaded = env.load_file(reader, name='raw')
        self.assertIs(loaded, reader)
        loaded.Position = 2
        self.assertEqual(reader.read(2), b'cd')
        self.assertIs(env.get_cab('raw'), reader)

    def test_nonstr_serialized_stream_name_saves_under_text_name(self):
        data = next(data for name, data in B.cases() if name == 'text')
        for name in (7, None, b'named.assets'):
            stream = io.BytesIO(data)
            stream.name = name
            env = unityrs.Environment(stream)
            file = next(iter(env.files.values()))
            self.assertIs(env.files[name], file)
            text_name = name.decode() if isinstance(name, bytes) else str(name)
            self.assertIs(env.get_cab(text_name), file)
            file.objects[7].patch({'m_Name': 'changed', 'm_Script': 'body'})
            with scratch.folder('wheel-y6-save-') as directory:
                env.save(out_path=directory)
                saved = (Path(directory) / text_name).read_bytes()
            reloaded = unityrs.Environment(saved)
            self.assertEqual(reloaded.objects[0].parse_as_dict()['m_Name'], 'changed')

    def test_forced_parser_budget_refusal_preserves_reader_and_registry(self):
        reader = unityrs.streams.EndianBinaryReader(b'raw')
        env = unityrs.Environment(budget=unityrs.Budget(0))
        with contextlib.redirect_stdout(io.StringIO()), self.assertRaises(MemoryError):
            unityrs.helpers.ImportHelper.parse_file(reader, env, 'forced', unityrs.enums.FileType.AssetsFile)
        self.assertEqual(reader.Position, 0)
        self.assertEqual(env.files, {})
        self.assertEqual(env.cabs, {})
