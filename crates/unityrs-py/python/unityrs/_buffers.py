"""Charge byte coercions by buffer size or incremental iterable growth."""
import operator


def snapshot_bytes(value, guard):
    guard.step(0, 256, 1)
    try:
        view = memoryview(value)
    except TypeError:
        view = None
    if view is not None:
        size = view.nbytes
    elif isinstance(value, int):
        size = max(0, int.__index__(value))
    elif hasattr(value, "__bytes__"):
        # User-defined callbacks own their allocations, like property getters.
        result = bytes(value)
        guard.step(0, len(result) + 256, len(result))
        return result
    elif hasattr(value, "__index__"):
        return snapshot_bytes(operator.index(value), guard)
    elif type(value) in (list, tuple, range):
        size = len(value)
    elif isinstance(value, str):
        return bytes(value)  # Preserve bytes()'s missing-encoding exception.
    else:
        # Never trust a custom length or length hint to bound bytes(iterable).
        # Keep only charged storage, then separately charge the final copy.
        try:
            iterator = iter(value)
        except TypeError:
            return bytes(value)
        staged = bytearray()
        for item in iterator:
            guard.step(0, 2, 1)
            staged.extend(bytes((item,)))
        guard.step(0, len(staged) + 256, len(staged))
        return bytes(staged)
    guard.step(0, min(size + 256, (1 << 64) - 1), min(size, (1 << 64) - 1))
    return bytes(value)
