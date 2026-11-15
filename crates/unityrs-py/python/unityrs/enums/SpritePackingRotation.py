# Ported from UnityPy 1.25.2 (MIT): enums/SpritePackingRotation.py:1-9.
# See THIRD_PARTY_LICENSES for the UnityPy notice.
from enum import IntEnum


class SpritePackingRotation(IntEnum):
    kSPRNone = 0
    kSPRFlipHorizontal = 1
    kSPRFlipVertical = 2
    kSPRRotate180 = 3
    kSPRRotate90 = 4
