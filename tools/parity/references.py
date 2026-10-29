"""The independent references for the TextureFormats UnityPy raises on (Tier B, PARITY.md section 17.1).

Each Tier B format gets one reference: a closed form stated in section 17.1 and written here,
or, for BC6H, a named decoder at a pinned version. `decode(name, data, width, height)` gives
what unityrs must return for a texture of that format: the Pillow mode and the pixels, top
row first after the vertical flip UnityPy applies to every image (section 17). The P3
divergence test for each format compares unityrs's output with it.

The closed forms need the standard library only. BC6H's reference, texture2ddecoder 1.0.5,
is in the oracle image (a dependency of UnityPy, which does not call it for BC6H).

The rules:
- a half or float channel (IEEE 754 binary16 or binary32, little-endian) becomes
  `floor(min(max(v, 0), 1) * 255 + 0.5)`, computed exactly in f64; NaN gives 0. 0.5 gives
  128, the only tie in [0, 1];
- an unsigned n-bit integer channel becomes its high byte, `v >> (n - 8)`, the reduction
  Pillow 9.5.0's 16-bit raw modes make (the RGB48 rule, section 19.2);
- a signed n-bit integer channel is offset by 2^(n-1) first (two's complement to offset
  binary: the most negative value gives 0, 0 gives 128), then reduced the same way;
- a format with one channel returns RGB `(v, 0, 0)` and one with two `(r, g, 0)`, as UnityPy
  returns `R8` and `BC5`;
- RGB9e5Float is the shared-exponent layout of `DXGI_FORMAT_R9G9B9E5_SHAREDEXP`: red in
  bits 0-8, green 9-17, blue 18-26, the exponent in 27-31, each channel
  `mantissa * 2^(exponent - 24)`, then the float rule;
- YUY2 is the FourCC `YUY2` layout, `Y0 U Y1 V` per pair of pixels, converted with the
  integer ITU-R BT.601 limited-range ("studio swing") formula below.
"""

import math
import struct

# Unsupported here, and Tier U in PARITY.md: no sample or document fixes ARGBFloat's channel order
# (its name says A, R, G, B; UnityPy's CONV_TABLE reorders it as if stored B, G, R, A).
UNSUPPORTED = {"ARGBFloat": "the channel order is unknown"}


def float_to_byte(value: float) -> int:
    """A half or float channel to 8 bits: clamped to [0, 1], times 255, rounded half up; NaN is 0."""
    if math.isnan(value):
        return 0
    return math.floor(min(max(value, 0.0), 1.0) * 255 + 0.5)


def unsigned_to_byte(value: int, bits: int) -> int:
    return value >> (bits - 8)


def signed_to_byte(value: int, bits: int) -> int:
    return (value + (1 << (bits - 1))) >> (bits - 8)


def _clip(value: int) -> int:
    return min(max(value, 0), 255)


def yuv_to_rgb(y: int, u: int, v: int) -> tuple[int, int, int]:
    """ITU-R BT.601, limited range (Y 16-235, U and V 16-240), in the 8-bit integer form."""
    c, d, e = y - 16, u - 128, v - 128
    return (
        _clip((298 * c + 409 * e + 128) >> 8),
        _clip((298 * c - 100 * d - 208 * e + 128) >> 8),
        _clip((298 * c + 516 * d + 128) >> 8),
    )


def rgb9e5_to_floats(word: int) -> tuple[float, float, float]:
    scale = 2.0 ** ((word >> 27 & 0x1F) - 24)
    return tuple((word >> shift & 0x1FF) * scale for shift in (0, 9, 18))


def _float_channels(values):
    return [float_to_byte(v) for v in values]


def _integer_channels(bits, signed):
    reduce = signed_to_byte if signed else unsigned_to_byte
    return lambda values: [reduce(v, bits) for v in values]


# name: (struct code of one pixel, channel conversion, mode)
_PIXEL_FORMATS = {
    "RHalf": ("<e", _float_channels, "RGB"),
    "RGHalf": ("<2e", _float_channels, "RGB"),
    "RFloat": ("<f", _float_channels, "RGB"),
    "RGFloat": ("<2f", _float_channels, "RGB"),
    "RGBFloat": ("<3f", _float_channels, "RGB"),
    "RGBAFloat": ("<4f", _float_channels, "RGBA"),
    "RG16": ("<2B", _integer_channels(8, False), "RGB"),
    "R16": ("<H", _integer_channels(16, False), "RGB"),
    "RG32": ("<2H", _integer_channels(16, False), "RGB"),
    "RGBA64": ("<4H", _integer_channels(16, False), "RGBA"),
    "R8_SIGNED": ("<b", _integer_channels(8, True), "RGB"),
    "RG16_SIGNED": ("<2b", _integer_channels(8, True), "RGB"),
    "RGB24_SIGNED": ("<3b", _integer_channels(8, True), "RGB"),
    "RGBA32_SIGNED": ("<4b", _integer_channels(8, True), "RGBA"),
    "R16_SIGNED": ("<h", _integer_channels(16, True), "RGB"),
    "RG32_SIGNED": ("<2h", _integer_channels(16, True), "RGB"),
    "RGB48_SIGNED": ("<3h", _integer_channels(16, True), "RGB"),
    "RGBA64_SIGNED": ("<4h", _integer_channels(16, True), "RGBA"),
}

# Every Tier B format, with the mode its reference returns.
TIER_B = {name: mode for name, (_, _, mode) in _PIXEL_FORMATS.items()}
TIER_B.update(RGB9e5Float="RGB", YUY2="RGB", BC6H="RGBA")


def _pad(channels, mode):
    return tuple(channels) + (0,) * (len(mode) - len(channels))


def _flip(pixels, width, height):
    return [pixel for y in reversed(range(height)) for pixel in pixels[y * width : (y + 1) * width]]


def decode(name: str, data: bytes, width: int, height: int) -> tuple[str, list[tuple[int, ...]]]:
    """(mode, pixels) for a `width` x `height` texture of the Tier B format `name`, flipped as UnityPy flips."""
    if name in UNSUPPORTED:
        raise NotImplementedError(f"{name}: {UNSUPPORTED[name]}")
    mode = TIER_B[name]
    if name in _PIXEL_FORMATS:
        code, convert, _ = _PIXEL_FORMATS[name]
        size = struct.calcsize(code)
        pixels = [_pad(convert(struct.unpack_from(code, data, i * size)), mode) for i in range(width * height)]
    elif name == "RGB9e5Float":
        words = struct.unpack_from(f"<{width * height}I", data)
        pixels = [tuple(float_to_byte(v) for v in rgb9e5_to_floats(word)) for word in words]
    elif name == "YUY2":
        if width % 2:
            raise NotImplementedError("YUY2: an odd width")  # Tier U: no sample shows how one is stored
        pixels = []
        for i in range(width * height // 2):
            y0, u, y1, v = data[4 * i : 4 * i + 4]
            pixels += [yuv_to_rgb(y0, u, v), yuv_to_rgb(y1, u, v)]
    else:  # BC6H: decoded at the size padded to whole 4x4 blocks, then cropped, as UnityPy pads and crops
        import texture2ddecoder

        padded_w, padded_h = -(-width // 4) * 4, -(-height // 4) * 4
        bgra = texture2ddecoder.decode_bc6(data, padded_w, padded_h)
        pixels = [
            (bgra[i + 2], bgra[i + 1], bgra[i], bgra[i + 3])
            for y in range(height)
            for i in range(4 * y * padded_w, 4 * (y * padded_w + width), 4)
        ]
    return mode, _flip(pixels, width, height)
