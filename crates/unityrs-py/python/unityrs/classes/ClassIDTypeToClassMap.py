# Ported from UnityPy 1.25.2 (MIT): UnityPy/classes/ClassIDTypeToClassMap.py. See LICENSE-UNITYPY and THIRD_PARTY_LICENSES.
"""Resolve IDs through TPK class names, including renamed enum members.

The installed map is checked at test time. No TPK-derived names are stored.
"""
from ..enums.ClassIDType import ClassIDType
from . import generated
from ._generator import TPK

CIT = ClassIDType


def _mapping():
    result = {}
    for member in ClassIDType:
        entries = TPK.ClassInformation.get(int(member), ())
        name = next((TPK.StringBuffer[entry.Name] for _, entry in reversed(entries) if entry), member.name)
        # Compatibility spellings in the MIT enum/map: prefer current TPK
        # names; absent IDs may retain a plural or Manager suffix in the enum.
        candidates = (member.name, name, member.name.removesuffix('Legacy'),
                      member.name.removesuffix('s'), member.name.removesuffix('Manager') + 'Settings')
        cls = next((getattr(generated, n) for n in candidates
                    if isinstance(getattr(generated, n, None), type)), None)
        if isinstance(cls, type):
            result[member] = cls
    return result


ClassIDTypeToClassMap = _mapping()
__all__ = ['ClassIDTypeToClassMap']
