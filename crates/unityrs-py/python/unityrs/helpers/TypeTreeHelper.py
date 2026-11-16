# Ported from UnityPy 1.25.2 (MIT): helpers/TypeTreeHelper.py:27-75, helpers/TypeTreeHelper.py:364-404. See LICENSE-UNITYPY and THIRD_PARTY_LICENSES.
"""Primitive extension tables shared with the public stream types."""
from ._primitive_maps import (
    FUNCTION_READ_MAP, FUNCTION_READ_MAP_ARRAY, FUNCTION_WRITE_MAP,
    kAlignBytes, clean_name, metaflag_is_aligned,
)

read_typetree_boost = None
