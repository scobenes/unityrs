"""B3a synthetic writes, UnityPy 1.25.2 and Pillow 9.5.0 only.

Record actual converter bytes, set_image fields and object serialization,
plus direct BICUBIC output. Tier C mode references call set_image on the
converted RGBA image. Never invoke etcpak on empty images (fatal upstream).
--check regenerates all records, including the fixed Little CMS CLUT.
"""
from __future__ import annotations
import argparse
import copy
import hashlib
import json
from pathlib import Path
from types import SimpleNamespace as NS
from unittest.mock import patch
import PIL
import UnityPy
from PIL import Image, ImageCms
from UnityPy.classes import Texture2D
from UnityPy.enums import TextureFormat as TF
from UnityPy.export import Texture2DConverter as CONVERTER
from UnityPy.classes.legacy_patch.Texture2D import _Texture2d_set_image as set_image
from UnityPy.helpers.TypeTreeHelper import write_typetree
from UnityPy.helpers.TypeTreeNode import TypeTreeNode
from UnityPy.streams import EndianBinaryWriter
import set_image_lab

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "crates/unityrs-texture/tests/fixtures/set_image.json"
MODES = ("1", "L", "LA", "La", "P", "PA", "RGB", "RGBA", "RGBX", "RGBa", "LAB",
         "CMYK", "YCbCr", "HSV", "I", "F", "I;16", "I;16L", "I;16B", "I;16N",
         "BGR;15", "BGR;16", "BGR;24")
ENCODERS = {10, 12, 25, 26, 27, 28, 29, 34, 45, 46, 47, 60, 64, 65, *range(48, 60), *range(66, 72)}


class EncoderBoundary(Exception):
    """Stop at the encoder: B3b characterises the build before pinning its bytes."""


def digest(data):
    return dict(length=len(data), sha256=hashlib.sha256(data).hexdigest())


def image(mode, width, height):
    channels = {"1": 1, "L": 1, "P": 1, "LA": 2, "La": 2, "PA": 2,
                "RGB": 3, "LAB": 3, "HSV": 3, "YCbCr": 3,
                "I;16": 2, "I;16L": 2, "I;16B": 2, "I;16N": 2,
                "BGR;15": 2, "BGR;16": 2, "BGR;24": 3}.get(mode, 4)
    length = ((width + 7) // 8 if mode == "1" else width * channels) * height
    data = bytes((i * 73 + i // 3 * 19 + 17) % 256 for i in range(length))
    if mode.startswith("BGR;"):
        return Image.new(mode, (width, height)), bytes(length)
    img = Image.frombytes(mode, (width, height), data) if width and height else Image.new(mode, (width, height))
    if mode in ("P", "PA"):
        img.putpalette(bytes((i * 29 + 7) % 256 for i in range(768)))
    return img, data


def schema(optional=True):
    rows = [[0, "Texture2D", "Base", 0]]
    rows += [[1, "int", name, 0] for name in ("m_Width", "m_Height", "m_TextureFormat", "m_CompleteImageSize")]
    if optional:
        rows += [[1, "bool", "m_MipMap", 0x4000], [1, "int", "m_MipCount", 0]]
    rows += [[1, "vector", "m_PlatformBlob", 0x4000], [2, "Array", "Array", 0],
             [3, "int", "size", 0], [3, "UInt8", "data", 0],
             [1, "TypelessData", "image data", 0x4000]]
    if optional:
        rows += [[1, "StreamingInfo", "m_StreamData", 0], [2, "UInt64", "offset", 0],
                 [2, "unsigned int", "size", 0], [2, "string", "path", 0]]
    return rows


def serialize(fields, rows, endian):
    nodes = TypeTreeNode.from_list([dict(m_Level=level, m_Type=typ, m_Name=name,
                                       m_MetaFlag=flag, m_ByteSize=-1, m_Version=1)
                                    for level, typ, name, flag in rows])
    writer = EndianBinaryWriter(endian=endian)
    write_typetree(fields, nodes, writer)
    return writer.bytes


def object_fields(texture, optional):
    fields = {name: getattr(texture, name) for name in
              ("m_Width", "m_Height", "m_TextureFormat", "m_CompleteImageSize")}
    fields.update(m_PlatformBlob=digest(bytes(texture.m_PlatformBlob)),
                  **{"image data": digest(texture.image_data)})
    if optional:
        fields.update(m_MipMap=texture.m_MipMap, m_MipCount=texture.m_MipCount,
                      m_StreamData=vars(texture.m_StreamData).copy())
    return fields


def record(fmt, size, mode="RGBA", platform=0, exponent=None, mips=1, flip=True, optional=True, auxiliary=None):
    width, height = size
    img, data = image(mode, width, height)
    if auxiliary is not None:
        img = Image.new(mode, (width, height))
        data = img.tobytes()
    blob = None if exponent is None else bytes(8) + exponent.to_bytes(4, "little")
    case = dict(format=int(fmt), size=list(size), mode=mode, source=data.hex(), platform=platform,
                blob=None if blob is None else blob.hex(), mips=mips, flip=flip, optional=optional)
    if auxiliary is not None:
        case["auxiliary"] = auxiliary
    if mode in ("P", "PA"):
        palette = img.getpalette("RGB")
        case["palette"] = bytes(v for n in range(256) for v in (*palette[n*3:n*3+3], 255)).hex()
    if platform == 38 and exponent and (not width or not height):
        case["policy"] = "switch_empty"
        return case
    if platform == 38 and exponent and int(fmt) in (34, 45, 46, 47, 60, 64, 65):
        try:
            CONVERTER.image_to_texture2d(img, fmt, platform, list(blob), flip)
            raise AssertionError("missing Switch block unexpectedly wrote")
        except KeyError as exc:
            case["oracle_error"] = type(exc).__name__
        case["policy"] = "switch_format"
        return case
    # Raw packer table, section 18.1. Correct the input only where packing fails.
    try:
        if fmt not in ENCODERS:
            CONVERTER.image_to_texture2d(img, fmt, platform, None if blob is None else list(blob), flip)
        reference = img
    except ValueError as exc:
        case["oracle_error"] = type(exc).__name__
        try:
            reference = img.convert("RGBA")
            case["correction"] = "convert_rgba"
        except ValueError:
            case["policy"] = "unsupported_mode"
            return case
    if platform == 38 and exponent and mips > 1 and width >= 8 and height >= 8:
        case["policy"] = "switch_mips"
        return case
    if platform == 38 and exponent and (not width or not height):
        case["policy"] = "switch_empty"
        return case
    if platform == 11 and fmt in (TF.DXT1, TF.DXT1Crunched, TF.DXT5, TF.DXT5Crunched) and width and height:
        case["policy"] = "xbox"
        return case
    if fmt in ENCODERS and (width and height):
        case["policy"] = "encoder"
        def capture(raw, padded_width, padded_height, written):
            case["encoder_input"] = dict(format=int(written), size=[padded_width, padded_height], **digest(raw))
            raise EncoderBoundary()
        with (patch.object(CONVERTER, "compress_etcpak", capture),
              patch.object(CONVERTER, "compress_astc", capture)):
            try:
                CONVERTER.image_to_texture2d(reference, fmt, platform, None if blob is None else list(blob), flip)
                raise AssertionError("encoder target did not call an encoder")
            except EncoderBoundary:
                pass
        if fmt == TF.BC7:
            # The actual, deterministic None-parameter exception, without
            # pinning any encoder output before decision 16's build probe.
            try:
                CONVERTER.image_to_texture2d(reference, fmt)
                raise AssertionError("BC7 unexpectedly accepted None parameters")
            except ValueError as exc:
                case["oracle_error"] = type(exc).__name__
        return case
    if fmt in ENCODERS:
        # Tier C empty writes: use the nonempty mapping, without calling C
        # on empty buffers. No encoder output is claimed verified by B3a.
        written = {28: 10, 29: 12, 64: 34, 46: 47, 65: 47}.get(int(fmt), int(fmt))
        if 66 <= written <= 71:
            written -= 18 if mode == "RGB" else 12
        output = b""
        actual_mips = min(mips, 1)
        case["policy"] = "empty_c"
    else:
        output, written = CONVERTER.image_to_texture2d(reference, fmt, platform, None if blob is None else list(blob), flip)
        texture = Texture2D(m_Width=91, m_Height=71, m_TextureFormat=int(fmt), m_CompleteImageSize=3,
                     m_MipMap=False if optional else None, m_MipCount=9 if optional else None,
                     image_data=b"old", m_PlatformBlob=[] if blob is None else list(blob),
                     m_StreamData=NS(path="old.resS", offset=123, size=456) if optional else None,
                     m_ImageCount=1, m_IsReadable=True, m_LightmapFormat=0, m_Name="synthetic",
                     m_TextureDimension=2, m_TextureSettings=None)
        texture.object_reader = NS(platform=platform)
        if flip:
            case["initial"] = {endian: serialize(texture, schema(optional), endian).hex() for endian in ("<", ">")}
        set_image(texture, reference, int(fmt), mips)
        if flip:
            output = texture.image_data
            case["object"] = {endian: digest(serialize(texture, schema(optional), endian)) for endian in ("<", ">")}
            case["object"]["fields"] = object_fields(texture, optional)
        actual_mips = texture.m_MipCount if optional else 1
        # For absent optional fields, determine mip termination independently
        # with a second oracle object that has those fields.
        if not optional:
            probe = copy.copy(texture)
            probe.m_MipCount = 9
            set_image(probe, reference, int(fmt), mips)
            actual_mips = probe.m_MipCount
    case["result"] = dict(format=int(written), mips=actual_mips, **digest(output))
    # Empty encoder cases and flip=False describe converter output only.
    return case


def generate():
    assert UnityPy.__version__ == "1.25.2" and PIL.__version__ == "9.5.0"
    cases = []
    for fmt in TF:
        for size in ((1, 1), (7, 9), (0, 5)):
            cases.append(record(fmt, size))
        if fmt not in ENCODERS:
            for platform, exponent in ((11, None), (38, 0), (38, 1), (38, 3)):
                cases.append(record(fmt, (19, 17), platform=platform, exponent=exponent))
    for mode in MODES:
        for fmt in (TF.Alpha8, TF.R8, TF.RGB24, TF.RGBA32):
            for size in ((5, 7), (0, 0), (0, 4), (4, 0)):
                cases.append(record(fmt, size, mode))
    for mode in ("RGB", "RGBA", "LAB", "RGBX", "L", "LA", "P", "CMYK", "RGBa", "HSV", "I", "F"):
        for mips in (-3, 0, 1, 2, 9):
            for fmt in (TF.RGBA32, TF.Alpha8):
                cases.append(record(fmt, (35, 19), mode, mips=mips))
    for fmt in (TF.RGBA32, TF.Alpha8, TF.RGB24):
        cases += [record(fmt, (17, 23), flip=False), record(fmt, (17, 23), mips=8, optional=False),
                  record(fmt, (17, 23), platform=38, exponent=1, mips=2)]
        # A requested chain that terminates at its top level is still the
        # characterised single-level Switch write, with the count reset to one.
        for size in ((7, 9), (9, 7)):
            cases.append(record(fmt, size, platform=38, exponent=1, mips=9))
    for fmt in (TF.DXT1, TF.DXT5):
        cases.append(record(fmt, (4, 4), platform=11))
    for size in ((0, 0), (0, 5)):
        cases.append(record(TF.RGBA32, size, platform=38, exponent=1))
    for fmt in (TF.ETC_RGB4, TF.ETC2_RGB, TF.ETC2_RGBA1, TF.ETC2_RGBA8, TF.ETC_RGB4_3DS, TF.ETC_RGB4Crunched, TF.ETC2_RGBA8Crunched):
        cases.append(record(fmt, (4, 4), platform=38, exponent=1))
    for fmt in (TF.ASTC_HDR_4x4, TF.ASTC_HDR_5x5, TF.ASTC_HDR_6x6, TF.ASTC_HDR_8x8, TF.ASTC_HDR_10x10, TF.ASTC_HDR_12x12):
        for size in ((0, 0), (0, 4), (4, 0)):
            cases.append(record(fmt, size, "RGB"))
    for mode in ("RGB", "LAB"):
        for fmt in (TF.RGBA32, TF.RGB24, TF.Alpha8):
            cases.append(record(fmt, (9, 7), mode, auxiliary=0))
    for mode in ("RGB", "LAB", "HSV", "YCbCr"):
        for fmt in (TF.RGB24, TF.RGBA32, TF.Alpha8, TF.R8):
            for size in ((1, 1), (7, 9), (16, 1), (1, 16)):
                cases.append(record(fmt, size, mode, platform=38, exponent=1, auxiliary=0))
    resizes = []
    for mode in ("L", "LA", "P", "RGB", "RGBA"):
        for size, dest in (((35, 19), (17, 9)), ((17, 9), (8, 4)), ((3, 2), (2, 3)),
                           ((1, 17), (1, 4)), ((8, 9), (8, 4)), ((9, 8), (4, 8)), ((9, 8), (9, 8))):
            img, data = image(mode, *size)
            resized = img.resize(dest, Image.Resampling.BICUBIC)
            resizes.append(dict(mode=mode, size=list(size), dest=list(dest), source=data.hex(), hex=resized.tobytes().hex()))
    # All 16 million LAB colours: one digest pins all tetrahedra and boundaries.
    lab = Image.frombytes("LAB", (65536, 256), bytes(v for l in range(256) for a in range(256) for b in range(256) for v in (l, a, b)))
    sources = list(dict.fromkeys(case["source"] for case in cases))
    for case in cases:
        case["source"] = sources.index(case["source"])
    return dict(sources=sources, unitypy=UnityPy.__version__, pillow=PIL.__version__, lcms=ImageCms.versions()[1],
                schema={"full": schema(), "minimal": schema(False)}, cases=cases, resizes=resizes,
                lab_rgba=digest(lab.convert("RGBA").tobytes()))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    fixture = (json.dumps(generate(), separators=(",", ":"), ensure_ascii=False) + "\n").encode()
    lut = set_image_lab.table()
    outputs = {OUT: fixture, ROOT / "crates/unityrs-texture/src/write/lab.bin": lut}
    for path, content in outputs.items():
        assert len(content) < 1_500_000, path
        if args.check:
            assert path.read_bytes() == content, f"stale {path.relative_to(ROOT)}"
        else:
            path.write_bytes(content)
    print(f"set_image: {len(json.loads(fixture)['cases'])} writes, 35 resizes, exhaustive LAB, fixed CLUT")


if __name__ == "__main__":
    main()
