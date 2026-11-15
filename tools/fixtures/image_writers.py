#!/usr/bin/env python3
"""Record Pillow 9.5.0's default writers and refusals on synthetic images (W2).

Complete files (including PNG fallbacks) record byte length and whole-file
SHA-256, plus full hex at up to 256 bytes or the first 64 bytes as head hex.
Regenerate and --check only in the pinned unityrs-dev oracle container.
"""

import argparse
import hashlib
import io
from pathlib import Path
import sys

import PIL
from PIL import Image

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools" / "fixtures"))
from imageio import dump

OUT = ROOT / "crates/unityrs-imageio/tests/fixtures/writers.json"
MODES = {"RGBA": 4, "RGB": 3, "L": 1, "LA": 2, "P": 1}
FORMATS = {
    "BMP": ".bmp", "DIB": ".dib", "TGA": ".tga", "PPM": ".ppm",
    "PCX": ".pcx", "SGI": ".sgi", "IM": ".im", "TIFF": ".tiff",
    "DDS": ".dds", "ICO": ".ico", "ICNS": ".icns", "EPS": ".eps",
}
FALLBACKS = [".blp", ".bufr", ".fit", ".grib", ".h5", ".msp", ".palm",
             ".wmf", ".xbm", ".psd", ".unknown", ""]
DEFERRED = {"GIF": ".gif", "JPEG": ".jpg", "MPO": ".mpo", "PDF": ".pdf",
            "JPEG2000": ".jp2"}


class File(io.BytesIO):
    """Image.save reads a file object's name, just as for a filesystem path."""

    def __init__(self, name):
        super().__init__()
        self.name = name


def pixels(mode, width, height, kind):
    n = width * height * MODES[mode]
    if kind == "flat":
        return bytes([73]) * n
    return bytes(((i * 37 + i // MODES[mode] * 11 + (i // max(1, width)) * 7) % 256)
                 for i in range(n))


def palette():
    return bytes((i * 19 + (i // 3) * 7) % 256 for i in range(768))


def file_record(data):
    record = dict(len=len(data), sha256=hashlib.sha256(data).hexdigest())
    if len(data) <= 256:
        record["hex"] = data.hex()
    else:
        record["head"] = data[:64].hex()
    return record


def record(fmt, mode, width, height, kind="pattern", name=None, policy="writer", implicit=False,
           resize=None):
    extension = ".png" if fmt == "PNG" else FORMATS.get(fmt, fmt)
    name = "image" + extension if name is None else name
    if kind == "solid":
        # One L row uses about 64 MiB, with no duplicate Python pixel buffer.
        # A zero side allocates no pixels (a tall image still has row pointers).
        image = Image.new(mode, (width, height), 73)
    elif kind == "overlap":
        # ICO requires at least 16 rows. Map overlapping flat L rows with stride
        # 1: about 43 MiB backing storage instead of 683 MiB. Pillow's ICO plugin
        # itself copies the image (~683 MiB); a permitted table uses ~2 GiB.
        data = bytes([73]) * (width + height - 1)
        image = Image.frombuffer(mode, (width, height), data, "raw", mode, 1, 1)
    else:
        data = pixels(mode, width, height, kind)
        image = (Image.frombytes(mode, (width, height), data) if width and height
                 else Image.new(mode, (width, height)))
    if mode == "P" and not implicit:
        image.putpalette(palette())
    saved = File(name)
    case = dict(format=fmt, mode=mode, width=width, height=height, kind=kind,
                name=name, policy=policy)
    if implicit:
        case["implicit_palette"] = True
    if resize is not None:
        case["resize"] = resize
    try:
        if resize is not None:
            image = image.resize(tuple(resize[:2]), getattr(Image.Resampling, resize[2]))
        # Unknown extensions exercise Image.save's dispatch, not a plugin.
        image.save(saved, fmt if fmt in FORMATS or fmt in DEFERRED or fmt == "PNG" else None)
    except Exception as error:
        case["exception"] = type(error).__name__
        case["message"] = str(error)
    else:
        if policy != "unsupported":
            case.update(file_record(saved.getvalue()))
    if resize is None and policy != "unsupported" and (policy == "fallback" or "exception" in case
                                    or (fmt == "ICO" and min(width, height) < 16)):
        png = io.BytesIO()
        try:
            image.save(png, "PNG")
        except Exception:
            case["fallback"] = None
        else:
            case["fallback"] = file_record(png.getvalue())
    return case


def fixture():
    cases = []
    for fmt in FORMATS:
        sizes = [(1, 1), (2, 3), (3, 2), (7, 5), (0, 0), (0, 3), (3, 0)]
        if fmt == "ICNS":
            sizes = [(1, 1), (3, 2), (0, 0), (33, 35), (1025, 1), (1, 1025)]
        if fmt == "ICO":
            sizes += [(15, 15), (16, 16), (17, 31), (24, 24), (32, 32),
                      (48, 48), (64, 64), (128, 128), (256, 256), (257, 259)]
            sizes += [(63, 47), (127, 257), (1025, 17)]
        for mode in MODES:
            for width, height in sizes:
                cases.append(record(fmt, mode, width, height))
            if fmt != "ICNS":
                cases.append(record(fmt, mode, 9, 3, "flat"))
            if fmt == "PCX":
                for width in (63, 64, 65):
                    cases.append(record(fmt, mode, width, 3, "flat"))
        if fmt in ("BMP", "DIB", "TGA", "PCX", "SGI", "TIFF", "DDS", "PPM", "IM", "EPS"):
            for side in (65534, 65535, 65536):
                for width, height in ((side, 1), (1, side)):
                    cases.append(record(fmt, "RGB", width, height, "flat"))
        if fmt in ("SGI", "IM"):
            for name in ("other.NAME", "café.im", "a" * 110 + ".im", "folder/.hidden",
                         "a." + "e" * 100, "folder/a\\b.im", "image.IM", "image.SGI"):
                cases.append(record(fmt, "RGB", 3, 2, name=name))
    for extension in FALLBACKS:
        for mode in MODES:
            if mode == "P":
                continue  # These plugins refuse every extractor mode; P is not one.
            cases.append(record(extension, mode, 3, 2, policy="fallback"))
    for fmt in (*FORMATS, "PNG"):
        for width, height in ((3, 2), (33, 35)):
            cases.append(record(fmt, "P", width, height, implicit=True))
    for fmt, extension in DEFERRED.items():
        for mode in MODES:
            cases.append(record(fmt, mode, 3, 2, name="image" + extension, policy="unsupported"))
    # Append to preserve every earlier record. These exercise the float box's
    # rounding before ceil(support), not just integer dimension arithmetic.
    for width in (67_108_353, 67_108_360):
        cases.append(record("PNG", "L", width, 1, "solid", policy="resize",
                            resize=[1024, 1, "BICUBIC"]))
        cases.append(record("ICNS", "L", width, 1, "solid"))
    for width, height in ((68_000_000, 0), (0, 68_000_000)):
        cases.append(record("ICNS", "RGBA", width, height, "solid"))
        cases.append(record("ICNS", "P", width, height, "solid"))
    for width in (44_739_237, 44_739_238):
        cases.append(record("PNG", "L", width, 1, "solid", policy="resize",
                            resize=[16, 1, "LANCZOS"]))
        cases.append(record("ICO", "L", width, 16, "overlap"))
    return dict(generator="tools/fixtures/image_writers.py", pillow=PIL.__version__,
                zlib=Image.core.zlib_version, cases=cases)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    if PIL.__version__ != "9.5.0" or Image.core.zlib_version != "1.2.13":
        parser.error("requires Pillow 9.5.0 over zlib 1.2.13")
    body = dump(fixture())
    if args.check:
        if not OUT.exists() or OUT.read_text() != body:
            print("stale fixture: writers.json", file=sys.stderr)
            return 1
    else:
        OUT.write_text(body)
    return 0


if __name__ == "__main__":
    sys.exit(main())
