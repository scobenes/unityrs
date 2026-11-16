# Ported from UnityPy 1.25.2 (MIT): classes/legacy_patch/Sprite.py and export/SpriteHelper.py. See LICENSE-UNITYPY and THIRD_PARTY_LICENSES.
"""Sprite atlas selection and cache, export/SpriteHelper.py:44-127."""

from contextlib import nullcontext

from .. import _native
from ..enums import ClassIDType
from ._images import _guard, _reader, get_image


def get_sprite_image(self):
    from PIL import Image
    from .generated import SpriteAtlasData

    atlas = None
    guard = _guard(self)
    if self.m_SpriteAtlas:
        atlas = self.m_SpriteAtlas.deref_parse_as_object()
    elif self.m_AtlasTags:
        assert self.assets_file, "Sprite assets file is not set!"
        for obj in self.assets_file.objects.values():
            guard.step(0, 0, 1)
            if obj.type == ClassIDType.SpriteAtlas:
                name = obj.peek_name()
                if name == self.m_AtlasTags[0]:
                    atlas = obj.parse_as_object()
                    break
                atlas = None
    if atlas:
        def matching():
            for key, value in atlas.m_RenderDataMap:
                guard.step(0, 0, 1)
                if key == self.m_RenderDataKey:
                    yield value
        rd = next(matching())
        assert isinstance(rd, SpriteAtlasData), "SpriteAtlasData not found!"
    else:
        rd = self.m_RD
    texture, alpha = rd.texture, rd.alphaTexture
    assert self.assets_file, "Sprite assets file is not set!"
    cache = self.assets_file._cache
    key = (texture.path_id, alpha.path_id) if alpha else texture.path_id
    environment = getattr(self.assets_file, 'environment', None)
    with (environment._lock if environment is not None else nullcontext()):
        if key not in cache:
            original = get_image(texture.deref_parse_as_object())
            guard.step(0, original.width * original.height * 8 + 1024, original.width * original.height)
            original = original.transpose(Image.Transpose.FLIP_TOP_BOTTOM)
            if alpha:
                alpha_image = get_image(alpha.deref_parse_as_object())
                guard.step(0, alpha_image.width * alpha_image.height * 8 + 1024,
                           alpha_image.width * alpha_image.height)
                alpha_image = alpha_image.transpose(Image.Transpose.FLIP_TOP_BOTTOM)
                original = Image.merge("RGBA", (*original.split()[:3], alpha_image.split()[0]))
            cache[key] = original
        original = cache[key]
    # Sheet reads flipped texture rows. Its stored image is the reverse of
    # UnityPy's cached unflipped image, including user cache replacements.
    guard.step(0, original.width * original.height * 16 + 1024, original.width * original.height)
    flipped = original.transpose(Image.Transpose.FLIP_TOP_BOTTOM)
    rect = rd.textureRect
    settings = rd.settingsRaw
    version = (0, 0, 0, 0, 0)
    if not ((settings >> 1) & 1):
        assert self.object_reader, "Sprite object reader is not set!"
        from itertools import islice
        version = (tuple(islice(getattr(self.object_reader, "version", ()), 5)) + (0,) * 5)[:5]
    mode, width, height, pixels = _native._sprite_render(
        flipped.tobytes(), flipped.mode, flipped.width, flipped.height,
        (rect.x, rect.y, rect.width, rect.height), settings, self.m_PixelsToUnits,
        self, _reader(self), version)
    guard.step(0, len(pixels) + 1024, len(pixels))
    return Image.frombytes(mode, (width, height), pixels)
