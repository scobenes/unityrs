"""Y6 export adapters on synthetic current fields; no generated schemas saved."""
import ctypes
import importlib
import struct
import sys
from pathlib import Path
from types import SimpleNamespace

from binding_y4_exports import _call, _new, mesh
from binding_y4_review_exports import material, renderer
from binding_y4_round6 import _texture_source


def _module(package, name):
    return importlib.import_module(package.__name__ + ".export." + name)


def textured_material(package):
    C = importlib.import_module(package.__name__ + ".classes")
    env, value = material(package)
    named = env.load_file(_texture_source("diffuse"), name="named.assets")
    empty = env.load_file(_texture_source(""), name="empty.assets")
    edited = env.load_file(_texture_source("changed"), name="edited.assets")
    pointer = C.PPtr(m_FileID=0, m_PathID=1, assetsfile=named)
    unnamed = C.PPtr(m_FileID=0, m_PathID=1, assetsfile=empty)
    missing = C.PPtr(m_FileID=0, m_PathID=0, assetsfile=named)

    def entry(ptr):
        return _new(C.UnityTexEnv, m_Texture=ptr, m_Offset=(0, 0), m_Scale=(1, 1))

    value.m_SavedProperties.m_TexEnvs = [
        ("_MainTex", entry(pointer)), ("_BumpMap", entry(unnamed)),
        (_new(C.FastPropertyName, name="_SpecularTex"), entry(pointer)),
        ("_Normal", entry(pointer)), ("_Other", entry(missing)), ("_Null", None),
    ]
    return value, pointer, unnamed, edited


def observe_material_textures(package, corrected=False):
    exporter = _module(package, "MeshRendererExporter")
    if corrected and package.__name__ == "UnityPy":
        sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "fixtures"))
        from obj_export import corrected as reference
        exporter = reference(exporter)
    value, pointer, unnamed, edited = textured_material(package)
    result = {"initial": _call(lambda: exporter.export_material(value))}
    C = importlib.import_module(package.__name__ + '.classes')
    bad = C.PPtr(m_FileID=0, m_PathID=9999, assetsfile=pointer.assetsfile)
    env_type = type(value.m_SavedProperties.m_TexEnvs[0][1])
    value.m_SavedProperties.m_TexEnvs.insert(0, ('_MainTex', _new(
        env_type, m_Texture=bad, m_Offset=(0, 0), m_Scale=(1, 1))))
    result['replaced_broken_pointer'] = _call(lambda: exporter.export_material(value))
    value.m_SavedProperties.m_TexEnvs.pop(0)
    pointer.assetsfile = edited
    result["changed_owner"] = _call(lambda: exporter.export_material(value))
    value.m_SavedProperties.m_TexEnvs += [("_MainTex", _new(
        type(value.m_SavedProperties.m_TexEnvs[0][1]),
        m_Texture=unnamed, m_Offset=(0, 0), m_Scale=(1, 1)))]
    result["duplicate"] = _call(lambda: exporter.export_material(value))
    return result


def observe_renderer_texture_collisions(package, corrected=False):
    """PNG writes retain duplicate properties and source-order collisions."""
    import io
    from PIL import Image

    class MemoryFS:
        sep = "/"

        def __init__(self):
            self.files = {}
            self.writes = []

        def makedirs(self, path, exist_ok=False):
            pass

        def open(self, path, mode, **kwargs):
            fs = self
            base = io.BytesIO if "b" in mode else io.StringIO

            class Stream(base):
                def close(self):
                    fs.files[path] = self.getvalue()
                    fs.writes.append(path)
                    super().close()

            stream = Stream()
            stream.name = path
            return stream

    C = importlib.import_module(package.__name__ + ".classes")
    env, value = material(package)

    def entry(name, pixel):
        texture = SimpleNamespace(m_Name=name, image=Image.new("RGBA", (1, 1), pixel))
        return _new(C.UnityTexEnv, m_Texture=SimpleNamespace(
            m_FileID=0, m_PathID=1, deref_parse_as_object=lambda: texture),
            m_Offset=(0, 0), m_Scale=(1, 1))

    value.m_SavedProperties.m_TexEnvs = [
        ("_MainTex", entry("old", (255, 0, 0, 255))),
        ("_MainTex", entry("shared", (0, 255, 0, 255))),
        ("_BumpMap", entry("shared", (0, 0, 255, 255))),
        (_new(C.FastPropertyName, name="_MainTex"), entry("shared", (255, 255, 0, 255))),
    ]
    fs = MemoryFS()
    target = renderer(package, mesh(package), [value], fs)
    exporter = _module(package, "MeshRendererExporter")
    if corrected and package.__name__ == "UnityPy":
        sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "fixtures"))
        from obj_export import corrected as reference
        exporter = reference(exporter)
    result = _call(lambda: exporter.export_mesh_renderer(target, "textures"))
    result["writes"] = fs.writes
    result["files"] = {
        name: [list(pixel) for pixel in Image.open(io.BytesIO(data)).getdata()]
        if name.endswith(".png") else data
        for name, data in sorted(fs.files.items())
    }
    return result


def float_bank(payload, channels=1):
    """FSB5 version 1, float codec, 44100 Hz, one synthetic subsound."""
    count = len(payload) // (4 * channels)
    header = struct.pack("<6I", 1, 1, 8, 0, len(payload), 5)
    sample = struct.pack("<Q", 8 << 1 | (channels - 1) << 5 | count << 34)
    return b"FSB5" + header + bytes(32) + sample + payload


def _float_reference(payload, channels, convert):
    # FMOD refuses these synthetic float banks. Keep its recorded refusal;
    # the Tier C reference runs its real WAV writer on a stand-in subsound.
    import fmod_toolkit.fmod
    import pyfmodex
    buffer = ctypes.create_string_buffer(payload, len(payload))
    sound = SimpleNamespace(
        format=SimpleNamespace(format=pyfmodex.enums.SOUND_FORMAT.PCMFLOAT,
                               channels=channels, bits=32),
        default_frequency=44100, get_length=lambda unit: len(payload),
        lock=lambda offset, size: [(ctypes.addressof(buffer), ctypes.c_uint(size))],
        unlock=lambda *parts: None,
    )
    return fmod_toolkit.fmod.subsound_to_wav(sound, convert_pcm_float=convert)


def observe_audio_float(package, corrected=False):
    C = importlib.import_module(package.__name__ + ".classes")
    converter = _module(package, "AudioClipConverter")
    result = {}
    for name, channels, payload in (
        ("mono", 1, struct.pack("<7f", .5, -.5, 1, -1, 2, .99999, -.0000305)),
        ("stereo", 2, struct.pack("<4f", .25, -.75, 0, 1.5)),
        ("one", 1, struct.pack("<f", .5)),
        ("bits", 1, bytes.fromhex("000000800000807f4523c17f")),
    ):
        clip = C.AudioClip(m_Name=name, m_AudioData=float_bank(payload, channels),
                           m_Channels=channels, m_Frequency=44100)
        if corrected and package.__name__ == "UnityPy":
            result[name] = _call(lambda: {name + ".wav": _float_reference(payload, channels, False)})
        else:
            result[name] = _call(lambda: converter.extract_audioclip_samples(clip, convert_pcm_float=False))
    return result


def observe_audio_options(package):
    C = importlib.import_module(package.__name__ + ".classes")
    converter = _module(package, "AudioClipConverter")
    pcm = struct.pack("<4h", 0, -1, 2, 32767)
    head = struct.pack("<6I", 1, 1, 8, 0, len(pcm), 2)
    bank = b"FSB5" + head + bytes(32) + struct.pack("<Q", 8 << 1 | 4 << 34) + pcm
    result = {}
    for label, payload in (("ogg", b"OggSsynthetic"), ("riff", b"RIFFsynthetic"),
                           ("m4a", b"1234ftypdata"), ("pcm16", bank)):
        clip = C.AudioClip(m_Name="current", m_AudioData=payload, m_Channels=1, m_Frequency=44100)
        result[label] = [_call(lambda: converter.extract_audioclip_samples(clip, convert_pcm_float=flag))
                         for flag in (True, False)]
    return result
