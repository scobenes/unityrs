# Ported from UnityPy 1.25.2 (MIT): UnityPy/classes/__init__.py. See LICENSE-UNITYPY and THIRD_PARTY_LICENSES.
"""Runtime TPK records and the base/math classes (PARITY sections 9.2, 14.1)."""
from .generated import *
from .Object import Object as Object
from .PPtr import PPtr as PPtr
from .UnknownObject import UnknownObject as UnknownObject
from ._exports import install as _install_exports
from ._images import install as _install_images
from ._gameobject import install as _install_gameobject

_install_exports(globals())
_install_images(globals())
_install_gameobject(globals())
del _install_exports, _install_images, _install_gameobject
