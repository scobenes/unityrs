"""Direct crypto and shader APIs on synthetic bytes and supplied fields only."""
import contextlib
import importlib
import io
import json
import re
import struct
from pathlib import Path
from types import SimpleNamespace as N

import scratch
from binding_y4_exports import _call


def _module(package, suffix):
    return importlib.import_module(package.__name__ + '.' + suffix)


def observe_crypto_block_indices(package):
    module = _module(package, 'helpers.ArchiveStorageManager')
    value = module.ArchiveStorageDecryptor.__new__(module.ArchiveStorageDecryptor)
    value.index = bytes(range(16))
    value.substitute = bytes(range(16))
    return [[index, _call(lambda: value.decrypt_block(b'\x10X', index))]
            for index in (-1, -(1 << 64) - 1, 0, (1 << 64) - 1, 1 << 64, (1 << 128) + 1)]


def observe_crypto_keys(package):
    module = _module(package, 'helpers.ArchiveStorageManager')
    result = {}
    for size in (0, 1, 15, 16, 17, 24, 32):
        for length in (0, 1, 16, 32):
            key = bytes(range(size))
            vector = bytes(range(length))
            result[f'{size}/{length}'] = _call(lambda: module.decrypt_key(vector, b'prefix' * 6, key))
    result['short_data'] = _call(lambda: module.decrypt_key(bytes(32), b'abc', bytes(16)))
    return result


def crypto_inputs():
    path = Path(__file__).resolve().parents[2] / 'crates/unityrs-core/tests/fixtures/unitycn/oracle.json'
    return json.loads(path.read_text())['inputs']


def observe_crypto_decryptor(package):
    module = _module(package, 'helpers.ArchiveStorageManager')
    Reader = _module(package, 'streams').EndianBinaryReader
    inputs = crypto_inputs()
    old = module.DECRYPT_KEY
    try:
        module.set_assetbundle_decrypt_key(bytes.fromhex(inputs['key']))
        reader = Reader(bytes.fromhex(inputs['header']))
        value = module.ArchiveStorageDecryptor(reader)
        result = dict(fields=[value.unknown_1, value.data.hex(), value.key.hex(), value.data_sig.hex(),
                              value.key_sig.hex(), value.index.hex(), value.substitute.hex(), reader.Position], blocks=[])
        for case in inputs['blocks']:
            if case['tier_u']:
                continue
            value.index = bytes.fromhex(case['index'])
            value.substitute = bytes.fromhex(case['substitute'])
            result['blocks'].append(_call(lambda: value.decrypt_block(bytes.fromhex(case['hex']), case['block_index'])))
        # Exercise byte and sequence helpers on safe literal-only synthetic data.
        value.index = bytes(range(16))
        value.substitute = bytes(16)
        data = bytearray(b'\x10X')
        result['byte'] = _call(lambda: value.decrypt_byte(data, 0, 257))
        result['sequence'] = _call(lambda: value.decrypt(data, 0, len(data)))
        result['mutated'] = data.hex()
        module.DECRYPT_KEY = None
        result['missing'] = _call(lambda: module.ArchiveStorageDecryptor(Reader(bytes.fromhex(inputs['header']))))
        module.DECRYPT_KEY = bytes(16)
        result['wrong'] = _call(lambda: module.ArchiveStorageDecryptor(Reader(bytes.fromhex(inputs['header']))))
        return result
    finally:
        module.DECRYPT_KEY = old


def observe_crypto_bruteforce(package):
    module = _module(package, 'helpers.ArchiveStorageManager')
    key = b'0123456789abcdef'
    vector = bytes(range(16))
    signature = module.decrypt_key(vector, module.UNITY3D_SIGNATURE, key)
    with scratch.folder('crypto-probe-') as root:
        path = root / 'synthetic.bin'
        path.write_bytes(b'!badbadbadbadbadb!' + key + b'!')
        result = {'found': _call(lambda: module.brute_force_key(str(path), vector, signature)),
                  'missing': _call(lambda: module.brute_force_key(str(path), vector, bytes(16))),
                  'custom': _call(lambda: module.brute_force_key(str(path), vector, signature, re.compile(rb'(0123456789abcdef)')))}
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            module.brute_force_key(str(path), vector, signature, verbose=True)
        result['verbose'] = output.getvalue()
        return result


def property_value(kind):
    return N(m_Attributes=['Synthetic'], m_Name='_Value', m_Description='Value', m_Type=kind,
             m_DefValue_0_=.5, m_DefValue_1_=0., m_DefValue_2_=2., m_DefValue_3_=1.,
             m_DefTexture=N(m_TexDim=2, m_DefaultName='white'))


def shader_state():
    return N(m_Name='pass', m_LOD=3, m_Tags=N(tags=[('LightMode', 'Synthetic')]),
             alphaToMask=N(val=1), zClip=N(val=0), zTest=N(val=2), zWrite=N(val=0),
             culling=N(val=1), offsetFactor=N(val=.5), offsetUnits=N(val=1.),
             lighting=True, gpuProgramID=7)


def observe_shader_stages(package):
    module = _module(package, 'export.ShaderConverter')
    props = N(m_Props=[property_value(k) for k in range(5)])
    use = N(m_Type=1, m_UseName='Synthetic/PASS')
    grab = N(m_Type=2, m_TextureName='_Grab')
    state = shader_state()
    normal = N(m_Type=0, m_State=state, **{name: N(m_SubPrograms=[]) for name in
               ('progVertex', 'progFragment', 'progGeometry', 'progHull', 'progDomain')})
    sub = N(m_LOD=12, m_Tags=N(tags=[('Queue', 'Geometry')]), m_Passes=[use, grab, normal])
    form = N(m_Name='Synthetic', m_PropInfo=props, m_SubShaders=[sub],
             m_FallbackName='Fallback', m_CustomEditorName='Editor')
    result = {f'property_{k}': _call(lambda k=k: module.ConvertSerializedProperty(property_value(k))) for k in range(5)}
    result.update(properties=_call(lambda: module.ConvertSerializedProperties(props)),
                  tags=_call(lambda: module.ConvertSerializedTagMap(sub.m_Tags, 3)),
                  negative_indent=_call(lambda: module.ConvertSerializedTagMap(sub.m_Tags, -1)),
                  empty_tags=_call(lambda: module.ConvertSerializedTagMap(N(tags=[]), 1)),
                  state=_call(lambda: module.ConvertSerializedShaderState(state)),
                  blend=_call(lambda: module.ConvertSerializedShaderRTBlendState(None)),
                  subshader=_call(lambda: module.ConvertSerializedSubShader(sub, [], [])),
                  parsed=_call(lambda: module.ConvertSerializedShaderParsedForm(form, [], [])),
                  empty_subs=_call(lambda: module.ConvertSerializedSubPrograms([], [], [])))
    result['invalid_property'] = _call(lambda: module.ConvertSerializedProperty(property_value(9)))
    blob = struct.pack('<3i', 1, 12, 0) + sub_blob()
    compression = _module(package, 'helpers.CompressionHelper')
    compressed = compression.compress_lz4(blob)
    shader = N(platforms=[0], compressedLengths=[len(compressed)],
               decompressedLengths=[len(blob)], offsets=[0], compressedBlob=compressed,
               m_ParsedForm=form, object_reader=N(version=(2018, 1)))
    result['serialized'] = _call(lambda: module.ConvertSerializedShader(shader))
    for name, value in [('use', use), ('grab', grab), ('normal', normal)]:
        result[name] = _call(lambda: module.ConvertSerializedPass(value, [], []))
    return result


def sub_blob(kind=1, code=b'void main() {}', local=True):
    def word(value):
        data = struct.pack('<i', len(value)) + value
        return data + bytes((-len(data)) % 4)
    version = 201806140 if local else 202012090
    data = struct.pack('<6i', version, kind, 0, 0, 0, 0) + struct.pack('<i', 1) + word(b'KEY')
    if local:
        data += struct.pack('<i', 1) + word(b'LOCAL')
    data += struct.pack('<i', len(code)) + code
    return data + bytes((-len(data)) % 4)


def observe_shader_programs(package):
    module = _module(package, 'export.ShaderConverter')
    Reader = _module(package, 'streams').EndianBinaryReader
    result = {}
    for kind, code in [(1, b'void main() {}'), (9, b'bytecode'), (23, b'entry\0metal source'),
                       (24, b'entry\0fragment'), (25, b'spirv'), (26, b'console'), (0, b'other')]:
        reader = Reader(sub_blob(kind, code), endian='<')
        sub = module.ShaderSubProgram(reader)
        result[f'sub_{kind}'] = [sub.m_Version, int(sub.m_ProgramType), sub.m_Keywords,
                                sub.m_LocalKeywords, sub.m_ProgramCode.hex(), reader.Position, _call(sub.Export)]
    for modern in (False, True):
        stride = 12 if modern else 8
        blob = struct.pack('<i', 1) + struct.pack('<i', 4 + stride) + bytes(stride - 4) + sub_blob()
        reader = Reader(blob, endian='<')
        program = module.ShaderProgram(reader, (2019, 3) if modern else (2018, 1))
        result[f'program_{modern}'] = [reader.Position, _call(lambda: program.Export('before\nGpuProgramIndex 0\nafter'))]
        sub = program.m_SubPrograms[0]
        sub.m_Keywords = ['EDIT']
        sub.m_ProgramCode = b'changed'
        result[f'edit_{modern}'] = _call(lambda: program.Export('GpuProgramIndex -1'))
        variants = [N(m_BlobIndex=0, m_GpuProgramType=1, m_ShaderHardwareTier=tier) for tier in (2, 1)]
        result[f'variants_{modern}'] = _call(lambda: module.ConvertSerializedSubPrograms(variants, [0], [program]))
    result['bad_type'] = _call(lambda: module.ShaderSubProgram(Reader(sub_blob(999), endian='<')))
    reader = Reader(sub_blob(1, b'plain', local=False), endian='<')
    sub = module.ShaderSubProgram(reader)
    result['no_local'] = [sub.m_LocalKeywords, reader.Position, _call(sub.Export)]
    sub.m_ProgramCode = b'\xff'
    result['invalid_utf8'] = _call(sub.Export)
    for kind in (999, 1 << 80, -(1 << 80)):
        for code in (b'\xff', b''):
            sub = module.ShaderSubProgram(Reader(sub_blob(), endian='<'))
            sub.m_ProgramType = kind
            sub.m_Keywords = ['EDIT', '//shader disassembly not supported on 0']
            sub.m_LocalKeywords = ['LOCAL EDIT']
            sub.m_ProgramCode = code
            result[f'current_type_{kind}_{bool(code)}'] = _call(sub.Export)
            program = module.ShaderProgram.__new__(module.ShaderProgram)
            program.m_SubPrograms = [sub]
            result[f'current_program_type_{kind}_{bool(code)}'] = _call(
                lambda: program.Export('before\nGpuProgramIndex 0\nafter'))
    result['truncated'] = _call(lambda: module.ShaderSubProgram(Reader(b'', endian='<')))
    return result


def observe_shader_platforms(package):
    module = _module(package, 'export.ShaderConverter')
    return {str(platform): [module.GetPlatformString(platform),
            [_call(lambda kind=kind: module.CheckGpuProgramUsable(platform, kind)) for kind in range(32)]]
            for platform in range(-1, 23)}


def observe_shader_mutable_stages(package):
    module = _module(package, 'export.ShaderConverter')
    Reader = _module(package, 'streams').EndianBinaryReader
    result = {}
    for kind in (999, 1 << 80, -(1 << 80)):
        for code in (b'\xff', b''):
            sub = module.ShaderSubProgram(Reader(sub_blob(), endian='<'))
            sub.m_ProgramType = kind
            sub.m_Keywords = ['EDIT']
            sub.m_LocalKeywords = ['LOCAL EDIT']
            sub.m_ProgramCode = code
            program = module.ShaderProgram.__new__(module.ShaderProgram)
            program.m_SubPrograms = [sub]
            variants = [N(m_BlobIndex=-1, m_GpuProgramType=1, m_ShaderHardwareTier=tier)
                        for tier in (2, 1)]
            normal = N(m_Type=0, m_State=shader_state(), **{
                name: N(m_SubPrograms=variants if name == 'progVertex' else [])
                for name in ('progVertex', 'progFragment', 'progGeometry', 'progHull', 'progDomain')})
            shader = N(m_LOD=0, m_Tags=N(tags=[]), m_Passes=[normal])
            form = N(m_Name='Synthetic', m_PropInfo=N(m_Props=[]), m_SubShaders=[shader],
                     m_FallbackName='', m_CustomEditorName='')
            programs = [program]
            result[f'{kind}/{bool(code)}'] = [
                _call(lambda: module.ConvertSerializedSubPrograms(variants, [0, 1], programs)),
                _call(lambda: module.ConvertSerializedPass(normal, [0, 1], programs)),
                _call(lambda: module.ConvertSerializedSubShader(shader, [0, 1], programs)),
                _call(lambda: module.ConvertSerializedShaderParsedForm(form, [0, 1], programs))]
    return result


def observe_shader_unused_programs(package):
    module = _module(package, 'export.ShaderConverter')
    Reader = _module(package, 'streams').EndianBinaryReader
    program = module.ShaderProgram.__new__(module.ShaderProgram)
    program.m_SubPrograms = [module.ShaderSubProgram(Reader(sub_blob(), endian='<')), None]
    variants = [N(m_BlobIndex=0, m_GpuProgramType=9, m_ShaderHardwareTier=0)]
    return {
        'use': _call(lambda: module.ConvertSerializedPass(
            N(m_Type=1, m_UseName='Synthetic/PASS'), [0], [None])),
        'grab': _call(lambda: module.ConvertSerializedPass(
            N(m_Type=2, m_TextureName='_Grab'), [0], [None])),
        'empty': _call(lambda: module.ConvertSerializedSubPrograms([], [0], [None])),
        'unusable': _call(lambda: module.ConvertSerializedSubPrograms(variants, [0], [None])),
        'missing': _call(lambda: module.ConvertSerializedSubPrograms(variants, [1], [])),
        'selected_only': _call(lambda: module.ConvertSerializedSubPrograms(
            [N(m_BlobIndex=0, m_GpuProgramType=1)], [0, 1], [program, None])),
    }
