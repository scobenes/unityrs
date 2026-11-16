"""Review regressions using explicit current material fields and safe outputs."""
import importlib
import json
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

from binding_cases import cases
from binding_y4_exports import _new, _call, mesh, _MemoryFS


def owner(module):
    env = module.Environment()
    env.load_file(next(data for name, data in cases() if name == "text"), name="owner.assets")
    return env, env.objects[0]


def material(module, floats=None, colors=None):
    C = importlib.import_module(module.__name__ + ".classes")
    env, reader = owner(module)
    value = _new(C.Material, m_Name="paint", m_Shader=C.PPtr(m_FileID=0, m_PathID=0),
                 m_SavedProperties=_new(C.UnityPropertySheet, m_Colors=colors or [],
                                        m_Floats=floats or [], m_TexEnvs=[]))
    value.object_reader = reader
    return env, value


def renderer(module, geometry, materials, fs):
    C = importlib.import_module(module.__name__ + ".classes")
    value = _new(C.SkinnedMeshRenderer,
        m_Mesh=SimpleNamespace(deref_parse_as_object=lambda: geometry),
        m_Materials=[SimpleNamespace(deref_parse_as_object=lambda m=m: m) for m in materials],
        m_StaticBatchInfo=_new(C.StaticBatchInfo, firstSubMesh=0, subMeshCount=1))
    value.object_reader = SimpleNamespace(assets_file=SimpleNamespace(environment=SimpleNamespace(fs=fs)))
    return value


def observe_materials(module):
    result = {}
    for name, floats, colors in (
        ("integers", [("_Shininess", 20), ("_Transparency", 1)], []),
        ("tuple_colors", [("_Shininess", 7.5)], [("_Color", (1, .5, .25, 1)),
                                                ("_SColor", (0, 1, 0, 1)),
                                                ("_SpecularColor", (.1, .2, .3, 1))]),
        ("edited", [("_Shininess", 1), ("_Shininess", 33)], [("_Color", (.3, .4, .5, 1))]),
    ):
        env, value = material(module, floats, colors)
        fs = _MemoryFS()
        value_renderer = renderer(module, mesh(module), [value], fs)
        result[name] = {"export": _call(lambda: value_renderer.export("materials")),
                        "files": fs.files}
    return result


def observe_shader_cap(module, corrected=False):
    """Safety reference applies the requested cap before the 80-byte decode."""
    if corrected and module.__name__ == "UnityPy":
        return {"refused": True}
    fixture = Path(__file__).resolve().parents[2] / "crates/unityrs-export/tests/fixtures/shader.json"
    case = next(c for c in json.loads(fixture.read_text())["cases"] if c["case"] == "version_5_5_0")
    env = (module.Environment(limits=module.Limits(max_block_output=1))
           if module.__name__ == "unityrs" else module.Environment())
    shader = env.load_file(bytes.fromhex(case["input"]), name="shader.assets").objects[1].read()
    try:
        shader.export()
        return {"refused": False}
    except Exception as error:
        if type(error).__name__ != "LimitError":
            raise
        return {"refused": True}


def observe_output_security(module, corrected=False):
    """Unsafe oracle writes stay in memory; the safety reference refuses them."""
    result = {}
    bad_names = ("", ".", "..", "../escape", "/absolute", "a/b", "a\\b", "C:escape", "a\0b")
    for name in bad_names:
        if corrected and module.__name__ == "UnityPy":
            result[name] = {"error": "ValueError", "files": []}
            continue
        geometry = mesh(module)
        geometry.m_Name = name
        fs = _MemoryFS()
        value = renderer(module, geometry, [], fs)
        try:
            value.export("safe")
            error = None
        except Exception as caught:
            error = type(caught).__name__
        result[name] = {"error": error, "files": sorted(fs.files)}
    for name, key in (("../escape", "_MainTex"), ("", "../escape"), ("/absolute", "_MainTex")):
        label = "texture:" + name + ":" + key
        if corrected and module.__name__ == "UnityPy":
            result[label] = {"error": "ValueError", "files": []}
            continue
        env, value = material(module)
        # A fake image writes text to the in-memory filesystem, avoiding Pillow
        # and any host I/O while isolating the exporter's filename handling.
        texture = SimpleNamespace(m_Name=name, image=SimpleNamespace(save=lambda s, **kw: s.write("image")))
        value.m_SavedProperties.m_TexEnvs = [(key, SimpleNamespace(
            m_Texture=SimpleNamespace(deref_parse_as_object=lambda: texture)))]
        fs = _MemoryFS()
        value_renderer = renderer(module, mesh(module), [value], fs)
        exporter = importlib.import_module(
            "unityrs._native" if module.__name__ == "unityrs" else "UnityPy.export.MeshRendererExporter")
        with patch.object(exporter, "export_material", return_value="newmtl paint"):
            try:
                value_renderer.export("safe")
                error = None
            except Exception as caught:
                error = type(caught).__name__
        result[label] = {"error": error, "files": sorted(fs.files)}
    for name in ("", ".", "..", "../escape", "/absolute", "a/../escape", "a\\b", "C:escape", "a\0b"):
        label = "environment:" + name
        if corrected and module.__name__ == "UnityPy":
            result[label] = {"error": "ValueError", "files": []}
            continue
        env = module.Environment()
        env.files = {name: SimpleNamespace(is_changed=True, save=lambda **kw: b"unsafe")}
        opened = []
        class Stream:
            def __enter__(self):
                return self
            def __exit__(self, *args):
                pass
            def write(self, value):
                pass
        def memory_open(path, *args, **kwargs):
            opened.append(path)
            return Stream()
        with patch("builtins.open", memory_open):
            try:
                env.save(out_path="safe")
                error = None
            except Exception as caught:
                error = type(caught).__name__
        result[label] = {"error": error, "files": opened}
    return result
