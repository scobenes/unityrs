"""Print the Pillow 9.5.0 tables PARITY.md holds, observed in the oracle image.

    scripts/dev python tools/parity/pillow_tables.py packers   # section 18.1: the image modes `set_image` packs
    scripts/dev python tools/parity/pillow_tables.py writers   # section 15.8: what `Image.save` writes, by extension
    scripts/dev python tools/parity/pillow_tables.py libraries # section 1: the libraries whose code writes the bytes

`tests/test_pillow_tables.py` requires PARITY.md to hold exactly what this prints.
"""

import argparse
import contextlib
import os
import re
import sys
import tempfile
import time
from pathlib import Path
from unittest import mock

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

# The modes the extractor hands `Image.save`: those of a decoded texture (PARITY.md section 17), and `LA`, which
# `SpriteHelper.mask_sprite` gives a sprite of an `L` texture (its `putalpha`, section 15.2).
SAVED_MODES = ("RGBA", "RGB", "L", "LA")
# The sizes probed beside the 40x24 image, and two more names beside `image`: another ASCII one, and a non-ASCII one.
SIZES = ((1, 1), (15, 15), (16383, 1), (1, 16383), (16384, 1), (1, 16384))
NAMES = ("other", "\u56fe\u50cf")
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


def sample(mode):
    from PIL import Image

    image = Image.new("RGBA", (40, 24))
    image.putdata([(x * 6, y * 10, (x * y) % 256, 255 - x * 3) for y in range(24) for x in range(40)])
    return image.convert(mode)


def attempt(image, path):
    """`Image.save(path)` at the first clock: the bytes, or the exception's name."""
    try:
        return save(image, path, CLOCKS[0])
    except Exception as error:  # noqa: BLE001 - the exception is the result
        return type(error).__name__


def writer_cell(fmt, extension, mode, folder):
    """What `Image.save` does with an image of `mode` under `extension`."""
    image = sample(mode)
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


def conditions(extension, modes, folder):
    """What the size and the file name change, for the modes a writer saves: (cell text, raises, holds the name).

    The probe is the same for every mode; a writer that answered differently per mode would stop the script."""
    found = set()
    for mode in modes:
        image = sample(mode)
        base = attempt(image, os.path.join(folder, "image" + extension))
        assert isinstance(base, bytes), (extension, mode)
        sizes = tuple((size, attempt(image.resize(size), os.path.join(folder, "image" + extension))) for size in SIZES)
        sizes = tuple((size, result) for size, result in sizes if not isinstance(result, bytes))
        other, foreign = (attempt(image, os.path.join(folder, name + extension)) for name in NAMES)
        assert isinstance(other, bytes), (extension, mode)
        holds_name = PDF_DATES.sub(b"", other) != PDF_DATES.sub(b"", base)
        foreign = None if isinstance(foreign, bytes) else foreign
        # The extension is matched case-insensitively: the upper-case spelling saves the same file.
        upper = attempt(image, os.path.join(folder, "image" + extension.upper()))
        assert isinstance(upper, bytes) and (holds_name or upper == base), (extension, mode)
        found.add((sizes, holds_name, foreign))
    assert len(found) == 1, (extension, found)
    sizes, holds_name, foreign = found.pop()
    parts = []
    if sizes:
        saved = [size for size in SIZES if size not in dict(sizes)]
        errors = {error for _, error in sizes}
        assert len(errors) == 1, (extension, sizes)
        parts.append(
            f"raises `{errors.pop()}` at {', '.join(f'{w}×{h}' for (w, h), _ in sizes)}; "
            f"{', '.join(f'{w}×{h}' for w, h in saved)} save"
        )
    if holds_name:
        parts.append("the bytes hold the file's basename")
    if foreign:
        parts.append(f"a non-ASCII basename raises `{foreign}`")
    return "; ".join(parts) or "—", bool(sizes or foreign), holds_name


def acceptance(kinds, raises_by_condition=False, holds_name=False):
    kinds = set(kinds) | ({"raises"} if raises_by_condition else set())
    if kinds == {"raises"}:
        return "C", "writes `{fp}{extension}.png` (section 19.2)"
    same = "byte-equal with Pillow 9.5.0's `Image.save(path)`" + (" to the same path" if holds_name else "")
    same += ", no options, in the oracle image"
    parts = {
        "saves": same,
        "dated": same + ", once the `/CreationDate` and `/ModDate` values are masked: they hold the time of the save",
        "empty": same + ", when both sides are 16 or more; otherwise `{fp}{extension}.png`, as where Pillow raises",
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


def writer_extensions():
    """The extensions of each format Pillow 9.5.0 can save, by format."""
    from PIL import Image

    Image.init()
    by_format = {}
    for extension, fmt in sorted(Image.registered_extensions().items()):
        if fmt in Image.SAVE:
            by_format.setdefault(fmt, []).append(extension)
    return by_format


def writers():
    by_format = writer_extensions()
    lines = [
        "| format | extensions | `RGBA` | `RGB` | `L` | `LA` | size and file name | tier "
        "| unityrs writes, and the acceptance |",
        "|---|---|---|---|---|---|---|---|---|",
    ]
    with tempfile.TemporaryDirectory(prefix="unityrs-writers-") as folder:
        for fmt in sorted(by_format):
            extensions = by_format[fmt]
            cells = [writer_cell(fmt, extensions[0], mode, folder) for mode in SAVED_MODES]
            for extension in extensions[1:]:  # every extension of a format behaves alike per mode
                assert [writer_cell(fmt, extension, m, folder)[0] for m in SAVED_MODES] == [c[0] for c in cells]
            saving = [mode for mode, (kind, _) in zip(SAVED_MODES, cells, strict=True) if kind != "raises"]
            condition, raises, holds_name = conditions(extensions[0], saving, folder) if saving else ("—", False, False)
            tier, text = acceptance((kind for kind, _ in cells), raises, holds_name)
            spans = ", ".join(f"`{e}`" for e in extensions)
            row = " | ".join(cell for _, cell in cells)
            lines.append(f"| {fmt} | {spans} | {row} | {condition} | {tier} | {text} |")
    count = sum(len(e) for e in by_format.values())
    lines += ["", f"{count} extensions, {len(by_format)} formats. The libraries the writers call are section 1's."]
    return "\n".join(lines)


# The libraries bundled with Pillow 9.5.0 that a writer can call, with `features.version`'s key, and the Pillow
# encoders that call each one. libwebp is called through `_webp.WebPEncode`, not through an encoder.
LIBRARIES = (
    ("libjpeg-turbo", "libjpeg_turbo", {"jpeg"}),
    ("libwebp", "webp", {"libwebp"}),
    ("OpenJPEG", "jpg_2000", {"jpeg2k"}),
    ("zlib", "zlib", {"zip"}),
    ("libtiff", "libtiff", {"libtiff"}),
)


def encoders():
    """The encoders each writer calls for the modes of SAVED_MODES it saves, `libwebp` for libwebp."""
    from PIL import Image, WebPImagePlugin

    getencoder, webp_encode = Image._getencoder, WebPImagePlugin._webp.WebPEncode
    found = {}
    with tempfile.TemporaryDirectory(prefix="unityrs-encoders-") as folder:
        for fmt, extensions in sorted(writer_extensions().items()):
            seen = set()

            def spy(mode, name, *args, seen=seen, **kw):
                seen.add(name)
                return getencoder(mode, name, *args, **kw)

            def webp_spy(*args, seen=seen, **kw):
                seen.add("libwebp")
                return webp_encode(*args, **kw)

            for mode in SAVED_MODES:
                patches = (
                    mock.patch.object(Image, "_getencoder", spy),
                    mock.patch.object(WebPImagePlugin._webp, "WebPEncode", webp_spy),
                )
                with patches[0], patches[1], contextlib.suppress(Exception):
                    save(sample(mode), os.path.join(folder, "image" + extensions[0]), CLOCKS[0])
            found[fmt] = seen
    return found


def libraries():
    """Section 1's row for the libraries bundled with Pillow: what each one decides, from the encoders called."""
    from PIL import TiffImagePlugin, features

    found = encoders()
    calls, idle = [], []
    for name, key, names in LIBRARIES:
        formats = sorted(fmt for fmt, seen in found.items() if seen & names)
        version = features.version(key)
        if formats:
            via = ", ".join(f"`{n}`" for n in sorted(names - {"libwebp"}))
            listed = ", ".join(formats[:-1]) + (" and " if len(formats) > 1 else "") + formats[-1]
            calls.append((name, version, listed + (f" ({via})" if via else "")))
        else:
            idle.append((name, version))
    assert [name for name, _ in idle] == ["libtiff"] and TiffImagePlugin.WRITE_LIBTIFF is False, idle
    assert found["TIFF"] == {"raw"}, found["TIFF"]
    library_names = set().union(*(names for _, _, names in LIBRARIES))
    own = sorted(set().union(*found.values()) - library_names)
    return (
        f"| {', '.join(name for name, _, _ in calls)} (bundled with Pillow) "
        f"| {', '.join(version for _, version, _ in calls)}, as `tools/parity/pillow_tables.py libraries` prints them "
        f"| the bytes of the Pillow writers that call them (section 15.8): {'; '.join(text for _, _, text in calls)}. "
        f"The other writers use encoders of Pillow's own ({', '.join(f'`{n}`' for n in own)}). TIFF is one of them: "
        f"`TiffImagePlugin.WRITE_LIBTIFF` is false, so Pillow writes TIFF with `raw`, and libtiff "
        f"{idle[0][1]}, also bundled with Pillow, decides no bytes |"
    )


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("table", choices=["packers", "writers", "libraries"])
    args = parser.parse_args(argv)
    print({"packers": packers, "writers": writers, "libraries": libraries}[args.table]())


if __name__ == "__main__":
    main()
