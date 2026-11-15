# Ported from UnityPy 1.25.2 (MIT): enums/FileType.py:1-9.
# See THIRD_PARTY_LICENSES for the UnityPy notice.
from enum import IntEnum


class FileType(IntEnum):
    AssetsFile = 0
    BundleFile = 1
    WebFile = 2
    ResourceFile = 9
    ZIP = 10
