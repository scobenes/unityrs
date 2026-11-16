# Ported from UnityPy 1.25.2 (MIT): UnityPy/classes/legacy_patch/GameObject.py. See LICENSE-UNITYPY and THIRD_PARTY_LICENSES.
"""Component convenience accessors (classes/legacy_patch/GameObject.py:9-33)."""
from ..enums import ClassIDType


def _components(self):
    if not self.m_Component:
        return []
    # Legacy entries contain the component PPtr itself.
    old = isinstance(self.m_Component[0], tuple)
    reader = self.object_reader
    guard = reader._native.tree_traversal() if reader is not None else None
    result = []
    for entry in self.m_Component:
        if guard is not None:
            guard.step(0, 64, 1)
        if old:
            _, component = entry
            result.append(component)
        else:
            result.append(entry.component)
    return result


def _get_component(self, kind):
    for component in self.m_Components:
        if component.type == kind:
            return component
    return None


def install(namespace):
    cls = namespace['GameObject']
    cls.m_Components = property(_components)
    for name in ('Animator', 'Animation', 'Transform', 'MeshRenderer', 'SkinnedMeshRenderer', 'MeshFilter'):
        kind = getattr(ClassIDType, name)
        setattr(cls, 'm_' + name, property(lambda self, kind=kind: _get_component(self, kind)))
