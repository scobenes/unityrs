# Ported from UnityPy 1.25.2 (MIT): UnityPy/exceptions.py. See THIRD_PARTY_LICENSES.
"""Public exception classes (exceptions.py:1-14)."""

from ._native import TypeTreeError


class UnityVersionFallbackError(Exception):
    pass


class UnityVersionFallbackWarning(UserWarning):
    pass
