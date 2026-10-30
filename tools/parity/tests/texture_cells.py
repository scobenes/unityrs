"""The oracle's side of each cell of PARITY.md's TextureFormat matrix, for `test_texture_matrix.py`; not a test module.

`expected_row` builds every cell of a format's row from UnityPy and from the pixel cases (`texture_cases.py`), and
nothing here reads PARITY.md. With them: the matrix's columns, the tier and phase rules, and the words a row adds to
its result (`RESULT_NOTES`, `RAISE_REASONS`). ORACLE is True in the oracle image (UnityPy 1.25.2).
"""

import inspect
import re
import sys
from pathlib import Path
from unittest import mock

HERE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE / "tests"))

from texture_cases import PIXEL_CASES, H, W  # noqa: E402

import inventory  # noqa: E402
import references  # noqa: E402

try:
    import texture2ddecoder
    import UnityPy
    from PIL import Image
    from UnityPy.enums import BuildTarget, TextureFormat
    from UnityPy.export import Texture2DConverter
    from UnityPy.helpers import TextureSwizzler

    ORACLE = UnityPy.__version__ == inventory.PINNED_VERSION
except ImportError:  # pragma: no cover - outside the oracle image
    ORACLE = False


COLUMNS = [
    "value",
    "format",
    "decoder",
    "mode",
    "conversion",
    "gates",
    "UnityPy 1.25.2",
    "`set_image` writes",
    "tier",
    "status",
]

# The tier each result gives (DESIGN.md section 3): a crunched row is Tier A like its base format.
KIND_TIER = {"works": "A", "crunch": "A", "raises": "B", "wrong": "C", "unverified": "U"}


def tier_of(name, kind):
    """A raise is Tier B where section 17.1 names a reference (`references.TIER_B`), else Tier U."""
    if kind == "raises" and name not in references.TIER_B:
        return "U"
    return KIND_TIER[kind]


# DESIGN.md section 5: the formats P1 implements; every other format is P3.
P1 = ["BC7", "DXT1", "DXT5", "RGB24", "RGBA32"]
VERSION = (2021, 3, 1, 3)  # the matrix's 2021.3.1f3
# A platform blob that makes TextureSwizzler.is_switch_swizzled hold: 1 << 1 = 2 GOBs per block.
GOBS = 2
SWITCH_BLOB = bytes(8) + (1).to_bytes(4, "little")
# Versions either side of the 2017.3 crunch boundary.
CRUNCH_VERSIONS = [(5, 6, 0, 0), (2017, 2, 9, 0), (2017, 3, 0, 0), (2021, 3, 1, 3)]

# What Unity stores for one pixel of each uncompressed TextureFormat, by name.
BYTES_PER_PIXEL = {
    name: size
    for size, names in {
        1: "Alpha8 R8 R8_SIGNED",
        2: "ARGB4444 RGBA4444 RGB565 R16 RHalf RG16 RG16_SIGNED R16_SIGNED YUY2",
        3: "RGB24 BGR24 RGB24_SIGNED",
        4: "RGBA32 ARGB32 BGRA32 RFloat RGHalf RGB9e5Float RG32 RGBA32_SIGNED RG32_SIGNED",
        6: "RGB48 RGB48_SIGNED",
        8: "RGBAHalf RGFloat RGBA64 RGBA64_SIGNED",
        12: "RGBFloat",
        16: "ARGBFloat RGBAFloat",
    }.items()
    for name in names.split()
}
# Block formats whose blocks are 8 bytes; every other block format has 16-byte blocks.
EIGHT_BYTE_BLOCKS = {
    "DXT1",
    "BC4",
    "ETC_RGB4",
    "ETC_RGB4_3DS",
    "ETC2_RGB",
    "ETC2_RGBA1",
    "EAC_R",
    "EAC_R_SIGNED",
    "ATC_RGB4",
    "PVRTC_RGB2",
    "PVRTC_RGBA2",
    "PVRTC_RGB4",
    "PVRTC_RGBA4",
}


def base_name(name):
    """The plain format a crunched one unpacks to: `DXT1` for `DXT1Crunched`."""
    return name.removesuffix("Crunched")


def unit(fmt):
    """(width, height, bytes) of the smallest unit the format stores: a pixel, or a block."""
    name = base_name(fmt.name)
    if name in BYTES_PER_PIXEL:
        return 1, 1, BYTES_PER_PIXEL[name]
    # UnityPy's own block table; ATC is absent from it (it pads nothing) and has 4x4 blocks.
    block_w, block_h = Texture2DConverter.TEXTURE_FORMAT_BLOCK_SIZE_TABLE[fmt] or (4, 4)
    return block_w, block_h, 8 if name in EIGHT_BYTE_BLOCKS else 16


def stored_size(fmt, width, height):
    """The bytes a real texture of `fmt` holds for its top level: what `parse_image_data` gets from Unity."""
    unit_w, unit_h, size = unit(fmt)
    return -(-width // unit_w) * -(-height // unit_h) * size


def exception_name(error):
    """`ValueError` for a builtin, `struct.error` for anything else: how the matrix names a raise."""
    kind = type(error)
    return kind.__qualname__ if kind.__module__ == "builtins" else f"{kind.__module__}.{kind.__qualname__}"


# Words a row's result adds after `wrong`, each shown by a test of AgainstTheOracle (named), and
# the source it cites after the CONV_TABLE entry: ("function", name) for a function of
# Texture2DConverter.py, ("call", regex) for a call from the line the regex matches to its `)` line.
RESULT_NOTES = {
    # half_values_outside_a_byte_raise
    "RGBAHalf": (
        "; a value at 1.0 or above, at -1/256 or below, or NaN raises `ValueError`, an infinity `OverflowError`",
        [("function", "half")],
    ),
    # pixels_match_the_matrix (the case's reference)
    "ETC2_RGBA1": (
        ": the Khronos Data Format Specification 1.3, section 22.9, makes a transparent punch-through pixel"
        " (0, 0, 0, 0)",
        [],
    ),
    **{f"ASTC_HDR_{n}x{n}": ("", [("call", r"config = astc_encoder\.ASTCConfig\(")]) for n in (4, 5, 6, 8, 10, 12)},
}
# The words after `raises `E`` for each exception and message the oracle gives on real-sized data.
RAISE_REASONS = {
    ("ValueError", "unknown raw mode for given image mode"): ", unknown raw mode",
    ("ValueError", "unrecognized image mode"): ", unrecognized image mode",
    ("ValueError", "bad image mode"): ", bad image mode",
    ("KeyError", "'raw'"): " `'raw'`",
    ("TypeError", "'float' object cannot be interpreted as an integer"): ", floats into a bytearray",
    ("NotImplementedError", "Not implemented texture format: YUY2"): "",
    ("NotImplementedError", "Not implemented texture format: RGBFloat"): "",
    # RHalf; half_values_outside_a_byte_raise shows the rest.
    ("struct.error", "unpack requires a buffer of 2 bytes"): (
        ": `half` reads three halves a pixel for the one stored, so real-sized data runs out; a value at 1.0"
        " or above, at -1/256 or below, or NaN raises `ValueError` first, an infinity `OverflowError`, and"
        " Pillow's missing mode `R` (`ValueError` too) is reached only with three times the data"
    ),
}
# texture2ddecoder's names for the modes `etc` and `eac` take.
ETC_MODES = {"ETC1": "ETC1", "ETC2_RGB": "ETC2", "ETC2_A1": "ETC2 A1", "ETC2_A8": "ETC2 A8"}
EAC_MODES = {"EAC_R": "EAC R", "EAC_R_SIGNED": "EAC R signed", "EAC_RG": "EAC RG", "EAC_RG_SIGNED": "EAC RG signed"}
# `set_image` writes plain ETC1 data under these tags.
PLAIN_ETC1 = ("ETC_RGB4_3DS", "ETC_RGB4Crunched")


# ---------------------------------------------------------------------------------------------
# The oracle's side of each cell.


def source_lines():
    return inspect.getsource(Texture2DConverter).splitlines()


def line_of(pattern):
    """The 1-based line of Texture2DConverter.py whose text matches `pattern`; exactly one must."""
    found = [i for i, text in enumerate(source_lines(), 1) if re.search(pattern, text)]
    assert len(found) == 1, (pattern, found)
    return found[0]


def cite(first, last=None):
    return f"`:{first}`" if last in (None, first) else f"`:{first}-{last}`"


def cite_source(anchor):
    """The lines of a function of Texture2DConverter.py, or of a call from the line a regex matches to its `)`."""
    kind, name = anchor
    if kind == "function":
        lines, start = inspect.getsourcelines(getattr(Texture2DConverter, name))
        return cite(start, start + len(lines) - 1)
    start = line_of(name)
    end = next(i for i, text in enumerate(source_lines(), 1) if i > start and text.strip() == ")")
    return cite(start, end)


def entry_line(fmt):
    return line_of(rf"^\s*TF\.{fmt.name}: \(")


def raise_line(error):
    """The line of Texture2DConverter.py the exception passed through last."""
    lines = [frame.lineno for frame in _frames(error.__traceback__) if frame.filename == Texture2DConverter.__file__]
    return lines[-1]


def _frames(tb):
    while tb is not None:
        yield inspect.getframeinfo(tb, context=0)
        tb = tb.tb_next


def decode(data, fmt, width=W, height=H, platform=None, blob=None, version=VERSION):
    platform = BuildTarget.StandaloneWindows64 if platform is None else platform
    return Texture2DConverter.parse_image_data(data, width, height, fmt, version, platform, blob)


def stand_in_unpackers():
    """Patches for both crunch unpackers: they record what they get and return it unchanged, so a
    crunched format's data reaches its base format's decoder as it was given."""
    calls = []

    def unpacker(name):
        def unpack(data):
            calls.append((name, bytes(data)))
            return bytes(data)

        return unpack

    patches = [
        mock.patch.object(texture2ddecoder, "unpack_unity_crunch", unpacker("unpack_unity_crunch")),
        mock.patch.object(texture2ddecoder, "unpack_crunch", unpacker("unpack_crunch")),
    ]
    return calls, patches


def with_stand_ins(fmt, width=1, height=1, platform=None, version=VERSION, data=None):
    """Run parse_image_data with every decoder, the deswizzle and the crunch unpackers replaced by
    recorders. Returns (calls, image size or None, exception name or None)."""
    calls, patches = stand_in_unpackers()

    def converter(key):
        def convert(image_data, w, h, *args):
            calls.append(("convert", key, bytes(image_data), w, h))
            return Image.new("RGBA", (w, h))

        return convert

    def deswizzle(image_data, w, h, block_w, block_h, gobs):
        calls.append(("deswizzle", w, h, block_w, block_h, gobs))
        return bytearray(image_data)

    table = {key: (converter(key), ()) for key in TextureFormat}
    patches += [
        mock.patch.dict(Texture2DConverter.CONV_TABLE, table),
        mock.patch.object(TextureSwizzler, "deswizzle", deswizzle),
    ]
    blob = SWITCH_BLOB if platform == BuildTarget.Switch else None
    data = bytes(range(256)) * 64 if data is None else data
    for patch in patches:
        patch.start()
    try:
        image = decode(data, fmt, width, height, platform, blob, version)
        return calls, image.size, None
    except Exception as error:  # noqa: BLE001 - the exception type is the result
        return calls, None, exception_name(error)
    finally:
        for patch in reversed(patches):
            patch.stop()


def gates_cell(fmt):
    """The gates cell, from what parse_image_data does on a 1x1 texture."""
    calls, size, error = with_stand_ins(fmt)
    assert error is None and size == (1, 1), (fmt, error, size)  # padded, decoded, cropped back
    (_, _, _, width, height) = next(call for call in calls if call[0] == "convert")
    cells = ["no pad" if (width, height) == (1, 1) else f"pad {width}×{height}"]

    calls, size, error = with_stand_ins(fmt, platform=BuildTarget.Switch)
    if error is not None:
        assert error == "KeyError", (fmt, error)
        cells.append("sw —")
    else:
        _, swizzled_w, swizzled_h, block_w, block_h, gobs = next(call for call in calls if call[0] == "deswizzle")
        _, read_as, _, width, height = next(call for call in calls if call[0] == "convert")
        assert gobs == GOBS and (width, height) == (swizzled_w, swizzled_h), (fmt, calls)  # in place of padding
        cells.append(f"sw {block_w}×{block_h}" + ("" if read_as == fmt else f", read as `{read_as.name}`"))

    data = bytes(range(256)) * 64
    calls, size, error = with_stand_ins(fmt, platform=BuildTarget.XBOX360, data=data)
    first = calls[0][1] if calls[0][0] != "convert" else calls[0][2]  # what the first stage got
    if first[:4] == bytes([1, 0, 3, 2]):
        cells.append("xbox")
    else:
        assert first[:4] == data[:4], (fmt, first[:4])

    unpackers = []
    for version in CRUNCH_VERSIONS:
        calls, size, error = with_stand_ins(fmt, version=version)
        unpackers.append(tuple(call[0] for call in calls if call[0].startswith("unpack")))
    if unpackers == [("unpack_unity_crunch",)] * 4:
        cells.append("crunch: `unpack_unity_crunch` at any version")
    elif unpackers == [("unpack_crunch",)] * 2 + [("unpack_unity_crunch",)] * 2:
        cells.append("crunch: `unpack_crunch` before 2017.3, `unpack_unity_crunch` from 2017.3")
    else:
        assert unpackers == [()] * 4, (fmt, unpackers)
    return "; ".join(cells)


def decoder_cell(fmt):
    entry = Texture2DConverter.CONV_TABLE.get(fmt)
    if entry is None:
        commented = [i for i, text in enumerate(source_lines(), 1) if re.match(rf"\s*# TF\.{fmt.name}:", text)]
        return f"none: the entry is commented out ({cite(commented[0])})" if commented else "none: no entry"
    func, args = entry
    T = Texture2DConverter
    if func is T.pillow:
        mode, codec, rawmode, *swap = args
        text = f"Pillow raw `{rawmode}`" if codec == "raw" else f"Pillow `{codec}` {rawmode}"
        text += f" into `{mode}`" + (f", bands {swap[0]}" if swap else "")
    elif func is T.half:
        mode, codec, rawmode = args
        text = f"`half`, then Pillow {codec} `{rawmode}` into `{mode}`"
    elif func is T.rg:
        text = "`rg` (" + ", ".join(f"`{arg}`" for arg in args) + ")"
    elif func is T.rgb9e5float:
        text = "`rgb9e5float`"
    elif func is T.pvrtc:
        text = f"texture2ddecoder PVRTC {2 if args[0] else 4} bpp"
    elif func is T.etc:
        text = f"texture2ddecoder {ETC_MODES[args[0]]}"
    elif func is T.eac:
        text = f"texture2ddecoder {EAC_MODES[args[0]]}"
    elif func is T.atc:
        text = "texture2ddecoder ATC RGBA" if args[0] else "texture2ddecoder ATC RGB"
    elif func is T.astc:
        text = "astcenc LDR, {}×{} blocks".format(*args[0])
    else:
        raise AssertionError(f"no rendering for {func.__name__}")
    return ("crunch, then " if "Crunched" in fmt.name else "") + text


def real_sized(fmt):
    return bytes([0x30]) * stored_size(fmt, W, H)


def run_on_real_sized_data(fmt):
    """(image, None) or (None, (exception name, message, raising line)), on 0x30 bytes, as many as a real
    8x8 texture holds; a crunched format gets as many as its base format and keeps them through unpacking."""
    calls, patches = stand_in_unpackers()
    for patch in patches:
        patch.start()
    try:
        return decode(real_sized(fmt), fmt), None
    except Exception as error:  # noqa: BLE001 - the exception is the result
        message = str(error)
        return None, (exception_name(error), message, raise_line(error))
    finally:
        for patch in reversed(patches):
            patch.stop()


def kind_of(fmt, raised):
    if raised:
        return "raises"
    if "Crunched" in fmt.name:
        return "crunch"
    case = PIXEL_CASES[fmt.name][1]()
    if case.pixels is None:
        return "unverified"
    return "works" if case.unitypy is None else "wrong"


def result_cell(fmt, kind, raised):
    cites = [cite(entry_line(fmt))] if fmt in Texture2DConverter.CONV_TABLE else []
    if kind == "raises":
        name, message, line = raised
        return f"raises `{name}`{RAISE_REASONS[name, message]} ({', '.join([*cites, cite(line)])})"
    note, anchors = RESULT_NOTES.get(fmt.name, ("", []))
    return f"{kind}{note} ({', '.join(cites + [cite_source(anchor) for anchor in anchors])})"


def set_image_cell(fmt):
    source = Image.new("RGBA", (W, H), (10, 20, 30, 40))
    try:
        data, written = Texture2DConverter.image_to_texture2d(source, fmt)
    except Exception as error:  # noqa: BLE001 - the exception is the result
        return f"raises `{exception_name(error)}` ({cite(raise_line(error))}); Tier C (writing), section 19.2"
    cell = f"`{written.name}`"
    if written.name in PLAIN_ETC1 and data == Texture2DConverter.image_to_texture2d(source, TextureFormat.ETC_RGB4)[0]:
        cell += " holding plain ETC1"
    try:
        from_rgb = Texture2DConverter.image_to_texture2d(source.convert("RGB"), fmt)[1]
    except Exception as error:  # noqa: BLE001 - the exception is the result
        cell += f" (an RGB image with no zero side raises `{exception_name(error)}`, section 18.1)"
    else:
        if from_rgb != written:
            cell += f" (`{from_rgb.name}` from an RGB image)"
    try:
        decode(data, written)
    except Exception:  # noqa: BLE001 - written data its own reader cannot read
        cell += "; Tier C (writing), section 19.2"
    return cell


def expected_row(fmt):
    """Every cell of the format's row, as the oracle and the pixel cases make it."""
    image, raised = run_on_real_sized_data(fmt)
    kind = kind_of(fmt, raised)
    conversion = "—" if raised else PIXEL_CASES[base_name(fmt.name)][0]
    return {
        "value": str(fmt.value),
        "format": f"`{fmt.name}`",
        "decoder": decoder_cell(fmt),
        "mode": "—" if raised else image.mode,
        "conversion": conversion,
        "gates": gates_cell(fmt),
        "UnityPy 1.25.2": result_cell(fmt, kind, raised),
        "`set_image` writes": set_image_cell(fmt),
        "tier": tier_of(fmt.name, kind),
        "status": "pinned P0; impl P1" if fmt.name in P1 else "pinned P0; impl P3",
    }
