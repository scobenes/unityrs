"""Caller-thread, schema and preallocation regressions on synthetic inputs."""

import array
import hashlib
import struct
import threading
import tracemalloc
from types import SimpleNamespace

from binding_cases import S
from binding_y4_save import _asset, _load
from binding_y4_images import texture, capture, image
from binding_y4_exports import mesh


def _container(module, web=False, bounded=False):
    source = (S.web_file([(b"child.assets", _asset())]) if web else
              S.fs_bundle([(b"child.assets", _asset(), 4)]))
    if bounded and module.__name__ == "unityrs":
        return module.Environment(budget=module.Budget(1_000_000)).load_file(source, name="owner")
    return _load(module, source)


def observe_cab_budget(module, corrected=False):
    result = {}
    for label in ("mutable", "padding"):
        cab = _container(module, bounded=True).get_writeable_cab("new.resS")
        cab.write(b"x")
        source = bytearray(2_000_000)
        tracemalloc.start()
        tracemalloc.reset_peak()
        try:
            cab.write(source) if label == "mutable" else cab.align_stream(2_000_000)
            error = None
        except Exception as caught:
            error = type(caught).__name__
        peak = tracemalloc.get_traced_memory()[1]
        tracemalloc.stop()
        result[label] = {"error": error, "copied": peak > 1_000_000,
                         "position": cab.Position}
    if corrected and module.__name__ == "UnityPy":
        return {key: {"error": "MemoryError", "copied": False, "position": 1} for key in result}
    return result


def observe_cab_length(module):
    cab = _container(module, bounded=True).get_writeable_cab("new.resS")
    cab.write(bytes(350_000))
    return {"length": cab.Length, "position": cab.Position}


def observe_cab_padding(module):
    cab = _container(module).get_writeable_cab("new.resS")
    cab.write(b"abcdefgh")
    cab.Position = 2
    cab.write(b"")
    cab.align_stream(4)
    result = {"overwrite": cab.bytes.hex(), "cursor": cab.Position, "length": cab.Length}
    cab.Position = 12
    cab.write(b"")
    cab.align_stream(16)
    result["gap"] = cab.bytes.hex()
    result["end"] = [cab.Position, cab.Length]
    return result


def observe_typed_sequences(module):
    result = {}
    for kind, fmt in (("int", "i"), ("UInt8", "B"), ("float", "f")):
        vector = S.Node("vector", "x", [S.Node("Array", "Array", [
            S.Node("int", "size"), S.Node(kind, "data")])])
        for label, value in (("view", memoryview(array.array("i", [1, 2, 3]))),
                             ("array", array.array("i", [1, 2, 3])), ("range", range(1, 4))):
            file = _load(module, S.serialized_file([S.Type(49, S.Node("Root", "Base", [vector]))],
                                                 [S.Obj(1, 0, bytes(4))]))
            raw = file.objects[1].save_typetree({"x": value})
            assert raw == struct.pack("<i3" + fmt, 3, 1, 2, 3)
            result[kind + ":" + label] = _load(module, file.save()).objects[1].parse_as_dict()
    return result


def observe_packer_threads(module):
    result = {}
    caller = threading.get_ident()
    for web in (False, True):
        file = _container(module, web=web)
        child = file.files["child.assets"].objects[1]

        class Packer(str):
            def __bool__(self):
                assert threading.get_ident() == caller, "packer callback left caller"
                assert child.parse_as_dict()["value"] == 7
                return True

        saved = file.save(packer=Packer("none"))
        result["web" if web else "bundle"] = _load(module, saved).files["child.assets"].objects[1].parse_as_dict()
    return result


def _sensitive(value):
    caller = threading.get_ident()
    original = type(value)

    class Sensitive(original):
        def __getattribute__(self, name):
            assert threading.get_ident() == caller, "export getter left caller"
            return super().__getattribute__(name)

    Sensitive.__name__ = original.__name__
    value.__class__ = Sensitive
    return value


def observe_mesh_thread(module):
    return _sensitive(mesh(module)).export()


def observe_shader_thread(module):
    # Keep the getter below the facade's fields dictionary, so this probes
    # native inspection rather than only the adapter's own Python reads.
    import json
    from pathlib import Path
    fixture = Path(__file__).resolve().parents[2] / "crates/unityrs-export/tests/fixtures/shader.json"
    case = next(c for c in json.loads(fixture.read_text())["cases"] if c["case"] == "version_2020_1_0")
    shader = _load(module, bytes.fromhex(case["input"])).objects[1].read()
    shader.m_ParsedForm = _sensitive(shader.m_ParsedForm)
    return shader.export()


def observe_sprite_thread(module):
    # The tight path reads the sprite a second time to decode its geometry.
    import sys
    from pathlib import Path
    sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "fixtures"))
    from sprite_build import SPRITE_ROWS, sheet_bytes
    from sprite_cases import cases
    from binding_y4_images import _tree
    case = next(c for c in cases() if c["name"] == "mask_triangle")
    file = _load(module, S.serialized_file([S.Type(213, _tree(SPRITE_ROWS))],
                                         [S.Obj(10, 0, bytes.fromhex(case["sprite"]))]))

    class TextureReader:
        def __init__(self, obj):
            self.obj = obj
        def parse_as_object(self):
            return self.obj

    for key, fields in case["textures"].items():
        file.objects[int(key)] = TextureReader(texture(module, sheet_bytes(fields),
            m_Width=fields["width"], m_Height=fields["height"], m_TextureFormat=4))
    return image(_sensitive(file.objects[10].read()).image)


def observe_entry_threads(module):
    caller = threading.get_ident()

    class Rows(list):
        def __len__(self):
            assert threading.get_ident() == caller, "row length left caller"
            return super().__len__()
        def __getitem__(self, index):
            assert threading.get_ident() == caller, "row item left caller"
            return super().__getitem__(index)
        def __iter__(self):
            assert threading.get_ident() == caller, "row iteration left caller"
            return super().__iter__()

    result = {}
    for web in (False, True):
        file = _container(module, web=web)
        payload = file.files["child.assets"].save()
        if module.__name__ == "unityrs":
            if web:
                raw = file._native.save_web_entries(Rows([(b"child.assets", payload)]), "none", b"UnityWebData1.0")
            else:
                guard = file._native.tree_traversal()
                raw = file._native.save_bundle_entries("save", Rows([(b"child.assets", 4, payload)]),
                    "none", (64, 64), file._save_header(guard))
        else:
            raw = file.save(packer="none")
        result["web" if web else "bundle"] = hashlib.sha256(raw).hexdigest()
    return result


def observe_switch_invalid_counts(module):
    from PIL import Image
    result = {}
    for label, count in (("float", 2.0), ("none", None), ("text", "2")):
        obj = texture(module, m_PlatformBlob=bytes(8) + b"\x01\0\0\0",
                      object_reader=SimpleNamespace(platform=38))
        outcome = capture(lambda: obj.set_image(Image.new("RGBA", (8, 8)), 4, count))
        result[label] = {"outcome": outcome, "size": [obj.m_Width, obj.m_Height],
                         "data": hashlib.sha256(obj.image_data).hexdigest(),
                         "mips": [obj.m_MipMap, obj.m_MipCount]}
    return result
