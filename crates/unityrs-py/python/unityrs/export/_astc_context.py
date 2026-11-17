# Ported from UnityPy 1.25.2 (MIT): export/Texture2DConverter.py:356-387. See LICENSE-UNITYPY and THIRD_PARTY_LICENSES.
"""Thread-keyed context facade over the metered native ASTC codecs."""
from functools import lru_cache
from types import SimpleNamespace
from .. import _native


class ASTCError(Exception):
    pass


class _ASTCContext:
    def __init__(self, block_size):
        if block_size not in ((4, 4), (5, 4), (5, 5), (6, 5), (6, 6), (8, 5), (8, 6),
                              (8, 8), (10, 5), (10, 6), (10, 8), (10, 10), (12, 10), (12, 12)):
            raise ASTCError("ASTCENC_ERR_BAD_BLOCK_SIZE")
        self.config = SimpleNamespace(profile=1, block_x=block_size[0], block_y=block_size[1],
                                      block_z=1, flags=2)
        self.threads = 1
        self._block_size = block_size

    def _validate(self, image, swizzle):
        if image.dim_z != 1 or int(image.data_type) != 0:
            raise NotImplementedError("ASTC context supports 2D U8 images")
        if tuple(int(getattr(swizzle, c)) for c in "rgba") != (0, 1, 2, 3):
            raise NotImplementedError("ASTC context supports RGBA swizzle")

    def compress(self, image, swizzle):
        self._validate(image, swizzle)
        return _native._astc_codec(image.data, image.dim_x, image.dim_y, self._block_size, True)

    def decompress(self, data, image, swizzle):
        self._validate(image, swizzle)
        image.data = _native._astc_codec(data, image.dim_x, image.dim_y, self._block_size, False)
        return image


@lru_cache(maxsize=128)
def _get_astc_context(ident, block_size):
    # The key deliberately includes ident even though codec state is native and
    # operation-local: identity and cache statistics are public extension hooks.
    return _ASTCContext(block_size)

