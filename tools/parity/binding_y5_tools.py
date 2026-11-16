"""Y5 extractor observations from supplied values and synthetic asset files.

PARITY sections 15.7, 19.2, 19.3 and 23. No TPK schema is recorded.
"""
import contextlib
import hashlib
import importlib
import inspect
import io
import json
import os
import struct
from pathlib import Path
from types import SimpleNamespace as NS

from binding_cases import S, typed
from binding_y4_exports import mesh
from binding_y4_images import make, texture
from binding_y4_save import _load
import scratch


def extractor(module):
    return importlib.import_module(module.__name__ + ".tools.extractor")


def _capture(call, folder):
    try:
        value = call()
        if isinstance(value, list) and value and isinstance(value[0], tuple):
            value = [[file if isinstance(file, str) else "file", path] for file, path in value]
        return {"value": typed(value)}
    except Exception as error:
        return {"error": type(error).__name__, "message": str(error).replace(str(folder), "<out>")}


def _tree(folder):
    return {os.fsencode(path.relative_to(folder)).hex(): path.read_bytes().hex()
            for path in sorted(folder.rglob("*")) if path.is_file()}


def _reader_file(module):
    text = S.Node("TextAsset", "Base", [S.Node("string", "m_Name"), S.Node("string", "m_Script")])
    mono = S.Node("MonoBehaviour", "Base", [S.Node("string", "m_Name"), S.Node("int", "value")])
    mat = S.Node("Material", "Base", [S.Node("string", "m_Name")])
    return _load(module, S.serialized_file([S.Type(49, text), S.Type(114, mono), S.Type(21, mat)], [
        S.Obj(1, 0, S.s32("named.old") + S.s32(b"hello\xff")),
        S.Obj(2, 1, S.s32("é") + struct.pack("<i", 7)), S.Obj(3, 2, S.s32("mat")),
        S.Obj(4, 0, S.s32("") + S.s32("empty"))]))


def observe_tools_dispatch(module):
    E = extractor(module)
    names = ["export_obj", "extract_assets", "exportTextAsset", "exportFont", "exportMesh", "exportShader",
             "exportMonoBehaviour", "exportAudioClip", "exportSprite", "exportTexture2D", "exportGameObject",
             "crawl_obj", "flatten"]
    signatures = {}
    for name in names:
        function = getattr(E, name)
        # Module-qualified annotations differ only by the package name.
        signature = str(inspect.signature(function)).replace(module.__name__, "package")
        for kind in ("ObjectReader", "SerializedFile"):
            signature = signature.replace(f".files.{kind}.{kind}", f".files.{kind}")
        signatures[name] = signature
    return {"signatures": signatures,
            "dispatch": [[key.value, function.__name__] for key, function in E.EXPORT_TYPES.items()],
            "registry": dict(E.MONOBEHAVIOUR_TYPETREES),
            "aliases": [E.ASSEMBLY_NAME_DLL is str, E.CLASS_NAME is str]}


def observe_tools_files(module):
    E = extractor(module)
    result = {}
    with scratch.folder("unityrs-y5-tools-") as folder:
        def run(name, function):
            result[name] = _capture(lambda: function(str(folder / name)), folder)

        reader = NS(assets_file="file", path_id=11)
        text = NS(m_Script="é\udcff\r\n", object_reader=reader, assets_file="file")
        run("text", lambda fp: E.exportTextAsset(text, fp, ""))
        run("text_bytes", lambda fp: E.exportTextAsset(text, fp, ".bytes"))
        for name, data in (("otf", b"OTTOabc"), ("ttf", [0, 1, 2]),
                           ("otf_list", list(b"OTTOabc")), ("empty_font", [])):
            font = NS(m_FontData=data, object_reader=reader, assets_file="file")
            run(name, lambda fp, font=font: E.exportFont(font, fp, ".ignored"))
        shader = NS(export=lambda: "é\r\n", object_reader=reader, assets_file="file")
        run("shader", lambda fp: E.exportShader(shader, fp, ""))
        value = mesh(module)
        value.object_reader.assets_file = "file"
        value.object_reader.path_id = 11
        run("mesh", lambda fp: E.exportMesh(value, fp, ""))
        for name, samples in (("audio0", {}), ("audio1", {"a.ogg": b"OggSsynthetic"}),
                              ("audio2", {"a.wav": b"one", "b.ogg": b"two"})):
            clip = NS(samples=samples, object_reader=reader)
            run(name, lambda fp, clip=clip: E.exportAudioClip(clip, fp, ".ignored"))
        for name, width in (("texture", 1), ("texture_empty", 0)):
            value = texture(module, object_reader=reader, m_Width=width)
            run(name, lambda fp, value=value: E.exportTexture2D(value, fp, ""))
        from PIL import Image
        sprite = NS(image=Image.new("RGBA", (1, 2), (1, 2, 3, 4)), object_reader=reader,
                    m_RD=NS(texture=NS(assetsfile="texture_file", path_id=12),
                            alphaTexture=NS(assetsfile="alpha_file", path_id=13)))
        run("sprite", lambda fp: E.exportSprite(sprite, fp))
        run("sprite_no_ext", lambda fp: E.exportSprite(sprite, fp, ""))
        sprite.m_RD.alphaTexture.assets_file = "alpha_file"
        run("sprite_alpha", lambda fp: E.exportSprite(sprite, fp))
        raw = NS(parse_as_dict=lambda: 1 / 0, get_raw_data=lambda: b"raw\xff", assets_file="file", path_id=11)
        run("mono_raw", lambda fp: E.exportMonoBehaviour(raw, fp, ".ignored"))
        file = _reader_file(module)
        run("mono", lambda fp: E.exportMonoBehaviour(file.objects[2], fp))
        mono = make(module, "MonoBehaviour", object_reader=file.objects[2], m_Name="é")
        run("mono_instance", lambda fp: E.exportMonoBehaviour(mono, fp))
        run("text_reader", lambda fp: E.exportTextAsset(file.objects[1], fp))
        result["tree"] = _tree(folder)
    return result


def observe_tools_objects(module):
    E = extractor(module)
    file = _reader_file(module)
    C = importlib.import_module(module.__name__ + ".classes")
    result = {}
    with scratch.folder("unityrs-y5-objects-") as folder:
        for name, obj, kwargs in (
            ("plain", file.objects[1], {}),
            ("append", file.objects[1], {"append_name": True, "append_path_id": True}),
            ("pointer", C.PPtr(m_FileID=0, m_PathID=1, assetsfile=file), {"append_name": True}),
            ("empty", file.objects[4], {"append_name": True, "append_path_id": True}),
            ("filtered", file.objects[1], {"asset_filter": lambda obj: False}),
            ("unknown", file.objects[3], {}),
            ("unknown_tree", file.objects[3], {"export_unknown_as_typetree": True}),
            ("mono_dispatch", file.objects[2], {}),
        ):
            target = folder / name
            if kwargs.get("append_name"):
                target.mkdir()
            result[name] = _capture(lambda: E.export_obj(obj, str(target), **kwargs), folder)
        result["tree"] = _tree(folder)
        # EXPORT_TYPES is a mutable extension point and uses current functions.
        calls = []
        original = E.EXPORT_TYPES[file.objects[1].type]
        try:
            def custom(instance, fp, extension):
                calls.append([instance.m_Name, Path(fp).name, extension])
                return [("custom", 77)]
            E.EXPORT_TYPES[file.objects[1].type] = custom
            result["patched"] = _capture(lambda: E.export_obj(file.objects[1], str(folder / "custom.ext")), folder)
            result["calls"] = calls
        finally:
            E.EXPORT_TYPES[file.objects[1].type] = original
    return result


def observe_tools_crawl(module):
    E = extractor(module)
    C = importlib.import_module(module.__name__ + ".classes")
    file = _reader_file(module)
    result = {}
    with scratch.folder("unityrs-y5-crawl-") as folder:
        for label, obj in (("other", object()), ("reader", file.objects[1]),
                           ("parsed", file.objects[1].read()),
                           ("pointer", C.PPtr(m_FileID=0, m_PathID=1, assetsfile=file)),
                           ("null", C.PPtr(m_FileID=0, m_PathID=0, assetsfile=file))):
            def call(obj=obj):
                return [[key, type(value).__name__] for key, value in E.crawl_obj(obj).items()]
            result[label] = _capture(call, folder)
        empty = {}
        nonempty = {8: "existing"}
        result["ret_identity"] = [E.crawl_obj(object(), empty) is empty,
                                  E.crawl_obj(object(), nonempty) is nonempty]
        obj = make(module, "GameObject", object_reader=NS(assets_file="file", path_id=1))
        result["gameobject"] = _capture(lambda: E.exportGameObject(obj, str(folder / "go")), folder)
        sequences = [[1, (2, [3]), {"a": 4, "b": {"c": (5,)}}], {"first": 1, "second": 2},
                     (x for x in [1, (2, 3)]), ["abc", b"bytes", None]]
        result["flatten"] = [typed(list(E.flatten(seq))) for seq in sequences]
    return result


def observe_tools_extract(module):
    """Replay the existing E4 synthetic scenes through the Python surface."""
    E = extractor(module)
    path = Path(__file__).resolve().parents[2] / "crates/unityrs-export/tests/fixtures/extractor.json"
    result = {}
    for case in json.loads(path.read_text())["cases"]:
        # These retain intentionally repaired Tier B/C semantics in Y4 or
        # involve unsafe output names, which have separate confinement tests.
        name = case["case"]
        if any(token in name for token in ("zero", "unsafe", "wide_version", "surrogate")):
            continue
        options = dict(case["options"])
        if "asset_filter" in options:
            accepted = options["asset_filter"]
            options["asset_filter"] = lambda obj: obj.type.value in accepted
        with scratch.folder("unityrs-y5-extract-") as folder:
            dst = folder / "out"
            dst.mkdir()
            with contextlib.redirect_stdout(io.StringIO()):
                outcome = _capture(lambda: E.extract_assets(bytes.fromhex(case["input"]), dst, **options), folder)
            tree = {key: {"size": len(bytes.fromhex(value)),
                          "sha256": hashlib.sha256(bytes.fromhex(value)).hexdigest()}
                    for key, value in _tree(dst).items()}
            result[name] = {"outcome": outcome, "tree": tree}
    return result
