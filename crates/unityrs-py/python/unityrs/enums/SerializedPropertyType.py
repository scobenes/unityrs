# Ported from UnityPy 1.25.2 (MIT): enums/SerializedPropertyType.py:1-9.
# See THIRD_PARTY_LICENSES for the UnityPy notice.
from enum import IntEnum


class SerializedPropertyType(IntEnum):
    kColor = 0
    kVector = 1
    kFloat = 2
    kRange = 3
    kTexture = 4
