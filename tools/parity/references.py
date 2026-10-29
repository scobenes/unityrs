"""The independent references for the TextureFormats UnityPy raises on (Tier B, PARITY.md section 17.1).

Each Tier B format gets one reference, stated in section 17.1 and written here: a closed form,
or, for BC6H, the decoding the Khronos Data Format Specification 1.3 gives in section 20.2.
`decode(name, data, width, height)` gives what unityrs must return for a texture of that
format: the Pillow mode and the pixels, top row first after the vertical flip UnityPy applies
to every image (section 17). The P3 divergence test for each format compares unityrs's output
with it. Everything here needs the standard library only.

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
  integer ITU-R BT.601 limited-range ("studio swing") formula below;
- BC6H is the unsigned form, decoded as the Khronos Data Format Specification 1.3, section
  20.2, decodes it (the same rules as ARB_texture_compression_bptc) into a half float per
  channel, then the half and float rule. Two decoders in the oracle image are not the
  reference, because each departs from that section (PARITY.md section 19.2):
  texture2ddecoder 1.0.5 unquantizes an endpoint equal to 2^(bits-1) to 0xFFFF, and
  Pillow 9.5.0 interpolates without the `+ 32` of the section's Equation 2.
"""

import math
import struct

# Unsupported here, and Tier U in PARITY.md: no sample or specification fixes ARGBFloat's channel order
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
TIER_B.update(RGB9e5Float="RGB", YUY2="RGB", BC6H="RGB")

# BC6H, unsigned: the Khronos Data Format Specification 1.3, section 20.2. Table 121, by mode number:
# (transformed endpoints, partition bits, endpoint bits, delta bits of red, green and blue).
BC6H_MODES = {
    0: (True, 5, 10, (5, 5, 5)),
    1: (True, 5, 7, (6, 6, 6)),
    2: (True, 5, 11, (5, 4, 4)),
    6: (True, 5, 11, (4, 5, 4)),
    10: (True, 5, 11, (4, 4, 5)),
    14: (True, 5, 9, (5, 5, 5)),
    18: (True, 5, 8, (6, 5, 5)),
    22: (True, 5, 8, (5, 6, 5)),
    26: (True, 5, 8, (5, 5, 6)),
    30: (False, 5, 6, (6, 6, 6)),
    3: (False, 0, 10, (10, 10, 10)),
    7: (True, 0, 11, (9, 9, 9)),
    11: (True, 0, 12, (8, 8, 8)),
    15: (True, 0, 16, (4, 4, 4)),
}
# Table 122 (Table.F of ARB_texture_compression_bptc): the bits of the block from bit 0 up. `v[a:b]` takes
# |a - b| + 1 bits into bits b, ..., a of v in that order, so `r0[10:15]` stores bit 15 of r0 first.
BC6H_LAYOUTS = {
    0: "m[1:0],g2[4],b2[4],b3[4],r0[9:0],g0[9:0],b0[9:0],r1[4:0],g3[4],g2[3:0],g1[4:0],b3[0],g3[3:0],b1[4:0],"
    "b3[1],b2[3:0],r2[4:0],b3[2],r3[4:0],b3[3]",
    1: "m[1:0],g2[5],g3[4],g3[5],r0[6:0],b3[0],b3[1],b2[4],g0[6:0],b2[5],b3[2],g2[4],b0[6:0],b3[3],b3[5],b3[4],"
    "r1[5:0],g2[3:0],g1[5:0],g3[3:0],b1[5:0],b2[3:0],r2[5:0],r3[5:0]",
    2: "m[4:0],r0[9:0],g0[9:0],b0[9:0],r1[4:0],r0[10],g2[3:0],g1[3:0],g0[10],b3[0],g3[3:0],b1[3:0],b0[10],b3[1],"
    "b2[3:0],r2[4:0],b3[2],r3[4:0],b3[3]",
    6: "m[4:0],r0[9:0],g0[9:0],b0[9:0],r1[3:0],r0[10],g3[4],g2[3:0],g1[4:0],g0[10],g3[3:0],b1[3:0],b0[10],b3[1],"
    "b2[3:0],r2[3:0],b3[0],b3[2],r3[3:0],g2[4],b3[3]",
    10: "m[4:0],r0[9:0],g0[9:0],b0[9:0],r1[3:0],r0[10],b2[4],g2[3:0],g1[3:0],g0[10],b3[0],g3[3:0],b1[4:0],b0[10],"
    "b2[3:0],r2[3:0],b3[1],b3[2],r3[3:0],b3[4],b3[3]",
    14: "m[4:0],r0[8:0],b2[4],g0[8:0],g2[4],b0[8:0],b3[4],r1[4:0],g3[4],g2[3:0],g1[4:0],b3[0],g3[3:0],b1[4:0],"
    "b3[1],b2[3:0],r2[4:0],b3[2],r3[4:0],b3[3]",
    18: "m[4:0],r0[7:0],g3[4],b2[4],g0[7:0],b3[2],g2[4],b0[7:0],b3[3],b3[4],r1[5:0],g2[3:0],g1[4:0],b3[0],g3[3:0],"
    "b1[4:0],b3[1],b2[3:0],r2[5:0],r3[5:0]",
    22: "m[4:0],r0[7:0],b3[0],b2[4],g0[7:0],g2[5],g2[4],b0[7:0],g3[5],b3[4],r1[4:0],g3[4],g2[3:0],g1[5:0],g3[3:0],"
    "b1[4:0],b3[1],b2[3:0],r2[4:0],b3[2],r3[4:0],b3[3]",
    26: "m[4:0],r0[7:0],b3[1],b2[4],g0[7:0],b2[5],g2[4],b0[7:0],b3[5],b3[4],r1[4:0],g3[4],g2[3:0],g1[4:0],b3[0],"
    "g3[3:0],b1[5:0],b2[3:0],r2[4:0],b3[2],r3[4:0],b3[3]",
    30: "m[4:0],r0[5:0],g3[4],b3[0],b3[1],b2[4],g0[5:0],g2[5],b2[5],b3[2],g2[4],b0[5:0],g3[5],b3[3],b3[5],b3[4],"
    "r1[5:0],g2[3:0],g1[5:0],g3[3:0],b1[5:0],b2[3:0],r2[5:0],r3[5:0]",
    3: "m[4:0],r0[9:0],g0[9:0],b0[9:0],r1[9:0],g1[9:0],b1[9:0]",
    7: "m[4:0],r0[9:0],g0[9:0],b0[9:0],r1[8:0],r0[10],g1[8:0],g0[10],b1[8:0],b0[10]",
    11: "m[4:0],r0[9:0],g0[9:0],b0[9:0],r1[7:0],r0[10:11],g1[7:0],g0[10:11],b1[7:0],b0[10:11]",
    15: "m[4:0],r0[9:0],g0[9:0],b0[9:0],r1[3:0],r0[10:15],g1[3:0],g0[10:15],b1[3:0],b0[10:15]",
}
# Table 114, the first 32 two-subset partitions: the subset of texels 0-15 (texel x + 4y).
BC6H_PARTITIONS = [
    "0011001100110011",
    "0001000100010001",
    "0111011101110111",
    "0001001100110111",
    "0000000100010011",
    "0011011101111111",
    "0001001101111111",
    "0000000100110111",
    "0000000000010011",
    "0011011111111111",
    "0000000101111111",
    "0000000000010111",
    "0001011111111111",
    "0000000011111111",
    "0000111111111111",
    "0000000000001111",
    "0000100011101111",
    "0111000100000000",
    "0000000010001110",
    "0111001100010000",
    "0011000100000000",
    "0000100011001110",
    "0000000010001100",
    "0111001100110001",
    "0011000100010000",
    "0000100010001100",
    "0110011001100110",
    "0011011001101100",
    "0001011111101000",
    "0000111111110000",
    "0111000110001110",
    "0011100110011100",
]
# Table 118 (Table.A2), first 32 entries: the anchor texel of subset 1.
BC6H_ANCHORS = (15,) * 16 + (15, 2, 8, 2, 2, 8, 8, 15, 2, 8, 2, 2, 8, 8, 2, 2)
# Table 119: the interpolation weights of 3- and 4-bit indices.
BC6H_WEIGHTS = {
    3: (0, 9, 18, 27, 37, 46, 55, 64),
    4: (0, 4, 9, 13, 17, 21, 26, 30, 34, 38, 43, 47, 51, 55, 60, 64),
}


def bc6h_unquantize(value: int, bits: int) -> int:
    """Section 20.2's unquantization of an unsigned endpoint of `bits` bits to 16 bits."""
    if bits >= 15:
        return value
    if value == 0:
        return 0
    if value == (1 << bits) - 1:
        return 0xFFFF
    return ((value << 15) + 0x4000) >> (bits - 1)


def _sign_extend(value: int, bits: int) -> int:
    return value - (1 << bits) if value >> (bits - 1) & 1 else value


def bc6h_block(block: bytes, unquantize=bc6h_unquantize, rounding: int = 32) -> list[tuple[int, int, int]]:
    """The 16 texels of an unsigned BC6H block (texel x + 4y) as binary16 bit patterns (red, green, blue).

    `unquantize` and `rounding` (the `+ 32` of Equation 2) are section 20.2's; the self-test passes other
    values to reproduce the decoders that depart from it."""
    bits = int.from_bytes(block, "little")
    mode = bits & 3 if bits & 3 < 2 else bits & 0x1F
    if mode not in BC6H_MODES:  # reserved: (0, 0, 0)
        return [(0, 0, 0)] * 16
    transformed, partition_bits, endpoint_bits, delta = BC6H_MODES[mode]
    fields, position = {}, 0
    for element in BC6H_LAYOUTS[mode].split(","):
        name, span = element[:-1].split("[")
        high, _, low = span.partition(":")
        a, b = int(high), int(low or high)
        step = 1 if a >= b else -1
        for target in range(b, a + step, step):
            fields[name] = fields.get(name, 0) | (bits >> position & 1) << target
            position += 1
    assert fields.pop("m") == mode
    subsets = 2 if partition_bits else 1
    assert position == (82 - partition_bits if subsets == 2 else 65)
    endpoints = [[fields.get(f"{channel}{i}", 0) for channel in "rgb"] for i in range(2 * subsets)]
    if transformed:  # the other endpoints are deltas from endpoint 0, wrapped at the endpoint bits
        for endpoint in endpoints[1:]:
            for c in range(3):
                endpoint[c] = (endpoints[0][c] + _sign_extend(endpoint[c], delta[c])) & ((1 << endpoint_bits) - 1)
    endpoints = [[unquantize(value, endpoint_bits) for value in endpoint] for endpoint in endpoints]
    partition = bits >> position & ((1 << partition_bits) - 1)
    position += partition_bits
    index_bits = 3 if subsets == 2 else 4
    anchor = BC6H_ANCHORS[partition] if subsets == 2 else 0
    texels = []
    for texel in range(16):
        width = index_bits - (texel in (0, anchor))
        index = bits >> position & ((1 << width) - 1)
        position += width
        subset = int(BC6H_PARTITIONS[partition][texel]) if subsets == 2 else 0
        low_end, high_end = endpoints[2 * subset], endpoints[2 * subset + 1]
        weight = BC6H_WEIGHTS[index_bits][index]
        interpolated = [
            ((64 - weight) * e0 + weight * e1 + rounding) >> 6 for e0, e1 in zip(low_end, high_end, strict=True)
        ]
        texels.append(tuple(value * 31 >> 6 for value in interpolated))  # the final unquantization, 31/64
    assert position == 128
    return texels


def half_bits_to_float(bits: int) -> float:
    return struct.unpack("<e", struct.pack("<H", bits))[0]


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
        across, down = -(-width // 4), -(-height // 4)
        blocks = [bc6h_block(data[16 * i : 16 * i + 16]) for i in range(across * down)]
        pixels = [
            tuple(float_to_byte(half_bits_to_float(h)) for h in blocks[y // 4 * across + x // 4][x % 4 + 4 * (y % 4)])
            for y in range(height)
            for x in range(width)
        ]
    return mode, _flip(pixels, width, height)
