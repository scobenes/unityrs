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
