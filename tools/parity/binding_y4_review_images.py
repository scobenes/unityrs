"""Mipmap arguments retain Python's unbounded integers and mutation order."""

from binding_y4_images import capture, texture


def observe_mipmap_arguments(module):
    from PIL import Image

    result = {}
    source = Image.frombytes("RGBA", (8, 8), bytes(range(256)))
    for label, count in (
        ("huge", 2**32), ("unbounded", 2**128), ("negative", -(2**128)),
        ("one_float", 1.0), ("two_float", 2.0), ("zero_float", 0.0),
        ("false", False), ("true", True), ("none", None), ("text", "2"),
    ):
        obj = texture(module)
        outcome = capture(lambda: obj.set_image(source, 4, count))
        result[label] = {
            "outcome": outcome, "size": [obj.m_Width, obj.m_Height],
            "mips": [obj.m_MipMap, obj.m_MipCount],
            "data": obj.image_data.hex(), "complete": obj.m_CompleteImageSize,
        }
    return result


def observe_buffer_budgets(module, corrected=False):
    from binding_y4_review_exports import owner

    result = {}
    for field in ("image_data", "m_PlatformBlob"):
        if corrected and module.__name__ == "UnityPy":
            result[field] = "MemoryError"
            continue
        if module.__name__ == "unityrs":
            from binding_cases import cases
            env = module.Environment(budget=module.Budget(1_000_000))
            env.load_file(next(data for name, data in cases() if name == "text"), name="owner.assets")
            reader = env.objects[0]
        else:
            env, reader = owner(module)
        obj = texture(module, object_reader=reader)
        setattr(obj, field, memoryview(bytes(2_000_000)).cast("B", (1, 2_000_000)))
        try:
            obj.image
            result[field] = None
        except Exception as error:
            result[field] = type(error).__name__
    return result
