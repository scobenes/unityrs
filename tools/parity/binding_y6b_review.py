"""Synthetic round-one helper regressions; no runtime schemas are recorded."""
import importlib
import io
import struct
import threading
from types import SimpleNamespace as N

import binding_cases as B
from binding_y4_exports import _call
from binding_y6b_tree import _node


def observe_schema_failure_cursor(package):
    data = B.S.serialized_file([B.S.Type(1234567)],
        [B.S.Obj(1, 0, b'\0' * 4)], enable_type_tree=False)
    rows = []
    for stream in (False, True):
        for wrap in (False, True):
            obj = package.Environment(io.BytesIO(data) if stream else data).objects[0]
            obj.Position = 7
            rows.append([stream, wrap, _call(lambda: obj.read_typetree(wrap=wrap)), obj.Position])
    return rows


def observe_truncated_object_cursor(package):
    root = B.S.Node('Synthetic', 'Base', [B.S.Node('int', 'value')])
    rows = []
    for payload in (struct.pack('<i', 23) + bytes(60), b'\x17'):
        data = B.S.serialized_file([B.S.Type(49, root)], [B.S.Obj(1, 0, payload, byte_size=100)])
        # The synthetic builder pads data to 64 bytes. Remove that padding
        # here so the one-byte case fails inside accelerated parsing too.
        if len(payload) < 64:
            data = data[:-(64 - len(payload))]
            data = data[:24] + struct.pack('>q', len(data)) + data[32:]
        for stream in (False, True):
            for wrap in (False, True):
                for check in (False, True):
                    obj = package.Environment(io.BytesIO(data) if stream else data).objects[0]
                    def read():
                        value = obj.read_typetree(wrap=wrap, check_read=check)
                        return value.value if wrap else value['value']
                    rows.append([len(payload), stream, wrap, check, _call(read),
                                 obj.Position - obj.byte_start])
    return rows


def observe_tree_live_callbacks(package):
    helper = importlib.import_module(package.__name__ + '.helpers.TypeTreeHelper')
    rows = []
    caller = threading.get_ident()
    for ty in ('string', 'TypelessData'):
        original = helper.FUNCTION_READ_MAP[ty]
        try:
            for mode in ('return', 'consume', 'fail'):
                for kind in ('tree', 'value', 'pair', 'array') if ty == 'string' else ('tree', 'value', 'pair'):
                    events = []
                    def callback(reader):
                        events.append([reader.Position, threading.get_ident() == caller])
                        # Re-enter native parsing from the callback on the caller.
                        value, count = helper.read_typetree_boost(b'\x17\0\0\0', _node(package), '<', True)
                        if (value, count) != (23, 4):
                            raise AssertionError('callback reentry')
                        if mode == 'fail':
                            raise LookupError('synthetic primitive callback')
                        if mode == 'consume':
                            return original(reader)
                        return 'custom' if ty == 'string' else b'custom'
                    helper.FUNCTION_READ_MAP[ty] = callback
                    reader = package.streams.EndianBinaryReader(b'\1\0\0\0x\0\0\0', '<')
                    node = _node(package, ty)
                    config = helper.TypeTreeConfig(True)
                    if kind == 'pair':
                        node = _node(package, 'pair', children=[node, _node(package)])
                    def read():
                        if kind == 'array':
                            return helper.read_value_array(node, reader, config, 1)
                        if kind == 'value':
                            return helper.read_value(node, reader, config)
                        return helper.read_typetree(node, reader, check_read=False)
                    rows.append([ty, mode, kind, _call(read), reader.Position, events])
        finally:
            helper.FUNCTION_READ_MAP[ty] = original
    return rows


def observe_shader_empty_blob(package):
    module = importlib.import_module(package.__name__ + '.export.ShaderConverter')
    form = N(m_Name='Synthetic', m_PropInfo=N(m_Props=[]), m_SubShaders=[],
             m_FallbackName='', m_CustomEditorName='')
    shader = N(compressedBlob=b'', m_ParsedForm=form, object_reader=N(version=(2021, 3)))
    rows = []
    for platforms, compressed, decompressed, offsets in (([], [], [], []),
            ([0], [1], [1], [0]), ([0], [[1]], [[1]], [[0]]),
            ([0], [], [], [])):
        shader.platforms, shader.compressedLengths = platforms, compressed
        shader.decompressedLengths, shader.offsets = decompressed, offsets
        rows.append(_call(lambda: module.ConvertSerializedShader(shader)))
    return rows


def observe_lz4_signed_sizes(package):
    helper = importlib.import_module(package.__name__ + '.helpers.CompressionHelper')
    rows = []
    # Out-of-range headers are rejected by python-lz4 before allocation. Keep
    # valid headers tiny: they exercise exact length rather than capacity.
    for size in (-2**31 - 1, -2**31, -2, -1, 2**31):
        rows.append(['argument', size, _call(lambda: helper.decompress_lz4(b'\1\0\0\0\x10x', size))])
    for size in (0, 1, 2, 2**31, 2**31 + 1, 2**32 - 1):
        data = struct.pack('<I', size) + b'\x10x'
        rows.append(['header', size, _call(lambda: helper.decompress_lz4(data, -1))])
    return rows


def shader_reader(package, count, program=False, **options):
    # Empty keywords isolate retained list charges from native byte-copy charges.
    blob = struct.pack('<7i', 202012090, 1, 0, 0, 0, 0, count) + bytes(count * 4 + 4)
    if program:
        blob = struct.pack('<i', 1) + blob
    data = B.S.serialized_file([B.S.Type(49)], [B.S.Obj(1, 0, blob)], enable_type_tree=False)
    if program:
        start = struct.unpack_from('>q', data, 32)[0]
        # Version 22 ignores the legacy size field. Use it as the program's
        # absolute offset table, so the real asset reader needs no replacement.
        data = data[:4] + struct.pack('<i', start + 4) + data[8:]
    obj = package.Environment(data, **options).objects[0]
    reader = obj.reader
    reader.Position = obj.byte_start
    return obj, reader


def observe_shader_asset_readers(package):
    module = importlib.import_module(package.__name__ + '.export.ShaderConverter')
    rows = []
    for program in (False, True):
        for count in (2, 20_000):
            obj, reader = shader_reader(package, count, program)
            value = module.ShaderProgram(reader, (2019, 3)) if program else module.ShaderSubProgram(reader)
            sub = value.m_SubPrograms[0] if program else value
            rows.append([program, count, len(sub.m_Keywords), sub.m_Keywords[:2],
                         sub.m_LocalKeywords, sub.m_ProgramCode.hex(), reader.Position - obj.byte_start])
    return rows


def observe_tree_memoryview_strings(package):
    helper = importlib.import_module(package.__name__ + '.helpers.TypeTreeHelper')
    rows = []
    for data in (b'x', b'\xc3\xa9', b'\xff'):
        for kind in ('tree', 'value', 'array'):
            reader = package.streams.EndianBinaryReader(struct.pack('<i', len(data)) + data + bytes(3), '<')
            original = reader.read_bytes
            reader.read_bytes = lambda size: memoryview(original(size))
            node, config = _node(package, 'string'), helper.TypeTreeConfig(True)
            def read():
                if kind == 'array':
                    return helper.read_value_array(node, reader, config, 1)
                if kind == 'value':
                    return helper.read_value(node, reader, config)
                return helper.read_typetree(node, reader, check_read=False)
            rows.append([data.hex(), kind, _call(read), reader.Position])
    return rows
