"""A fixture bundle's textures: their declared texel storage in `unityrs show`, and their decode by `unityrs extract`.

A streamed texture's bytes are in the bundle's `.resS` node, which show does not read. `unityrs extract`
resolves that range and decodes it; a PNG at the declared size with nonzero texels is the evidence that the
range exists and holds data of the declared format. With `--inspect-dir`, `<bundle>.extract.json` holds the
saved extract report and the bundle's digest, and `<bundle>.extract/` the PNGs it names.
"""

import struct
import zlib

import child
import scratch

INLINE_MAX = 64  # canonical show writes longer byte values as a length and SHA-256
PNG = b"\x89PNG\r\n\x1a\n"
CHANNELS = {0: 1, 2: 3, 4: 2, 6: 4}
MAX_EDGE = 16384


def check_storage(view, image, size, label):
    """The image data is inline, exactly as canonical show writes it, or a range of the bundle's .resS node."""
    data = image.get("image data", {"t": "bytes", "hex": ""})
    stream = image.get("m_StreamData") or {}
    offset, streamed, path = stream.get("offset", 0), stream.get("size", 0), stream.get("path", "")
    view.require(isinstance(data, dict) and data.get("t") in ("bytes", "u8s"), f"invalid built image data: {label}")
    if "hex" in data:
        text = data["hex"]
        view.require(set(data) == {"t", "hex"} and isinstance(text, str) and len(text) % 2 == 0
                     and len(text) // 2 <= INLINE_MAX, f"invalid inline image data: {label}")
        bytes.fromhex(text)
        inline = len(text) // 2
    else:
        view.require(set(data) == {"t", "len", "sha256"} and type(data["len"]) is int and data["len"] > INLINE_MAX,
                     f"invalid summarised image data: {label}")
        inline = data["len"]
    view.require(type(offset) is int and type(streamed) is int and offset >= 0 and isinstance(path, str),
                 f"invalid built texture stream: {label}")
    if inline:
        view.require(inline == size and streamed == 0 and not path, f"built texture data size mismatch: {label}")
        return
    view.require(streamed == size, f"built texture data size mismatch: {label}")
    # inspect classifies a node by its first bytes, as UnityPy does, so a resource whose texels start with
    # zeros can be listed as an AssetsFile; a serialized CAB always carries its Unity version.
    name = path.rsplit("/", 1)[-1].lower()
    rows = [row for row in view.files.values() if row["file"].rsplit("/", 1)[-1].lower() == name]
    view.require(name.endswith(".ress") and len(rows) == 1 and rows[0]["kind"] in ("ResourceFile", "AssetsFile")
                 and (rows[0]["kind"] == "ResourceFile" or rows[0].get("unity_version") is None),
                 f"built texture stream must name the bundle's .resS node: {label} ({path!r})")


def png_pixels(data, require, label):
    """Width, height, channels and unfiltered 8-bit pixels of a non-interlaced PNG."""
    require(data.startswith(PNG), f"extracted texture is not a PNG: {label}")
    at, header, chunks = len(PNG), None, []
    while True:
        require(at + 12 <= len(data), f"truncated extracted PNG: {label}")
        length, kind = struct.unpack_from(">I4s", data, at)
        body = data[at + 8:at + 8 + length]
        require(len(body) == length, f"truncated extracted PNG: {label}")
        at += 12 + length
        if kind == b"IHDR":
            require(length == 13 and header is None, f"invalid extracted PNG header: {label}")
            header = struct.unpack(">IIBBBBB", body)
        elif kind == b"IDAT":
            chunks.append(body)
        elif kind == b"IEND":
            break
    require(header is not None, f"extracted PNG has no header: {label}")
    width, height, depth, color, _, _, interlace = header
    require(depth == 8 and color in CHANNELS and interlace == 0 and 0 < width <= MAX_EDGE and 0 < height <= MAX_EDGE,
            f"unsupported extracted PNG layout: {label}")
    channels = CHANNELS[color]
    stride = width * channels
    expected = (stride + 1) * height
    try:
        raw = zlib.decompressobj().decompress(b"".join(chunks), expected + 1)
    except zlib.error as exc:
        raise ValueError(f"invalid extracted PNG data: {label}: {exc}") from exc
    require(len(raw) == expected, f"extracted PNG data size mismatch: {label}")
    pixels, prior = bytearray(), bytearray(stride)
    for row in range(height):
        start = row * (stride + 1)
        kind, line = raw[start], bytearray(raw[start + 1:start + 1 + stride])
        require(kind <= 4, f"invalid extracted PNG filter: {label}")
        for i in range(stride):
            a = line[i - channels] if i >= channels else 0
            b = prior[i]
            c = prior[i - channels] if i >= channels else 0
            if kind == 1:
                line[i] = (line[i] + a) & 255
            elif kind == 2:
                line[i] = (line[i] + b) & 255
            elif kind == 3:
                line[i] = (line[i] + (a + b) // 2) & 255
            elif kind == 4:
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                line[i] = (line[i] + (a if pa <= pb and pa <= pc else b if pb <= pc else c)) & 255
        pixels += line
        prior = line
    return width, height, channels, bytes(pixels)


def check_decoded(view, image, decoded, label):
    """The decode matches the declared size and has structure: zeroed texels decode to one flat colour."""
    width, height, channels, pixels = decoded
    view.require((width, height) == (image["m_Width"], image["m_Height"]),
                 f"unityrs extract decoded {label} at {width}x{height}, not its declared size")
    colours = {pixels[i:i + channels] for i in range(0, len(pixels), channels)}
    view.require(len(colours) > 1, f"the built texture {label} decodes to a single colour")


def decoded_textures(view, labels):
    """Map each Texture2D key in `labels` to its decoded (width, height, channels, pixels)."""
    name = view.path.name
    if view.inspect_dir is None:
        with scratch.folder() as out:
            result = child.run([view.unityrs, "extract", name, "-o", str(out), "--asset-filter", "Texture2D"],
                               cwd=view.path.parent, capture_output=True, text=True)
            view.require(result.returncode == 0,
                         f"unityrs extract {name}: {(result.stderr or result.stdout).strip()}")
            return _decoded(view, labels, view.read_json(result.stdout), out)
    doc = view.read_json((view.inspect_dir / f"{name}.extract.json").read_text(encoding="utf-8"))
    view.require(isinstance(doc, dict) and doc.get("bundle_sha256") == view.digest,
                 f"extraction bundle digest mismatch: {name}.extract.json")
    return _decoded(view, labels, doc["report"], view.inspect_dir / f"{name}.extract")


def _decoded(view, labels, report, root):
    written, failures = {}, {}
    for row in report["written"]:
        written.setdefault((row["serialized_file"], row["path_id"]), []).append(row["path"])
    for row in report["failures"]:
        failures[row["serialized_file"], row["path_id"]] = row["reason"]
    images, base = {}, root.resolve()
    for key, label in labels.items():
        identity = view.headers[key]["file"], key[1]
        view.require(identity not in failures,
                     f"unityrs extract could not decode the built texture {label}: {failures.get(identity)}")
        paths = [path for path in written.get(identity, []) if path.lower().endswith(".png")]
        view.require(len(paths) == 1, f"unityrs extract wrote no PNG for the built texture {label}")
        path = (root / paths[0]).resolve()
        view.require(path.is_relative_to(base) and path.is_file(), f"missing extracted PNG for {label}")
        images[key] = png_pixels(path.read_bytes(), view.require, label)
    return images
