"""Synthetic aggregate reader staging and value/cursor observations."""
import importlib
import io
import struct

import scratch
from binding_cases import S


def array_asset(payload, endian='<'):
    tree = S.Node('ArrayStaging', 'Base', [S.Node('int', 'count')])
    return S.serialized_file([S.Type(49, tree)], [S.Obj(1, 0, payload)], endian=endian)


def read_case(reader, name):
    if name == 'nested':
        return reader.read_array(reader.read_byte_array, reader.read_int())
    if name == 'fixed_string':
        return reader.read_string(4)
    if name == 'null_string':
        return reader.read_string_to_null(5)
    if name == 'raw':
        return reader.read_bytes(4)
    if name == 'bytes':
        return reader.bytes[-8:]
    return getattr(reader, name)()


def observe_array_reads(module):
    maps = importlib.import_module(module.__name__ + '.helpers.TypeTreeHelper')
    rows = []
    with scratch.folder('array-staging-') as folder:
        for endian in ('<', '>'):
            count = struct.pack(endian + 'i', 2)
            span = struct.pack(endian + 'i', 4) + b'ab\xffd'
            cases = [('read_byte_array', span), ('raw', b'ab\xffd'),
                     ('fixed_string', b'ab\xffd'), ('null_string', b'ab\xffd\0'),
                     ('read_aligned_string', struct.pack(endian + 'i', 3) + b'a\xffb\0'),
                     ('nested', count + span * 2),
                     ('read_string_array', count + span * 2), ('bytes', b'ab\xffd')]
            cases += [(name, count + bytes.fromhex('80ff0102030405060708090a0b0c0d0e'))
                      for name in ('read_boolean_array', 'read_u_byte_array',
                                   'read_short_array', 'read_u_short_array',
                                   'read_int_array', 'read_u_int_array',
                                   'read_long_array', 'read_u_long_array',
                                   'read_float_array', 'read_double_array')]
            for name, entry in cases:
                # Typed entries need exactly their declared width, so the next
                # entry starts where the first read leaves the cursor.
                if name.endswith('_array') and name not in (
                        'read_byte_array', 'read_string_array'):
                    width = {'read_boolean_array': 1, 'read_u_byte_array': 1,
                             'read_short_array': 2, 'read_u_short_array': 2,
                             'read_int_array': 4, 'read_u_int_array': 4,
                             'read_float_array': 4}.get(name, 8)
                    entry = entry[:4 + 2 * width]
                raw = array_asset(count + entry * 2, endian)
                path = folder / 'synthetic.assets'
                path.write_bytes(raw)
                for source in (raw, io.BytesIO(raw), str(path)):
                    obj = module.Environment(source).objects[0]
                    obj.reset()
                    start = obj.Position
                    reader = obj.reader
                    value = reader.read_array(lambda: read_case(reader, name), reader.read_int())
                    rows.append([endian, name, normalize(value), reader.Position - start])
                # The map aliases must dispatch into the same guarded methods.
            raw = array_asset(count + span * 2, endian)
            for read in (maps.FUNCTION_READ_MAP['TypelessData'],
                         maps.FUNCTION_READ_MAP_ARRAY['SInt8']):
                obj = module.Environment(raw).objects[0]
                obj.reset()
                start = obj.Position
                value = obj.reader.read_array(lambda: read(obj.reader), obj.read_int())
                rows.append([endian, 'map_bytes', normalize(value), obj.Position - start])
    return rows


def normalize(value):
    if isinstance(value, bytes):
        return ['bytes', value.hex()]
    if isinstance(value, str):
        return ['str', value.encode('utf8', 'surrogateescape').hex()]
    if isinstance(value, (list, tuple)):
        return [normalize(item) for item in value]
    return value


def observe_aggregate_bytes(module, corrected=False):
    rows = []
    for count in (2, 64):
        span = struct.pack('<i', 65_536) + b'\xff' * 65_536
        raw = array_asset(struct.pack('<i', count) + span * count)
        kwargs = {'budget': module.Budget(200_000)} if module.__name__ == 'unityrs' else {}
        obj = module.Environment(raw, **kwargs).objects[0]
        obj.reset()
        reader = obj.reader
        try:
            if corrected and module.__name__ == 'UnityPy' and count == 64:
                raise MemoryError()
            value = reader.read_array(reader.read_byte_array, reader.read_int())
            rows.append(['value', len(value), all(item == b'\xff' * 65_536 for item in value)])
        except MemoryError:
            rows.append(['MemoryError'])
    return rows
