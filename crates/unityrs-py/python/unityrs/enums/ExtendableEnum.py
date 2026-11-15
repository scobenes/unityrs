# Ported from UnityPy 1.25.2 (MIT): UnityPy/enums/ExtendableEnum.py. See LICENSE-UNITYPY and THIRD_PARTY_LICENSES.
from enum import IntEnum


class ExtendableEnum(IntEnum):
    @classmethod
    def _missing_(cls, value):
        if isinstance(value, int):
            member = int.__new__(cls, value)
            member._name_ = f'Unknown ({value})'
            member._value_ = value
            return cls._value2member_map_.setdefault(value, member)
        return None
