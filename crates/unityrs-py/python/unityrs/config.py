# Ported from UnityPy 1.25.2 (MIT): UnityPy/config.py. See THIRD_PARTY_LICENSES.
"""Mutable loader settings and fallback warnings (config.py:1-40)."""

import warnings

from .exceptions import UnityVersionFallbackError, UnityVersionFallbackWarning

FALLBACK_UNITY_VERSION = None
SERIALIZED_FILE_PARSE_TYPETREE = True
warnings.simplefilter("once", UnityVersionFallbackWarning)
_fallback_registries = ({}, {})


def get_fallback_version():
    if not isinstance(FALLBACK_UNITY_VERSION, str):
        raise UnityVersionFallbackError(
            "No valid Unity version found, and the fallback version is not correctly configured. "
            "Please explicitly set the value of UnityPy.config.FALLBACK_UNITY_VERSION."
        )
    warnings.warn(
        f"No valid Unity version found, defaulting to UnityPy.config.FALLBACK_UNITY_VERSION ({FALLBACK_UNITY_VERSION})",
        category=UnityVersionFallbackWarning,
        stacklevel=2,
    )
    return FALLBACK_UNITY_VERSION


def _load(native, method, *args):
    """Initialize settings; warning callbacks refresh them before parsing resumes."""
    native.configure(
        # A placeholder enables the lazy resolver callback. The Python setting
        # is fetched and converted by _warn_fallback only when it is needed.
        b"",
        SERIALIZED_FILE_PARSE_TYPETREE is not False,
    )
    try:
        return method(*args)
    finally:
        native.take_fallback_warnings()


def _warn_fallback(_value, bundle):
    """Called only during unlocked version resolution, before publication."""
    value = FALLBACK_UNITY_VERSION
    if not isinstance(value, str):
        # Validation precedes warning delivery, as in get_fallback_version.
        return get_fallback_version()
    kind = "BundleFile" if bundle else "SerializedFile"
    warnings.warn_explicit(
        f"No valid Unity version found, defaulting to UnityPy.config.FALLBACK_UNITY_VERSION ({value})",
        category=UnityVersionFallbackWarning,
        filename=__file__, lineno=1, module=f"unityrs.files.{kind}",
        registry=_fallback_registries[bool(bundle)],
    )
    current = FALLBACK_UNITY_VERSION
    if not isinstance(current, str):
        from .helpers.UnityVersion import UnityVersion
        UnityVersion.from_str(current)
    return current.encode("utf-8", "surrogateescape"), SERIALIZED_FILE_PARSE_TYPETREE is not False
