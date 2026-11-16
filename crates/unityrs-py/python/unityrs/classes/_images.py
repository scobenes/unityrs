# Ported from UnityPy 1.25.2 (MIT): classes/legacy_patch/Texture2D.py, classes/legacy_patch/Texture2DArray.py and export/Texture2DConverter.py. See LICENSE-UNITYPY and THIRD_PARTY_LICENSES.
"""Lazy optional Pillow facade over the metered Rust texture ports.

Texture fields and mutation ordering follow classes/legacy_patch/Texture2D.py:8-80.
Array extraction follows classes/legacy_patch/Texture2DArray.py:12-43.
"""

from .. import _native
from ..enums import TextureFormat
from .._buffers import snapshot_bytes


def _guard(obj):
    native = getattr(getattr(obj, "object_reader", None), "_native", None)
    return native.tree_traversal() if native is not None else _native.TreeTraversal()


def _reader(obj):
    return getattr(getattr(obj, "object_reader", None), "_native", None)


def _blob(obj):
    blob = getattr(obj, "m_PlatformBlob", None)
    if blob is not None:
        guard = _guard(obj)
        return snapshot_bytes(blob, guard)
    return None


def _resource(obj):
    stream = obj.m_StreamData
    file = obj.object_reader.assets_file
    guard = _guard(obj)
    size = str.__len__(stream.path) if isinstance(stream.path, str) else len(stream.path)
    guard.step(0, size * 4 + 256, size)
    return _native._image_resource(file.environment._native,
        file._native, stream.path.encode("utf-8", "surrogateescape"), stream.offset, stream.size)


def get_image_data(self):
    if self.image_data:
        return self.image_data
    if self.m_StreamData:
        return _resource(self)
    raise ValueError("No image data found")


def _decode(obj, data, width, height, texture_format, platform, blob, version):
    from PIL import Image

    # Converter ordering is export/Texture2DConverter.py:254-320.
    # The zero-size result precedes both data and enum validation.
    if not width or not height:
        return Image.new("RGBA", (0, 0))
    if not data:
        raise ValueError("Texture2D has no image data")
    texture_format = TextureFormat(texture_format)
    version = tuple(version[:2]) if "Crunched" in texture_format.name else (0, 0)
    guard = _guard(obj)
    data = snapshot_bytes(data, guard)
    mode, w, h, pixels = _native._image_decode(
        data, width, height, int(texture_format), version,
        int(platform), blob, _reader(obj))
    guard.step(0, len(pixels) + 1024, len(pixels))
    return Image.frombytes(mode, (w, h), pixels)


def get_image(self):
    from PIL import Image

    # PARITY 19.2's Tier B boundary precedes even a missing resource fetch.
    if self.m_Width == 0 or self.m_Height == 0:
        return Image.new("RGBA", (0, 0))
    reader = self.object_reader
    return _decode(self, self.get_image_data(), self.m_Width, self.m_Height,
                   self.m_TextureFormat, getattr(reader, "platform", 0), _blob(self),
                   getattr(reader, "version", (0, 0, 0, 0)))


def _pack_format(target, mode):
    """Converter dispatch, export/Texture2DConverter.py:118-234."""
    value = int(target)
    if value in (28, 29):
        value = {28: 10, 29: 12}[value]
    elif value in (46, 65):
        value = 47
    elif value in (34, 64):
        value = 34
    elif 66 <= value <= 71:
        value -= 18 if mode == "RGB" else 12
    if value == 1:
        return value, "A"
    if value in (63, 9, 15, 18, 41, 42):
        return 63, "R"
    if value in (7, 3, 8, 22, 30, 32, 35):
        return 3, "RGB"
    if value in (10, 12, 25, 26, 27, 34, 45, 47, 60) or 48 <= value <= 59:
        return value, "RGBA"
    return 4, "RGBA"


def _encode(obj, img, target, platform, blob, guard):
    from PIL import Image

    target, pack = _pack_format(target, img.mode)
    if platform == 38 and blob and len(blob) >= 12 and any(blob[8:12]) and pack == "RGB":
        pack = "RGBA"
    guard.step(0, img.width * img.height * 16 + 1024, img.width * img.height)
    flipped = img.transpose(Image.Transpose.FLIP_TOP_BOTTOM)
    try:
        raw = flipped.tobytes("raw", pack)
    except ValueError as error:
        if not str(error).startswith("No packer found from "):
            raise
        # PARITY 18.1: keep working packers (including LAB's A band and
        # RGB's hidden storage byte), and convert only the Tier C failures.
        if img.mode in ("BGR;15", "BGR;16", "BGR;24", "La"):
            raise NotImplementedError("unsupported set_image: Pillow cannot convert image mode to RGBA") from None
        guard.step(0, img.width * img.height * 12 + 1024, img.width * img.height)
        img = img.convert("RGBA")
        raw = img.transpose(Image.Transpose.FLIP_TOP_BOTTOM).tobytes("raw", pack)
    data, written = _native._image_encode(raw, img.width, img.height, pack, target, platform, blob,
                                          _reader(obj))
    return data, written, img


def set_image(self, img, target_format=None, mipmap_count=1):
    from PIL import Image

    if not target_format:
        target_format = self.m_TextureFormat
    if not isinstance(img, Image.Image):
        img = Image.open(img)
    target_format = TextureFormat(target_format)
    platform = self.object_reader.platform if self.object_reader is not None else 0
    guard = _guard(self)
    blob = _blob(self)
    img_data, tex_format, img = _encode(self, img, target_format, platform, blob, guard)
    old_size = self.m_Width, self.m_Height
    self.m_Width = img.width
    self.m_Height = img.height
    if mipmap_count > 1:
        # Validate Python's range protocol after updating dimensions. Invalid
        # counts retain those updates, matching the normal writer ordering.
        levels = range(mipmap_count - 1)
        if (platform == 38 and blob and len(blob) >= 12 and any(blob[8:12])
                and img.width >= 8 and img.height >= 8):
            # Valid Switch chains remain an atomic, explicit capability gap.
            self.m_Width, self.m_Height = old_size
            raise NotImplementedError("unsupported set_image: Switch mip chain")
        width, height = self.m_Width, self.m_Height
        re_img = img
        for i in levels:
            width //= 2
            height //= 2
            if width < 4 or height < 4:
                mipmap_count = i + 1
                break
            guard.step(0, width * height * 16 + len(img_data) + 1024, width * height)
            re_img = re_img.resize((width, height), Image.Resampling.BICUBIC)
            mip = _encode(self, re_img, target_format, 0, None, guard)[0]
            guard.step(0, len(img_data) + len(mip) + 256, 0)
            img_data += mip
    if self.m_MipMap is not None:
        self.m_MipMap = mipmap_count > 1
    if self.m_MipCount is not None:
        self.m_MipCount = mipmap_count
    self.image_data = img_data
    self.m_CompleteImageSize = len(img_data)
    self.m_TextureFormat = TextureFormat(tex_format)
    if self.m_StreamData is not None:
        self.m_StreamData.path = ""
        self.m_StreamData.offset = 0
        self.m_StreamData.size = 0


def get_images(self):
    from ..enums.GraphicsFormat import GRAPHICS_TO_TEXTURE_MAP, GraphicsFormat

    platform = getattr(self.object_reader, "platform", 0)
    if platform in (11, 38):
        raise NotImplementedError(f"unsupported Texture2DArray platform {platform}")
    graphics_format = GraphicsFormat(self.m_Format)
    texture_format = GRAPHICS_TO_TEXTURE_MAP.get(graphics_format)
    if 13 <= graphics_format <= 20 or 145 <= graphics_format <= 150:
        # Preserve the array port's documented 8-bit/HDR corrections.
        texture_format = _native._array_format(int(graphics_format))
    if not texture_format:
        raise NotImplementedError(f"GraphicsFormat {self.m_Format} not supported yet")
    image_data = self.image_data
    if image_data is None or (not image_data and self.m_StreamData and self.m_StreamData.path):
        image_data = _resource(self)
    texture_size = self.m_DataSize // self.m_Depth
    offsets = range(0, self.m_DataSize, texture_size)
    guard = _guard(self)
    guard.step(0, len(offsets) * 256 + 128, len(offsets))
    result = []
    for offset in offsets:
        guard.step(0, texture_size + max(0, self.m_Width) * max(0, self.m_Height) * 8 + 1024, texture_size)
        result.append(_decode(self, image_data[offset:offset + texture_size], self.m_Width,
                              self.m_Height, texture_format, 0, None, self.object_reader.version))
    return result


def install(namespace):
    from ._images_sprite import get_sprite_image

    texture = namespace.get("Texture2D")
    if texture is not None:
        texture.image = property(get_image, set_image)
        texture.set_image = set_image
        texture.get_image_data = get_image_data
    sprite = namespace.get("Sprite")
    if sprite is not None:
        sprite.image = property(get_sprite_image)
    array = namespace.get("Texture2DArray")
    if array is not None:
        array.images = property(get_images)
