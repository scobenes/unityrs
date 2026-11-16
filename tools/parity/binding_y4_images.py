"""Synthetic Y4 image observations, with no saved TPK fields or defaults."""

import importlib
import io
import contextlib
import sys
from pathlib import Path
from types import SimpleNamespace


def make(module, name, **values):
    cls = getattr(importlib.import_module(module.__name__ + ".classes"), name)
    result = cls.__new__(cls)
    for key, value in values.items():
        setattr(result, key, value)
    return result


def texture(module, data=b"\x01\x02\x03\x04\x05\x06\x07\x08", **values):
    fields = dict(image_data=data, m_Width=1, m_Height=2, m_TextureFormat=4,
                  m_PlatformBlob=None, m_MipMap=False, m_MipCount=1,
                  m_CompleteImageSize=len(data), m_StreamData=None)
    fields.update(values)
    return make(module, "Texture2D", **fields)


def image(value):
    return {"mode": value.mode, "size": list(value.size), "pixels": value.tobytes().hex()}


def capture(call):
    try:
        return {"value": call()}
    except Exception as error:
        return {"error": type(error).__name__, "message": str(error)}


def observe_textures(module, corrected=False):
    result = {}
    for label, data, fields in (
        ("rgba", bytes(range(24)), dict(m_Width=3, m_Height=2)),
        ("rgb", bytes(range(18)), dict(m_Width=3, m_Height=2, m_TextureFormat=3)),
        ("alpha", b"\x11\xf0", dict(m_TextureFormat=1)),
        ("r8", b"\x11\xf0", dict(m_TextureFormat=63)),
        ("short", b"x", {}), ("missing", b"", {}),
        ("zero", b"x", dict(m_Width=0, m_Height=-1, m_TextureFormat=999)),
        ("unknown", b"x", dict(m_TextureFormat=999)),
        ("negative", b"x", dict(m_Width=-1)),
        ("unsupported", b"x", dict(m_TextureFormat=0)),
    ):
        obj = texture(module, data, **fields)
        result[label] = capture(lambda: image(obj.image))
    obj = texture(module)
    result["inline"] = obj.get_image_data().hex()
    obj.image_data = b"\x90\x80\x70\x60"
    obj.m_Height = 1
    result["edited_fields"] = image(obj.image)
    result["missing_data"] = capture(lambda: texture(module, b"").get_image_data())
    reader = SimpleNamespace(version=None, platform=0)
    result["noncrunch_ignores_version"] = capture(lambda: image(texture(module, object_reader=reader).image))
    result["crunch_checks_version"] = capture(lambda: image(texture(module, object_reader=reader, m_TextureFormat=28).image))
    result["crunch_missing_data_first"] = capture(lambda: image(texture(module, b"", object_reader=reader, m_TextureFormat=28).image))
    obj = texture(module, b"", m_Width=0, m_Height=0)
    if corrected and module.__name__ == "UnityPy":
        from PIL import Image
        result["zero_empty_no_stream"] = {"value": image(Image.new("RGBA", (0, 0)))}
    else:
        result["zero_empty_no_stream"] = capture(lambda: image(obj.image))
    result["streams"] = _streams(module)
    return result


def _stream_file(module):
    from binding_cases import S
    tree = S.Node("TextAsset", "Base", [S.Node("string", "m_Name"), S.Node("string", "m_Script")])
    data = S.serialized_file([S.Type(49, tree)], [S.Obj(1, 0, S.s32("a") + S.s32("b"))])
    data = S.fs_bundle([(b"CAB-images", data, 4), (b"CAB-images.resS", bytes(range(16)), 0)])
    with contextlib.redirect_stdout(io.StringIO()):
        bundle = module.Environment().load_file(data, name="images.bundle")
    return bundle.files["CAB-images"]


def _streams(module):
    file = _stream_file(module)
    result = {}
    for offset, size in ((2, 8), (-8, 4), (0, -1), (2, -2), (100, 4), (0, 0)):
        obj = texture(module, b"", object_reader=file.objects[1],
                      m_StreamData=SimpleNamespace(path="CAB-images.resS", offset=offset, size=size))
        result[f"{offset}/{size}"] = capture(lambda: obj.get_image_data().hex())
    obj.image_data = b"inline"
    obj.m_StreamData.path = "missing.resS"
    result["inline_wins"] = obj.get_image_data().hex()
    obj.image_data = b""
    result["missing_stream"] = capture(lambda: obj.get_image_data().hex())
    return result


def _set_image(module, obj, src, target, mips, corrected):
    """Keep raw oracle failures separately from PARITY 18.1's reference."""
    try:
        return obj.set_image(src, target, mips)
    except ValueError as error:
        if not (corrected and module.__name__ == "UnityPy" and str(error).startswith("No packer found from ")):
            raise
        return obj.set_image(src.convert("RGBA"), target, mips)


def observe_set_image(module, corrected=False):
    from PIL import Image
    result = {}
    for mode in ("RGBA", "RGB", "L", "LA", "P", "RGBX", "CMYK", "I", "F", "1"):
        for target in (1, 3, 4, 9, 14):
            def run():
                obj = texture(module)
                src = Image.new(mode, (8, 8))
                obj.m_StreamData = SimpleNamespace(path="old.resS", offset=7, size=9)
                returned = _set_image(module, obj, src, target, 4, corrected)
                return {"return": returned, "data": obj.image_data.hex(),
                        "format": int(obj.m_TextureFormat), "size": [obj.m_Width, obj.m_Height],
                        "mips": [obj.m_MipMap, obj.m_MipCount], "complete": obj.m_CompleteImageSize,
                        "stream": vars(obj.m_StreamData)}
            result[f"{mode}-{target}"] = capture(run)
    obj = texture(module)
    src = Image.frombytes("RGBA", (1, 2), bytes(range(8)))
    obj.image = src
    result["setter"] = image(obj.image)
    result["setter_data"] = obj.image_data.hex()
    stream = io.BytesIO()
    src.save(stream, "PNG")
    stream.seek(0)
    obj.set_image(stream, target_format=0, mipmap_count=0)
    result["filelike"] = [image(obj.image), obj.m_MipCount]
    result["bad_source"] = capture(lambda: obj.set_image(io.BytesIO(b"bad image")))
    result["bad_source"].pop("message", None)  # BytesIO's repr contains its address.
    return result


def observe_image_formats(module, corrected=False):
    """Nonuniform image data makes channel selection and mip filtering visible."""
    from PIL import Image
    result = {}
    for mode in ("RGB", "RGBA"):
        bands = len(mode)
        data = bytes((i * 37 + i // 11) % 256 for i in range(16 * 8 * bands))
        for target in (1, 2, 3, 4, 5, 7, 8, 9, 13, 14, 15, 16, 17, 18, 19, 20,
                       21, 22, 23, 30, 32, 35, 41, 42, 63, 75, 82):
            def run():
                obj = texture(module)
                _set_image(module, obj, Image.frombytes(mode, (16, 8), data), target, 7, corrected)
                return {"data": obj.image_data.hex(), "format": int(obj.m_TextureFormat),
                        "mips": obj.m_MipCount, "decoded": image(obj.image)}
            result[f"{mode}-{target}"] = capture(run)
        encoded_targets = (10, 12, 26, 27, 28, 29, 34, 45, 46, 47, 60, 65, *range(48, 60), *range(66, 72))
        for target in encoded_targets:
            def run_encoded():
                import hashlib
                obj = texture(module)
                _set_image(module, obj, Image.frombytes(mode, (16, 8), data), target, 2, corrected)
                written_format = int(obj.m_TextureFormat)
                if written_format == 60:
                    # The 3DS layout has no independent decode reference (U).
                    # Its writer emits ETC1 blocks, verified through ETC_RGB4.
                    obj.m_TextureFormat = 34
                decoded = obj.image
                return {"format": written_format, "mips": obj.m_MipCount,
                        "bytes": len(obj.image_data), "mode": decoded.mode, "size": list(decoded.size),
                        "sha256": hashlib.sha256(decoded.tobytes()).hexdigest()}
            result[f"encoded-{mode}-{target}"] = capture(run_encoded)
    return result


def observe_arrays(module):
    result = {}
    for label, fields in (
        ("rgba", {}), ("rgb", dict(m_Format=3, image_data=bytes(range(12)), m_DataSize=12)),
        ("empty", dict(image_data=b"")), ("depth_zero", dict(m_Depth=0)),
        ("step_zero", dict(m_DataSize=1)), ("unmapped", dict(m_Format=0)),
        ("unknown", dict(m_Format=999)),
    ):
        values = dict(m_Format=4, m_Width=1, m_Height=2, m_Depth=2,
                      m_DataSize=16, image_data=bytes(range(16)), m_StreamData=None,
                      object_reader=SimpleNamespace(version=(2020, 3, 0, 0)))
        values.update(fields)
        obj = make(module, "Texture2DArray", **values)
        result[label] = capture(lambda: [image(value) for value in obj.images])
    return result


def _tree(rows):
    from binding_cases import S
    stack = []
    for level, ty, name, flags in rows:
        node = S.Node(ty, name, meta=flags)
        if stack:
            stack[level - 1].children.append(node)
        stack[level:] = [node]
        if level == 0:
            root = node
    return root


def observe_sprites(module):
    """Reuse the renderer's independently built synthetic input trees."""
    from binding_cases import S
    sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "fixtures"))
    from sprite_build import SPRITE_ROWS, TREES, sheet_bytes
    from sprite_cases import cases
    selected = {"crop_inside", "crop_half_even", "crop_negative_width", "crop_negative_height",
                "crop_nan", "crop_infinity", "packed_rotation_0", "packed_rotation_1",
                "packed_rotation_2", "packed_rotation_3", "packed_rotation_4", "packed_rotation_5",
                "atlas_pointer", "atlas_tag", "atlas_missing_key", "atlas_tight_mask"}
    class TextureReader:
        type = module.enums.ClassIDType.Texture2D
        def __init__(self, obj):
            self.obj = obj
        def parse_as_object(self):
            return self.obj
    result = {}
    for case in cases():
        if case["name"] not in selected:
            continue
        types = [S.Type(213, _tree(SPRITE_ROWS))]
        objects = [S.Obj(10, 0, bytes.fromhex(case["sprite"]))]
        if "atlas" in case:
            types.append(S.Type(687078895, _tree(TREES[case.get("atlas_tree", "atlas")])))
            objects.append(S.Obj(5, 1, bytes.fromhex(case["atlas"])))
        with contextlib.redirect_stdout(io.StringIO()):
            file = module.Environment().load_file(S.serialized_file(types, objects), name="sprites.assets")
        for key, fields in case["textures"].items():
            tex = texture(module, sheet_bytes(fields), m_Width=fields["width"], m_Height=fields["height"],
                          m_TextureFormat={"RGBA32": 4, "RGB24": 3}[fields["format"]])
            file.objects[int(key)] = TextureReader(tex)
        sprite = file.objects[10].read()
        result[case["name"]] = capture(lambda: image(sprite.image))
        if case["name"] == "crop_inside":
            sprite.m_RD.textureRect.width = 2.0
            result["edited_render_fields"] = capture(lambda: image(sprite.image))
            file.objects[1].obj.image_data = bytes([255]) * len(file.objects[1].obj.image_data)
            result["cached_sheet"] = capture(lambda: image(sprite.image))
            file._cache.clear()
            result["cleared_cache"] = capture(lambda: image(sprite.image))
    return result
