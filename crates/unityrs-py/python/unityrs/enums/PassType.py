# Ported from UnityPy 1.25.2 (MIT): enums/PassType.py:1-7.
# See THIRD_PARTY_LICENSES for the UnityPy notice.
from enum import IntEnum


class PassType(IntEnum):
    kPassTypeNormal = 0
    kPassTypeUse = 1
    kPassTypeGrab = 2
