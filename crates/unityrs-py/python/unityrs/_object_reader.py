# Ported from UnityPy 1.25.2 (MIT): UnityPy/files/ObjectReader.py, UnityPy/streams/EndianBinaryReader.py. See LICENSE-UNITYPY and THIRD_PARTY_LICENSES.
"""A shared native file reader without copying the whole input buffer.

Forwarding follows files/ObjectReader.py:199-228; stream methods follow
streams/EndianBinaryReader.py:34-228. Native reads meter each returned span.
"""
from .streams.EndianBinaryReader import EndianBinaryReader, STREAM_FUNCTIONS
from struct import calcsize
from operator import index
from sys import maxsize


class ObjectFileReader(EndianBinaryReader):
    _function_map = STREAM_FUNCTIONS

    def __new__(cls, native, bytes_io=False):
        return object.__new__(cls)

    def __init__(self, native, bytes_io=False):
        self._native = native
        self._bytes_io = bytes_io
        self.Length, endian, self._stream_kind, initial_position = native.serialized_reader_info()
        self._array_staging = None
        self.stream = self
        super().__init__(None, endian=endian)
        self._position = initial_position

    @property
    def Position(self):
        return self._position

    @Position.setter
    def Position(self, value):
        # Native seek validation includes the path reader's negative positions.
        self._native.serialized_read(value, 0)
        self._position = value

    def read(self, size=-1):
        if size is None:
            size = -1 if self._stream_kind else 0
        else:
            size = index(size)
            # BytesIO accepts every representable negative size. Buffered
            # path readers reject sizes below -1; retain native validation.
            if self._bytes_io and -maxsize - 1 <= size < 0:
                size = -1
        data, self._position = self._native.serialized_read(self._position, size)
        return data

    @property
    def bytes(self):
        return self._native.serialized_read(0, -1)[0]

    def read_string(self, size=None):
        if size is None:
            return self.read_string_to_null()
        remaining = max(self.Length - max(self.Position, 0), 0)
        span = remaining if size < 0 else min(size, remaining)
        guard = self._native.tree_traversal()
        guard.step(0, 256 + span * 5, 1)
        self._retain(span * 4)
        return self.read_bytes(size).decode('utf8', 'surrogateescape')

    def _retain(self, memory):
        pending = self._array_staging
        while pending is not None:
            guard, pending = pending
            guard.step(0, memory, 1)

    def read_array(self, command, length):
        guard = self._native.tree_traversal()
        guard.step(0, 256 + max(length, 0) * 128, max(length, 0))
        self._retain(max(length, 0) * 128)
        previous = self._array_staging
        self._array_staging = (guard, previous)
        try:
            return [command() for _ in range(length)]
        finally:
            self._array_staging = previous

    def read_aligned_string(self):
        length = self.read_int()
        if 0 < length <= self.Length - self.Position:
            result = self.read_string(length)
            self.align_stream()
            return result
        return ''

    def read_array_struct(self, param, length=None):
        if length is None:
            length = self.read_int()
        guard = self._native.tree_traversal()
        guard.step(0, 256 + max(length, 0) * (64 + calcsize(self.endian + param)), max(length, 0))
        self._retain(max(length, 0) * 64)
        return super().read_array_struct(param, length)

    def read_string_to_null(self, max_length=32767):
        # streams/EndianBinaryReader.py:275-284: the memory reader searches a
        # bounded window and leaves its position when an unterminated tail fails.
        guard = self._native.tree_traversal()
        if not self._stream_kind:
            start = max(self.Position, 0)
            count = min(max(max_length, 0), max(self.Length - start, 0))
            guard.step(0, 256 + count * 7, count)
            data = self._native.serialized_read(start, count)[0]
            end = data.find(b'\0')
            if end < 0:
                if self.Position + max_length >= self.Length:
                    raise Exception('String not terminated')
                return self.read_string(max_length)
            self.Position = start + end + 1
            self._retain(end * 4)
            return data[:end].decode('utf8', 'surrogateescape')
        ret, char = [], b''
        while char != b'\0' and len(ret) < max_length and self.Position != self.Length:
            guard.step(0, 128, 1)
            ret.append(char)
            char = self.read(1)
            if not char:
                raise ValueError('Unterminated string: %r' % ret)
        guard.step(0, len(ret) * 5, 1)
        self._retain(len(ret) * 4)
        return b''.join(ret).decode('utf8', 'surrogateescape')
