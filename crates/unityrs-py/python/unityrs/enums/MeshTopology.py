# Ported from UnityPy 1.25.2 (MIT): enums/MeshTopology.py:1-10.
# See THIRD_PARTY_LICENSES for the UnityPy notice.
from enum import IntEnum


class MeshTopology(IntEnum):
    Triangles = 0
    TriangleStrip = 1  # deprecated
    Quads = 2
    Lines = 3
    LineStrip = 4
    Points = 5
