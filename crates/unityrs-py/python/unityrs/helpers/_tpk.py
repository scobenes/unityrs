# Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/TypeTreeNode.py, UnityPy/helpers/Tpk.py. See THIRD_PARTY_LICENSES.
"""Runtime tables (helpers/TypeTreeNode.py:308-328, helpers/Tpk.py:472-481).

Locate a user-supplied TPK without importing UnityPy or shipping its data.
"""

import importlib.util
import os
from pathlib import Path
from threading import RLock

from .._native import NativeTpk, mit_common_strings

_lock = RLock()
_providers = {}


def path():
    configured = os.environ.get("UNITYRS_TPK")
    if configured:
        return configured
    spec = importlib.util.find_spec("UnityPy")
    if spec is not None and spec.submodule_search_locations:
        for directory in spec.submodule_search_locations:
            candidate = Path(directory) / "resources" / "lzma.tpk"
            if candidate.is_file():
                return str(candidate)
    return None


def provider():
    source = path()
    if source is None:
        return None
    with _lock:
        if source not in _providers:
            _providers[source] = NativeTpk(source)
        return _providers[source]


def common_strings(version=None, *, _guard=None):
    from ._bounded_tree import guard_for
    guard = guard_for(guard=_guard)
    source = provider()
    if source is None:
        return mit_common_strings(guard)
    strings = source.common_strings(guard)
    if version:
        from .UnityVersion import UnityVersion
        count = None
        for candidate, value in source.common_string_versions(guard):
            guard.step(0, 512, 1)
            if version >= UnityVersion(candidate):
                count = value
            else:
                break
        if not count:
            raise ValueError("Could not find exact version")
        result = {}
        for index, (offset, text) in enumerate(strings.items()):
            if index >= count:
                break
            guard.step(0, 256, 1)
            result[offset] = text
        return result
    return strings
