"""Synthetic reader ownership cases, independent of runtime schema data."""
import importlib
import io
import struct
from types import SimpleNamespace as N

import binding_cases as B
from binding_y6b_tree import _node


def tree_reader(package, count=3, stream=False, **options):
    schema = B.S.Node('vector', 'value', [B.S.Node('Array', 'Array', [
        B.S.Node('int', 'size'), B.S.Node('int', 'data')])])
    payload = struct.pack('<i', count) + struct.pack('<i', 23) * count
    data = B.S.serialized_file([B.S.Type(49, schema)], [B.S.Obj(1, 0, payload)])
    obj = package.Environment(io.BytesIO(data) if stream else data, **options).objects[0]
    obj.reader.Position = obj.byte_start
    return obj, payload


def detached_node(package):
    return _node(package, 'vector', children=[_node(package, 'Array', children=[
        _node(package, 'int', 'size'), _node(package, 'int', 'data')])])


def tree_call(package, kind, node, obj, payload, assetsfile=None, **options):
    helper = importlib.import_module(package.__name__ + '.helpers.TypeTreeHelper')
    reader = obj.reader
    if kind == 'boost':
        return helper.read_typetree_boost(payload, node, '<', True, assetsfile, **options)
    if kind == 'value':
        return helper.read_value(node, reader, helper.TypeTreeConfig(True, assetsfile), **options)
    if kind == 'array':
        count = reader.read_int()
        return helper.read_value_array(node.m_Children[0].m_Children[1], reader,
                                       helper.TypeTreeConfig(True, assetsfile), count, **options)
    return helper.read_typetree(node, reader, byte_size=len(payload) if kind == 'accelerated' else None,
                                check_read=False, assetsfile=assetsfile, **options)


def observe_tree_asset_meter(package):
    rows = []
    for stream in (False, True):
        for owned in (False, True):
            for attached in (False, True):
                for kind in ('pure', 'accelerated', 'value', 'array', 'boost'):
                    obj, payload = tree_reader(package, stream=stream)
                    node = obj._get_typetree_node() if owned else detached_node(package)
                    file = obj.assets_file if attached else None
                    value = tree_call(package, kind, node, obj, payload, file)
                    writer = package.streams.EndianBinaryWriter(endian='<')
                    helper = importlib.import_module(package.__name__ + '.helpers.TypeTreeHelper')
                    helper.write_typetree([23] * 3, node, writer, file)
                    rows.append([stream, owned, attached, kind, value,
                                 obj.reader.Position - obj.byte_start, writer.bytes.hex()])
    return rows


def observe_tree_count_expansion(package):
    rows = []
    for stream in (False, True):
        for kind in ('pure', 'accelerated', 'value', 'array'):
            obj, payload = tree_reader(package, 20_000, stream=stream)
            value = tree_call(package, kind, detached_node(package), obj, payload)
            rows.append([stream, kind, len(value), value[0], value[-1], obj.reader.Position - obj.byte_start])
    return rows


def observe_reference_asset_meter(package):
    helper = importlib.import_module(package.__name__ + '.helpers.TypeTreeHelper')
    obj, _ = tree_reader(package)
    node = _node(package)
    obj.assets_file.ref_types = [N(m_ClassName='Other'), N(
        m_ClassName='Synthetic', m_NameSpace='', m_AssemblyName='', node=node)]
    value = helper.get_ref_type_node({'type': {'class': 'Synthetic', 'ns': '', 'asm': ''}}, obj.assets_file)
    return [value is node, value.m_Type, value.m_Name, obj.reader.Position - obj.byte_start]


def observe_shader_asset_stages(package):
    helper = importlib.import_module(package.__name__ + '.export.ShaderConverter')
    from binding_y6b_crypto_shader import property_value
    obj, _ = tree_reader(package)
    rows = []
    for count in (0, 2):
        form = N(m_Name='Synthetic', m_PropInfo=N(m_Props=[property_value(0)] * count),
                 m_SubShaders=[], m_FallbackName='', m_CustomEditorName='')
        shader = N(object_reader=obj, platforms=[], compressedLengths=[], decompressedLengths=[],
                   offsets=[], compressedBlob=b'', m_ParsedForm=form)
        rows.append(helper.ConvertSerializedShader(shader))
    return rows


def observe_decryptor_asset_meter(package):
    helper = importlib.import_module(package.__name__ + '.helpers.ArchiveStorageManager')
    from binding_y6b_crypto_shader import crypto_inputs
    inputs = crypto_inputs()
    payload = bytes.fromhex(inputs['header'])
    data = B.S.serialized_file([B.S.Type(49)], [B.S.Obj(1, 0, payload)], enable_type_tree=False)
    rows = []
    old_key = helper.DECRYPT_KEY
    try:
        helper.DECRYPT_KEY = bytes.fromhex(inputs['key'])
        for stream in (False, True):
            obj = package.Environment(io.BytesIO(data) if stream else data).objects[0]
            reader = obj.reader
            reader.Position, reader.endian = obj.byte_start, '>'
            value = helper.ArchiveStorageDecryptor(reader)
            rows.append([stream, value.unknown_1, value.index.hex(), value.substitute.hex(),
                         value.data.hex(), value.key.hex(), value.data_sig.hex(), value.key_sig.hex(),
                         reader.Position - obj.byte_start])
    finally:
        helper.DECRYPT_KEY = old_key
    return rows
