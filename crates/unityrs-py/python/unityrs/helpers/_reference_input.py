# Ported from UnityPy 1.25.2 (MIT): UnityPyBoost/TypeTreeHelper.cpp:585-681. See LICENSE-UNITYPY and THIRD_PARTY_LICENSES.
"""Current caller-owned reference descriptors, streamed into charged native storage."""
from ._bounded_tree import walk, text_bound


def reference_input(root, assetsfile, guard):
    # Ordinary trees must not observe otherwise unused assets-file attributes.
    if not any(_is_reference(node) for node, _ in walk(root, guard)):
        return None
    if assetsfile is None:
        return (0, None)
    references = getattr(assetsfile, 'ref_types', None)
    if not isinstance(references, list):
        return (1, None)
    return (2, _rows(references, guard))


def _is_reference(node):
    # Boost retains its constructor-time type cache and compares C strings.
    # Inspect a fixed prefix without copying a potentially large type string.
    ty = node._cached_type
    return (str.startswith(ty, 'ReferencedObjectData')
            and (str.__len__(ty) == 20 or str.__getitem__(ty, 20) == '\0'))


def _rows(references, guard):
    # The accelerated oracle uses the concrete list storage, bypassing overrides.
    for index in range(list.__len__(references)):
        guard.step(0, 512, 1)
        reference = list.__getitem__(references, index)
        names = (reference.m_ClassName, reference.m_NameSpace, reference.m_AssemblyName)
        for value in names:
            if isinstance(value, str):
                guard.step(0, 256 + text_bound(value), text_bound(value))
        node = reference.node
        yield (*names, None if node is None else node._native_rows(_guard=guard))
