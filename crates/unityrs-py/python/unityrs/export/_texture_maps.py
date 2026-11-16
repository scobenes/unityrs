# Ported from UnityPy 1.25.2 (MIT): export/Texture2DConverter.py:20-50, export/Texture2DConverter.py:450-456, export/Texture2DConverter.py:488-569. See LICENSE-UNITYPY and THIRD_PARTY_LICENSES.
from __future__ import annotations
from ..enums import TextureFormat
from .Texture2DConverter import pillow, atc, astc, pvrtc, etc, eac, half, rg, rgb9e5float

TF = TextureFormat

TEXTURE_FORMAT_BLOCK_SIZE_TABLE: Dict[TF, Optional[Tuple[int, int]]] = {}
for tf in TF:
    if tf.name.startswith("ASTC"):
        split = tf.name.rsplit("_", 1)[1].split("x")
        block_size = (int(split[0]), int(split[1]))
    elif tf.name.startswith(("DXT", "BC", "ETC", "EAC")):
        block_size = (4, 4)
    elif tf.name.startswith("PVRTC"):
        block_size = (8 if tf.name.endswith("2") else 4, 4)
    else:
        block_size = None
    TEXTURE_FORMAT_BLOCK_SIZE_TABLE[tf] = block_size


def get_compressed_image_size(width: int, height: int, texture_format: TextureFormat):
    block_size = TEXTURE_FORMAT_BLOCK_SIZE_TABLE[texture_format]
    if block_size is None:
        return (width, height)
    block_width, block_height = block_size

    def pad(value: int, pad_by: int) -> int:
        to_pad = value % pad_by
        if to_pad:
            value += pad_by - to_pad
        return value

    width = pad(width, block_width)
    height = pad(height, block_height)
    return width, height
RG_PADDING_MAP = {
    "RGE": 16,
    "RGF": 32,
    "RG;16": 16,
    "RG;16s": 16,
    "RG;8s": 8,
}
# Mapping TextureFormat -> (converter function, (additional args, ...))
CONV_TABLE: Dict[TF, Tuple[Callable[..., Image.Image], Tuple[Any, ...]]] = {
    TF.Alpha8: (pillow, ("RGBA", "raw", "A")),
    TF.ARGB4444: (pillow, ("RGBA", "raw", "RGBA;4B", (2, 1, 0, 3))),
    TF.RGB24: (pillow, ("RGB", "raw", "RGB")),
    TF.RGBA32: (pillow, ("RGBA", "raw", "RGBA")),
    TF.ARGB32: (pillow, ("RGBA", "raw", "ARGB")),
    TF.ARGBFloat: (pillow, ("RGBA", "raw", "RGBAF", (2, 1, 0, 3))),
    TF.RGB565: (pillow, ("RGB", "raw", "BGR;16")),
    TF.BGR24: (pillow, ("RGB", "raw", "BGR")),
    TF.R8: (pillow, ("RGB", "raw", "R")),
    TF.R16: (pillow, ("RGB", "raw", "R;16")),
    TF.RG16: (rg, ("RGB", "raw", "RG")),
    TF.DXT1: (pillow, ("RGBA", "bcn", 1)),
    TF.DXT3: (pillow, ("RGBA", "bcn", 2)),
    TF.DXT5: (pillow, ("RGBA", "bcn", 3)),
    TF.RGBA4444: (pillow, ("RGBA", "raw", "RGBA;4B", (3, 2, 1, 0))),
    TF.BGRA32: (pillow, ("RGBA", "raw", "BGRA")),
    TF.RHalf: (half, ("R", "raw", "R")),
    TF.RGHalf: (rg, ("RGB", "raw", "RGE")),
    TF.RGBAHalf: (half, ("RGB", "raw", "RGB")),
    TF.RFloat: (pillow, ("RGB", "raw", "RF")),
    TF.RGFloat: (rg, ("RGB", "raw", "RGF")),
    TF.RGBAFloat: (pillow, ("RGBA", "raw", "RGBAF")),
    # TF.YUY2: NotImplementedError("YUY2 not implemented"),
    TF.RGB9e5Float: (rgb9e5float, ()),
    TF.BC4: (pillow, ("L", "bcn", 4)),
    TF.BC5: (pillow, ("RGB", "bcn", 5)),
    TF.BC6H: (pillow, ("RGBA", "bcn", 6)),
    TF.BC7: (pillow, ("RGBA", "bcn", 7)),
    TF.DXT1Crunched: (pillow, ("RGBA", "bcn", 1)),
    TF.DXT5Crunched: (pillow, ("RGBA", "bcn", 3)),
    TF.PVRTC_RGB2: (pvrtc, (True,)),
    TF.PVRTC_RGBA2: (pvrtc, (True,)),
    TF.PVRTC_RGB4: (pvrtc, (False,)),
    TF.PVRTC_RGBA4: (pvrtc, (False,)),
    TF.ETC_RGB4: (etc, ("ETC1",)),
    TF.ATC_RGB4: (atc, (False,)),
    TF.ATC_RGBA8: (atc, (True,)),
    TF.EAC_R: (eac, ("EAC_R",)),
    TF.EAC_R_SIGNED: (eac, ("EAC_R_SIGNED",)),
    TF.EAC_RG: (eac, ("EAC_RG",)),
    TF.EAC_RG_SIGNED: (eac, ("EAC_RG_SIGNED",)),
    TF.ETC2_RGB: (etc, ("ETC2_RGB",)),
    TF.ETC2_RGBA1: (etc, ("ETC2_A1",)),
    TF.ETC2_RGBA8: (etc, ("ETC2_A8",)),
    TF.ASTC_RGB_4x4: (astc, ((4, 4),)),
    TF.ASTC_RGB_5x5: (astc, ((5, 5),)),
    TF.ASTC_RGB_6x6: (astc, ((6, 6),)),
    TF.ASTC_RGB_8x8: (astc, ((8, 8),)),
    TF.ASTC_RGB_10x10: (astc, ((10, 10),)),
    TF.ASTC_RGB_12x12: (astc, ((12, 12),)),
    TF.ASTC_RGBA_4x4: (astc, ((4, 4),)),
    TF.ASTC_RGBA_5x5: (astc, ((5, 5),)),
    TF.ASTC_RGBA_6x6: (astc, ((6, 6),)),
    TF.ASTC_RGBA_8x8: (astc, ((8, 8),)),
    TF.ASTC_RGBA_10x10: (astc, ((10, 10),)),
    TF.ASTC_RGBA_12x12: (astc, ((12, 12),)),
    TF.ETC_RGB4_3DS: (etc, ("ETC1",)),
    TF.ETC_RGBA8_3DS: (etc, ("ETC1",)),
    TF.ETC_RGB4Crunched: (etc, ("ETC1",)),
    TF.ETC2_RGBA8Crunched: (etc, ("ETC2_A8",)),
    TF.ASTC_HDR_4x4: (astc, ((4, 4),)),
    TF.ASTC_HDR_5x5: (astc, ((5, 5),)),
    TF.ASTC_HDR_6x6: (astc, ((6, 6),)),
    TF.ASTC_HDR_8x8: (astc, ((8, 8),)),
    TF.ASTC_HDR_10x10: (astc, ((10, 10),)),
    TF.ASTC_HDR_12x12: (astc, ((12, 12),)),
    TF.RG32: (rg, ("RGB", "raw", "RG;16")),
    TF.RGB48: (pillow, ("RGB", "raw", "RGB;16")),
    TF.RGBA64: (pillow, ("RGBA", "raw", "RGBA;16")),
    TF.R8_SIGNED: (pillow, ("R", "raw", "R;8s")),
    TF.RG16_SIGNED: (rg, ("RGB", "raw", "RG;8s")),
    TF.RGB24_SIGNED: (pillow, ("RGB", "raw", "RGB;8s")),
    TF.RGBA32_SIGNED: (pillow, ("RGBA", "raw", "RGBA;8s")),
    TF.R16_SIGNED: (pillow, ("R", "raw", "R;16s")),
    TF.RG32_SIGNED: (rg, ("RGB", "raw", "RG;16s")),
    TF.RGB48_SIGNED: (pillow, ("RGB", "raw", "RGB;16s")),
    TF.RGBA64_SIGNED: (pillow, ("RGBA", "raw", "RGBA;16s")),
}

# XBOX Swap Formats
XBOX_SWAP_FORMATS = [TF.RGB565, TF.DXT1, TF.DXT1Crunched, TF.DXT5, TF.DXT5Crunched]
