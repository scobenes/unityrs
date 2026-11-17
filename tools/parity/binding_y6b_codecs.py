"""Synthetic codec registry callbacks, malformed helpers and reader redirection."""
import importlib
import io
import struct
import threading
from contextlib import redirect_stdout
from types import MethodType

import binding_cases as B


def _module(package, path):
    return importlib.import_module(package.__name__ + '.' + path)


def _answer(call):
    try:
        value = call()
        return ['bytes', bytes(value).hex()] if isinstance(value, (bytes, bytearray, memoryview)) else ['value', value]
    except Exception as error:
        return [type(error).__name__, str(error)]


def observe_codec_failures(package):
    codec = _module(package, 'helpers.CompressionHelper')
    rows = []
    for data in (b'', b'1234', b'\xff\0\0\0\0', b'\x5d\0\0\x80\0\xff' * 2):
        rows.append(['lzma', data.hex(), _answer(lambda: codec.decompress_lzma(data))])
    for data, size in ((b'', 0), (b'\x10', 1), (b'\x10x', 0), (b'\x10x', 1), (b'\0', -1), (b'\0', 2**31)):
        rows.append(['lz4', data.hex(), size, _answer(lambda: codec.decompress_lz4(data, size))])
    for data in (b'', b'\xff', b'not compressed'):
        rows.append(['brotli', data.hex(), _answer(lambda: codec.decompress_brotli(data))])
        rows.append(['gzip', data.hex(), _answer(lambda: codec.decompress_gzip(data))])
    return rows


def observe_codec_callbacks(package):
    codec = _module(package, 'helpers.CompressionHelper')
    original = dict(codec.DECOMPRESSION_MAP)
    caller = threading.get_ident()
    events = []
    env = package.Environment()
    bundle = B.S.fs_bundle([(b'payload', b'synthetic payload', 0)], codec=0)
    builtin = codec.DECOMPRESSION_MAP[0]

    def decode(data, size):
        events.append([len(data), size, threading.get_ident() == caller])
        # Re-enter the same environment while its load is in flight.
        env.load_file(b'reentrant bytes', name='nested-resource')
        codec.decompress_lz4(b'\x10x', 1)
        return builtin(data, size)

    try:
        codec.DECOMPRESSION_MAP[0] = decode
        loaded = env.load_file(bundle, name='bundle')
        result = [events, loaded.files['payload'].bytes.hex(), env.files['nested-resource'].bytes.hex()]
        codec.DECOMPRESSION_MAP[0] = lambda data, size: (_ for _ in ()).throw(LookupError('synthetic callback failure'))
        # Environment prints its expected load diagnostics with a randomized
        # buffer name; keep them out of the suite runner's final verdict line.
        with redirect_stdout(io.StringIO()):
            result.append(_answer(lambda: package.Environment(bundle)))
            del codec.DECOMPRESSION_MAP[0]
            result.append(_answer(lambda: package.Environment(bundle)))
        return result
    finally:
        codec.DECOMPRESSION_MAP.clear()
        codec.DECOMPRESSION_MAP.update(original)


def observe_codec_compression(package):
    codec = _module(package, 'helpers.CompressionHelper')
    original = dict(codec.COMPRESSION_MAP)
    chunks = dict(codec.COMPRESSION_CHUNK_SIZE_MAP)
    decompression = dict(codec.DECOMPRESSION_MAP)
    env = package.Environment(B.S.fs_bundle([(b'payload', b'abcd' * 4, 0)], codec=0))
    bundle = next(iter(env.files.values()))
    caller = threading.get_ident()
    events = []

    def compress(data):
        events.append([bytes(data).hex(), threading.get_ident() == caller])
        env.load_file(b'reentrant write', name='nested-write')
        return bytes(data)

    try:
        codec.COMPRESSION_MAP[5] = compress
        codec.COMPRESSION_CHUNK_SIZE_MAP[5] = 5
        codec.DECOMPRESSION_MAP[5] = lambda data, size: data
        packed = bundle.save(packer=(0x45, 5))
        reloaded = package.Environment(packed)
        result = [events, reloaded.objects == [], next(iter(reloaded.files.values())).files['payload'].bytes.hex()]
        mutation = []
        def captured(data):
            mutation.append(bytes(data).hex())
            codec.COMPRESSION_MAP[5] = lambda data: (_ for _ in ()).throw(AssertionError('compressor was reselected'))
            codec.COMPRESSION_CHUNK_SIZE_MAP[5] = 1000
            return bytes(data)
        codec.COMPRESSION_MAP[5] = captured
        packed = bundle.save(packer=(0x40, 5))
        result.append([mutation, next(iter(package.Environment(packed).files.values())).files['payload'].bytes.hex()])
        codec.COMPRESSION_CHUNK_SIZE_MAP[5] = 5
        released = []
        class Compressor:
            def __call__(self, data):
                codec.COMPRESSION_MAP.pop(5, None)
                return bytes(data)
            def __del__(self):
                released.append(threading.get_ident() == caller)
                env.load_file(b'finalizer', name='finalizer-resource')
        codec.COMPRESSION_MAP[5] = Compressor()
        bundle.save(packer=(0x40, 5))
        result.append(released)
        byte_released = []
        class ReturnedBytes(bytes):
            def __del__(self):
                byte_released.append(threading.get_ident() == caller)
        codec.COMPRESSION_MAP[0] = ReturnedBytes
        bundle.save(packer=(0x40, 0))
        result.append(byte_released)
        codec.COMPRESSION_MAP[0] = original[0]
        result.append(_answer(lambda: bundle.save(packer=(0x45, 5))))
        codec.COMPRESSION_MAP[5] = compress
        del codec.COMPRESSION_CHUNK_SIZE_MAP[5]
        result.append(_answer(lambda: bundle.save(packer=(0x45, 5))))
        return result
    finally:
        for target, source in ((codec.COMPRESSION_MAP, original), (codec.COMPRESSION_CHUNK_SIZE_MAP, chunks), (codec.DECOMPRESSION_MAP, decompression)):
            target.clear()
            target.update(source)


def observe_reader_redirect(package):
    rows = []
    data = next(data for name, data in B.cases() if name == 'text')
    for stream in (False, True):
        env = package.Environment(data)
        obj = env.objects[0]
        for endian in ('<', '>'):
            payload = B.S.s32('redirected', endian) + B.S.s32('reader', endian)
            obj.reader = package.streams.EndianBinaryReader(io.BytesIO(b'\0' * obj.byte_start + payload) if stream else b'\0' * obj.byte_start + payload, endian)
            obj.byte_size = len(payload)
            rows.append([endian, stream, obj.parse_as_dict(), obj.Position - obj.byte_start])
            position = obj.Position
            rows.append(['raw', obj.get_raw_data().hex(), obj.Position == position])
        obj.reader.endian = '<'
        rows.append(['changed-endian', stream, _answer(obj.parse_as_dict), obj.Position - obj.byte_start])
        obj.Position = 7
        rows.append(['invalid-nodes', stream, _answer(lambda: obj.parse_as_dict(node=123)), obj.Position])
    for subclass in (False, True):
        obj = package.Environment(data).objects[0]
        reader = obj.reader
        payload = B.S.s32('clone') + B.S.s32('new!')
        def read_bytes(self, size):
            start = self.Position
            self.Position += size
            return self._payload[start:start + size]
        if subclass:
            replacement = type('ReplacementReader', (type(reader),), {'read_bytes': read_bytes})
            changed = object.__new__(replacement)
            changed.__dict__.update(reader.__dict__)
            for base in type(reader).__mro__:
                for name in getattr(base, '__slots__', ()):
                    if name not in ('__dict__', '__weakref__'):
                        setattr(changed, name, getattr(reader, name))
        else:
            changed = reader
            changed.read_bytes = MethodType(read_bytes, changed)
        changed._payload = b'\0' * obj.byte_start + payload
        obj.reader = changed
        rows.append(['override', subclass, obj.parse_as_dict(), obj.get_raw_data().hex(), obj.Position - obj.byte_start])
    return rows


def observe_reader_failures(package):
    rows = []
    for name, data in B.cases():
        if name not in ('short_read', 'short_int', 'short_bool', 'short_string', 'short_TypelessData', 'check_read'):
            continue
        for stream in (False, True):
            obj = package.Environment(io.BytesIO(data) if stream else data).objects[0]
            rows.append([name, stream, _answer(obj.parse_as_dict), obj.Position - obj.byte_start])
    tree = B.S.Node('Synthetic', 'Base', [B.S.Node('int', 'first'), B.S.Node('int', 'second')])
    data = B.S.serialized_file([B.S.Type(49, tree)], [B.S.Obj(1, 0, struct.pack('<i', 7), byte_size=0)])
    for stream in (False, True):
        obj = package.Environment(io.BytesIO(data) if stream else data).objects[0]
        rows.append(['pure', stream, _answer(obj.parse_as_dict), obj.Position - obj.byte_start])
        payload = struct.pack('<i', 7)
        obj.reader = package.streams.EndianBinaryReader(io.BytesIO(payload) if stream else payload, '<')
        obj.byte_start = 0
        rows.append(['pure-truncated', stream, _answer(obj.parse_as_dict), obj.Position])
    return rows
