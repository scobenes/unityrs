# Ported from UnityPy 1.25.2 (MIT): UnityPy/enums/ClassIDType.py. See LICENSE-UNITYPY and THIRD_PARTY_LICENSES.
from .._native import _class_ids
from .ExtendableEnum import ExtendableEnum

ClassIDType = ExtendableEnum('ClassIDType', _class_ids, module=__name__)
