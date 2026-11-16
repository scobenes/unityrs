"""Round-six lifetime, resource ownership and save protocol regressions."""
import array
import importlib
import io
import struct
import sys
import tracemalloc
from pathlib import Path
from unittest.mock import patch

from binding_cases import S
from binding_y4_exports import mesh, _MemoryFS
from binding_y4_images import make
from binding_y4_review_exports import material, renderer
from binding_y4_save import _asset, _load


class _ImageFS(_MemoryFS):
    def open(self, path, mode, **kwargs):
        if "b" not in mode:
            return super().open(path, mode, **kwargs)
        fs = self
        class Stream(io.BytesIO):
            def close(self):
                fs.files[path] = self.getvalue().hex()
                super().close()
        stream = Stream()
        stream.name = path
        return stream


def _texture_source(name, size=4):
    integers = dict(m_Width=1, m_Height=1, m_TextureFormat=4,
                    m_CompleteImageSize=size, m_ImageCount=1, m_IsReadable=1,
                    m_LightmapFormat=0, m_TextureDimension=2, m_TextureSettings=0)
    fields = [S.Node("string", "m_Name")]
    fields += [S.Node("int", key) for key in integers]
    fields += [S.vec(key, S.Node("UInt8", "data")) for key in ("image data", "m_PlatformBlob")]
    payload = S.s32(name) + struct.pack("<" + "i" * len(integers), *integers.values())
    payload += struct.pack("<i", size) + bytes(size) + struct.pack("<i", size) + bytes(size)
    return S.serialized_file([S.Type(28, S.Node("Texture2D", "Base", fields))], [S.Obj(1, 0, payload)])


def observe_blob_lifetime(module, corrected=False):
    from PIL import Image
    if module.__name__ == "UnityPy":
        # The oracle has no budget API. Tier B requires refusal before the
        # second staging allocation, and before Pillow's encode staging.
        return {"decode": ["MemoryError", False, True], "encode": ["MemoryError", False, True]} if corrected else None
    budget = module.Budget(200_000_000)
    file = module.Environment(budget=budget).load_file(_texture_source("pixels", 100_000), name="pixels.assets")
    obj = file.objects[1].read()
    assert type(obj).__name__ == "Texture2D"
    result = {}
    for action, remaining in (("decode", 150_000), ("encode", 210_000)):
        held = file._native.tree_traversal()
        held.step(0, budget.limit - budget.used - remaining, 0)
        before = budget.used
        transformed = []
        class Source(Image.Image):
            def transpose(self, method):
                transformed.append(True)
                return super().transpose(method)
        source = Image.new("RGBA", (100, 100))
        source.__class__ = Source
        tracemalloc.start()
        try:
            obj.image if action == "decode" else obj.set_image(source)
            error = None
        except Exception as caught:
            error = type(caught).__name__
        peak = tracemalloc.get_traced_memory()[1]
        tracemalloc.stop()
        result[action] = [error, peak > remaining if action == "decode" else bool(transformed), budget.used == before]
        del held
    return result


def observe_resource_inheritance(module):
    result = {}
    for web in (False, True):
        source = (S.web_file([(b"old.resS", b"resource")]) if web else
                  S.fs_bundle([(b"old.resS", b"resource", 7)]))
        file = _load(module, source)
        resource = file.files["old.resS"]
        initial = resource.endian
        resource.endian = "<"
        writer = file.get_writeable_cab("new.resS")
        writer.write_int(0x01020304)
        result[str(web)] = [initial, writer.endian, writer.flags, writer.bytes.hex()]
    return result


def observe_pointer_owner(module, corrected=False):
    env, value = material(module)
    owner = env.load_file(_texture_source("owner"), name="owner-texture.assets")
    other = env.load_file(_texture_source("other"), name="other-texture.assets")
    C = importlib.import_module(module.__name__ + ".classes")
    pointer = C.PPtr(m_FileID=0, m_PathID=1, assetsfile=owner)
    pointer.assetsfile = other
    value.object_reader = owner.objects[1]
    value.m_SavedProperties.m_TexEnvs = [("_MainTex", make(module, "UnityTexEnv", m_Texture=pointer,
                                                         m_Offset=(0, 0), m_Scale=(1, 1)))]
    fs = _ImageFS()
    value_renderer = renderer(module, mesh(module), [value], fs)
    if module.__name__ == "UnityPy" and corrected:
        sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "fixtures"))
        from obj_export import corrected as correct
        exporter = importlib.import_module("UnityPy.export.MeshRendererExporter")
        with patch.object(exporter, "export_material", correct(exporter).export_material):
            value_renderer.export("textures")
    else:
        try:
            value_renderer.export("textures")
        except ValueError as error:
            if module.__name__ != "UnityPy":
                raise
            return {"error": type(error).__name__}
    return {"files": fs.files}


def observe_payload_lengths(module):
    from binding_y4_review_save import _ReviewWriter
    result = {}
    for label, data in (("array", array.array("I", [1, 2, 3])),
                        ("view", memoryview(array.array("I", [1, 2, 3]))),
                        ("matrix", memoryview(bytes(range(12))).cast("B", (3, 4)))):
        file = _load(module, _asset())
        obj = file.objects[1]
        obj.data = data
        header, payload = _ReviewWriter(endian="<"), _ReviewWriter(endian="<")
        obj.write(file.header, header, payload)
        direct_size = struct.unpack_from("<I", header.bytes, 16)[0]
        saved = file.save()
        reloaded = _load(module, saved).objects[1]
        result[label] = [direct_size, reloaded.byte_size, reloaded.get_raw_data().hex(),
                         saved[-16:].hex()]
        assert direct_size == 3
    return result


def observe_clear_replacement(module):
    result = {}
    for web in (None, False, True):
        source = _asset()
        if web is not None:
            source = (S.web_file([(b"child.assets", source)]) if web else
                      S.fs_bundle([(b"child.assets", source, 4)]))
        parent = _load(module, source)
        file = parent if web is None else parent.files["child.assets"]
        obj = file.objects[1]
        original = obj.get_raw_data()
        states = []
        for replacement in (None, b"replacement", None, None):
            file.is_changed = parent.is_changed = False
            obj.set_raw_data(replacement)
            saved = _load(module, parent.save())
            saved_file = saved if web is None else saved.files["child.assets"]
            if module.__name__ == "unityrs":
                native_saved = _load(module, file._native.save("", None, 0, 0, ""))
                assert native_saved.objects[1].get_raw_data() == (original if replacement is None else replacement)
            states.append([obj.data is None, file.is_changed, parent.is_changed,
                           saved_file.objects[1].get_raw_data() == (original if replacement is None else replacement)])
        result[str(web)] = states
    return result


def observe_detached_material(module):
    _, value = material(module, floats=[("_Shininess", 7.5)])
    value.object_reader = None
    fs = _MemoryFS()
    renderer(module, mesh(module), [value], fs).export("detached")
    return fs.files
