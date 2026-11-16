"""Aggregate allocations stay charged through every enclosing array handoff."""
import io
import struct
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[4] / 'tools/parity'))
from binding_y5_round8 import array_asset
import unityrs


class ObjectReaderArrayBounds(unittest.TestCase):
    def boundary(self, payload, command, allowance, expected, stream=False):
        raw = array_asset(payload)
        budget = unityrs.Budget(2_000_000)
        source = io.BytesIO(raw) if stream else raw
        obj = unityrs.Environment(source, budget=budget).objects[0]
        reader = obj.reader
        for available in (allowance - 1, allowance):
            obj.reset()
            baseline = budget.used
            held = obj._native.tree_traversal()
            held.step(0, budget.limit - baseline - available, 0)
            retained = budget.used
            try:
                if available < allowance:
                    with self.assertRaises(MemoryError):
                        reader.read_array(lambda: command(reader), reader.read_int())
                else:
                    value = reader.read_array(lambda: command(reader), reader.read_int())
                    self.assertEqual(value, expected)
                self.assertEqual(budget.used, retained)
                self.assertIsNone(reader._array_staging)
            finally:
                del held
            self.assertEqual(budget.used, baseline)
        # Returned values now belong to Python, and a refusal must leave no
        # stale staging guard that poisons an independent read on this reader.
        obj.reset()
        self.assertEqual(reader.read_array(lambda: command(reader), reader.read_int()), expected)
        self.assertEqual(budget.used, baseline)

    def test_byte_arrays_refuse_the_reported_aggregate(self):
        span = struct.pack('<i', 65_536) + b'\xff' * 65_536
        budget = unityrs.Budget(200_000)
        obj = unityrs.Environment(array_asset(struct.pack('<i', 64) + span * 64),
                                  budget=budget).objects[0]
        obj.reset()
        baseline = budget.used
        with self.assertRaises(MemoryError):
            obj.reader.read_array(obj.reader.read_byte_array, obj.read_int())
        self.assertEqual(budget.used, baseline)

    def test_byte_arrays_refuse_one_byte_below_the_aggregate_boundary(self):
        span = struct.pack('<i', 4096) + b'x' * 4096
        # List 512, retained first span/lengths 4104, native copy 4368.
        for stream in (False, True):
            with self.subTest(stream=stream):
                self.boundary(struct.pack('<i', 2) + span * 2,
                              lambda r: r.read_byte_array(), 8984, [b'x' * 4096] * 2, stream)

    def test_raw_reads_refuse_one_byte_below_the_aggregate_boundary(self):
        for command in (lambda r: r.read(4096), lambda r: r.read_bytes(4096),
                        lambda r: r.read_the_rest(r.Position, 4096)):
            with self.subTest(command=command):
                self.boundary(struct.pack('<i', 2) + b'x' * 8192,
                              command, 8976, [b'x' * 4096] * 2)

    def test_fixed_strings_keep_decoding_and_byte_spans_charged(self):
        # Last decode overlaps three retained strings/spans and its raw copy:
        # 768 + 3*20480 + 20736 + 16384 + 4368.
        for stream in (False, True):
            self.boundary(struct.pack('<i', 4) + b'\xff' * 16384,
                          lambda r: r.read_string(4096), 103696,
                          ['\udcff' * 4096] * 4, stream)

    def test_aligned_strings_keep_decoding_and_byte_spans_charged(self):
        span = struct.pack('<i', 4096) + b'\xff' * 4096
        for stream in (False, True):
            self.boundary(struct.pack('<i', 4) + span * 4,
                          lambda r: r.read_aligned_string(), 103712,
                          ['\udcff' * 4096] * 4, stream)

    def test_null_strings_keep_decoding_charged_in_both_reader_kinds(self):
        payload = struct.pack('<i', 4) + (b'\xff' * 4096 + b'\0') * 4
        # Memory: 768 + 4*16384 + (256 + 4097*7).
        # Stream: 768 + 4*(4097 + 4097*4) + 4097*(128 + 5).
        for stream, allowance in ((False, 95239), (True, 627609)):
            with self.subTest(stream=stream):
                self.boundary(payload, lambda r: r.read_string_to_null(4097),
                              allowance, ['\udcff' * 4096] * 4, stream)

    def test_typed_arrays_keep_tuples_and_byte_spans_charged(self):
        from unityrs.helpers.TypeTreeHelper import FUNCTION_READ_MAP_ARRAY
        widths = {'bool': 1, 'SInt8': 1, 'UInt8': 1, 'char': 1,
                  'SInt16': 2, 'UInt16': 2, 'short': 2, 'unsigned short': 2,
                  'SInt64': 8, 'UInt64': 8, 'long long': 8, 'unsigned long long': 8,
                  'double': 8, 'FileSize': 8}
        for name, read in FUNCTION_READ_MAP_ARRAY.items():
            if name == 'SInt8':
                continue  # Covered by the byte-array boundary above.
            width = widths.get(name, 4)
            entry = struct.pack('<i', 256) + bytes(256 * width)
            for stream in (False, True):
                with self.subTest(name=name, stream=stream):
                    # Three tuples, raw copies, length words and the last native
                    # copy overlap. The peak rises by 1024 per element byte.
                    self.boundary(struct.pack('<i', 3) + entry * 3, read,
                                  66716 + 1024 * width, [(0,) * 256] * 3, stream)

    def test_nested_arrays_charge_byte_spans_to_every_active_guard(self):
        entry = struct.pack('<ii', 1, 4096) + b'x' * 4096
        # At the second inner handoff, the outer guard holds 8976 and the
        # inner guard holds 4484. Both must retain the inner bytes.
        for stream in (False, True):
            self.boundary(struct.pack('<i', 2) + entry * 2,
                          lambda r: r.read_array(r.read_byte_array, r.read_int()),
                          13460, [[b'x' * 4096]] * 2, stream)

    def test_bytes_property_charges_retained_whole_file_copies(self):
        raw = array_asset(struct.pack('<i', 2) + b'x' * 8192)
        length = len(raw)
        # Two whole-file copies overlap at the second native copy. This input
        # is below mmap's threshold; its chunk rounds (length + 8) up to 16.
        allowance = 512 + length + ((length + 23) // 16) * 16 + 256
        self.boundary(struct.pack('<i', 2) + b'x' * 8192,
                      lambda r: r.bytes, allowance, [raw, raw])

    def test_nested_string_arrays_charge_every_active_guard(self):
        entry = struct.pack('<ii', 1, 4096) + b'\xff' * 4096
        # The second string overlaps its decode guard (20736), its inner
        # array (20868), and the retained outer array (41744).
        for stream in (False, True):
            self.boundary(struct.pack('<i', 2) + entry * 2,
                          lambda r: r.read_string_array(), 83348,
                          [['\udcff' * 4096]] * 2, stream)
