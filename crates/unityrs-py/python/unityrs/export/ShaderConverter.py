# Ported from UnityPy 1.25.2 (MIT): export/ShaderConverter.py:30-689. See LICENSE-UNITYPY and THIRD_PARTY_LICENSES.
"""Shader converter stages backed by the bounded native exporter."""
import re
import struct
from itertools import groupby
from .. import _native
from .._buffers import snapshot_bytes
from ..enums import ShaderGpuProgramType

HEADER = """//////////////////////////////////////////
//
// NOTE: This is *not* a valid shader file
//
///////////////////////////////////////////
"""


def export_shader(m_Shader):
    from ..classes._exports import _shader_export
    return _shader_export(m_Shader)


def ConvertSerializedShader(m_Shader):
    from ..classes._exports import _reader
    from ..classes._images import _guard
    guard = _guard(m_Shader)
    fields = {name: getattr(m_Shader, name) for name in (
        "platforms", "compressedLengths", "decompressedLengths", "offsets", "m_ParsedForm")}
    fields["compressedBlob"] = snapshot_bytes(m_Shader.compressedBlob, guard)
    if not fields["compressedBlob"]:
        from ..helpers import CompressionHelper
        from ..streams import EndianBinaryReader
        programs = []
        def entry(values, i):
            value = values[i]
            return value[0] if isinstance(value, list) else value
        for i in range(len(fields["platforms"])):
            guard.step(0, 256, 1)
            if i >= len(fields["compressedLengths"]) or i >= len(fields["decompressedLengths"]):
                break
            size = entry(fields["compressedLengths"], i)
            decoded_size = entry(fields["decompressedLengths"], i)
            offset = entry(fields["offsets"], i)
            data = fields["compressedBlob"][offset:offset + size]
            # Even an empty slice must reach the decoder. Its builtin rejects
            # it before allocating output, retaining the oracle's error order.
            decoded = CompressionHelper.decompress_lz4(data, decoded_size)
            programs.append(ShaderProgram(EndianBinaryReader(decoded, '<'), m_Shader.object_reader.version))
        return ConvertSerializedShaderParsedForm(m_Shader.m_ParsedForm, m_Shader.platforms, programs)
    version = tuple(m_Shader.object_reader.version[:2]) if (m_Shader.platforms and
              m_Shader.compressedLengths and m_Shader.decompressedLengths) else (0, 0)
    return _native.export_shader(fields, version, _reader(m_Shader))[len(HEADER):]


def _stage(stage, value, platforms=(), programs=(), **extra):
    if stage in ('shader', 'subshader', 'pass', 'subprograms'):
        out = _StageOutput()
        _convert(stage, value, platforms, programs, out)
        return out.finish()
    guard = _native.TreeTraversal()
    blobs = []
    for program in programs:
        guard.step(0, 256, 1)
        blobs.append(program._blob(guard))
    return _native.shader_stage(dict(value=value, platforms=platforms,
                                    program_blobs=blobs, **extra), stage)


class _StageOutput:
    """Keep stage assembly charged while selected Python exports stay mutable."""
    def __init__(self):
        self.guard = _native.TreeTraversal()
        self.parts = []
        self.size = 0

    def add(self, *parts):
        for part in parts:
            part = str(part)
            self.guard.step(0, len(part) * 8 + 256, len(part) + 1)
            self.parts.append(part)
            self.size += len(part)

    def items(self, values):
        for value in values:
            self.guard.step(0, 256, 1)
            yield value

    def finish(self):
        self.guard.step(0, self.size * 4 + 256, self.size)
        return ''.join(self.parts)


def _convert(stage, value, platforms, programs, out):
    # Do not serialize/reparse shaderPrograms: UnityPy calls Export only on the
    # selected current subprogram, without validating its type as a new enum.
    if stage == 'shader':
        out.add('Shader "', value.m_Name, '" {\n', ConvertSerializedProperties(value.m_PropInfo))
        for sub in out.items(value.m_SubShaders):
            _convert('subshader', sub, platforms, programs, out)
        for name, label in ((value.m_FallbackName, 'Fall back'),
                            (value.m_CustomEditorName, 'CustomEditor')):
            if name:
                out.add(label, ' "', name, '"\n')
        out.add('}')
    elif stage == 'subshader':
        out.add('SubShader {\n')
        if value.m_LOD != 0:
            out.add(' LOD ', value.m_LOD, '\n')
        out.add(ConvertSerializedTagMap(value.m_Tags, 1))
        for item in out.items(value.m_Passes):
            _convert('pass', item, platforms, programs, out)
        out.add('}\n')
    elif stage == 'pass':
        kind = value.m_Type
        out.add({0: ' Pass ', 1: ' UsePass ', 2: ' GrabPass '}.get(kind, ''))
        if kind == 1:
            out.add('"', value.m_UseName, '"\n')
            return
        out.add('{\n')
        if kind == 2:
            if value.m_TextureName:
                out.add(' "', value.m_TextureName, '"\n')
        else:
            out.add(ConvertSerializedShaderState(value.m_State))
            for name, label in (('progVertex', 'vp'), ('progFragment', 'fp'),
                                ('progGeometry', 'gp'), ('progHull', 'hp'), ('progDomain', 'dp')):
                subs = getattr(value, name).m_SubPrograms
                if len(subs) > 0:
                    out.add('Program "', label, '" {\n')
                    _convert('subprograms', subs, platforms, programs, out)
                    out.add('}\n')
        out.add('}\n')
    else:
        ordered = list(out.items(value))
        count = len(ordered)
        out.guard.step(0, count * 256 + 256, count * max(1, count.bit_length()))
        ordered.sort(key=lambda sub: (sub.m_BlobIndex, sub.m_GpuProgramType))
        for (_, kind), group in groupby(ordered, lambda sub: (sub.m_BlobIndex, sub.m_GpuProgramType)):
            group = list(out.items(group))
            for i in range(min(len(platforms), len(programs))):
                out.guard.step(0, 256, 1)
                platform = platforms[i]
                if CheckGpuProgramUsable(platform, kind):
                    for sub in out.items(group):
                        out.add('SubProgram "', GetPlatformString(platform), ' ')
                        if len(group) > 1:
                            out.add('hw_tier', format(sub.m_ShaderHardwareTier, '02'), ' ')
                        out.add('" {\n', programs[i].m_SubPrograms[sub.m_BlobIndex].Export(), '\n}\n')
                    break


def ConvertSerializedShaderParsedForm(m_ParsedForm, platforms, shaderPrograms):
    return _stage("shader", m_ParsedForm, platforms, shaderPrograms)


def ConvertSerializedSubShader(m_SubShader, platforms, shaderPrograms):
    return _stage("subshader", m_SubShader, platforms, shaderPrograms)


def ConvertSerializedPass(m_Passe, platforms, shaderPrograms):
    return _stage("pass", m_Passe, platforms, shaderPrograms)


def ConvertSerializedSubPrograms(m_SubPrograms, platforms, shaderPrograms):
    return _stage("subprograms", m_SubPrograms, platforms, shaderPrograms)


def ConvertSerializedShaderState(m_State):
    return _stage("state", m_State)


def ConvertSerializedShaderRTBlendState(rbBlend):
    return ""


def ConvertSerializedTagMap(m_Tags, intent):
    return _stage("tags", m_Tags, indent=intent)


def ConvertSerializedProperties(m_PropInfo):
    return _stage("properties", m_PropInfo)


def ConvertSerializedProperty(m_Prop):
    return _stage("property", m_Prop)


def CheckGpuProgramUsable(platform, programType):
    return _stage("usable", platform, kind=programType) == "1"


def GetPlatformString(platform):
    return _stage("platform", platform)


def _put(out, data, guard):
    guard.step(0, len(data) * 2 + 256, len(data))
    out.extend(data)


def _int(out, value, guard):
    _put(out, struct.pack("<i", value), guard)


def _words(out, words, guard):
    _int(out, len(words), guard)
    for word in words:
        guard.step(0, len(word) * 4 + 256, len(word))
        data = word.encode("utf-8", "surrogateescape")
        _int(out, len(data), guard)
        _put(out, data, guard)
        _put(out, bytes((-len(out)) % 4), guard)


class ShaderProgram:
    def __init__(self, reader, version):
        native = getattr(reader, '_native', None)
        guard = native.tree_traversal() if native is not None else _native.TreeTraversal()
        count = reader.read_int()
        guard.step(0, max(0, count) * 8 + 256, max(0, count))
        self.m_SubPrograms = [None] * count
        stride = 12 if version >= (2019, 3) else 8
        for i in range(count):
            reader.Position = 4 + i * stride
            offset = reader.read_int()
            reader.Position = offset
            self.m_SubPrograms[i] = ShaderSubProgram(reader, _guard=guard)

    def _blob(self, guard):
        count = len(self.m_SubPrograms)
        guard.step(0, 4 + count * 8 + 256, count)
        out = bytearray(4 + count * 8)
        struct.pack_into("<i", out, 0, count)
        for i, program in enumerate(self.m_SubPrograms):
            struct.pack_into("<i", out, 4 + i * 8, len(out))
            _put(out, program._blob(guard), guard)
        guard.step(0, len(out) + 256, len(out))
        return bytes(out)

    def Export(self, shader):
        guard = _native.TreeTraversal()
        guard.step(0, len(shader) * 4 + 256, len(shader))
        def replace(match):
            text = self.m_SubPrograms[int(match.group(1))].Export()
            guard.step(0, len(text) * 8 + 256, len(text))
            return text
        return re.sub(r"GpuProgramIndex (.+)", replace, shader)


class ShaderSubProgram:
    def __init__(self, reader, *, _guard=None):
        native = getattr(reader, '_native', None)
        guard = _guard if _guard is not None else (
            native.tree_traversal() if native is not None else _native.TreeTraversal())
        self.m_Version = reader.read_int()
        self.m_ProgramType = ShaderGpuProgramType(reader.read_int())
        reader.Position += 12
        if self.m_Version >= 201608170:
            reader.Position += 4
        def words():
            count = reader.read_int()
            result = []
            for _ in range(count):
                guard.step(0, 256, 1)
                size = reader.read_int()
                if 0 < size <= reader.Length - reader.Position:
                    guard.step(0, size * 5 + 256, size)
                    word = bytes(reader.read_bytes(size)).decode("utf8", "surrogateescape")
                    reader.align_stream()
                else:
                    word = ""
                result.append(word)
            return result
        self.m_Keywords = words()
        self.m_LocalKeywords = words() if 201806140 <= self.m_Version < 202012090 else None
        size = reader.read_int()
        bound = reader.Length if size < 0 else min(size, reader.Length)
        guard.step(0, max(0, bound) + 256, 1)
        self.m_ProgramCode = reader.read_bytes(size)
        reader.align_stream()

    def _blob(self, guard, kind_override=None):
        out = bytearray()
        # Normalize to a layout which contains both mutable keyword arrays.
        _int(out, 201806140, guard)
        _int(out, self.m_ProgramType if kind_override is None else kind_override, guard)
        _put(out, bytes(16), guard)
        _words(out, self.m_Keywords, guard)
        _words(out, [] if self.m_LocalKeywords is None else self.m_LocalKeywords, guard)
        code = snapshot_bytes(self.m_ProgramCode, guard)
        _int(out, len(code), guard)
        _put(out, code, guard)
        _put(out, bytes((-len(out)) % 4), guard)
        guard.step(0, len(out) + 256, len(out))
        return bytes(out)

    def Export(self):
        if self.m_ProgramType not in range(32):
            # Reading validates the enum; exporting mutable fields falls through
            # to a comment for any other current value, including unbounded ints.
            guard = _native.TreeTraversal()
            blob = bytearray(struct.pack("<3i", 1, 12, 0))
            _put(blob, self._blob(guard, kind_override=0), guard)
            guard.step(0, len(blob) + 256, len(blob))
            text = _native.shader_stage(dict(value=0, platforms=(),
                                             program_blobs=[bytes(blob)]), "program")
            suffix = '//shader disassembly not supported on 0"'
            if text.endswith(suffix):
                replacement = '//shader disassembly not supported on {0}"'.format(self.m_ProgramType)
                guard.step(0, (len(text) + len(replacement)) * 4 + 256, len(replacement))
                return text[:-len(suffix)] + replacement
            return text
        program = ShaderProgram.__new__(ShaderProgram)
        program.m_SubPrograms = [self]
        return _stage("program", 0, programs=[program])
