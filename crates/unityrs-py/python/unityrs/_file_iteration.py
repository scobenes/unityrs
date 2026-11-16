# Ported from UnityPy 1.25.2 (MIT): UnityPy/files/File.py. See LICENSE-UNITYPY and THIRD_PARTY_LICENSES.
"""Tier B/C iteration corrections to files/File.py:41-74 (PARITY 19.2)."""


def assets(file):
    from .files import File, SerializedFile
    guard = file._native.tree_traversal()
    guard.step(0, 256, 1)
    pending = [(iter((file,)), 0)]
    while pending:
        children, depth = pending[-1]
        try:
            child = next(children)
        except StopIteration:
            pending.pop()
            continue
        guard.step(depth, 128, 1)
        if isinstance(child, SerializedFile):
            yield child
        elif isinstance(child, File):
            guard.step(depth + 1, 128, 1)
            pending.append((iter(child.files.values()), depth + 1))


def objects(file, obj_types=None):
    from .files import File, ObjectReader, SerializedFile
    guard = file._native.tree_traversal()
    guard.step(0, 256, 1)
    filtered = obj_types is not None and len(obj_types) != 0
    pending = [(iter((file,)), 0)]
    while pending:
        children, depth = pending[-1]
        try:
            child = next(children)
        except StopIteration:
            pending.pop()
            continue
        guard.step(depth, 128, 1)
        if isinstance(child, ObjectReader):
            if not filtered or child.type in obj_types:
                yield child
        elif isinstance(child, File):
            # A serialized file aliases files to objects, including mutable
            # caller mappings, so the same walk handles both public surfaces.
            guard.step(depth + (not isinstance(child, SerializedFile)), 128, 1)
            pending.append((iter(child.files.values()), depth + (not isinstance(child, SerializedFile))))
