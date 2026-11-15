"""Charged, iterative helper expansion. Returned Python values own their memory."""
from .._native import TreeTraversal


def guard_for(node=None, limits=None, budget=None, guard=None):
    if guard is not None:
        return guard
    source_ref = getattr(node, "_tree_source", None)
    source = source_ref() if source_ref is not None else None
    if source is not None and limits is None and budget is None:
        return source.tree_traversal()
    return TreeTraversal(limits, budget)


def text_bound(value):
    # Bypass str subclass callbacks, including a forged __len__.
    return 4 * str.__len__(value)


def charge_node(guard, depth, ty, name):
    size = text_bound(ty) + text_bound(name)
    guard.step(depth + 1, 2048 + size * 4, 1 + size)


def walk(root, guard):
    active = set()
    path = []
    stack = [iter((root,))]
    while stack:
        node = next(stack[-1], None)
        if node is None:
            stack.pop()
            if path:
                active.remove(path.pop())
            continue
        if id(node) in active:
            raise ValueError("Type tree contains a cycle")
        depth = len(stack) - 1
        # Charge the path and output allowance before exposing this expansion.
        guard.step(depth + 1, 512, 1)
        charge_node(guard, depth, node.m_Type, node.m_Name)
        yield node, depth
        active.add(id(node))
        path.append(id(node))
        stack.append(iter(node.m_Children))


def read_bytes(reader, size, guard):
    if size < 0:
        length = getattr(reader, "Length", None)
        if length is None:
            length = len(reader.data)
        # Negative read sizes retain the reader's own slicing semantics.
        bound = max(0, length - reader.Position)
    else:
        bound = size
    guard.step(0, 256 + bound * 4, bound)
    return reader.read(size)


def read_string(reader, guard):
    start = reader.Position
    data = bytearray()
    for _ in range(32767):
        guard.step(0, 16, 1)
        char = reader.read(1)
        if char == b"\0":
            return data.decode("utf-8", "surrogateescape")
        if not char:
            reader.Position = start
            raise Exception("String not terminated")
        data.extend(char)
    length = getattr(reader, "Length", None)
    if length is None and hasattr(reader, "data"):
        length = len(reader.data)
    if length is not None and reader.Position >= length:
        reader.Position = start
        raise Exception("String not terminated")
    return data.decode("utf-8", "surrogateescape")
