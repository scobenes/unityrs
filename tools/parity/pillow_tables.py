"""Print the two Pillow 9.5.0 tables PARITY.md holds, observed in the oracle image.

    scripts/dev python tools/parity/pillow_tables.py packers   # section 18.1: the image modes `set_image` packs
    scripts/dev python tools/parity/pillow_tables.py writers   # section 15.8: what `Image.save` writes, by extension

`tests/test_pillow_tables.py` requires PARITY.md to hold exactly what this prints.
"""

import argparse
import os
import re
import sys
import tempfile
import time
from pathlib import Path
from unittest import mock

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

# The modes a decoded texture or sprite has (PARITY.md section 17), so the modes the extractor hands `Image.save`.
SAVED_MODES = ("RGBA", "RGB", "L")
# Two different clocks: a writer whose bytes differ between them embeds the time.
CLOCKS = (time.struct_time((2001, 2, 3, 4, 5, 6, 5, 34, 0)), time.struct_time((2011, 12, 13, 14, 15, 16, 1, 347, 0)))
PDF_DATES = re.compile(rb"(/(?:CreationDate|ModDate) )\(D:\d{14}Z\)")


def image_modes():
    """Every mode Pillow 9.5.0's `Image.new` accepts."""
    from PIL import Image, ImageMode

    ImageMode.getmode("L")  # fills the table
    modes = []
    for mode in ImageMode._modes:
        try:
            Image.new(mode, (2, 2))
        except (ValueError, KeyError):
            continue
        modes.append(mode)
    return modes


def packed_raw_mode(fmt, platform=0, blob=None):
    """The raw mode `image_to_texture2d` hands `Image.tobytes` for `fmt`, from an RGBA image."""
    from PIL import Image
    from UnityPy.export import Texture2DConverter

    seen = []
    tobytes = Image.Image.tobytes

    class Packed(Exception):
        pass

    def record(image, encoder_name="raw", *args):
        seen.append(args[0] if args else "RGBA")
        raise Packed

    with mock.patch.object(Image.Image, "tobytes", record):
        try:
            Texture2DConverter.image_to_texture2d(Image.new("RGBA", (8, 8)), fmt, platform, blob)
        except Packed:
            pass
        except Exception:  # noqa: BLE001 - a target that fails before it packs (a Switch target with no block)
            return None
    assert tobytes is Image.Image.tobytes
    return seen[0] if seen else None


def packers():
    from PIL import Image
    from UnityPy.enums import BuildTarget, TextureFormat

    switch_blob = bytes(8) + (1).to_bytes(4, "little")
    targets, switch = {}, {}
    for fmt in TextureFormat:
        targets.setdefault(packed_raw_mode(fmt), []).append(fmt.name)
        switch[fmt.name] = packed_raw_mode(fmt, BuildTarget.Switch, switch_blob)
    modes = image_modes()
    packs = {}
    for raw in targets:
        packs[raw] = []
        for mode in modes:
            try:
                Image.new(mode, (2, 2)).tobytes("raw", raw)
            except ValueError:
                continue
            packs[raw].append(mode)
    lines = [
        "| raw mode | targets that pack it | image modes Pillow 9.5.0 packs into it |",
        "|---|---|---|",
    ]
    for raw in sorted(targets, key=lambda r: (r == "RGBA", r)):
        names = targets[raw]
        if raw == "RGBA":
            cell = f"every other target ({len(names)})"
        else:
            cell = ", ".join(f"`{name}`" for name in sorted(names))
            on_switch = {switch[name] for name in names}
            if on_switch != {raw}:
                cell += "; on a Switch-swizzled texture they pack " + ", ".join(f"`{m}`" for m in sorted(on_switch))
        lines.append(f"| `{raw}` | {cell} | {', '.join(f'`{m}`' for m in packs[raw])} |")
    never = [mode for mode in modes if not any(mode in found for found in packs.values())]
    lines += ["", "Packed into none of them: " + ", ".join(f"`{mode}`" for mode in never) + "."]
    unconvertible = []
    for mode in modes:
        try:
            Image.new(mode, (2, 2)).convert("RGBA")
        except ValueError:
            unconvertible.append(mode)
    lines += ['`Image.convert("RGBA")` raises for: ' + ", ".join(f"`{mode}`" for mode in unconvertible) + "."]
    return "\n".join(lines)


def save(image, path, clock):
    with mock.patch.object(time, "gmtime", lambda *args: clock):
        image.save(path)
    return Path(path).read_bytes()


def writer_cell(fmt, extension, mode, folder):
    """What `Image.save` does with an image of `mode` under `extension`."""
    from PIL import Image

    image = Image.new("RGBA", (40, 24))
    image.putdata([(x * 6, y * 10, (x * y) % 256, 255 - x * 3) for y in range(24) for x in range(40)])
    image = image.convert(mode)
    path = os.path.join(folder, "image" + extension)
    try:
        first, second = (save(image, path, clock) for clock in CLOCKS)
    except Exception as error:  # noqa: BLE001 - the exception is the result
        return "raises", f"raises `{type(error).__name__}`"
    if first != second:
        assert PDF_DATES.sub(rb"\1()", first) == PDF_DATES.sub(rb"\1()", second), (fmt, mode)
        return "dated", "saves, with the time"
    if fmt == "ICO":
        # ICO keeps the sizes of its default list (16x16 to 256x256) that fit the image: none when a side is below 16.
        for size in ((15, 24), (40, 15), (16, 16)):
            small = save(image.resize(size), path, CLOCKS[0])
            assert (int.from_bytes(small[4:6], "little") == 0) == (min(size) < 16), (size, small[:6])
        return "empty", "saves; with a side below 16, a file holding no image"
    return "saves", "saves"


def acceptance(kinds):
    kinds = set(kinds)
    if kinds == {"raises"}:
        return "C", "writes `{fp}{extension}.png` (section 19.2)"
    parts = {
        "saves": "byte-equal with Pillow 9.5.0's `Image.save(path)`, no options, in the oracle image",
        "dated": "byte-equal with Pillow 9.5.0's `Image.save(path)`, no options, in the oracle image, "
        "once the `/CreationDate` and `/ModDate` values are masked: they hold the time of the save",
        "empty": "byte-equal with Pillow 9.5.0's `Image.save(path)`, no options, in the oracle image, "
        "when both sides are 16 or more; otherwise `{fp}{extension}.png`, as where Pillow raises",
    }
    saved = sorted(kinds - {"raises"})
    assert len(saved) == 1, kinds
    tier = "A"
    if "empty" in kinds:
        tier = "A; C with a side below 16"
    if "raises" in kinds:
        tier += "; C where it raises"
    text = parts[saved[0]]
    if "raises" in kinds:
        text += "; where Pillow raises, `{fp}{extension}.png` (section 19.2)"
    return tier, text


def writers():
    from PIL import Image, features

    Image.init()
    by_format = {}
    for extension, fmt in sorted(Image.registered_extensions().items()):
        if fmt in Image.SAVE:
            by_format.setdefault(fmt, []).append(extension)
    lines = [
        "| format | extensions | `RGBA` | `RGB` | `L` | tier | unityrs writes, and the acceptance |",
        "|---|---|---|---|---|---|---|",
    ]
    with tempfile.TemporaryDirectory(prefix="unityrs-writers-") as folder:
        for fmt in sorted(by_format):
            extensions = by_format[fmt]
            cells = [writer_cell(fmt, extensions[0], mode, folder) for mode in SAVED_MODES]
            for extension in extensions[1:]:  # every extension of a format behaves alike per mode
                assert [writer_cell(fmt, extension, m, folder)[0] for m in SAVED_MODES] == [c[0] for c in cells]
            tier, text = acceptance(kind for kind, _ in cells)
            spans = ", ".join(f"`{e}`" for e in extensions)
            lines.append(f"| {fmt} | {spans} | {' | '.join(cell for _, cell in cells)} | {tier} | {text} |")
    count = sum(len(e) for e in by_format.values())
    versions = ", ".join(
        f"{name} {features.version(key)}"
        for name, key in (
            ("libjpeg-turbo", "libjpeg_turbo"),
            ("libwebp", "webp"),
            ("OpenJPEG", "jpg_2000"),
            ("zlib", "zlib"),
            ("libtiff", "libtiff"),
        )
    )
    lines += ["", f"{count} extensions, {len(by_format)} formats. Libraries in the oracle image: {versions}."]
    return "\n".join(lines)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("table", choices=["packers", "writers"])
    args = parser.parse_args(argv)
    print(packers() if args.table == "packers" else writers())


if __name__ == "__main__":
    main()
