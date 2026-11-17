# Ported from UnityPy 1.25.2 (MIT): export/Texture2DConverter.py:20-80, export/Texture2DConverter.py:106-115, export/Texture2DConverter.py:237-344, export/Texture2DConverter.py:356-397, export/Texture2DConverter.py:488-569. See LICENSE-UNITYPY and THIRD_PARTY_LICENSES.
"""Optional Pillow facade backed by the existing native texture codecs."""
from types import SimpleNamespace
from io import BytesIO
import struct
from ..enums import TextureFormat, BuildTarget
from .. import _native

from ._astc_context import _get_astc_context
from threading import get_ident

TF = TextureFormat


def get_astc_context(block_size):
    return _get_astc_context(get_ident(), block_size)


def _decode(data, width, height, texture_format, version=(0, 0), platform=0, platform_blob=None, *, _guard=None, _reader=None):
    from PIL import Image
    from .._buffers import snapshot_bytes
    guard = _native.TreeTraversal() if _guard is None else _guard
    data = snapshot_bytes(data, guard)
    blob = None if platform_blob is None else snapshot_bytes(platform_blob, guard)
    version = tuple(version[:2]) if "Crunched" in TextureFormat(texture_format).name else (0, 0)
    mode, width, height, pixels = _native._image_decode(data, width, height, int(texture_format), version, int(platform), blob, _reader)
    guard.step(0, len(pixels) + 1024, len(pixels))
    return Image.frombytes(mode, (width, height), pixels).transpose(Image.Transpose.FLIP_TOP_BOTTOM)


def pillow(image_data, width, height, mode, codec, args, swap=None):
    from PIL import Image
    image = Image.frombytes(mode, (width, height), image_data, codec, args) if width else Image.new(mode, (width, height))
    if swap:
        channels = image.split()
        image = Image.merge(mode, [channels[x] for x in swap])
    return image


def atc(image_data, width, height, alpha):
    return _decode(image_data, width, height, TF.ATC_RGBA8 if alpha else TF.ATC_RGB4)


def astc(image_data, width, height, block_size):
    from PIL import Image
    size = calculate_astc_compressed_size(width, height, block_size)
    if len(image_data) < size:
        raise ValueError(f"Invalid ASTC data size: {len(image_data)} < {size}")
    image = SimpleNamespace(data_type=0, dim_x=width, dim_y=height, dim_z=1, data=None)
    get_astc_context(block_size).decompress(image_data[:size], image, SimpleNamespace(r=0, g=1, b=2, a=3))
    assert image.data is not None, "Decompression failed, image data is None"
    return Image.frombytes("RGBA", (width, height), image.data, "raw", "RGBA")


def pvrtc(image_data, width, height, fmt):
    return _decode(image_data, width, height, TF.PVRTC_RGBA2 if fmt else TF.PVRTC_RGBA4)


def etc(image_data, width, height, fmt):
    target = {"ETC1": TF.ETC_RGB4, "ETC2_RGB": TF.ETC2_RGB, "ETC2_A1": TF.ETC2_RGBA1, "ETC2_A8": TF.ETC2_RGBA8}
    if fmt not in target:
        raise NotImplementedError(f"Unknown ETC mode: {fmt}")
    return _decode(image_data, width, height, target[fmt])


def eac(image_data, width, height, fmt):
    if fmt not in ("EAC_R", "EAC_R_SIGNED", "EAC_RG", "EAC_RG_SIGNED"):
        raise NotImplementedError(f"Unknown EAC mode: {fmt}")
    return _decode(image_data, width, height, getattr(TF, fmt))


def half(image_data, width, height, mode, codec, args, swap=None):
    # export/Texture2DConverter.py:435-447: the source counts codec letters,
    # multiplies each half by 256, and preserves bytes()'s range refusal.
    guard = _native.TreeTraversal()
    count = max(0, width * height * len(codec))
    guard.step(0, count + len(image_data) + 1024, count)
    stream = BytesIO(image_data)
    data = bytes(int(struct.unpack("e", stream.read(2))[0] * 256) for _ in range(count))
    return pillow(data, width, height, mode, codec, args, swap)


def rg(image_data, width, height, mode, codec, args):
    # export/Texture2DConverter.py:459-468: padding constants are used as
    # byte counts in 1.25.2, including its missing "RG" table member.
    padding_size = RG_PADDING_MAP[codec]
    guard = _native.TreeTraversal()
    guard.step(0, len(image_data) * 4 + 1024, len(image_data))
    stream = BytesIO(image_data)
    padding = bytes(padding_size)
    data = b"".join(stream.read(padding_size * 2) + padding for _ in range(len(image_data) // (2 * padding_size)))
    if codec == "RGE":
        return half(data, width, height, mode, "RGB", args)
    return pillow(data, width, height, mode, codec.replace("RG", "RGB"), args)


def rgb9e5float(image_data, width, height):
    # export/Texture2DConverter.py:471-484 retains the float-byte refusal.
    from PIL import Image
    guard = _native.TreeTraversal()
    guard.step(0, max(0, width * height * 3) + 1024, len(image_data))
    rgb = bytearray(width * height * 3)
    for i, (n,) in enumerate(struct.iter_unpack("<i", image_data)):
        scale = n >> 27 & 0x1F
        scaleb = 2 ** (scale - 24) * 255.0
        b = (n >> 18 & 0x1FF) * scaleb
        g = (n >> 9 & 0x1FF) * scaleb
        r = (n & 0x1FF) * scaleb
        rgb[i * 3:i * 3 + 3] = [r, g, b]
    return Image.frombytes("RGB", (width, height), rgb, "raw", "RGB")


from ._texture_maps import (TEXTURE_FORMAT_BLOCK_SIZE_TABLE, RG_PADDING_MAP, CONV_TABLE,
                           XBOX_SWAP_FORMATS, tf, split, block_size, get_compressed_image_size)
_DEFAULT_CONV_TABLE = dict(CONV_TABLE)


def parse_image_data(image_data, width, height, texture_format, version, platform, platform_blob=None, flip=True):
    return _parse_image_data(image_data, width, height, texture_format, version, platform, platform_blob, flip)


def _parse_image_data(image_data, width, height, texture_format, version, platform, platform_blob=None, flip=True, *, _guard=None, _reader=None):
    from PIL import Image
    if not width or not height:
        return Image.new("RGBA", (0, 0))
    if not image_data:
        raise ValueError("Texture2D has no image data")
    texture_format = TextureFormat(texture_format)
    from ..helpers import TextureSwizzler
    promoted = texture_format
    if TextureSwizzler.is_switch_swizzled(platform, platform_blob):
        promoted = {TF.RGB24: TF.RGBA32, TF.BGR24: TF.BGRA32}.get(texture_format, texture_format)
    selected = CONV_TABLE.get(promoted)
    if promoted not in CONV_TABLE or promoted not in _DEFAULT_CONV_TABLE or selected != _DEFAULT_CONV_TABLE.get(promoted):
        from .._buffers import snapshot_bytes
        guard = _native.TreeTraversal() if _guard is None else _guard
        image_data = snapshot_bytes(image_data, guard)
        blob = None if platform_blob is None else snapshot_bytes(platform_blob, guard)
        data, padded_width, padded_height, prepared_format = _native._texture_preprocess(
            image_data, width, height, int(texture_format),
            tuple(version[:2]) if "Crunched" in texture_format.name else (0, 0), int(platform), blob, _reader)
        if not TextureSwizzler.is_switch_swizzled(platform, platform_blob):
            padded_width, padded_height = get_compressed_image_size(width, height, promoted)
        guard.step(0, len(data) + padded_width * padded_height * 8 + 1024, len(data))
        if TF(prepared_format) not in CONV_TABLE:
            raise NotImplementedError(f"Not implemented texture format: {TF(prepared_format).name}")
        func, args = CONV_TABLE[TF(prepared_format)]
        image = func(data, padded_width, padded_height, *args)
        if image and (width, height) != (padded_width, padded_height):
            image = image.crop((0, 0, width, height))
    else:
        image = _decode(image_data, width, height, texture_format, version, platform, platform_blob,
                        _guard=_guard, _reader=_reader)
    return image.transpose(Image.Transpose.FLIP_TOP_BOTTOM) if image and flip else image


def get_image_from_texture2d(texture_2d, flip=True):
    from ..classes import _images
    guard = _images._guard(texture_2d)
    getter = texture_2d.get_image_data
    data = getter(_guard=guard) if getattr(getter, "__func__", None) is _images.get_image_data else getter()
    return _parse_image_data(data, texture_2d.m_Width, texture_2d.m_Height,
        texture_2d.m_TextureFormat, getattr(texture_2d.object_reader, "version", (0, 0, 0, 0)),
        getattr(texture_2d.object_reader, "platform", BuildTarget.UnknownPlatform),
        getattr(texture_2d, "m_PlatformBlob", None), flip,
        _guard=guard, _reader=_images._reader(texture_2d))


def image_to_texture2d(img, target_texture_format, platform=BuildTarget.UnknownPlatform, platform_blob=None, flip=True):
    from PIL import Image
    from ..classes._images import _encode
    guard = _native.TreeTraversal()
    dummy = SimpleNamespace(object_reader=None)
    if not flip:
        img = img.transpose(Image.Transpose.FLIP_TOP_BOTTOM)
    data, texture_format, _ = _encode(dummy, img, target_texture_format, platform, platform_blob, guard)
    return data, TF(texture_format)


def compress_etcpak(data, width, height, target_texture_format):
    supported = (TF.DXT1, TF.DXT1Crunched, TF.DXT5, TF.DXT5Crunched, TF.BC4, TF.BC5, TF.BC7,
                 TF.ETC_RGB4, TF.ETC_RGB4Crunched, TF.ETC_RGB4_3DS, TF.ETC2_RGB,
                 TF.ETC2_RGBA8, TF.ETC2_RGBA8Crunched, TF.ETC2_RGBA1)
    if target_texture_format not in supported:
        raise NotImplementedError(f"etcpak has no compress function for {target_texture_format.name}")
    from ..classes._images import _pack_format
    target, _ = _pack_format(target_texture_format, "RGBA")
    return _native._image_encode(data, width, height, "RGBA", target, 0, None, None)[0]


def compress_astc(data, width, height, target_texture_format):
    block_size = TEXTURE_FORMAT_BLOCK_SIZE_TABLE[target_texture_format]
    assert block_size is not None, f"failed to get block size for {target_texture_format.name}"
    image = SimpleNamespace(data_type=0, dim_x=width, dim_y=height, dim_z=1, data=data)
    return get_astc_context(block_size).compress(image, SimpleNamespace(r=0, g=1, b=2, a=3))


def swap_bytes_for_xbox(image_data):
    image_data = bytearray(image_data)
    for i in range(0, len(image_data), 2):
        image_data[i:i + 2] = image_data[i:i + 2][::-1]
    return image_data


def calculate_astc_compressed_size(width, height, block_size):
    return ((width + block_size[0] - 1) // block_size[0]) * ((height + block_size[1] - 1) // block_size[1]) * 16


def pad_image(img, pad_width, pad_height):
    from PIL import Image
    width, height = img.size
    if (width, height) == (pad_width, pad_height):
        return img
    out = Image.new(img.mode, (pad_width, pad_height))
    out.paste(img, (0, 0))
    if pad_width != width:
        out.paste(img.crop((width - 1, 0, width, height)).resize((pad_width - width, height), Image.Resampling.NEAREST), (width, 0))
    if pad_height != height:
        out.paste(img.crop((0, height - 1, width, height)).resize((width, pad_height - height), Image.Resampling.NEAREST), (0, height))
    if pad_width != width and pad_height != height:
        out.paste(Image.new(img.mode, (pad_width - width, pad_height - height), img.getpixel((width - 1, height - 1))), (width, height))
    return out
