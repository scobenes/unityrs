# Ported from UnityPy 1.25.2 (MIT): UnityPy/classes/legacy_patch/AudioClip.py, UnityPy/classes/legacy_patch/Mesh.py, UnityPy/classes/legacy_patch/Shader.py, UnityPy/classes/legacy_patch/Renderer.py, UnityPy/export/AudioClipConverter.py, UnityPy/export/MeshRendererExporter.py. See LICENSE-UNITYPY and THIRD_PARTY_LICENSES.
"""Class export adapters over the bounded native exporters."""
import os

from .. import _native
from ..enums import AUDIO_TYPE_EXTEMSION
from .._output import OutputDirectory, output_name
from ._images import _guard


def _bytes(value, guard):
    # Keep Python's bytes() coercion, charging sized results before allocation.
    if isinstance(value, int):
        size = max(0, value)
    elif isinstance(value, memoryview):
        size = value.nbytes
    else:
        try:
            size = len(value)
        except TypeError:
            # Unsized iterables grow one bounded byte at a time. A user-defined
            # __bytes__ callback owns its own allocation, just as a custom getter.
            if hasattr(value, "__bytes__"):
                result = bytes(value)
                guard.step(0, len(result) + 256, len(result))
                return result
            if hasattr(value, "__index__"):
                import operator
                return _bytes(operator.index(value), guard)
            try:
                iterator = iter(value)
            except TypeError:
                return bytes(value)
            guard.step(0, 256, 1)
            staged = bytearray()
            for item in iterator:
                guard.step(0, 2, 1)
                staged.append(item)
            guard.step(0, len(staged) + 256, len(staged))
            return bytes(staged)
        else:
            # A multidimensional buffer's len is only its first dimension.
            # A view exposes its full byte count without copying its storage.
            guard.step(0, 256, 1)
            try:
                view = memoryview(value)
            except TypeError:
                pass
            else:
                size = view.nbytes
    guard.step(0, min(size + 256, (1 << 64) - 1), min(size, (1 << 64) - 1))
    result = bytes(value)
    if len(result) > size:
        guard.step(0, len(result) - size, len(result) - size)
    return result


def _reader(value):
    return getattr(value.object_reader, "_native", None)


def _environment(value):
    reader = value.object_reader
    if reader is None:
        return None
    file = getattr(reader, "assets_file", None)
    return getattr(getattr(file, "environment", None), "_native", None)


def _audio_extension(self):
    # classes/legacy_patch/AudioClip.py:5-6.
    return AUDIO_TYPE_EXTEMSION.get(self.m_CompressionFormat, ".audioclip")


def _audio_samples(self):
    # export/AudioClipConverter.py:13-49. Inline truth precedes resources;
    # bytes() preserves Python's byte-range and missing-encoding exceptions.
    guard = _guard(self)
    if self.m_AudioData:
        data = _bytes(self.m_AudioData, guard)
    elif self.m_Resource:
        # PARITY 19.2: an empty streamed clip emits no samples, without
        # resolving a meaningless source or requiring an ObjectReader.
        if self.m_Resource.m_Size == 0:
            return {}
        assert self.object_reader is not None, "AudioClip uses an external resource but object_reader is not set"
        file = self.object_reader.assets_file
        resource = self.m_Resource
        guard.step(0, len(resource.m_Source) * 4 + 256, len(resource.m_Source))
        data = file.environment._native.resource(
            file._native, resource.m_Source.encode("utf-8", "surrogateescape"),
            resource.m_Offset, resource.m_Size,
        )
    else:
        raise ValueError("AudioClip with neither m_AudioData nor m_Resource")
    guard.step(0, len(data) + 256, 0)
    name = f"{self.m_Name}"
    guard.step(0, len(name) * 4 + 256, len(name))
    from .. import config
    source = getattr(config, "AUDIO_VORBIS_HEADERS", os.environ.get("UNITYRS_VORBIS_HEADERS"))
    headers = _bytes(source, guard) if isinstance(source, (bytes, bytearray, memoryview)) else None
    path = None if source is None or headers is not None else os.fspath(source)
    return _native.export_audio(data, name.encode("utf-8", "surrogateescape"),
                                _reader(self), headers, path)


def _shader_export(self):
    # export/ShaderConverter.py:41-55. Coerce only the selected branch so
    # unused malformed optional byte arrays never change the first error.
    guard = _guard(self)
    guard.step(0, 512, 1)
    fields = {}
    version = (0, 0)
    if self.m_SubProgramBlob:
        fields["m_SubProgramBlob"] = _bytes(self.m_SubProgramBlob, guard)
        fields["decompressedSize"] = self.decompressedSize
        fields["m_Script"] = self.m_Script
        version = tuple(self.object_reader.version[:2])
    elif self.compressedBlob:
        fields = {name: getattr(self, name) for name in (
            "platforms", "compressedLengths", "decompressedLengths", "offsets", "m_ParsedForm")
            if getattr(self, name) is not None}
        fields["compressedBlob"] = _bytes(self.compressedBlob, guard)
        # No ShaderProgram is constructed when the platform/length guard
        # immediately stops the loop; an unbound class works in that case.
        if self.platforms and self.compressedLengths and self.decompressedLengths:
            version = tuple(self.object_reader.version[:2])
    else:
        script = _bytes(self.m_Script, guard)
        # Strict Python decoding preserves UnicodeDecodeError's exact span,
        # reason and original object, which a Rust UTF-8 refusal cannot carry.
        guard.step(0, len(script) * 4 + 256, len(script))
        script.decode("utf8")
        fields["m_Script"] = script
    return _native.export_shader(fields, version, _reader(self))


def _mesh_obj(value, material_names=None):
    if value.object_reader is None:
        raise ValueError("No version provided and no object reader found")
    guard = _guard(value)
    names = None
    if material_names is not None:
        guard.step(0, 256, 1)
        names = []
        for name in material_names:
            name = name or ""
            guard.step(0, 128 + (len(name) * 4 if isinstance(name, str) else 0), 1)
            names.append(name.encode("utf-8", "surrogateescape"))
    return _native.export_mesh(value, int(value.object_reader.version), _reader(value),
                               _environment(value), names)


def _mesh_export(self, format="obj"):
    # export/MeshExporter.py:11-14.
    if format == "obj":
        return _mesh_obj(self)
    raise NotImplementedError(f"Export format {format} not implemented")


def _get_mesh(renderer, guard=None):
    # export/MeshRendererExporter.py:25-45.
    from . import SkinnedMeshRenderer
    guard = _guard(renderer) if guard is None else guard
    if isinstance(renderer, SkinnedMeshRenderer):
        if renderer.m_Mesh:
            return renderer.m_Mesh.deref_parse_as_object()
    else:
        game_object = renderer.m_GameObject.deref_parse_as_object()
        for component in game_object.m_Component:
            guard.step(0, 0, 1)
            pointer = component[1] if isinstance(component, tuple) else component.component
            if not pointer:
                continue
            obj = pointer.deref()
            if obj and obj.type.name == "MeshFilter":
                mesh_filter = pointer.deref_parse_as_object()
                if mesh_filter.m_Mesh:
                    return mesh_filter.m_Mesh.deref_parse_as_object()
    return None


class _LocalFS:
    sep = os.sep
    makedirs = staticmethod(os.makedirs)
    open = staticmethod(open)


def _renderer_export(self, export_dir):
    # export/MeshRendererExporter.py:48-103. Resolution uses current class
    # attributes; geometry/material values enter the existing native writers.
    env = self.object_reader.assets_file.environment
    guard = _guard(self)
    fs = getattr(env, "fs", _LocalFS)
    fs.makedirs(export_dir, exist_ok=True)
    mesh = _get_mesh(self, guard)
    if not mesh:
        return
    mesh_name = output_name(mesh.m_Name)
    first = 0
    batch = getattr(self, "m_StaticBatchInfo", None)
    subset = getattr(self, "m_SubsetIndices", None)
    if batch is not None and batch.subMeshCount > 0:
        first = batch.firstSubMesh
    elif subset is not None:
        if hasattr(subset, "__len__"):
            guard.step(0, 0, len(subset))
        first = min(subset)
    guard.step(0, 256, 1)
    materials, names, textures = [], [], []
    for index in range(len(mesh.m_SubMeshes)):
        guard.step(0, 128, 1)
        slot = index - first
        # Tier C: preserve submesh slots before a nonzero static batch offset.
        if slot < 0 or slot >= len(self.m_Materials) or not self.m_Materials[slot]:
            names.append(None)
            continue
        material = self.m_Materials[slot].deref_parse_as_object()
        exported = _native.export_material(material, _reader(material), _environment(material))
        guard.step(0, len(exported) * 4 + 256, 0)
        materials.append(exported)
        names.append(material.m_Name)
        for key, texture_env in material.m_SavedProperties.m_TexEnvs:
            guard.step(0, 0, 1)
            if not texture_env.m_Texture:
                continue
            if not isinstance(key, str):
                key = key.name
            texture = texture_env.m_Texture.deref_parse_as_object()
            chosen = texture.m_Name if texture.m_Name else key
            if isinstance(chosen, str):
                guard.step(0, len(chosen) * 4 + 256, len(chosen))
            name = output_name(chosen) + ".png"
            guard.step(0, 128, 1)
            textures.append((name, texture))
    # Preflight every asset-derived name before opening any output. Local
    # leaves are replaced against a held directory, so symlinks and hardlinks
    # cannot redirect writes outside the destination.
    with OutputDirectory(export_dir, fs) as directory:
        for name, texture in textures:
            with directory.open(name, "wb") as stream:
                texture.image.save(stream, format="PNG")
        with directory.open(mesh_name + ".obj", "wt", encoding="utf8", newline="") as stream:
            stream.write(_mesh_obj(mesh, names))
        with directory.open(mesh_name + ".mtl", "wt", encoding="utf8", newline="") as stream:
            guard.step(0, (sum(map(len, materials)) + len(materials)) * 4 + 256, len(materials))
            stream.write("\n".join(materials))


def install(namespace):
    """Apply only the class export APIs actually shipped by UnityPy 1.25.2."""
    namespace["AudioClip"].extension = property(_audio_extension)
    namespace["AudioClip"].samples = property(_audio_samples)
    namespace["Shader"].export = _shader_export
    namespace["Mesh"].export = _mesh_export
    namespace["Renderer"].export = _renderer_export
