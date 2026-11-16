"""Charged staging for extractor buffers and traversal."""
from .._native import TreeTraversal


def guard_for(obj=None):
    native = getattr(obj, "_native", None)
    if native is None:
        reader = getattr(obj, "object_reader", None)
        native = getattr(reader, "_native", None)
    if native is None:
        native = getattr(getattr(obj, "assetsfile", None), "_native", None)
    return native.tree_traversal() if native is not None else TreeTraversal()


def text(guard, value):
    if isinstance(value, str):
        size = str.__len__(value)
        guard.step(0, size * 8 + 256, size)


def json_bound(guard, root):
    """Reserve the JSON encoder's staging before encoding, including indent."""
    guard.step(0, 512, 1)
    active = set()
    stack = [(root, 0, False)]
    while stack:
        value, depth, leaving = stack.pop()
        if leaving:
            active.remove(id(value))
            continue
        guard.step(depth, 1024 + depth * 16, 1)
        if isinstance(value, str):
            guard.step(depth, str.__len__(value) * 24 + 256, str.__len__(value))
        elif isinstance(value, (list, tuple, dict)):
            if id(value) in active:
                raise ValueError("Circular reference detected")
            active.add(id(value))
            guard.step(depth, len(value) * 512 + 256, len(value))
            stack.append((value, depth, True))
            if isinstance(value, dict):
                for key, child in value.items():
                    stack.append((key, depth + 1, False))
                    stack.append((child, depth + 1, False))
            else:
                for child in value:
                    stack.append((child, depth + 1, False))
