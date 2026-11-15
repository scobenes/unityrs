"""B3b synthetic encoder writes from the pinned amd64 UnityPy oracle.

Use the existing unityrs-dev container only. This never calls a C encoder on
an empty image. BC7 replaces only compress_etcpak's rejected None parameters
with etcpak's default BC7CompressBlockParams(); ETC_RGB4Crunched uses the
specified Tier C ETC_RGB4 reference. Keep raw encoded bytes for independent
Rust re-decode comparisons, including every generated mip level.

Oracle calls: UnityPy 1.25.2 export/Texture2DConverter.py:83-103 (etcpak),
export/Texture2DConverter.py:106-115 (ASTC),
export/Texture2DConverter.py:118-234 (preprocessing and format mapping),
export/Texture2DConverter.py:369-387 (ASTC context), and
classes/legacy_patch/Texture2D.py:14-60 (mips and object fields).
"""
from __future__ import annotations

import argparse
import importlib.metadata
import json
import platform
import sys
from types import SimpleNamespace as NS
from unittest.mock import patch

import etcpak
import PIL
import UnityPy
from UnityPy.classes import Texture2D
from UnityPy.enums import TextureFormat as TF
from UnityPy.export import Texture2DConverter as CONVERTER

from set_image import ROOT, ENCODERS, digest, image, object_fields, schema, serialize, set_image

OUT = ROOT / "crates/unityrs-texture/tests/fixtures/set_image_encoders.json"
TARGETS = sorted(ENCODERS | {41, 42, 43, 44})
ETCPAK = CONVERTER.compress_etcpak
CONVERT = CONVERTER.image_to_texture2d


def bc7_reference(data, width, height, fmt):
    if fmt == TF.BC7:
        return etcpak.compress_bc7(data, width, height, etcpak.BC7CompressBlockParams())
    return ETCPAK(data, width, height, fmt)


def record(fmt, size, mode="RGBA", mips=1, exponent=None, optional=True, flip=True, platform_id=0):
    img, source = image(mode, *size)
    blob = None if exponent is None else bytes(8) + exponent.to_bytes(4, "little")
    if exponent is not None:
        platform_id = 38
    case = dict(format=fmt, size=list(size), mode=mode, source=source.hex(),
                mips=mips, platform=platform_id, blob=None if blob is None else blob.hex(),
                optional=optional, flip=flip, acceptance="bytes")
    if mode in ("P", "PA"):
        palette = img.getpalette("RGB")
        case["palette"] = bytes(value for n in range(256)
                                for value in (*palette[n * 3:n * 3 + 3], 255)).hex()
    reference = img
    try:
        img.tobytes("raw", "RGBA")
    except ValueError as error:
        case["oracle_error"] = type(error).__name__
        reference = img.convert("RGBA")
        case["correction"] = "convert_rgba"
    if fmt == int(TF.BC7):
        case["reference"] = "tier_c_bc7_default_params"
    elif fmt == int(TF.ETC_RGB4Crunched):
        case["reference"] = "tier_c_plain_etc1"
    else:
        case["reference"] = "unitypy"
    target = TF.ETC_RGB4 if fmt == int(TF.ETC_RGB4Crunched) else TF(fmt)
    texture = Texture2D(m_Width=91, m_Height=71, m_TextureFormat=fmt, m_CompleteImageSize=3,
                        m_MipMap=False if optional else None, m_MipCount=9 if optional else None,
                        image_data=b"old", m_PlatformBlob=[] if blob is None else list(blob),
                        m_StreamData=NS(path="old.resS", offset=123, size=456) if optional else None,
                        m_ImageCount=1, m_IsReadable=True, m_LightmapFormat=0, m_Name="synthetic",
                        m_TextureDimension=2, m_TextureSettings=None)
    texture.object_reader = NS(platform=platform_id)
    levels = []
    encoder_inputs = []

    def capture_etcpak(raw, width, height, written):
        encoder_inputs.append(dict(encoder="etcpak", format=int(written),
                                   size=[width, height], **digest(raw)))
        return bc7_reference(raw, width, height, written)

    def capture_astc(raw, width, height, written):
        encoder_inputs.append(dict(encoder="astcenc", format=int(written),
                                   size=[width, height], **digest(raw)))
        return astc(raw, width, height, written)

    def capture_convert(level_img, level_target, level_platform=0, level_blob=None, level_flip=True):
        data, written = CONVERT(level_img, level_target, level_platform, level_blob, level_flip)
        levels.append(dict(size=list(level_img.size), format=int(written), platform=int(level_platform),
                           blob=None if level_blob is None else bytes(level_blob).hex(), length=len(data)))
        return data, written

    astc = CONVERTER.compress_astc
    with (patch.object(CONVERTER, "compress_etcpak", capture_etcpak),
          patch.object(CONVERTER, "compress_astc", capture_astc),
          patch.object(CONVERTER, "image_to_texture2d", capture_convert)):
        if flip:
            case["initial"] = {endian: serialize(texture, schema(optional), endian).hex() for endian in ("<", ">")}
            set_image(texture, reference, int(target), mips)
            output, written = texture.image_data, texture.m_TextureFormat
            case["object"] = {endian: digest(serialize(texture, schema(optional), endian)) for endian in ("<", ">")}
            case["object"]["fields"] = object_fields(texture, optional)
            # Every profile uses the same mip termination, including objects
            # whose schema omits both optional mip fields.
            count = mips
            width, height = size
            for n in range(max(0, mips - 1)):
                width, height = width // 2, height // 2
                if min(width, height) < 4:
                    count = n + 1
                    break
        else:
            output, written = capture_convert(reference, target, platform_id, None if blob is None else list(blob), False)
            count = 1
    case["result"] = dict(format=int(written), mips=count, hex=output.hex(), **digest(output))
    case["levels"] = levels
    case["encoder_inputs"] = encoder_inputs
    assert sum(level["length"] for level in levels) == len(output)
    return case


def generate():
    assert platform.machine() == "x86_64", "ASTC fixtures are recorded on amd64 only (rest decision 4)"
    assert UnityPy.__version__ == "1.25.2" and PIL.__version__ == "9.5.0"
    assert importlib.metadata.version("etcpak") == "0.9.14"
    assert importlib.metadata.version("astc-encoder-py") == "0.1.12"
    etcpak_backend = sorted(name for name in sys.modules if name.startswith("etcpak._etcpak_"))
    assert etcpak_backend == ["etcpak._etcpak_none"], etcpak_backend
    defaults = etcpak.BC7CompressBlockParams()
    cases = [record(fmt, size, mode, mips=mips)
             for fmt in TARGETS for mode in ("RGB", "RGBA")
             for size, mips in (((1, 1), 1), ((7, 9), 1), ((19, 17), 1), ((35, 19), 9))]
    # ETC Switch block layouts are deliberately refused; EAC requests are
    # promoted to raw R8/RGBA32 and therefore do have a characterised layout.
    for fmt in TARGETS:
        if fmt not in (34, 45, 46, 47, 60, 64, 65):
            cases.append(record(fmt, (9, 7), exponent=1, mips=9))
    for fmt in (10, 27, 34, 54):
        cases.append(record(fmt, (17, 23), flip=False))
        cases.append(record(fmt, (35, 19), mips=3, optional=False))
    for mode in ("L", "LA", "P", "PA", "CMYK", "LAB", "RGBa", "I", "F"):
        for fmt in (TF.DXT1, TF.BC7, TF.ASTC_RGBA_4x4):
            cases.append(record(fmt, (9, 7), mode))
    for fmt in (TF.BC7, TF.BC4, TF.ETC_RGB4, TF.ASTC_RGB_4x4):
        cases.append(record(fmt, (7, 9), platform_id=11))
    sources = list(dict.fromkeys(case["source"] for case in cases))
    for case in cases:
        case["source"] = sources.index(case["source"])
    return dict(unitypy=UnityPy.__version__, pillow=PIL.__version__, machine=platform.machine(),
                etcpak="0.9.14", astc_encoder_py="0.1.12", sources=sources,
                oracle_backends=dict(etcpak=etcpak_backend,
                                     astc=sorted(name for name in sys.modules
                                                 if name.startswith("astc_encoder._encoder_"))),
                settings=dict(astc=dict(profile="LDR", quality=100, flags="USE_DECODE_UNORM8",
                                        type="U8", swizzle="RGBA", threads=1),
                              bc7={name: getattr(defaults, name) for name in dir(defaults) if name.startswith("m_")},
                              etcpak=dict(bc1="compress_bc1", bc3="compress_bc3", bc4="compress_bc4",
                                          bc5="compress_bc5", etc1="compress_etc1_rgb", etc2="compress_etc2_rgb",
                                          etc2_rgba="compress_etc2_rgba", rgba_input=True)),
                schema={"full": schema(), "minimal": schema(False)}, cases=cases)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    fixture = generate()
    content = (json.dumps(fixture, separators=(",", ":")) + "\n").encode()
    assert len(content) < 1_500_000, len(content)
    if args.check:
        assert OUT.read_bytes() == content, f"stale {OUT.relative_to(ROOT)}"
    else:
        OUT.write_bytes(content)
    print(f"set_image_encoders: {len(fixture['cases'])} writes, {len(TARGETS)} targets, {len(content)} bytes")


if __name__ == "__main__":
    main()
