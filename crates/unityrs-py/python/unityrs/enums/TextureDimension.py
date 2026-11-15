# Ported from UnityPy 1.25.2 (MIT): enums/TextureDimension.py:1-13.
# See THIRD_PARTY_LICENSES for the UnityPy notice.
from enum import IntEnum


class TextureDimension(IntEnum):
    kTexDimUnknown = -1
    kTexDimNone = 0
    kTexDimAny = 1
    kTexDim2D = 2
    kTexDim3D = 3
    kTexDimCUBE = 4
    kTexDim2DArray = 5
    kTexDimCubeArray = 6
    kTexDimForce32Bit = 2147483647
