# Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/Tpk.py, UnityPy/helpers/CompressionHelper.py. See LICENSE-UNITYPY and THIRD_PARTY_LICENSES.
"""Private generator input. Tables live only in memory, never in a wheel.

The core provider deliberately discards editor/name/base metadata. Generation
needs those records too (helpers/Tpk.py:215-240, helpers/Tpk.py:289-345,
helpers/Tpk.py:351-397, helpers/Tpk.py:404-423, helpers/Tpk.py:445-456).
Python allocations raise MemoryError; input, decoder and table sizes are bounded.
"""
import importlib.util
import lzma
import os
import struct
from pathlib import Path
from types import SimpleNamespace

MAX_BYTES = 256 * 1024 * 1024
MAX_RECORDS = 4_000_000
MAX_TABLE_BYTES = 128 * 1024 * 1024


class _TableBudget:
    """Conservative Python table storage, charged before lists/tuples/strings."""
    def __init__(self):
        self.records = 0
        self.bytes = 0

    def charge(self, records, size):
        if self.records + records > MAX_RECORDS:
            raise ValueError('TPK tables exceed cumulative record limit')
        if self.bytes + size > MAX_TABLE_BYTES:
            raise ValueError('TPK tables exceed allocation limit')
        self.records += records
        self.bytes += size


def source():
    # Y3 owns config. Accept its module if present without creating a competing
    # config module here. Explicit bytes/path precede the environment/default.
    from . import __dict__ as package
    config = package.get('config')
    if config is None and importlib.util.find_spec('unityrs.config') is not None:
        from . import config
    value = getattr(config, 'TPK', None) if config is not None else None
    if value is None:
        value = os.environ.get('UNITYRS_TPK')
    if value is None:
        spec = importlib.util.find_spec('UnityPy')
        if spec is not None and spec.submodule_search_locations:
            value = Path(next(iter(spec.submodule_search_locations))) / 'resources' / 'lzma.tpk'
    if value is None:
        raise ImportError('unityrs.classes requires a TPK: set UNITYRS_TPK to a path or unityrs.config.TPK to a path or bytes; alternatively install UnityPy')
    if isinstance(value, (bytes, bytearray, memoryview)):
        if len(value) > MAX_BYTES:
            raise ImportError('class-generation TPK exceeds 256 MiB')
        return value
    try:
        with open(value, 'rb') as stream:
            data = stream.read(MAX_BYTES + 1)
        if len(data) > MAX_BYTES:
            raise ValueError('class-generation TPK exceeds 256 MiB')
        return data
    except (OSError, ValueError, TypeError) as error:
        raise ImportError('cannot read class-generation TPK supplied through config.TPK or UNITYRS_TPK') from error


class Stream:
    def __init__(self, data, budget=None):
        self.data = memoryview(data)
        self.at = 0
        self.budget = budget

    def read(self, n):
        if n < 0 or n > len(self.data) - self.at:
            raise ValueError('truncated TPK')
        data = self.data[self.at:self.at+n]
        self.at += n
        return data

    def unpack(self, fmt):
        return struct.unpack(fmt, self.read(struct.calcsize(fmt)))

    def count(self, item_bytes=0):
        n, = self.unpack('<i')
        if n > MAX_RECORDS:
            raise ValueError('TPK table exceeds record limit')
        n = max(0, n)
        if self.budget is not None:
            self.budget.charge(n, n * item_bytes)
        return n

    def string(self):
        size = 0
        for shift in range(0, 35, 7):
            b, = self.unpack('B')
            size |= (b & 127) << shift
            if b < 128:
                if self.budget is not None:
                    # The input byte copy and a four-byte Unicode representation.
                    self.budget.charge(0, 5 * size)
                return bytes(self.read(size)).decode('utf8')
        raise ValueError('invalid TPK string length')


def load(data):
    try:
        if len(data) > MAX_BYTES:
            raise ValueError('TPK input exceeds 256 MiB')
        s = Stream(data)
        magic, version, compression, kind, _, _, size, decoded = s.unpack('<IbbbbIII')
        if magic != 0x2A4B5054 or version != 1 or kind != 0:
            raise ValueError('expected a version 1 type-tree TPK')
        if decoded > MAX_BYTES:
            raise ValueError('TPK decoded payload exceeds 256 MiB')
        payload = s.read(size)
        if compression == 2:
            # helpers/CompressionHelper.py:17-44 (Unity raw LZMA properties).
            props, dictionary = struct.unpack('<BI', payload[:5])
            if dictionary > MAX_BYTES:
                raise ValueError('TPK decoder dictionary exceeds 256 MiB')
            decoder = lzma.LZMADecompressor(format=lzma.FORMAT_RAW, filters=[{
                'id': lzma.FILTER_LZMA1, 'dict_size': dictionary,
                'lc': props % 9, 'lp': (props // 9) % 5, 'pb': props // 45}])
            payload = decoder.decompress(payload[5:], max_length=decoded + 1)
            if len(payload) != decoded:
                raise ValueError('TPK decoded payload size differs from header')
        elif compression == 1:
            from ._native import class_tpk_lz4
            payload = class_tpk_lz4(payload, decoded)
        elif compression != 0:
            raise ValueError('class generation requires an uncompressed, LZ4 or LZMA TPK')
        table_budget = _TableBudget()
        s = Stream(payload, table_budget)
        s.read(8)
        s.read(s.count() * 8)
        classes = {}
        for _ in range(s.count(192)):
            class_id, = s.unpack('<i')
            entries = []
            # Namespace/dict, version tuple, integer values and list capacity.
            count = s.count(512)
            for _ in range(count):
                version, present = s.unpack('<QB')
                item = None
                if present:
                    name, base, flags = s.unpack('<HHB')
                    editor = s.unpack('<H')[0] if flags & 64 else None
                    release = s.unpack('<H')[0] if flags & 128 else None
                    item = SimpleNamespace(Name=name, Base=base, Flags=flags,
                                           EditorRootNode=editor, ReleaseRootNode=release)
                entries.append((version, item))
            classes[class_id] = entries
        s.read(s.count() * 9)
        s.read(s.count() * 2)
        nodes = []
        for _ in range(s.count(384)):
            ty, name, size, version, flags, meta, count = s.unpack('<HHihbIH')
            table_budget.charge(count, 36 * count)
            children = s.unpack('<' + str(count) + 'H')
            nodes.append(SimpleNamespace(TypeName=ty, Name=name, SubNodes=children))
        strings = [s.string() for _ in range(s.count(96))]
        return SimpleNamespace(ClassInformation=classes, NodeBuffer=nodes, StringBuffer=strings)
    except (ValueError, IndexError, struct.error, lzma.LZMAError) as error:
        raise ImportError('invalid class-generation TPK: ' + str(error)) from error
