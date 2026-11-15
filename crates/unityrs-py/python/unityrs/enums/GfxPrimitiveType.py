# Ported from UnityPy 1.25.2 (MIT): enums/GfxPrimitiveType.py:1-10.
# See THIRD_PARTY_LICENSES for the UnityPy notice.
from enum import IntEnum


class GfxPrimitiveType(IntEnum):
    kPrimitiveTriangles = 0
    kPrimitiveTriangleStrip = 1
    kPrimitiveQuads = 2
    kPrimitiveLines = 3
    kPrimitiveLineStrip = 4
    kPrimitivePoints = 5
