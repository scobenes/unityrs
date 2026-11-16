"""Y4 class exporter observations from synthetic current Python attributes.

Only explicitly supplied data and API behavior are recorded, never generated
class definitions, default schemas, or a provider's TPK data.
"""
import importlib
import io
import json
import struct
from pathlib import Path
from types import SimpleNamespace

import attrs

from binding_cases import typed


def _call(function):
    try:
        return {"value": typed(function())}
    except Exception as error:
        return {"error": type(error).__name__, "message": str(error)}


def _new(cls, **fields):
    required = {f.name: None for f in attrs.fields(cls) if f.default is attrs.NOTHING}
    required.update(fields)
    return cls(**required)


def _samples(module, clip, corrected=False):
    if (corrected and module.__name__ == "UnityPy" and not clip.m_AudioData
            and clip.m_Resource and clip.m_Resource.m_Size == 0):
        return {}
    return clip.samples


def observe_audio(module, corrected=False):
    C = importlib.import_module(module.__name__ + ".classes")
    clip = C.AudioClip(m_Name="tone")
    result = {"extensions": []}
    for fmt in (None, -1, *range(26), 999):
        clip.m_CompressionFormat = fmt
        result["extensions"].append([fmt, _call(lambda: clip.extension)])
    for label, data in (("ogg", b"OggS\x00\x80synthetic"), ("riff", b"RIFFfake"),
                        ("m4a", b"1234ftypabc"), ("list", list(b"OggSabc")),
                        ("bad_byte", [999]), ("string", "RIFF"),
                        ("none", None), ("empty", [])):
        clip.m_AudioData = data
        result[label] = _call(lambda: _samples(module, clip, corrected))
    clip.m_Resource = _new(C.StreamedResource, m_Source="missing.resS", m_Offset=0, m_Size=0)
    result["external_without_reader"] = _call(lambda: _samples(module, clip, corrected))
    clip.m_AudioData = b"RIFFinline"
    clip.m_Name = "changed\udcff"
    result["inline_wins"] = _call(lambda: _samples(module, clip, corrected))
    return result


def observe_shader(module):
    C = importlib.import_module(module.__name__ + ".classes")
    shader = C.Shader(m_Name="synthetic")
    result = {}
    for label, script in (("bytes", b"Shader { synthetic }"), ("list", list(b"changed")),
                          ("empty", []), ("none", None), ("string", "text"),
                          ("invalid_utf8", b"\xff"), ("bad_byte", [256]),
                          ("surrogate", b"\xed\xa0\x80")):
        shader.m_Script = script
        result[label] = _call(shader.export)
    return result


def observe_shader_cases(module):
    """Existing synthetic E2 Tier A inputs, exercised through class methods."""
    fixture = Path(__file__).resolve().parents[2] / "crates/unityrs-export/tests/fixtures/shader.json"
    result = {}
    for case in json.loads(fixture.read_text())["cases"]:
        # E3 intentionally repairs these player-subprogram omissions. The
        # core's corrected-reference suite covers them separately from Tier A.
        if case["case"].startswith("player_only_"):
            continue
        env = module.Environment()
        file = env.load_file(bytes.fromhex(case["input"]), name="shader.assets")
        shader = file.objects[1].read()
        result[case["case"]] = _call(shader.export)
    return result


def mesh(module, vertices=True):
    C = importlib.import_module(module.__name__ + ".classes")
    packed = _new(C.PackedBitVector, m_BitSize=0, m_Data=b"", m_NumItems=0,
                  m_Range=0.0, m_Start=0.0)
    compressed = _new(C.CompressedMesh, **{name: packed for name in (
        "m_BoneIndices", "m_NormalSigns", "m_Normals", "m_TangentSigns", "m_Tangents",
        "m_Triangles", "m_UV", "m_Vertices", "m_Weights")})
    box = _new(C.AABB, m_Center=C.Vector3f(0., 0., 0.), m_Extent=C.Vector3f(0., 0., 0.))
    submesh = _new(C.SubMesh, firstByte=0, firstVertex=0, indexCount=3, localAABB=box,
                   vertexCount=3, topology=0)
    channel = _new(C.ChannelInfo, stream=0, offset=0, format=0, dimension=3 if vertices else 0)
    data = struct.pack("<9f", 0., 0., 0., 1., 0., 0., 0., 1., 0.) if vertices else b""
    vertex = _new(C.VertexData, m_DataSize=data, m_VertexCount=3, m_Channels=[channel])
    value = C.Mesh(m_BindPose=[], m_CompressedMesh=compressed,
                   m_IndexBuffer=list(struct.pack("<3H", 0, 1, 2)), m_LocalAABB=box,
                   m_MeshCompression=0, m_MeshUsageFlags=0, m_Name="triangle",
                   m_SubMeshes=[submesh], m_VertexData=vertex, m_IndexFormat=0)
    Version = importlib.import_module(module.__name__ + ".helpers.UnityVersion").UnityVersion
    value.object_reader = SimpleNamespace(version=Version.from_str("2020.3.0f1"))
    return value


def observe_mesh(module):
    value = mesh(module)
    result = {"obj": _call(value.export), "explicit_obj": _call(lambda: value.export("obj")),
              "unsupported": _call(lambda: value.export("fbx"))}
    value.m_Name = "changed\udcff"
    result["changed_name"] = _call(value.export)
    value.m_IndexBuffer = list(struct.pack("<3H", 2, 1, 0))
    result["changed_indices"] = _call(value.export)
    value.m_IndexBuffer = bytearray(struct.pack("<3H", 0, 2, 1))
    result["bytearray_indices"] = _call(value.export)
    value.m_IndexBuffer = memoryview(struct.pack("<3H", 1, 0, 2))
    result["memoryview_indices"] = _call(value.export)
    value.m_SubMeshes[0].topology = 3
    result["lines"] = _call(value.export)
    result["empty"] = _call(mesh(module, False).export)
    value.object_reader = None
    result["missing_reader"] = _call(value.export)
    return result


def observe_surface(module):
    C = importlib.import_module(module.__name__ + ".classes")
    return {name: [hasattr(getattr(C, name), "export"), hasattr(getattr(C, name), "save")]
            for name in ("Shader", "Mesh", "Renderer", "MeshRenderer", "SkinnedMeshRenderer",
                         "AudioClip", "Font", "TextAsset", "MonoBehaviour", "VideoClip")}


def observe_renderer_errors(module):
    C = importlib.import_module(module.__name__ + ".classes")
    result = {}
    for name in ("MeshRenderer", "SkinnedMeshRenderer"):
        renderer = _new(getattr(C, name))
        result[name] = _call(lambda: renderer.export("unused"))
    return result


def observe_audio_resources(module, corrected=False):
    from binding_cases import S
    C = importlib.import_module(module.__name__ + ".classes")
    root = S.Node("TextAsset", "Base", [S.Node("string", "m_Name"), S.Node("string", "m_Script")])
    source = S.serialized_file([S.Type(49, root)], [S.Obj(1, 0, S.s32("owner") + S.s32("body"))])
    bundle = S.fs_bundle([(b"CAB-audio", source, 4), (b"audio.resS", b"xxOggSresource", 0)])
    env = module.Environment()
    env.load_file(bundle, name="audio.bundle")
    clip = C.AudioClip(m_Name="external", m_Resource=_new(
        C.StreamedResource, m_Source="archive:/CAB/audio.resS", m_Offset=2, m_Size=12))
    clip.object_reader = env.objects[0]
    result = {"resource": _call(lambda: clip.samples)}
    clip.m_Resource.m_Size = -1
    result["resource_read_to_end"] = _call(lambda: clip.samples)
    clip.m_Resource.m_Offset = -12
    result["resource_negative_offset"] = _call(lambda: clip.samples)
    clip.m_Resource.m_Offset, clip.m_Resource.m_Size = 2, 12
    clip.m_Resource.m_Source = "missing.resS"
    result["missing"] = _call(lambda: clip.samples)
    clip.m_Resource.m_Size = 0
    result["missing_zero"] = _call(lambda: _samples(module, clip, corrected))
    clip.m_AudioData = b"RIFFedited"
    result["edited_inline"] = _call(lambda: clip.samples)
    return result


def observe_audio_pcm(module):
    """Known valid PCM16, IMA, and FADPCM banks; FMOD only on oracle side."""
    C = importlib.import_module(module.__name__ + ".classes")
    fixture = Path(__file__).resolve().parents[2] / "crates/unityrs-audio/tests/fixtures/audio.json"
    # FMOD 2.00.22 rejects FSB5 version 0 with VERSION; the Rust decoder's
    # supported version-0 path is covered by its own documented Tier C tests.
    selected = {"pcm16_c1_r8", "pcm16_c2_r9", "named_multi", "overrides",
                "ima_c1_frames3", "ima_c2_frames3", "ima_c6_frames3",
                "fadpcm_c1_frames3", "fadpcm_c2_frames3", "fadpcm_c6_frames3"}
    result = {}
    for case in json.loads(fixture.read_text())["cases"]:
        if case["case"] in selected:
            clip = C.AudioClip(m_Name="edited", m_AudioData=bytes.fromhex(case["data"]),
                               m_Channels=2, m_Frequency=44100)
            result[case["case"]] = _call(lambda: clip.samples)
    return result


class _MemoryFS:
    sep = "/"

    def __init__(self):
        self.files = {}
        self.directories = []

    def makedirs(self, path, exist_ok=False):
        self.directories.append([path, exist_ok])

    def open(self, path, mode, **kwargs):
        fs = self
        class Stream(io.StringIO):
            def close(self):
                fs.files[path] = self.getvalue()
                super().close()
        return Stream()


def observe_renderer(module):
    C = importlib.import_module(module.__name__ + ".classes")
    geometry = mesh(module)
    fs = _MemoryFS()
    renderer = _new(C.SkinnedMeshRenderer,
        m_Mesh=SimpleNamespace(deref_parse_as_object=lambda: geometry),
        m_Materials=[None], m_StaticBatchInfo=_new(C.StaticBatchInfo, firstSubMesh=0, subMeshCount=1))
    renderer.object_reader = SimpleNamespace(assets_file=SimpleNamespace(environment=SimpleNamespace(fs=fs)))
    result = {"export": _call(lambda: renderer.export("synthetic"))}
    result["files"] = dict(fs.files)
    result["directories"] = list(fs.directories)
    geometry.m_Name = "edited"
    geometry.m_IndexBuffer = list(struct.pack("<3H", 2, 1, 0))
    result["edited"] = _call(lambda: renderer.export("changed"))
    result["edited_files"] = {key: value for key, value in fs.files.items() if key.startswith("changed/")}
    renderer.m_Mesh = None
    result["no_mesh"] = _call(lambda: renderer.export("empty"))
    result["no_mesh_mkdir"] = fs.directories[-1]
    return result
